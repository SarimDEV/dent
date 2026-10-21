//! Remote devices against a real daemon ([remote-plan.md], stage 1): a
//! device connects over the network, pairs with a code, then speaks the
//! same protocol as the local socket, framed and encrypted ([`dent_link`]).
//!
//! Connections come in over loopback here, so account trust never applies
//! (loopback is never trusted by Tailscale account) and the LAN setting
//! doesn't either.
//!
//! [remote-plan.md]: ../../../docs/remote-plan.md

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dent_link::{Control, Goodbye, Hello, Initiator, Keys, Opened, Refusal, Transport, Trust};
use dent_protocol::{Envelope, Out, PROTOCOL, PROTOCOL_MIN, Reply, Request, encode};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

struct Daemon {
    state: PathBuf,
    port: u16,
    child: Option<Child>,
}

impl Daemon {
    fn start(env: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dent-remote-{}-{:x}",
            std::process::id(),
            rand_u64()
        ));
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();
        // A free port: bind, note it, let it go.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut d = Self {
            state,
            port,
            child: None,
        };
        d.spawn(env);
        d
    }

    fn spawn(&mut self, env: &[(&str, &str)]) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_dent-daemon"));
        c.arg("--state")
            .arg(&self.state)
            .env("DENT_REMOTE_PORT", self.port.to_string())
            // No Tailscale in tests: account trust is simply unavailable.
            .env("DENT_TAILSCALE_LOCALAPI", "127.0.0.1:1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (k, v) in env {
            c.env(k, v);
        }
        self.child = Some(c.spawn().unwrap());
        let until = Instant::now() + Duration::from_secs(10);
        while std::net::TcpStream::connect(("127.0.0.1", self.port)).is_err()
            || UnixStream::connect(dent_protocol::daemon_socket(&self.state)).is_err()
        {
            assert!(Instant::now() < until, "daemon didn't start");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// One request over the local socket.
    fn ask(&self, req: Request) -> Reply {
        let mut w = UnixStream::connect(dent_protocol::daemon_socket(&self.state)).unwrap();
        let mut r = BufReader::new(w.try_clone().unwrap());
        w.write_all(&encode(
            1,
            &Request::Hello {
                proto: PROTOCOL,
                client: "test".into(),
            },
        ))
        .unwrap();
        w.write_all(&encode(2, &req)).unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            assert!(r.read_line(&mut line).unwrap() > 0, "socket closed");
            let Ok(env) = serde_json::from_str::<Envelope<Out>>(&line) else {
                continue;
            };
            if env.req == 2 {
                match env.body {
                    Out::Reply { reply } => return reply,
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
    }

    fn code(&self) -> String {
        match self.ask(Request::OpenPairing) {
            Reply::Pairing { code, .. } => code,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn signal(&mut self, sig: i32) {
        if let Some(c) = self.child.as_ref() {
            // SAFETY: kill has no memory preconditions.
            unsafe { libc::kill(c.id() as i32, sig) };
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(root) = self.state.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

/// A different number each call, for unique temp folders.
fn rand_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ (N.fetch_add(1, Ordering::Relaxed) << 48)
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A remote device.
struct Device {
    ws: Ws,
    t: Transport,
    trust: Trust,
    next: u64,
}

fn hello(name: &str, lo: u32, hi: u32) -> Hello {
    Hello {
        name: name.into(),
        proto_min: lo,
        proto: hi,
        probe: false,
    }
}

async fn recv_bin(ws: &mut Ws) -> Vec<u8> {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("no frame within 10 s")
        {
            Some(Ok(Message::Binary(b))) => return b.to_vec(),
            Some(Ok(Message::Close(_))) | None => panic!("closed"),
            Some(Ok(_)) => continue,
            Some(Err(e)) => panic!("{e}"),
        }
    }
}

/// Connects and returns the first thing the daemon says after the handshake.
async fn connect(port: u16, keys: &Keys, h: &Hello) -> (Ws, Transport, Control) {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/"))
        .await
        .unwrap();
    let (ini, m1) = Initiator::start(keys).unwrap();
    ws.send(Message::Binary(m1.into())).await.unwrap();
    let m2 = recv_bin(&mut ws).await;
    let ans = ini.read(&m2, None).unwrap();
    let (mut t, m3) = ans.finish(h).unwrap();
    ws.send(Message::Binary(m3.into())).await.unwrap();
    let first = loop {
        let f = recv_bin(&mut ws).await;
        if let Some(Opened::Control(c)) = t.open(&f).unwrap() {
            break c;
        }
    };
    (ws, t, first)
}

impl Device {
    async fn connect(port: u16, keys: &Keys) -> Self {
        let (ws, t, first) = connect(port, keys, &hello("phone", PROTOCOL_MIN, PROTOCOL)).await;
        let Control::Trust(trust) = first else {
            panic!("expected a trust verdict, got {first:?}")
        };
        Self {
            ws,
            t,
            trust,
            next: 1,
        }
    }

    async fn control(&mut self, c: &Control) {
        let f = self.t.seal_control(c).unwrap();
        self.ws.send(Message::Binary(f.into())).await.unwrap();
    }

    async fn next(&mut self) -> Opened {
        loop {
            let f = recv_bin(&mut self.ws).await;
            if let Some(o) = self.t.open(&f).unwrap() {
                return o;
            }
        }
    }

    async fn next_control(&mut self) -> Control {
        loop {
            if let Opened::Control(c) = self.next().await {
                return c;
            }
        }
    }

    async fn pair(&mut self, code: &str) -> Control {
        let mut p = dent_link::pair::Pairing::start(code);
        self.control(&Control::Pair {
            spake: hex::encode(&p.outbound),
        })
        .await;
        let spake = match self.next_control().await {
            Control::Pair { spake } => spake,
            other => return other,
        };
        let key = p.finish(&hex::decode(spake).unwrap()).unwrap();
        let mac = dent_link::pair::confirm(&key, &self.t.hash, dent_link::pair::Role::Client);
        self.control(&Control::PairConfirm {
            mac: hex::encode(mac),
        })
        .await;
        match self.next_control().await {
            Control::PairConfirm { mac } => {
                assert!(dent_link::pair::verify(
                    &key,
                    &self.t.hash,
                    dent_link::pair::Role::Daemon,
                    &hex::decode(mac).unwrap()
                ));
                self.next_control().await
            }
            other => other,
        }
    }

    async fn request(&mut self, req: Request) -> u64 {
        let n = self.next;
        self.next += 1;
        let mut line = encode(n, &req);
        line.pop();
        for f in self.t.seal_data(&line).unwrap() {
            self.ws.send(Message::Binary(f.into())).await.unwrap();
        }
        n
    }

    async fn out(&mut self) -> Envelope<Out> {
        loop {
            if let Opened::Data(d) = self.next().await {
                return serde_json::from_slice(&d).unwrap();
            }
        }
    }

    async fn reply(&mut self, req: Request) -> Out {
        let n = self.request(req).await;
        loop {
            let e = self.out().await;
            if e.req == n {
                return e.body;
            }
        }
    }
}

#[tokio::test]
async fn pair_then_use_the_daemon_remotely() {
    let d = Daemon::start(&[]);
    let keys = Keys::generate().unwrap();

    // A stranger is told it isn't trusted, and that pairing is closed.
    let mut dev = Device::connect(d.port, &keys).await;
    assert_eq!(
        dev.trust,
        Trust::Untrusted {
            pairing_open: false
        }
    );
    assert_eq!(
        dev.pair("123456").await,
        Control::Refuse(Refusal::PairClosed)
    );

    // A code shows; a wrong one is refused, the right one pairs.
    let code = d.code();
    let wrong = if code == "000000" { "111111" } else { "000000" };
    assert_eq!(dev.pair(wrong).await, Control::Refuse(Refusal::PairWrong));
    let paired = dev.pair(&code).await;
    assert_eq!(
        paired,
        Control::Paired {
            access: dent_link::Access::Full
        }
    );

    // Trusted now: the same protocol as the local socket.
    let w = dev
        .reply(Request::Hello {
            proto: PROTOCOL,
            client: "phone".into(),
        })
        .await;
    let Out::Reply {
        reply: Reply::Welcome { id, epoch, .. },
    } = w
    else {
        panic!("expected a welcome, got {w:?}")
    };
    assert_eq!(id.len(), 12, "the machine's key fingerprint");
    assert!(!epoch.is_empty());

    // Pings are answered on the control lane.
    dev.control(&Control::Ping { t: 42 }).await;
    assert_eq!(dev.next_control().await, Control::Pong { t: 42 });

    // A subscription with a stale epoch is told to resync.
    dev.request(Request::Subscribe {
        after: 5,
        epoch: Some("old".into()),
    })
    .await;
    let mut resynced = false;
    let mut caught_up = false;
    for _ in 0..10 {
        match dev.out().await.body {
            Out::Resync { epoch: e } => {
                assert_eq!(e, epoch);
                resynced = true;
            }
            Out::CaughtUp { .. } => {
                caught_up = true;
                break;
            }
            _ => {}
        }
    }
    assert!(resynced && caught_up);

    // Coming back later needs no code, and the code was spent anyway.
    drop(dev);
    let again = Device::connect(d.port, &keys).await;
    assert_eq!(
        again.trust,
        Trust::Paired {
            access: dent_link::Access::Full
        }
    );

    // The device is listed, and removing it cuts it off at once.
    let Reply::Devices { devices } = d.ask(Request::Devices) else {
        panic!()
    };
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "phone");
    let mut again = again;
    let id = devices[0].id.clone();
    d.ask(Request::RemoveDevice { id });
    assert_eq!(
        again.next_control().await,
        Control::Refuse(Refusal::Removed)
    );
    let after = Device::connect(d.port, &keys).await;
    assert_eq!(
        after.trust,
        Trust::Untrusted {
            pairing_open: false
        }
    );
}

#[tokio::test]
async fn incompatible_versions_are_refused_with_both_ranges() {
    let d = Daemon::start(&[]);
    let keys = Keys::generate().unwrap();
    let (_, _, first) = connect(d.port, &keys, &hello("future", 90, 99)).await;
    assert_eq!(
        first,
        Control::Refuse(Refusal::Incompatible {
            daemon: (PROTOCOL_MIN, PROTOCOL),
            client: (90, 99),
        })
    );
}

#[tokio::test]
async fn going_away_says_goodbye() {
    let mut d = Daemon::start(&[("DENT_STOP_REASON", "restart")]);
    let keys = Keys::generate().unwrap();
    let mut dev = Device::connect(d.port, &keys).await;
    let code = d.code();
    assert_eq!(
        dev.pair(&code).await,
        Control::Paired {
            access: dent_link::Access::Full
        }
    );
    d.signal(libc::SIGTERM);
    assert_eq!(
        dev.next_control().await,
        Control::Goodbye(Goodbye::Restart {
            expected_down_ms: 3000
        })
    );
}

#[tokio::test]
async fn a_large_reply_arrives_whole_while_pings_still_answer() {
    let d = Daemon::start(&[]);
    let keys = Keys::generate().unwrap();
    let mut dev = Device::connect(d.port, &keys).await;
    let code = d.code();
    dev.pair(&code).await;
    dev.reply(Request::Hello {
        proto: PROTOCOL,
        client: "phone".into(),
    })
    .await;
    // A listing of 3,000 files is a reply bigger than one frame.
    let many = d.state.join("many");
    std::fs::create_dir_all(&many).unwrap();
    for i in 0..3000 {
        std::fs::write(many.join(format!("file-{i:04}-{}", "x".repeat(60))), "").unwrap();
    }
    let n = dev
        .request(Request::FilesIn {
            dir: many.display().to_string(),
        })
        .await;
    dev.control(&Control::Ping { t: 9 }).await;
    let mut got_pong = false;
    let mut got_reply = false;
    while !(got_pong && got_reply) {
        match dev.next().await {
            Opened::Control(Control::Pong { t: 9 }) => got_pong = true,
            Opened::Data(bytes) => {
                let e: Envelope<Out> = serde_json::from_slice(&bytes).unwrap();
                if e.req == n {
                    assert!(
                        bytes.len() > dent_link::noise::PART,
                        "reply spans frames: {}",
                        bytes.len()
                    );
                    got_reply = true;
                }
            }
            _ => {}
        }
    }
}
