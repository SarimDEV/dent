//! Adversarial tests for remote devices against a real daemon: peers that
//! guess, lie, flood, stall or come back after being removed. Each test
//! asserts what a user or an honest peer would see go wrong, not how the
//! daemon is built. Harness as in `remote.rs`: connections come in over
//! loopback, so account trust and the LAN switch never apply, except where
//! a test comes in by this machine's own network address with a stand-in
//! for Tailscale.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dent_link::{Access, Control, Hello, Initiator, Keys, Opened, Refusal, Transport, Trust};
use dent_protocol::{Body, Envelope, Out, PROTOCOL, PROTOCOL_MIN, Reply, Request, encode};
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
            "dent-break-{}-{:x}",
            std::process::id(),
            rand_u64()
        ));
        build_helpers();
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let mut d = Self {
            state,
            port: 0,
            child: None,
        };
        for _ in 0..20 {
            d.port = free_port();
            if d.try_spawn(env) {
                return d;
            }
            if let Some(mut c) = d.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        panic!("no free port");
    }

    /// A second daemon, its own identity, on `port`.
    fn start_on(port: u16, env: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dent-break-{}-{:x}",
            std::process::id(),
            rand_u64()
        ));
        build_helpers();
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let mut d = Self {
            state,
            port,
            child: None,
        };
        d.spawn(env);
        d
    }

    /// Restarted on the port it had, which must still be free.
    fn spawn(&mut self, env: &[(&str, &str)]) {
        assert!(self.try_spawn(env), "port {} was taken", self.port);
    }

    fn try_spawn(&mut self, env: &[(&str, &str)]) -> bool {
        let log = self.state.with_file_name("daemon.log");
        let _ = std::fs::remove_file(&log);
        let mut c = Command::new(env!("CARGO_BIN_EXE_dent-daemon"));
        c.arg("--state")
            .arg(&self.state)
            .env("DENT_REMOTE_PORT", self.port.to_string())
            .env("DENT_TAILSCALE_LOCALAPI", "127.0.0.1:1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap());
        for (k, v) in env {
            c.env(k, v);
        }
        self.child = Some(c.spawn().unwrap());
        if !got_port(&log, self.port) {
            return false;
        }
        let until = Instant::now() + Duration::from_secs(10);
        while std::net::TcpStream::connect(("127.0.0.1", self.port)).is_err()
            || UnixStream::connect(dent_protocol::daemon_socket(&self.state)).is_err()
        {
            assert!(Instant::now() < until, "daemon didn't start");
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

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

    /// Every event in the log, over the local socket.
    fn events(&self) -> Vec<dent_protocol::Event> {
        let mut w = UnixStream::connect(dent_protocol::daemon_socket(&self.state)).unwrap();
        w.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut r = BufReader::new(w.try_clone().unwrap());
        w.write_all(&encode(
            1,
            &Request::Hello {
                proto: PROTOCOL,
                client: "test".into(),
            },
        ))
        .unwrap();
        w.write_all(&encode(
            2,
            &Request::Subscribe {
                after: 0,
                epoch: None,
            },
        ))
        .unwrap();
        let mut all = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            assert!(r.read_line(&mut line).unwrap() > 0, "socket closed");
            let Ok(env) = serde_json::from_str::<Envelope<Out>>(&line) else {
                continue;
            };
            match env.body {
                Out::Events { events } => all.extend(events),
                Out::CaughtUp { .. } => return all,
                _ => {}
            }
        }
    }

    fn code(&self) -> String {
        match self.ask(Request::OpenPairing) {
            Reply::Pairing { code, .. } => code,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn alive(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_none())
    }

    fn signal(&mut self, sig: i32) {
        if let Some(c) = self.child.as_ref() {
            // SAFETY: kill has no memory preconditions.
            unsafe { libc::kill(c.id() as i32, sig) };
        }
    }

    fn reap(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.wait();
        }
    }
}

/// Whether the daemon logging to `log` got `port`. Picking a free port and
/// letting it go races the other tests doing the same; a daemon that lost
/// runs without remote access, and the test would quietly talk to the
/// winner instead, which the winner's test then kills mid-way.
fn got_port(log: &std::path::Path, port: u16) -> bool {
    let mine = format!("remote access on port {port} ");
    // Generous: dozens start at once when test files run side by side.
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let said = std::fs::read_to_string(log).unwrap_or_default();
        if said.contains(&mine) {
            return true;
        }
        if said.contains("remote access off") {
            return false;
        }
        assert!(
            Instant::now() < until,
            "daemon never said whether it listens"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A port to try, from below the range the system hands out to outgoing
/// connections. Never checked by binding it first: a check's listener on
/// 127.0.0.1 can sit on top of another test's daemon listening on every
/// address, take its connections and reset them as it closes. A port
/// that's taken shows in the daemon's log instead ([`got_port`]).
fn free_port() -> u16 {
    10_000 + (rand_u64() % 22_000) as u16
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        common::stop_all(&self.state);
        // A failing test shows what its daemon said.
        if std::thread::panicking() {
            let log = self.state.with_file_name("daemon.log");
            eprintln!(
                "daemon on port {} said:\n{}",
                self.port,
                std::fs::read_to_string(log).unwrap_or_default()
            );
        }
        if let Some(root) = self.state.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn rand_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ (N.fetch_add(1, Ordering::Relaxed) << 48)
}

fn fake_claude() -> String {
    build_helpers();
    std::path::Path::new(env!("CARGO_BIN_EXE_dent-daemon"))
        .with_file_name("dent-fake-claude")
        .to_str()
        .unwrap()
        .to_string()
}

/// The agent stand-in and the session holder, built once, before any
/// daemon starts: building while daemons run can take the holder away
/// for a moment from one starting a session.
fn build_helpers() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let built = Command::new(env!("CARGO"))
            .args([
                "build",
                "-q",
                "-p",
                "dent-fake-claude",
                "-p",
                "dent-session-holder",
            ])
            .status()
            .unwrap();
        assert!(built.success());
    });
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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

async fn recv_bin(ws: &mut Ws) -> Option<Vec<u8>> {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .ok()?
        {
            Some(Ok(Message::Binary(b))) => return Some(b.to_vec()),
            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return None,
            Some(Ok(_)) => continue,
        }
    }
}

