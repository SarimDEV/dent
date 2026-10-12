//! Edge cases: what happens when machines go away, come back wrong, or are
//! attacked. Part one drives the protocol directly; part two drives the
//! device manager (`devices`), the same code the Settings screen uses, and
//! checks what a person would see.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex, mpsc};

use crate::client::{self, Note, PairError, Tally};
use crate::devices::{self, Cmd, Device, Devices, Options, ReplyState, Snapshot, Status};
use crate::harness::{
    Daemon, Report, Spec, code, collect, free_port, is_end, lan_ip, signal, spawn,
};
use crate::identity::{Identity, Peer, Trust, fingerprint};
use crate::proto::{Event, new_id};

const ME: &str = "you@example.com";

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

fn log(state: &Path, session: &str) -> Vec<Event> {
    std::fs::read_to_string(state.join(format!("sessions/{session}.jsonl")))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn reply_text(events: &[Event]) -> String {
    events
        .iter()
        .filter(|e| e.kind == "delta")
        .map(|e| e.text.as_str())
        .collect()
}

fn prompts_run(state: &Path) -> usize {
    std::fs::read_to_string(state.join("prompts"))
        .unwrap_or_default()
        .lines()
        .count()
}

/// Wait for a pairing code file, as a person waits to read it.
async fn wait_code(state: &Path) -> String {
    for _ in 0..100 {
        let c = code(state);
        if c.len() == 6 {
            return c;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    String::new()
}

pub async fn run() -> Result<()> {
    let root = std::env::temp_dir().join(format!("dent-edges-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    let mut r = Report { rows: Vec::new() };
    protocol(&root, &mut r).await?;
    manager(&root, &mut r).await?;
    let ok = r.rows.iter().filter(|r| r.0).count();
    println!("{ok}/{} passed", r.rows.len());
    crate::harness::stop_holders(&root);
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

// ─── Part one: the protocol ───────────────────────────────────────────────

async fn protocol(root: &Path, r: &mut Report) -> Result<()> {
    let me_dir = root.join("laptop");
    let me = Identity::load(&me_dir, "laptop")?;
    let mut studio = Spec {
        tailnet_port: Some(free_port()),
        sim: Some((ME, ME)),
        mdns: false,
        ..Spec::new("studio", root.join("studio"))
    };
    let mut b = spawn(&studio).await?;

    // Pairing is paused after three wrong codes, even for the right one.
    let mut o = client::open(b.lan, &me).await?;
    let first = client::pair(&mut o.ch, "000001").await;
    let mut results = vec![first];
    for guess in ["000002", "000003"] {
        let mut o = client::open(b.lan, &me).await?;
        results.push(client::pair(&mut o.ch, guess).await);
    }
    let right = code(&b.state);
    let mut o = client::open(b.lan, &me).await?;
    let during = client::pair(&mut o.ch, &right).await;
    r.check(
        "Three wrong codes show a new code and pause pairing, even for the right code",
        results[..2].iter().all(|x| *x == Err(PairError::Wrong))
            && results[2] == Err(PairError::WrongNewCode)
            && matches!(during, Err(PairError::Locked(s)) if s >= 1),
        format!("{results:?}, then the right code: {during:?}"),
    );

    // Ten wrong codes close pairing until someone shows a new code.
    let t = Instant::now();
    let mut tries = 3;
    let mut last = Ok(());
    while tries < 20 {
        let mut o = client::open(b.lan, &me).await?;
        last = client::pair(&mut o.ch, "999999").await;
        match &last {
            Err(PairError::Locked(s)) => {
                tokio::time::sleep(Duration::from_secs(*s)).await;
                continue;
            }
            Err(PairError::Closed) => break,
            _ => tries += 1,
        }
    }
    let closed_after = tries + 1;
    let gone = code(&b.state).is_empty();
    signal(&b, "-HUP");
    let fresh = wait_code(&b.state).await;
    let mut o = client::open(b.lan, &me).await?;
    let reopened = client::pair(&mut o.ch, &fresh).await;
    let laptop_key = o.ch.remote.clone();
    let stranger = Identity::load(&root.join("stranger"), "stranger")?;
    let mut o = client::open(b.lan, &stranger).await?;
    let after_use = client::pair(&mut o.ch, &fresh).await;
    r.check(
        "Ten wrong codes close pairing; a newly shown code works once, then pairing closes",
        last == Err(PairError::Closed)
            && closed_after == 10
            && gone
            && reopened.is_ok()
            && after_use == Err(PairError::Closed),
        format!(
            "closed after {closed_after} wrong codes in {:.1} s (code hidden {gone}); new code pairs {:?}; same code again: {after_use:?}. At most 10 guesses per code shown: 1 in 100,000",
            t.elapsed().as_secs_f32(),
            reopened.is_ok()
        ),
    );
    Trust::load(&me_dir).add(
        &laptop_key,
        Peer {
            name: "studio".into(),
            key: hex::encode(&laptop_key),
            via: "code".into(),
            ..Default::default()
        },
    )?;

    // A shown code expires.
    let short = Spec {
        extra: vec!["--pair-ttl".into(), "2".into()],
        mdns: false,
        ..Spec::new("short", root.join("short"))
    };
    let s = spawn(&short).await?;
    let c = wait_code(&s.state).await;
    tokio::time::sleep(Duration::from_millis(2300)).await;
    let mut o = client::open(s.lan, &stranger).await?;
    let late = client::pair(&mut o.ch, &c).await;
    r.check(
        "A pairing code stops working after its time is up",
        late == Err(PairError::Closed),
        format!("right code after 2.3 s with a 2 s limit: {late:?}"),
    );
    drop(s);

    // Tailscale trust: live, tailnet-only, and never from a probe.
    let phone = Identity::load(&root.join("phone"), "phone")?;
    let tail = b.tailnet.context("tailnet")?;
    let on_tail = client::open(tail, &phone).await?;
    let on_lan = client::open(b.lan, &phone).await?;
    r.check(
        "Trust from your Tailscale account holds only on Tailscale, not on the LAN",
        on_tail.trusted && !on_lan.trusted && on_lan.via == "tailnet-only",
        format!(
            "tailnet: trusted {} via {}; LAN: trusted {} via {}",
            on_tail.trusted, on_tail.via, on_lan.trusted, on_lan.via
        ),
    );
    drop((on_tail, on_lan));
    let looker = Identity::load(&root.join("looker"), "looker")?;
    let p = client::probe(tail, &looker).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stored = Trust::load(&b.state).peers.contains_key(&looker.id());
    r.check(
        "Looking for machines never makes them trust you",
        p.trusted && !stored,
        format!(
            "probe welcomed as trusted {}, saved on the other side {stored}",
            p.trusted
        ),
    );
    drop(b);
    studio.sim = Some((ME, "someone@else.com"));
    b = spawn(&studio).await?;
    let left = client::open(tail, &phone).await?;
    r.check(
        "Leaving the Tailscale account ends that trust",
        !left.trusted,
        format!(
            "same key, now another account's: trusted {} via {}",
            left.trusted, left.via
        ),
    );
    drop((left, b));
    studio.sim = Some((ME, ME));
    b = spawn(&studio).await?;
    Trust::update(&b.state, |p| {
        if let Some(e) = p.get_mut(&phone.id()) {
            e.via = "removed".into();
        }
    })?;
    let back = client::open(tail, &phone).await?;
    r.check(
        "A Tailscale device removed on the other machine stays out, though still on your account",
        !back.trusted,
        format!("trusted {} via {}", back.trusted, back.via),
    );
    drop(back);

    // Unpaired peers learn nothing about the machine.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s = client::open(b.lan, &stranger).await?;
    let mine = client::open(b.lan, &me).await?;
    r.check(
        "Only paired devices learn what the machine is",
        s.info == Default::default() && !mine.info.os.is_empty(),
        format!(
            "stranger sees os {:?}, agents {}; paired sees {} {} with {:?}",
            s.info.os,
            s.info.agents.len(),
            mine.info.os,
            mine.info.os_version,
            mine.info
                .agents
                .iter()
                .map(|a| format!("{} {}", a.name, a.version.clone().unwrap_or_default()))
                .collect::<Vec<_>>()
        ),
    );
    drop((s, mine));

    // A flood of silent connections from one machine doesn't lock out
    // another, and they're closed.
    let local = SocketAddr::from(([127, 0, 0, 1], b.lan.port()));
    let mut silent = Vec::new();
    for _ in 0..200 {
        if let Ok(c) = tokio::net::TcpStream::connect(local).await {
            silent.push(c);
        }
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let t = Instant::now();
    let ok = client::open(b.lan, &me)
        .await
        .map(|o| o.trusted)
        .unwrap_or(false);
    let open_ms = ms(t.elapsed());
    let mut closed_now = 0;
    for c in silent.iter_mut() {
        let mut buf = [0u8; 1];
        if let Ok(Ok(0)) = tokio::time::timeout(Duration::from_millis(5), c.read(&mut buf)).await {
            closed_now += 1;
        }
    }
    tokio::time::sleep(Duration::from_millis(5500)).await;
    let mut closed_later = 0;
    for c in silent.iter_mut() {
        let mut buf = [0u8; 1];
        if matches!(
            tokio::time::timeout(Duration::from_millis(5), c.read(&mut buf)).await,
            Ok(Ok(0)) | Ok(Err(_))
        ) {
            closed_later += 1;
        }
    }
    r.check(
        "200 silent connections from one machine don't lock out another, and are closed",
        ok && open_ms < 1000 && closed_now >= 190 && closed_later == silent.len(),
        format!(
            "paired device connected in {open_ms} ms during the flood; {closed_now} of {} closed at once (8 allowed per address), all {closed_later} closed within 5.5 s",
            silent.len()
        ),
    );
    drop(silent);

    // An oversized message before pairing is refused.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/", b.lan)).await?;
    use futures_util::{SinkExt, StreamExt};
    let big = vec![7u8; 1 << 20];
    let _ = ws
        .send(tokio_tungstenite::tungstenite::Message::Binary(big.into()))
        .await;
    let end = tokio::time::timeout(Duration::from_secs(2), ws.next()).await;
    let refused = !matches!(
        end,
        Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(_))))
    );
    let still = client::open(b.lan, &me).await.is_ok();
    r.check(
        "A 1 MB message from an unpaired peer is refused and the daemon carries on",
        refused && still,
        format!("connection ended: {refused}; daemon still answers: {still}"),
    );

    // Addresses that never answer fail fast, and don't hold up good ones.
    let mute = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let mute_addr = mute.local_addr()?;
    let _hold = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = mute.accept().await {
            held.push(s);
        }
    });
    let hole: SocketAddr = "192.0.2.1:7480".parse()?;
    let t = Instant::now();
    let a = client::open(hole, &me).await.is_err();
    let hole_ms = ms(t.elapsed());
    let t = Instant::now();
    let m = client::open(mute_addr, &me).await.is_err();
    let mute_ms = ms(t.elapsed());
    let t = Instant::now();
    let any = client::open_any(&[hole, mute_addr, b.lan], &me, None).await;
    let any_ms = ms(t.elapsed());
    r.check(
        "An address that never answers fails in 3 s, not a minute, and doesn't delay a good one",
        a && m && hole_ms < 3300 && mute_ms < 3300 && any.is_ok() && any_ms < 1000,
        format!(
            "no route: {hole_ms} ms; accepts but never speaks: {mute_ms} ms; good address listed third connected in {any_ms} ms"
        ),
    );
    drop(any);

    // Versions that don't overlap are named, not mysterious failures.
    let old = Spec {
        extra: vec!["--proto".into(), "1".into()],
        mdns: false,
        ..Spec::new("old", root.join("old"))
    };
    let od = spawn(&old).await?;
    let o = client::open(od.lan, &me).await?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let lan = od.lan;
    let f = client::follow(
        me.clone(),
        o.ch.remote.clone(),
        Arc::new(move || vec![lan]),
        "_status".into(),
        0,
        Arc::new(Mutex::new(Tally::default())),
        tx,
    );
    let notes = collect(&mut rx, Duration::from_secs(3), |n| {
        matches!(n, Note::Incompatible { .. })
    })
    .await;
    let named = notes
        .iter()
        .any(|(_, n)| matches!(n, Note::Incompatible { proto: 1, .. }));
    r.check(
        "A machine running an incompatible dent shows as needing an update",
        !o.compatible() && named,
        format!(
            "theirs {}..{}, ours {}..{}; follower reported it: {named}",
            o.min,
            o.proto,
            crate::proto::PROTO_MIN,
            crate::proto::PROTO
        ),
    );
    f.abort();
    drop(od);

    // A retried prompt runs once.
    let key = client::open(b.lan, &me).await?.ch.remote.clone();
    let (id, session) = (new_id(), new_id());
    let one = client::prompt(&me, &key, &[b.lan], &id, &session, "once please").await?;
    let two = client::prompt(&me, &key, &[b.lan], &id, &session, "once please").await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let users = log(&b.state, &session)
        .iter()
        .filter(|e| e.kind == "user")
        .count();
    r.check(
        "A prompt sent twice (a retry after a lost reply) runs once",
        !one.1 && two.1 && users == 1,
        format!(
            "first dup {}, second dup {}, prompts logged {users}",
            one.1, two.1
        ),
    );

    // Two devices watching the same reply both get all of it.
    let desk = Identity::load(&root.join("desk"), "desk")?;
    Trust::load(&b.state).add(
        &desk.public,
        Peer {
            name: "desk".into(),
            key: hex::encode(&desk.public),
            via: "code".into(),
            ..Default::default()
        },
    )?;
    let session = new_id();
    let mut watchers = Vec::new();
    for who in [me.clone(), desk.clone()] {
        let (tx, rx) = mpsc::unbounded_channel();
        let lan = b.lan;
        let tally = Arc::new(Mutex::new(Tally::default()));
        let f = client::follow(
            who,
            key.clone(),
            Arc::new(move || vec![lan]),
            session.clone(),
            0,
            tally.clone(),
            tx,
        );
        watchers.push((f, rx, tally));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    client::prompt(&me, &key, &[b.lan], &new_id(), &session, "both of you").await?;
    let mut texts = Vec::new();
    for (f, rx, tally) in watchers.iter_mut() {
        let notes = collect(rx, Duration::from_secs(10), is_end).await;
        let t: String = notes
            .iter()
            .filter_map(|(_, n)| match n {
                Note::Event(e) if e.kind == "delta" => Some(e.text.clone()),
                _ => None,
            })
            .collect();
        let tl = tally.lock().await.clone();
        texts.push((t, tl.gaps + tl.dups));
        f.abort();
    }
    let logged = reply_text(&log(&b.state, &session));
    r.check(
        "Two devices watching the same reply both get every word",
        texts.iter().all(|(t, bad)| *t == logged && *bad == 0) && !logged.is_empty(),
        format!(
            "{} and {} characters of {}, no gaps or repeats",
            texts[0].0.len(),
            texts[1].0.len(),
            logged.len()
        ),
    );

    // A damaged trust file fails safe; a damaged key refuses to start.
    drop(b);
    std::fs::write(studio.state.join("trusted.json"), "{ not json")?;
    b = spawn(&studio).await?;
    let o = client::open(b.lan, &me).await?;
    let aside = std::fs::read_dir(&studio.state)?
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains("corrupt"));
    r.check(
        "A damaged trust file is set aside and nobody is trusted until paired again",
        !o.trusted && aside,
        format!(
            "daemon started; laptop trusted {}; damaged file kept {aside}",
            o.trusted
        ),
    );
    drop((o, b));
    let key_file = studio.state.join("identity.key");
    let good = std::fs::read_to_string(&key_file)?;
    std::fs::write(&key_file, &good[..40])?;
    let refused = spawn(&studio).await.is_err();
    std::fs::write(&key_file, good)?;
    r.check(
        "A damaged identity key stops the daemon instead of quietly becoming a new machine",
        refused,
        format!("daemon refused to start: {refused}"),
    );

    // Trust changes from many tasks at once all land.
    let dir = root.join("race");
    std::fs::create_dir_all(&dir)?;
    let handles: Vec<_> = (0..50)
        .map(|i| {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let k = Identity::load(&dir.join(format!("k{i}")), "k").expect("key");
                Trust::load(&dir).add(
                    &k.public,
                    Peer {
                        name: format!("k{i}"),
                        key: hex::encode(&k.public),
                        via: "code".into(),
                        ..Default::default()
                    },
                )
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
    let n = Trust::load(&dir).peers.len();
    r.check(
        "50 pairings saved at the same moment all stick",
        n == 50,
        format!("{n} of 50 in the trust file"),
    );

    // Wake-on-LAN packets are well formed.
    let sock = std::net::UdpSocket::bind("127.0.0.1:0")?;
    sock.set_read_timeout(Some(Duration::from_secs(1)))?;
    let to = sock.local_addr()?;
    let sent = crate::info::wake(&["ba:cf:2b:12:48:06".into()], &[to]);
    let mut buf = [0u8; 200];
    let got = sock.recv(&mut buf).unwrap_or(0);
    r.check(
        "Wake sends a correct magic packet",
        sent == 1
            && got == 102
            && buf[..6] == [0xFF; 6]
            && buf[96..102] == [0xba, 0xcf, 0x2b, 0x12, 0x48, 0x06],
        format!("{got} bytes: 6 × FF then the address 16 times"),
    );
    Ok(())
}

// ─── Part two: what the device manager shows ──────────────────────────────

/// The client app, on its own runtime so it can be quit abruptly.
struct App {
    devices: Devices,
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl App {
    fn start(dir: PathBuf, opts: Options) -> Result<App> {
        let (dtx, drx) = std::sync::mpsc::channel();
        let (stx, srx) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("runtime");
            let d = rt.block_on(async { devices::start_with(dir, "laptop", opts) });
            let _ = dtx.send(d);
            let _ = srx.recv();
            // Like the app being force-quit: nothing gets to finish.
            rt.shutdown_background();
        });
        let devices = drx.recv()??;
        Ok(App {
            devices,
            stop: Some(stx),
            thread: Some(thread),
        })
    }

    fn snap(&self) -> Snapshot {
        self.devices
            .state
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    fn dev(&self, id: &str) -> Option<Device> {
        self.snap().devices.into_iter().find(|d| d.id == id)
    }

    fn status(&self, id: &str) -> Option<Status> {
        self.dev(id).map(|d| d.status)
    }

    fn send(&self, c: Cmd) {
        let _ = self.devices.cmd.send(c);
    }

    /// Time until `f` holds, polling as a UI would redraw.
    async fn until(&self, limit: Duration, f: impl Fn(&Snapshot) -> bool) -> Option<Duration> {
        let t = Instant::now();
        while t.elapsed() < limit {
            if f(&self.snap()) {
                return Some(t.elapsed());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        None
    }

    fn quit(mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn opts() -> Options {
    Options {
        tailnet_peers: Vec::new(),
        wake_to: None,
        bonjour: true,
    }
}

fn status_is(s: &Snapshot, id: &str, f: impl Fn(&Status) -> bool) -> bool {
    s.devices.iter().any(|d| d.id == id && f(&d.status))
}

fn last_reply(s: &Snapshot) -> Option<devices::Reply> {
    s.replies.last().cloned()
}

/// Find, add and pair with a machine through the device manager.
async fn pair_with(app: &App, d: &Daemon, id: &str) -> Result<Duration> {
    let t = Instant::now();
    app.until(Duration::from_secs(10), |s| {
        s.devices.iter().any(|x| x.id == id)
    })
    .await
    .context("never found")?;
    app.send(Cmd::Add(id.into()));
    app.until(Duration::from_secs(5), |s| {
        status_is(s, id, |st| matches!(st, Status::NeedsCode { .. }))
    })
    .await
    .context("never asked for a code")?;
    let c = wait_code(&d.state).await;
    // Typed the way people do: "123 456".
    app.send(Cmd::Code(id.into(), format!("{} {}", &c[..3], &c[3..])));
    app.until(Duration::from_secs(8), |s| status_is(s, id, Status::online))
        .await
        .context("never came online")?;
    Ok(t.elapsed())
}

async fn manager(root: &Path, r: &mut Report) -> Result<()> {
    let dir = root.join("app");
    let studio = Spec {
        extra: vec!["--agent-ms".into(), "70".into()],
        ..Spec::new("studio", root.join("m-studio"))
    };
    let id = Identity::load(&studio.state, "studio")?.id();
    let mut b = spawn(&studio).await?;
    let app = App::start(dir.clone(), opts())?;
    let paired = pair_with(&app, &b, &id).await?;

    // What a machine tells you about itself.
    let got = app
        .until(Duration::from_secs(8), |s| {
            s.devices.iter().any(|d| {
                d.id == id
                    && d.health.is_some()
                    && d.rtt_ms.is_some()
                    && d.info.as_ref().is_some_and(|i| !i.os.is_empty())
            })
        })
        .await;
    let d = app.dev(&id).context("studio")?;
    let info = d.info.clone().unwrap_or_default();
    let h = d.health.clone().map(|h| h.0).unwrap_or_default();
    r.check(
        "A paired machine reports what it is and how it's doing",
        got.is_some() && h.mem_total > 0 && h.disk_total > 0,
        format!(
            "paired in {} ms (code typed as \"123 456\"); {} {} {} ({} cores), dent {}, agents {:?}, {} wake address(es); load {:.2}, memory {:.1}/{:.0} GB, disk {:.0} GB free, battery {:?}, round trip {:?} ms",
            ms(paired),
            info.os,
            info.os_version,
            info.model,
            info.cores,
            info.dent_version,
            info.agents.iter().map(|a| format!("{} {}", a.name, a.version.clone().unwrap_or_default())).collect::<Vec<_>>(),
            info.macs.len(),
            h.load,
            h.mem_used as f64 / 1e9,
            h.mem_total as f64 / 1e9,
            h.disk_free as f64 / 1e9,
            h.battery.as_ref().map(|b| b.percent),
            d.rtt_ms
        ),
    );

    // It goes to sleep mid-reply, then wakes.
    app.send(Cmd::Prompt(id.clone(), "sleep test".into()));
    app.until(Duration::from_secs(5), |s| {
        last_reply(s).is_some_and(|r| r.text.len() > 40)
    })
    .await;
    let before = prompts_run(&b.state);
    signal(&b, "-USR1");
    let t = Instant::now();
    let asleep = app
        .until(Duration::from_secs(5), |s| {
            status_is(s, &id, |st| matches!(st, Status::Asleep { .. }))
        })
        .await;
    let paused = last_reply(&app.snap()).map(|r| r.state);
    app.send(Cmd::Prompt(id.clone(), "while asleep".into()));
    app.until(Duration::from_secs(1), |s| s.replies.len() == 2)
        .await;
    let refused = last_reply(&app.snap()).map(|r| r.state);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let ran = prompts_run(&b.state) - before;
    signal(&b, "-CONT");
    let woke = Instant::now();
    let back = app
        .until(Duration::from_secs(8), |s| {
            status_is(s, &id, Status::online)
        })
        .await;
    let done = app
        .until(Duration::from_secs(10), |s| {
            s.replies
                .first()
                .is_some_and(|r| r.state == ReplyState::Done)
        })
        .await;
    let first = app.snap().replies.first().cloned().context("reply")?;
    let full = reply_text(&log(&b.state, &first.session));
    r.check(
        "A machine going to sleep shows asleep at once; its reply pauses and finishes on waking",
        asleep.is_some_and(|d| d < Duration::from_millis(1000))
            && matches!(&paused, Some(ReplyState::Paused(w)) if w.contains("asleep"))
            && back.is_some()
            && done.is_some()
            && first.text == full,
        format!(
            "asleep shown {:?} ms after it said so (a silent machine takes 3 s); reply: {paused:?}; back {:?} ms after waking; reply complete {} ({} of {} characters)",
            asleep.map(ms),
            back.map(|_| ms(woke.elapsed())),
            first.text == full,
            first.text.len(),
            full.len()
        ),
    );
    let _ = t;
    r.check(
        "A prompt for a machine that's away is refused on the spot, never queued",
        matches!(&refused, Some(ReplyState::NotSent(w)) if w.contains("asleep")) && ran == 0,
        format!("{refused:?}; prompts that ran there: {ran}"),
    );

    // dent crashes mid-reply: reconnecting, then offline. The agent runs on
    // in the session holder, so after a restart the reply finishes.
    app.send(Cmd::Prompt(id.clone(), "crash test".into()));
    app.until(Duration::from_secs(5), |s| {
        last_reply(s).is_some_and(|r| r.text.contains("crash test"))
    })
    .await;
    signal(&b, "-KILL");
    let killed = Instant::now();
    let recon = app
        .until(Duration::from_secs(6), |s| {
            status_is(s, &id, |st| matches!(st, Status::Reconnecting { .. }))
        })
        .await;
    let mid = last_reply(&app.snap()).map(|r| r.state);
    let off = app
        .until(Duration::from_secs(15), |s| {
            status_is(s, &id, |st| matches!(st, Status::Offline { .. }))
        })
        .await;
    let off_ms = ms(killed.elapsed());
    drop(b);
    b = spawn(&studio).await?;
    let up = app
        .until(Duration::from_secs(8), |s| {
            status_is(s, &id, Status::online)
        })
        .await;
    let ended = app
        .until(Duration::from_secs(10), |s| {
            last_reply(s).is_some_and(|r| r.state == ReplyState::Done)
        })
        .await;
    let (text, session) = last_reply(&app.snap())
        .map(|r| (r.text, r.session))
        .unwrap_or_default();
    let full = reply_text(&log(&b.state, &session));
    r.check(
        "A crash shows reconnecting, then offline after 10 s; the agent keeps going and the reply finishes after a restart",
        recon.is_some() && off.is_some() && up.is_some() && ended.is_some() && text == full && full.contains("crash test") && full.contains("left off."),
        format!(
            "reconnecting after {:?} ms (reply: {mid:?}); offline after {off_ms} ms; online {:?} ms after restart; reply finished whole: {}",
            recon.map(ms),
            up.map(ms),
            ended.is_some() && text == full
        ),
    );

    // The whole machine goes (dent and the holder with it, as in a power
    // cut): after a restart the reply shows as interrupted.
    app.send(Cmd::Prompt(id.clone(), "power cut".into()));
    app.until(Duration::from_secs(5), |s| {
        last_reply(s).is_some_and(|r| r.text.contains("power cut"))
    })
    .await;
    signal(&b, "-KILL");
    let holder = std::fs::read_to_string(crate::daemon::holder_dir(&b.state).join("holder.lock"))
        .unwrap_or_default();
    let _ = std::process::Command::new("kill")
        .args(["-KILL", holder.trim()])
        .status();
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(b);
    b = spawn(&studio).await?;
    let cut = app
        .until(Duration::from_secs(10), |s| {
            last_reply(s).is_some_and(|r| matches!(r.state, ReplyState::Interrupted(_)))
        })
        .await;
    r.check(
        "When the machine loses the agent too, the cut-off reply shows as interrupted",
        cut.is_some() && !holder.trim().is_empty(),
        format!(
            "interrupted shown {:?} ms after dent restarted (holder pid {})",
            cut.map(ms),
            holder.trim()
        ),
    );

    // dent quits, or restarts to update: said, not guessed.
    signal(&b, "-USR2");
    let restarting = app
        .until(Duration::from_secs(3), |s| {
            status_is(s, &id, |st| {
                matches!(
                    st,
                    Status::Stopped {
                        restarting: true,
                        ..
                    }
                )
            })
        })
        .await;
    drop(b);
    b = spawn(&studio).await?;
    app.until(Duration::from_secs(8), |s| {
        status_is(s, &id, Status::online)
    })
    .await;
    signal(&b, "-TERM");
    let quit = app
        .until(Duration::from_secs(3), |s| {
            status_is(s, &id, |st| {
                matches!(
                    st,
                    Status::Stopped {
                        restarting: false,
                        ..
                    }
                )
            })
        })
        .await;
    let held = {
        tokio::time::sleep(Duration::from_secs(4)).await;
        app.status(&id)
    };
    r.check(
        "Quitting and restarting are shown as such, and don't decay into offline",
        restarting.is_some() && quit.is_some() && matches!(held, Some(Status::Stopped { .. })),
        format!(
            "restarting shown after {:?} ms; quit shown after {:?} ms; 4 s later: {held:?}",
            restarting.map(ms),
            quit.map(ms)
        ),
    );
    drop(b);

    // The app starts while the machine is off: offline with last seen, at once.
    let seen_before = app.dev(&id).and_then(|d| d.last_seen);
    app.quit();
    let t = Instant::now();
    let app = App::start(dir.clone(), opts())?;
    let shown = app
        .until(Duration::from_secs(5), |s| {
            status_is(s, &id, |st| matches!(st, Status::Offline { .. }))
        })
        .await;
    let d = app.dev(&id).context("studio")?;
    r.check(
        "Opening the app while a machine is off shows it offline with when it was last seen",
        shown.is_some() && d.last_seen.is_some() && d.last_seen == seen_before,
        format!(
            "offline shown {} ms after start, last seen {} s ago (kept from before the restart)",
            ms(t.elapsed()),
            d.last_seen
                .map(|l| (crate::proto::now_ms() - l) / 1000)
                .unwrap_or(0)
        ),
    );

    // The app quits mid-reply, then reopens: the reply resumes from cache.
    b = spawn(&studio).await?;
    app.until(Duration::from_secs(8), |s| {
        status_is(s, &id, Status::online)
    })
    .await;
    app.send(Cmd::Prompt(id.clone(), "resume test".into()));
    app.until(Duration::from_secs(5), |s| {
        last_reply(s).is_some_and(|r| r.text.len() > 60)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let partial = last_reply(&app.snap()).map(|r| r.text.len()).unwrap_or(0);
    app.quit();
    let app = App::start(dir.clone(), opts())?;
    let cached = last_reply(&app.snap()).map(|r| r.text.len()).unwrap_or(0);
    let done = app
        .until(Duration::from_secs(10), |s| {
            last_reply(s).is_some_and(|r| r.state == ReplyState::Done)
        })
        .await;
    let rp = last_reply(&app.snap()).context("reply")?;
    let full = reply_text(&log(&b.state, &rp.session));
    r.check(
        "Quitting the app mid-reply loses nothing: reopened, the reply shows at once and finishes",
        cached > 0 && done.is_some() && rp.text == full,
        format!(
            "{partial} characters when quit (saved {cached}, shown on reopening before any network); finished with {} of {} characters, no repeats",
            rp.text.len(),
            full.len()
        ),
    );

    // The app is closed for the whole reply; it's all there on return.
    app.send(Cmd::Prompt(id.clone(), "away test".into()));
    app.until(Duration::from_secs(5), |s| {
        last_reply(s).is_some_and(|r| r.state == ReplyState::Running)
    })
    .await;
    let session = last_reply(&app.snap())
        .map(|r| r.session)
        .unwrap_or_default();
    app.quit();
    for _ in 0..200 {
        if log(&b.state, &session)
            .last()
            .is_some_and(|e| e.kind == "end")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let t = Instant::now();
    let app = App::start(dir.clone(), opts())?;
    let caught = app
        .until(Duration::from_secs(5), |s| {
            last_reply(s).is_some_and(|r| r.state == ReplyState::Done)
        })
        .await;
    let rp = last_reply(&app.snap()).context("reply")?;
    r.check(
        "With the app closed, the agent keeps going there; reopening catches up the whole reply",
        caught.is_some() && rp.text == reply_text(&log(&b.state, &session)),
        format!(
            "finished reply caught up {} ms after reopening",
            ms(t.elapsed())
        ),
    );

    // The machine freezes just as a prompt is sent: no false success, and
    // Retry runs it exactly once.
    signal(&b, "-STOP");
    app.send(Cmd::Prompt(id.clone(), "lost ack".into()));
    let t = Instant::now();
    let not_sent = app
        .until(Duration::from_secs(14), |s| {
            last_reply(s).is_some_and(|r| matches!(r.state, ReplyState::NotSent(_)))
        })
        .await;
    let why = last_reply(&app.snap()).map(|r| r.state);
    signal(&b, "-CONT");
    app.until(Duration::from_secs(8), |s| {
        status_is(s, &id, Status::online)
    })
    .await;
    let i = app.snap().replies.len() - 1;
    app.send(Cmd::Retry(i));
    app.until(Duration::from_secs(10), |s| {
        last_reply(s).is_some_and(|r| r.state == ReplyState::Done)
    })
    .await;
    let rp = last_reply(&app.snap()).context("reply")?;
    let users = log(&b.state, &rp.session)
        .iter()
        .filter(|e| e.kind == "user")
        .count();
    r.check(
        "A prompt the machine never confirmed says so, and Retry runs it exactly once",
        not_sent.is_some() && rp.state == ReplyState::Done && users == 1,
        format!(
            "not confirmed after {} ms: {why:?}; after Retry: {:?}, prompts logged there {users}",
            ms(t.elapsed()),
            rp.state
        ),
    );

    // A spoofed announcement can't take over or rename a paired machine.
    let impostor = Spec {
        mdns: false,
        ..Spec::new("impostor", root.join("m-impostor"))
    };
    let fake = spawn(&impostor).await?;
    let spoof = crate::discovery::advertise("evil", &id, fake.lan.port())?;
    tokio::time::sleep(Duration::from_secs(4)).await;
    signal(&b, "-KILL");
    drop(b);
    b = spawn(&studio).await?;
    let back = app
        .until(Duration::from_secs(10), |s| {
            status_is(s, &id, Status::online)
        })
        .await;
    let d = app.dev(&id).context("studio")?;
    r.check(
        "A fake announcement using a paired machine's id can't rename it or take its place",
        back.is_some() && d.name == "studio" && d.route.is_some_and(|a| a.port() == b.lan.port()),
        format!(
            "name {:?}; reconnected to the real one at {:?} ({:?} ms), {} routes known",
            d.name,
            d.route,
            back.map(ms),
            d.routes.len()
        ),
    );
    let _ = spoof.shutdown();
    drop(fake);

    // Removed on the other machine: said plainly, and no retry loop.
    Trust::update(&b.state, |p| {
        p.remove(&fingerprint(
            &Identity::load(&dir, "laptop").expect("me").public,
        ));
    })?;
    let t = Instant::now();
    let removed = app
        .until(Duration::from_secs(5), |s| {
            status_is(s, &id, |st| *st == Status::Removed)
        })
        .await;
    let mine = Trust::load(&dir).peers.contains_key(&id);
    r.check(
        "Removed on the other machine: shown as removed, and its key is forgotten here",
        removed.is_some() && !mine,
        format!(
            "shown {} ms after removal; still trusted here: {mine}",
            ms(t.elapsed())
        ),
    );

    // Adding it again works (the person shows a new code there).
    signal(&b, "-HUP");
    app.send(Cmd::Add(id.clone()));
    app.until(Duration::from_secs(5), |s| {
        status_is(s, &id, |st| matches!(st, Status::NeedsCode { .. }))
    })
    .await;
    app.send(Cmd::Code(id.clone(), "12a456".into()));
    let fmt = app
        .until(Duration::from_secs(2), |s| {
            status_is(
                s,
                &id,
                |st| matches!(st, Status::NeedsCode { error: Some(e) } if e.contains("6 digits")),
            )
        })
        .await;
    let c = wait_code(&b.state).await;
    let wrong = if c == "111111" { "222222" } else { "111111" };
    app.send(Cmd::Code(id.clone(), wrong.into()));
    let bad = app
        .until(Duration::from_secs(5), |s| status_is(s, &id, |st| matches!(st, Status::NeedsCode { error: Some(e) } if e.contains("didn't match"))))
        .await;
    app.send(Cmd::Code(id.clone(), c));
    let again = app
        .until(Duration::from_secs(8), |s| {
            status_is(s, &id, Status::online)
        })
        .await;
    r.check(
        "Codes with letters or a wrong code get a clear message; the right one pairs again",
        fmt.is_some() && bad.is_some() && again.is_some(),
        format!("\"12a456\": told codes are 6 digits; wrong code: told it didn't match; paired again in {:?} ms", again.map(ms)),
    );

    // Removing here tells the other machine, now or when it's next seen.
    let me_id = Identity::load(&dir, "laptop")?.id();
    app.send(Cmd::Remove(id.clone()));
    let t = Instant::now();
    let mut told = None;
    for _ in 0..100 {
        if !Trust::load(&b.state).peers.contains_key(&me_id) {
            told = Some(t.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    signal(&b, "-HUP");
    pair_with(&app, &b, &id).await?;
    signal(&b, "-KILL");
    drop(b);
    app.send(Cmd::Remove(id.clone()));
    tokio::time::sleep(Duration::from_millis(500)).await;
    b = spawn(&studio).await?;
    let t2 = Instant::now();
    let mut later = None;
    for _ in 0..400 {
        if !Trust::load(&b.state).peers.contains_key(&me_id) {
            later = Some(t2.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    r.check(
        "Removing a machine here makes it forget this one too, even if it was off at the time",
        told.is_some() && later.is_some(),
        format!(
            "online: forgotten there after {:?} ms; offline: forgotten {:?} ms after it came back",
            told.map(ms),
            later.map(ms)
        ),
    );

    // A machine reinstalled with a new identity is pointed out.
    signal(&b, "-HUP");
    pair_with(&app, &b, &id).await?;
    signal(&b, "-KILL");
    drop(b);
    let reborn = Spec {
        lan_port: studio.lan_port,
        ..Spec::new("studio", root.join("m-studio-new"))
    };
    let nb = spawn(&reborn).await?;
    let new_id_ = Identity::load(&reborn.state, "studio")?.id();
    let hinted = app
        .until(Duration::from_secs(12), |s| {
            s.devices
                .iter()
                .any(|d| d.id == new_id_ && d.same_name_as.as_deref() == Some("studio"))
        })
        .await;
    r.check(
        "A reinstalled machine (same name, new identity) is listed as new, with a hint",
        hinted.is_some(),
        format!("old studio offline, new one available and flagged after {:?} ms; it still needs a code", hinted.map(ms)),
    );

    // An unpaired machine that goes away leaves the list.
    drop(nb);
    let t = Instant::now();
    let left = app
        .until(Duration::from_secs(20), |s| {
            !s.devices.iter().any(|d| d.id == new_id_)
        })
        .await;
    r.check(
        "An unpaired machine that goes away leaves the list",
        left.is_some(),
        format!("gone {} ms after it stopped", ms(t.elapsed())),
    );
    app.quit();

    // Tailscale-trusted, also visible on the LAN: stays on Tailscale.
    let both = Spec {
        tailnet_port: Some(free_port()),
        sim: Some((ME, ME)),
        ..Spec::new("build-box", root.join("m-build"))
    };
    let bb = spawn(&both).await?;
    let bid = Identity::load(&both.state, "build-box")?.id();
    let tail = bb.tailnet.context("tailnet")?;
    let app = App::start(
        root.join("app2"),
        Options {
            tailnet_peers: vec![tail],
            wake_to: None,
            bonjour: true,
        },
    )?;
    app.until(Duration::from_secs(10), |s| {
        s.devices.iter().any(|d| d.id == bid && d.routes.len() >= 2)
    })
    .await;
    app.send(Cmd::Add(bid.clone()));
    let on = app
        .until(Duration::from_secs(6), |s| {
            status_is(s, &bid, Status::online)
        })
        .await;
    let d = app.dev(&bid).context("build-box")?;
    let lan_seen = d.routes.iter().any(|r| r.addr.ip() == lan_ip());
    signal(&bb, "-KILL");
    drop(bb);
    let bb = spawn(&both).await?;
    app.until(Duration::from_secs(4), |s| {
        !status_is(s, &bid, Status::online)
    })
    .await;
    let again = app
        .until(Duration::from_secs(10), |s| {
            status_is(s, &bid, Status::online)
        })
        .await;
    let d2 = app.dev(&bid).context("build-box")?;
    r.check(
        "A machine trusted through Tailscale and also on the LAN connects over Tailscale, with no code",
        on.is_some() && lan_seen && d.route == Some(tail) && again.is_some() && d2.route == Some(tail),
        format!(
            "found on {} routes; added with no code, online over {:?}; after a restart back over {:?}",
            d.routes.len(),
            d.route,
            d2.route
        ),
    );
    drop(bb);
    app.quit();
    Ok(())
}