/// Connects and returns the first thing the daemon says after the
/// handshake; `None` when the daemon drops the connection first.
async fn try_connect(
    host: &str,
    port: u16,
    keys: &Keys,
    h: &Hello,
) -> Option<(Ws, Transport, Control)> {
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(format!("ws://{host}:{port}/")),
    )
    .await
    .ok()?
    .ok()?;
    let (ini, m1) = Initiator::start(keys).unwrap();
    ws.send(Message::Binary(m1.into())).await.ok()?;
    let m2 = recv_bin(&mut ws).await?;
    let ans = ini.read(&m2, None).ok()?;
    let (mut t, m3) = ans.finish(h).unwrap();
    ws.send(Message::Binary(m3.into())).await.ok()?;
    let first = loop {
        let f = recv_bin(&mut ws).await?;
        if let Some(Opened::Control(c)) = t.open(&f).ok()? {
            break c;
        }
    };
    Some((ws, t, first))
}

impl Device {
    async fn connect(port: u16, keys: &Keys) -> Self {
        Self::connect_as(port, keys, &hello("phone", PROTOCOL_MIN, PROTOCOL)).await
    }

    async fn connect_as(port: u16, keys: &Keys, h: &Hello) -> Self {
        Self::connect_at("127.0.0.1", port, keys, h).await
    }

    async fn connect_at(host: &str, port: u16, keys: &Keys, h: &Hello) -> Self {
        // As a real client does: a connection the daemon sheds under load
        // is dialled again.
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some((ws, t, first)) = try_connect(host, port, keys, h).await {
                let Control::Trust(trust) = first else {
                    panic!("expected a trust verdict, got {first:?}")
                };
                return Self {
                    ws,
                    t,
                    trust,
                    next: 1,
                };
            }
            assert!(Instant::now() < until, "couldn't connect within 20 s");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn send_frame(&mut self, f: Vec<u8>) -> bool {
        self.ws.send(Message::Binary(f.into())).await.is_ok()
    }

    async fn control(&mut self, c: &Control) {
        let f = self.t.seal_control(c).unwrap();
        self.ws.send(Message::Binary(f.into())).await.unwrap();
    }

    async fn next(&mut self) -> Opened {
        loop {
            let f = recv_bin(&mut self.ws).await.expect("closed or silent");
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

    /// Pings and waits for the pong, past anything else; how long it took.
    async fn ping(&mut self, t: u64) -> Duration {
        let start = Instant::now();
        self.control(&Control::Ping { t }).await;
        loop {
            if let Opened::Control(Control::Pong { t: n }) = self.next().await
                && n == t
            {
                return start.elapsed();
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
            Control::PairConfirm { .. } => self.next_control().await,
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

    async fn hello(&mut self) {
        let w = self
            .reply(Request::Hello {
                proto: PROTOCOL,
                client: "phone".into(),
            })
            .await;
        assert!(
            matches!(
                w,
                Out::Reply {
                    reply: Reply::Welcome { .. }
                }
            ),
            "{w:?}"
        );
    }
}

/// A fresh device, paired.
async fn paired(d: &Daemon) -> (Keys, Device) {
    let keys = Keys::generate().unwrap();
    let mut dev = Device::connect(d.port, &keys).await;
    let code = d.code();
    assert_eq!(
        dev.pair(&code).await,
        Control::Paired {
            access: Access::Full
        }
    );
    dev.t.limit = dent_link::LARGE;
    (keys, dev)
}

fn wrong_code(right: &str) -> String {
    if right == "000000" {
        "111111".into()
    } else {
        "000000".into()
    }
}

// ---- pairing ----

#[tokio::test]
async fn a_spent_code_never_pairs_a_second_device() {
    let d = Daemon::start(&[]);
    let code = d.code();
    let mut first = Device::connect(d.port, &Keys::generate().unwrap()).await;
    assert_eq!(
        first.pair(&code).await,
        Control::Paired {
            access: Access::Full
        }
    );
    // Someone who read the same code over a shoulder, right after.
    let stranger = Keys::generate().unwrap();
    let mut second = Device::connect(d.port, &stranger).await;
    assert_eq!(
        second.pair(&code).await,
        Control::Refuse(Refusal::PairClosed)
    );
    drop(second);
    let again = Device::connect(d.port, &stranger).await;
    assert!(
        matches!(again.trust, Trust::Untrusted { .. }),
        "{:?}",
        again.trust
    );
}

/// After three wrong codes the daemon swaps the code and tells the device
/// "the machine shows a new one", but nothing tells the machine's screen:
/// the app only learns a code from the `OpenPairing` reply. So the owner
/// keeps reading out a code that can never work again.
#[tokio::test]
async fn after_three_wrong_codes_the_screen_shows_the_code_that_pairs() {
    let d = Daemon::start(&[]);
    let shown = d.code();
    let mut dev = Device::connect(d.port, &Keys::generate().unwrap()).await;
    let typo = wrong_code(&shown);
    assert_eq!(dev.pair(&typo).await, Control::Refuse(Refusal::PairWrong));
    assert_eq!(dev.pair(&typo).await, Control::Refuse(Refusal::PairWrong));
    assert_eq!(
        dev.pair(&typo).await,
        Control::Refuse(Refusal::PairWrongNewCode)
    );
    // The screen asks what's showing now (as the app does every couple of
    // seconds while a code shows): the new one, not the old.
    let now = match d.ask(Request::Pairing) {
        Reply::Pairing { code, .. } => code,
        other => panic!("unexpected {other:?}"),
    };
    assert!(!now.is_empty() && now != shown, "still showing {shown:?}");
    // The pause (1 s) passes; the person types the code on the screen.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(dev.pair(&shown).await, Control::Refuse(Refusal::PairWrong));
    assert_eq!(
        dev.pair(&now).await,
        Control::Paired {
            access: Access::Full
        }
    );
    // Used, it's no longer showing.
    match d.ask(Request::Pairing) {
        Reply::Pairing { code, .. } => assert_eq!(code, ""),
        other => panic!("unexpected {other:?}"),
    }
}

/// An attacker who drops each connection right after its guess, before the
/// verdict, must still spend a guess each time: otherwise a 6-digit code
/// falls to a script.
#[tokio::test]
async fn guesses_from_dropped_connections_still_count() {
    let d = Daemon::start(&[]);
    let code = d.code();
    let guess = wrong_code(&code);
    let mut spent = 0;
    let deadline = Instant::now() + Duration::from_secs(40);
    while spent < 10 {
        assert!(Instant::now() < deadline, "only {spent} guesses in 40 s");
        let mut dev = Device::connect(d.port, &Keys::generate().unwrap()).await;
        let mut p = dent_link::pair::Pairing::start(&guess);
        dev.control(&Control::Pair {
            spake: hex::encode(&p.outbound),
        })
        .await;
        match dev.next_control().await {
            Control::Pair { spake } => {
                let key = p.finish(&hex::decode(spake).unwrap()).unwrap();
                let mac =
                    dent_link::pair::confirm(&key, &dev.t.hash, dent_link::pair::Role::Client);
                dev.control(&Control::PairConfirm {
                    mac: hex::encode(mac),
                })
                .await;
                // Gone before the verdict.
                drop(dev);
                spent += 1;
            }
            Control::Refuse(Refusal::PairLocked { secs }) => {
                tokio::time::sleep(Duration::from_secs(secs)).await;
            }
            Control::Refuse(Refusal::Busy { .. }) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Control::Refuse(Refusal::PairClosed) => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    // Ten wrong guesses close the window: nothing more can even start,
    // however long one waits out a pause.
    tokio::time::sleep(Duration::from_secs(9)).await;
    let mut dev = Device::connect(d.port, &Keys::generate().unwrap()).await;
    assert_eq!(dev.pair(&guess).await, Control::Refuse(Refusal::PairClosed));
}

// ---- framing ----

/// Garbage, oversized and truncated frames, and a stalled handshake, each
/// cost only their own connection: another paired device keeps getting its
/// pongs throughout, the daemon stays up, and the offender's trust is kept.
#[tokio::test]
async fn bad_frames_from_one_device_never_hurt_another() {
    let mut d = Daemon::start(&[]);
    let (_, mut good) = paired(&d).await;
    let (bad_keys, mut bad) = paired(&d).await;

    // Not Noise at all.
    assert!(bad.send_frame(vec![0xde, 0xad, 0xbe, 0xef]).await);
    assert!(good.ping(1).await < Duration::from_secs(1));
    // A truncated frame: a real one cut short.
    let mut bad = Device::connect(d.port, &bad_keys).await;
    assert!(matches!(bad.trust, Trust::Paired { .. }), "{:?}", bad.trust);
    let f = bad.t.seal_control(&Control::Ping { t: 5 }).unwrap();
    bad.send_frame(f[..f.len() / 2].to_vec()).await;
    assert!(good.ping(2).await < Duration::from_secs(1));
    // One WebSocket message bigger than any Noise message.
    let mut bad = Device::connect(d.port, &bad_keys).await;
    bad.send_frame(vec![7u8; 70_000]).await;
    assert!(good.ping(3).await < Duration::from_secs(1));
    // A whole data message over the trusted limit, part by part.
    let mut bad = Device::connect(d.port, &bad_keys).await;
    let huge = vec![b'x'; dent_link::LARGE + 1];
    let mut i = 0;
    while let Some(p) = bad.t.seal_part(&huge, i).unwrap() {
        if !bad.send_frame(p).await {
            break;
        }
        i += 1;
    }
    assert!(good.ping(4).await < Duration::from_secs(1));
    // A stranger sending more than allowed before trust.
    let mut stranger = Device::connect(d.port, &Keys::generate().unwrap()).await;
    for f in stranger
        .t
        .seal_data(&vec![b'y'; dent_link::SMALL + 1])
        .unwrap()
    {
        if !stranger.send_frame(f).await {
            break;
        }
    }
    assert!(good.ping(5).await < Duration::from_secs(1));
    // Sockets that open and never finish the handshake.
    let mut idle = Vec::new();
    for _ in 0..20 {
        if let Ok(s) = tokio::net::TcpStream::connect(("127.0.0.1", d.port)).await {
            idle.push(s);
        }
    }
    assert!(good.ping(6).await < Duration::from_secs(1));

    assert!(d.alive(), "the daemon died");
    // The offender is still paired, and served.
    drop(idle);
    let mut back = Device::connect(d.port, &bad_keys).await;
    assert!(
        matches!(back.trust, Trust::Paired { .. }),
        "{:?}",
        back.trust
    );
    back.t.limit = dent_link::LARGE;
    back.hello().await;
}

/// A device that asks for a lot and then stops reading holds up nobody
/// else: another device's pongs and live events keep coming.
#[tokio::test]
async fn a_device_that_stops_reading_never_stalls_the_others() {
    let d = Daemon::start(&[("DENT_CLAUDE", &fake_claude())]);
    let many = d.state.parent().unwrap().join("many");
    std::fs::create_dir_all(&many).unwrap();
    for i in 0..3000 {
        std::fs::write(many.join(format!("file-{i:04}-{}", "x".repeat(60))), "").unwrap();
    }
    let (_, mut stuck) = paired(&d).await;
    stuck.hello().await;
    stuck
        .reply(Request::Subscribe {
            after: 0,
            epoch: None,
        })
        .await;
    let (_, mut good) = paired(&d).await;
    good.hello().await;
    good.reply(Request::Subscribe {
        after: 0,
        epoch: None,
    })
    .await;
    // Asks for far more than its socket buffers hold, then never reads.
    for _ in 0..200 {
        stuck
            .request(Request::FilesIn {
                dir: many.display().to_string(),
            })
            .await;
    }
    // Live work streams to everyone.
    let work = d.state.parent().unwrap().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let created = good
        .reply(Request::Create {
            agent: None,
            cwd: work.display().to_string(),
            worktree: None,
            model: None,
            permission: Default::default(),
            effort: None,
            id: "m1".into(),
            text: "long slowly".into(),
            attachments: Vec::new(),
            files: Vec::new(),
            bring: None,
        })
        .await;
    assert!(
        matches!(
            created,
            Out::Reply {
                reply: Reply::Created { .. }
            }
        ),
        "{created:?}"
    );
    let start = Instant::now();
    let mut live = 0;
    let mut last = Instant::now();
    let mut worst = Duration::ZERO;
    let mut t = 100;
    while start.elapsed() < Duration::from_secs(6) {
        t += 1;
        good.control(&Control::Ping { t }).await;
        let sent = Instant::now();
        loop {
            match tokio::time::timeout(Duration::from_secs(2), good.next())
                .await
                .expect("the other device heard nothing for 2 s")
            {
                Opened::Control(Control::Pong { t: n }) if n == t => break,
                Opened::Data(_) => {
                    live += 1;
                    worst = worst.max(last.elapsed());
                    last = Instant::now();
                }
                _ => {}
            }
        }
        assert!(
            sent.elapsed() < Duration::from_secs(1),
            "pong took {:?}",
            sent.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(live >= 5, "live messages while the other stalled: {live}");
    assert!(worst < Duration::from_secs(2), "longest live gap {worst:?}");
    drop(stuck);
}

/// Many devices at once: seventy pair one after another (more than every
/// cap on connections in progress and untrusted ones, so a slot that's
/// never given back would show), all stay connected and answered, then
/// all drop and come back together.
#[tokio::test]
async fn seventy_devices_pair_stay_answered_and_reconnect_together() {
    let mut d = Daemon::start(&[]);
    let mut devs = Vec::new();
    for _ in 0..70 {
        devs.push(paired(&d).await);
    }
    let pings = devs
        .iter_mut()
        .enumerate()
        .map(|(i, (_, dev))| async move { dev.ping(i as u64 + 1).await });
    let took = tokio::time::timeout(
        Duration::from_secs(10),
        futures_util::future::join_all(pings),
    )
    .await
    .expect("not every device was answered");
    assert!(took.iter().all(|t| *t < Duration::from_secs(3)), "{took:?}");
    // The storm: everyone at once.
    let keys: Vec<Keys> = devs.into_iter().map(|(k, _)| k).collect();
    let port = d.port;
    let back = futures_util::future::join_all(
        keys.iter()
            .map(|k| async move { Device::connect(port, k).await.trust }),
    )
    .await;
    assert!(
        back.iter().all(|t| matches!(t, Trust::Paired { .. })),
        "{back:?}"
    );
    assert!(d.alive());
}

// ---- versions ----

/// A newer app whose range still covers this daemon is served, not
/// refused: the main protocol is a range so new clients reach old daemons.
#[tokio::test]
async fn a_newer_client_whose_range_overlaps_is_served() {
    let d = Daemon::start(&[]);
    let keys = Keys::generate().unwrap();
    let newer = hello("phone", PROTOCOL_MIN, PROTOCOL + 50);
    let mut dev = Device::connect_as(d.port, &keys, &newer).await;
    let code = d.code();
    assert_eq!(
        dev.pair(&code).await,
        Control::Paired {
            access: Access::Full
        }
    );
    let w = dev
        .reply(Request::Hello {
            proto: PROTOCOL + 50,
            client: "phone".into(),
        })
        .await;
    let Out::Reply {
        reply: Reply::Welcome { proto, .. },
    } = w
    else {
        panic!("a newer client was refused: {w:?}")
    };
    assert_eq!(proto, PROTOCOL);
}

// ---- resends ----

/// The same create and send arriving twice (the first link cut before its
/// answer, the client resending on the next) run once: one session, one
/// first message, one copy of each carried file.
#[tokio::test]
async fn a_resent_create_and_send_run_once() {
    let d = Daemon::start(&[("DENT_CLAUDE", &fake_claude())]);
    let work = d.state.parent().unwrap().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let create = Request::Create {
        agent: None,
        cwd: work.display().to_string(),
        worktree: None,
        model: None,
        permission: Default::default(),
        effort: None,
        id: "c1".into(),
        text: "hello".into(),
        attachments: Vec::new(),
        files: vec![dent_protocol::Carried {
            name: "note.txt".into(),
            bytes_b64: dent_protocol::b64::encode(b"carried"),
        }],
        bring: None,
    };
    // Cut off right after asking.
    let (keys, mut first) = paired(&d).await;
    first.hello().await;
    first.request(create.clone()).await;
    drop(first);
    // The resend.
    let mut dev = Device::connect(d.port, &keys).await;
    dev.t.limit = dent_link::LARGE;
    dev.hello().await;
    let Out::Reply {
        reply: Reply::Created { session },
    } = dev.reply(create.clone()).await
    else {
        panic!("the resend wasn't answered with its session")
    };
    let send = Request::Send {
        session: session.clone(),
        id: "s1".into(),
        text: "again".into(),
        steer: false,
        attachments: Vec::new(),
        files: Vec::new(),
    };
    let a = dev.reply(send.clone()).await;
    drop(dev);
    let mut dev = Device::connect(d.port, &keys).await;
    dev.t.limit = dent_link::LARGE;
    dev.hello().await;
    let b = dev.reply(send).await;
    assert!(
        matches!(a, Out::Reply { .. }) && matches!(b, Out::Reply { .. }),
        "a resend was answered as a failure: {a:?} / {b:?}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    let events = d.events();
    let created = events
        .iter()
        .filter(|e| matches!(e.body, Body::Created { .. }))
        .count();
    let users = |id: &str| {
        events
            .iter()
            .filter(|e| matches!(&e.body, Body::User { id: i, .. } if i == id))
            .count()
    };
    assert_eq!(created, 1, "sessions made");
    assert_eq!(users("c1"), 1, "first message logged");
    assert_eq!(users("s1"), 1, "second message logged");
    let carried = d.state.join("attachments").join("carried").join("c1");
    let n = std::fs::read_dir(&carried).map(|r| r.count()).unwrap_or(0);
    assert_eq!(n, 1, "carried files in {}", carried.display());
}

// ---- the client side against real daemons ----

async fn until<T>(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<dent_remote::Event>,
    secs: u64,
    mut f: impl FnMut(dent_remote::Event) -> Option<T>,
) -> Option<T> {
    tokio::time::timeout(Duration::from_secs(secs), async {
        loop {
            if let Some(t) = f(rx.recv().await.expect("the client stopped")) {
                return t;
            }
        }
    })
    .await
    .ok()
}

/// The paired machine goes away and another dent machine answers on its
/// address. The client must flag it and never hand it the request it was
/// holding (a prompt meant for your own machine).
#[tokio::test]
async fn an_impostor_on_a_known_address_gets_flagged_and_nothing_sent() {
    use dent_remote::Event;
    let mut d = Daemon::start(&[]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let r = dent_remote::spawn(
        &tokio::runtime::Handle::current(),
        Keys::generate().unwrap(),
        "test",
        dent_remote::Saved {
            routes: vec![format!("127.0.0.1:{}", d.port)],
            ..Default::default()
        },
        move |e| {
            let _ = tx.send(e);
        },
    );
    until(&mut rx, 20, |e| {
        matches!(
            e,
            Event::State(dent_client_core::machine::State::NeedsCode { .. })
        )
        .then_some(())
    })
    .await
    .expect("never asked for a code");
    r.pair(&d.code());
    until(&mut rx, 20, |e| {
        matches!(e, Event::Welcome { .. }).then_some(())
    })
    .await
    .expect("no welcome");
    // Our machine goes; another takes its address.
    d.signal(libc::SIGKILL);
    d.reap();
    let _other = Daemon::start_on(d.port, &[]);
    r.send(
        10,
        Request::Rename {
            session: "nope".into(),
            title: "secret prompt".into(),
        },
    );
    let flagged = until(&mut rx, 20, |e| match e {
        Event::DifferentMachine(_) => Some(true),
        Event::Out { req: 10, .. } => Some(false),
        Event::Welcome { .. } => Some(false),
        _ => None,
    })
    .await;
    assert_eq!(
        flagged,
        Some(true),
        "the other machine wasn't flagged first"
    );
    // Nothing reaches it, however long the client keeps trying.
    let leaked = until(&mut rx, 5, |e| {
        matches!(e, Event::Out { req: 10, .. } | Event::Welcome { .. }).then_some(())
    })
    .await;
    assert!(leaked.is_none(), "the request went to another machine");
    r.stop();
}

/// A create with a file of this device's is cut off by the machine
/// crashing, the file is then deleted here, and the machine comes back:
/// the resend still carries the file (read once, kept), and the daemon
/// makes one session, not two.
#[tokio::test]
async fn a_request_cut_off_by_a_crash_is_resent_once_with_its_file() {
    use dent_remote::Event;
    let fake = fake_claude();
    let mut d = Daemon::start(&[("DENT_CLAUDE", &fake)]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let r = dent_remote::spawn(
        &tokio::runtime::Handle::current(),
        Keys::generate().unwrap(),
        "test",
        dent_remote::Saved {
            routes: vec![format!("127.0.0.1:{}", d.port)],
            ..Default::default()
        },
        move |e| {
            let _ = tx.send(e);
        },
    );
    until(&mut rx, 20, |e| {
        matches!(
            e,
            Event::State(dent_client_core::machine::State::NeedsCode { .. })
        )
        .then_some(())
    })
    .await
    .expect("never asked for a code");
    r.pair(&d.code());
    until(&mut rx, 20, |e| {
        matches!(e, Event::Welcome { .. }).then_some(())
    })
    .await
    .expect("no welcome");
    let here = d.state.parent().unwrap().join("on this device");
    std::fs::create_dir_all(&here).unwrap();
    let file = here.join("plan é.md");
    std::fs::write(&file, b"the plan").unwrap();
    let work = d.state.parent().unwrap().join("work");
    std::fs::create_dir_all(&work).unwrap();
    r.send(
        10,
        Request::Create {
            agent: None,
            cwd: work.display().to_string(),
            worktree: None,
            model: None,
            permission: Default::default(),
            effort: None,
            id: "x1".into(),
            text: "hello".into(),
            attachments: vec![file.display().to_string()],
            files: Vec::new(),
            bring: None,
        },
    );
    // Crash at once, answer or not.
    tokio::time::sleep(Duration::from_millis(30)).await;
    d.signal(libc::SIGKILL);
    d.reap();
    std::fs::remove_file(&file).unwrap();
    let answered_before = matches!(rx.try_recv(), Ok(Event::Out { req: 10, .. }));
    d.spawn(&[("DENT_CLAUDE", &fake)]);
    if !answered_before {
        let out = until(&mut rx, 30, |e| match e {
            Event::Out { req: 10, out } => Some(out),
            _ => None,
        })
        .await
        .expect("the request was never answered after the restart");
        assert!(
            matches!(
                out,
                Out::Reply {
                    reply: Reply::Created { .. }
                }
            ),
            "{out:?}"
        );
    }
    let events = d.events();
    let created = events
        .iter()
        .filter(|e| matches!(e.body, Body::Created { .. }))
        .count();
    assert_eq!(created, 1, "sessions made");
    let carried = d.state.join("attachments").join("carried").join("x1");
    let got: Vec<_> = std::fs::read_dir(&carried)
        .map(|r| r.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(std::fs::read(&got[0]).unwrap(), b"the plan");
    r.stop();
}

/// A stand-in for Tailscale's LocalAPI: this machine is user 7's node, and
/// `owner` says who owns every peer. Notices go to whoever is watching.
struct FakeTailscale {
    addr: String,
    owner: std::sync::Arc<std::sync::Mutex<Owner>>,
    /// Peer lookups asked of it so far.
    lookups: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    watchers: std::sync::Arc<std::sync::Mutex<Vec<std::net::TcpStream>>>,
}

#[derive(Clone, Copy)]
enum Owner {
    User(u64),
    /// Tailscale fails to answer.
    Failing,
    /// Just restarted: not running yet, and it knows no peers.
    Restarting,
}

impl FakeTailscale {
    fn start() -> Self {
        use std::sync::atomic::Ordering;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let owner = std::sync::Arc::new(std::sync::Mutex::new(Owner::User(7)));
        let lookups = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let watchers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (o, n, w) = (owner.clone(), lookups.clone(), watchers.clone());
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                let mut line = String::new();
                let mut r = BufReader::new(s.try_clone().unwrap());
                if r.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let ok = |body: &str| format!("HTTP/1.0 200 OK\r\n\r\n{body}");
                let owner = *o.lock().unwrap();
                let answer = if path.starts_with("/localapi/v0/status") {
                    let state = match owner {
                        Owner::Restarting => "Starting",
                        _ => "Running",
                    };
                    ok(&format!(
                        r#"{{"BackendState":"{state}","Self":{{"UserID":7,"TailscaleIPs":["100.64.0.1"],"DNSName":"studio.ts.net.","Tags":null}}}}"#
                    ))
                } else if path.starts_with("/localapi/v0/whois") {
                    n.fetch_add(1, Ordering::SeqCst);
                    match owner {
                        Owner::User(u) => ok(&format!(
                            r#"{{"Node":{{"User":{u},"Name":"phone.ts.net.","Tags":null}}}}"#
                        )),
                        Owner::Failing => "HTTP/1.0 500 Internal Server Error\r\n\r\n".into(),
                        Owner::Restarting => "HTTP/1.0 404 Not Found\r\n\r\n".into(),
                    }
                } else if path.starts_with("/localapi/v0/watch-ipn-bus") {
                    let _ =
                        s.write_all(b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n");
                    w.lock().unwrap().push(s);
                    continue;
                } else {
                    "HTTP/1.0 404 Not Found\r\n\r\n".into()
                };
                let _ = s.write_all(answer.as_bytes());
            }
        });
        Self {
            addr,
            owner,
            lookups,
            watchers,
        }
    }

    /// Tailscale's answers become `o`, and it tells its watchers something
    /// changed. Returns once the daemon has looked the peer up again.
    async fn change(&self, o: Owner) {
        use std::sync::atomic::Ordering;
        *self.owner.lock().unwrap() = o;
        let before = self.lookups.load(Ordering::SeqCst);
        for s in self.watchers.lock().unwrap().iter_mut() {
            let _ = s.write_all(b"{\"PeersChanged\":[{}]}\n");
        }
        let until = Instant::now() + Duration::from_secs(25);
        while self.lookups.load(Ordering::SeqCst) == before {
            assert!(Instant::now() < until, "no check 25 s after the notice");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// This machine's address on its network: not loopback, so a connection
/// from it is a route the daemon asks Tailscale about.
fn own_address() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // No packet is sent: connecting picks the route and its address.
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_leaves_the_account_is_dropped_when_tailscale_says_so() {
    let Some(host) = own_address() else {
        eprintln!("skipped: no network address but loopback");
        return;
    };
    let ts = FakeTailscale::start();
    let d = Daemon::start(&[("DENT_TAILSCALE_LOCALAPI", ts.addr.as_str())]);
    let keys = Keys::generate().unwrap();
    let mut dev = Device::connect_at(
        &host,
        d.port,
        &keys,
        &hello("phone", PROTOCOL_MIN, PROTOCOL),
    )
    .await;
    assert!(
        matches!(dev.trust, Trust::Tailscale { .. }),
        "{:?}",
        dev.trust
    );
    dev.hello().await;
    // Uncertainty never drops anyone: Tailscale failing to answer, or just
    // restarted and knowing no peers yet. Each is checked, and the device
    // stays.
    for o in [Owner::Failing, Owner::Restarting] {
        ts.change(o).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        dev.ping(1).await;
    }
    // The phone moves to another account: gone well before the 5-minute
    // check, told why.
    ts.change(Owner::User(8)).await;
    let c = tokio::time::timeout(Duration::from_secs(5), dev.next_control())
        .await
        .expect("still connected after the check");
    assert_eq!(c, Control::Refuse(Refusal::NotTrusted));
}
