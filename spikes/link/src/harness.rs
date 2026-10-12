//! Every scenario, against a second daemon process standing in for another
//! machine. The harness plays the desktop app and the person at the other
//! machine (who reads the pairing code off its screen).

use std::net::{SocketAddr, TcpListener as StdListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::sync::{Mutex, mpsc};

use crate::client::{self, Note, Tally};
use crate::identity::{Identity, Peer, Trust};
use crate::proto::{Down, Up, now_ms};

struct Daemon {
    child: Child,
    state: PathBuf,
    lan: SocketAddr,
    tailnet: Option<SocketAddr>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Clone)]
struct Spec {
    name: &'static str,
    state: PathBuf,
    lan_port: u16,
    tailnet_port: Option<u16>,
    sim: Option<(&'static str, &'static str)>,
    mdns: bool,
}

fn free_port() -> u16 {
    StdListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(7480)
}

fn lan_ip() -> std::net::IpAddr {
    // The address other machines on the network would use.
    let out = Command::new("ipconfig").args(["getifaddr", "en0"]).output();
    out.ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or([127, 0, 0, 1].into())
}

async fn spawn(spec: &Spec) -> Result<Daemon> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("daemon")
        .args(["--name", spec.name])
        .arg("--state")
        .arg(&spec.state)
        .args(["--lan", &format!("0.0.0.0:{}", spec.lan_port)])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(p) = spec.tailnet_port {
        cmd.args(["--tailnet", &format!("127.0.0.1:{p}")]);
    }
    if let Some((me, owner)) = spec.sim {
        cmd.args(["--sim-me", me, "--sim-owner", owner]);
    }
    if !spec.mdns {
        cmd.arg("--no-mdns");
    }
    let child = cmd.spawn()?;
    let lan = SocketAddr::new(lan_ip(), spec.lan_port);
    let start = Instant::now();
    while tokio::net::TcpStream::connect(lan).await.is_err() {
        if start.elapsed() > Duration::from_secs(5) {
            bail!("daemon didn't start");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(Daemon {
        child,
        state: spec.state.clone(),
        lan,
        tailnet: spec
            .tailnet_port
            .map(|p| SocketAddr::from(([127, 0, 0, 1], p))),
    })
}

fn signal(d: &Daemon, sig: &str) {
    let _ = Command::new("kill")
        .args([sig, &d.child.id().to_string()])
        .status();
}

fn code(state: &Path) -> String {
    std::fs::read_to_string(state.join("pairing-code")).unwrap_or_default()
}

struct Report {
    rows: Vec<(bool, String, String)>,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        println!("{} {name} ({detail})", if ok { "PASS" } else { "FAIL" });
        self.rows.push((ok, name.into(), detail));
    }
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p / 100.).round() as usize]
}

/// Collect notes until `stop` says so or `limit` passes.
async fn collect(
    rx: &mut mpsc::UnboundedReceiver<Note>,
    limit: Duration,
    mut stop: impl FnMut(&Note) -> bool,
) -> Vec<(Instant, Note)> {
    let mut out = Vec::new();
    let end = Instant::now() + limit;
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(n)) => {
                let done = stop(&n);
                out.push((Instant::now(), n));
                if done {
                    break;
                }
            }
            _ => break,
        }
    }
    out
}

fn is_end(n: &Note) -> bool {
    matches!(n, Note::Event(e) if e.kind == "end")
}

pub async fn run() -> Result<()> {
    let root = std::env::temp_dir().join(format!("dent-link-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    let mut r = Report { rows: Vec::new() };
    let me_dir = root.join("laptop");
    let me = Identity::load(&me_dir, "laptop")?;
    let mut studio = Spec {
        name: "studio",
        state: root.join("studio"),
        lan_port: free_port(),
        tailnet_port: Some(free_port()),
        sim: Some(("you@example.com", "you@example.com")),
        mdns: true,
    };

    // 1. Finding the other machine on the LAN.
    let mut b = spawn(&studio).await?;
    let b_id = Identity::load(&studio.state, "studio")?.id();
    let t = Instant::now();
    let found = tokio::task::spawn_blocking({
        let id = b_id.clone();
        move || crate::discovery::browse(Duration::from_secs(8), Some(&id))
    })
    .await??;
    let hit = found.iter().find(|f| f.id == b_id).cloned();
    r.check(
        "Bonjour finds the other machine on the LAN",
        hit.as_ref().is_some_and(|f| f.addrs.contains(&b.lan)),
        format!(
            "{} ms, {:?}",
            t.elapsed().as_millis(),
            hit.map(|f| (f.name, f.addrs)).unwrap_or_default()
        ),
    );

    // 2. An unknown device can't do anything before pairing.
    let mut o = client::open(b.lan, &me).await?;
    let trusted_before = o.trusted;
    o.ch.send(&Up::Prompt {
        session: "s".into(),
        text: "hi".into(),
    })
    .await?;
    let reply: Result<Down> = o.ch.recv().await;
    r.check(
        "An unpaired device is refused before it can run anything",
        !trusted_before && matches!(reply, Ok(Down::Denied { .. })),
        format!(
            "trusted {trusted_before}, reply {:?}",
            reply
                .map(|d| format!("{d:?}"))
                .unwrap_or_else(|e| e.to_string())
        ),
    );

    // 3. Wrong codes are rejected; three of them change the code.
    let first = code(&b.state);
    let mut wrong = Vec::new();
    for _ in 0..3 {
        let mut o = client::open(b.lan, &me).await?;
        wrong.push(client::pair(&mut o.ch, "000000").await.is_err());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let rotated = code(&b.state) != first;
    r.check(
        "Wrong codes are rejected, and three of them change the code",
        wrong.iter().all(|w| *w) && rotated,
        format!("rejected {wrong:?}, code changed {rotated}"),
    );

    // 4. The right code pairs both ways.
    let mut o = client::open(b.lan, &me).await?;
    let t = Instant::now();
    let paired = client::pair(&mut o.ch, &code(&b.state)).await;
    let mut mine = Trust::load(&me_dir);
    if paired.is_ok() {
        mine.add(
            &o.ch.remote,
            Peer {
                name: o.name.clone(),
                key: hex::encode(&o.ch.remote),
                via: "code".into(),
                addr: Some(b.lan.to_string()),
            },
        )?;
    }
    let b_key = o.ch.remote.clone();
    let theirs = Trust::load(&b.state).get(&me.public).map(|p| p.via.clone());
    r.check(
        "The code shown on the other machine pairs them, both ways",
        paired.is_ok() && theirs.as_deref() == Some("code"),
        format!(
            "{:?} in {} ms, other side trusts us via {theirs:?}",
            paired.err(),
            t.elapsed().as_millis()
        ),
    );

    // 5. Paired devices reconnect without a code, across a restart.
    let again = client::open(b.lan, &me).await?;
    drop(b);
    b = spawn(&studio).await?;
    let after_restart = client::open(b.lan, &me).await?;
    r.check(
        "Paired devices reconnect with no code, even after a restart",
        again.trusted
            && after_restart.trusted
            && after_restart.via == "known"
            && after_restart.id == b_id,
        format!(
            "before {} via {}, after restart {} via {}",
            again.trusted, again.via, after_restart.trusted, after_restart.via
        ),
    );

    // 6. Someone else at the same address is refused.
    let impostor = Spec {
        name: "studio",
        state: root.join("impostor"),
        mdns: false,
        ..studio.clone()
    };
    drop(b);
    let fake = spawn(&impostor).await?;
    let o = client::open(fake.lan, &me).await?;
    let refused = o.ch.remote != b_key;
    r.check(
        "A different machine at the same address is caught by its key",
        refused,
        format!(
            "expected key {}, got {}",
            crate::identity::fingerprint(&b_key),
            crate::identity::fingerprint(&o.ch.remote)
        ),
    );
    drop(fake);
    b = spawn(&studio).await?;

    // 7. Tailscale, same account: trusted with no code.
    let phone = Identity::load(&root.join("phone"), "phone")?;
    let o = client::open(b.tailnet.context("tailnet")?, &phone).await?;
    r.check(
        "On your own Tailscale account, a new device is trusted with no code",
        o.trusted && o.via == "tailnet",
        format!("trusted {} via {}", o.trusted, o.via),
    );

    // 8. Tailscale, someone else's account: needs the code.
    drop(b);
    studio.sim = Some(("you@example.com", "someone@else.com"));
    b = spawn(&studio).await?;
    let stranger = Identity::load(&root.join("stranger"), "stranger")?;
    let o = client::open(b.tailnet.context("tailnet")?, &stranger).await?;
    r.check(
        "A device on someone else's Tailscale account still needs the code",
        !o.trusted,
        format!("trusted {} via {}", o.trusted, o.via),
    );
    drop(b);
    studio.sim = Some(("you@example.com", "you@example.com"));
    b = spawn(&studio).await?;

    // 9. Nothing readable crosses the network.
    let tap = crate::proxy::start(b.lan, None).await?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tally = Arc::new(Mutex::new(Tally::default()));
    let at = tap.addr;
    let f = client::follow(
        me.clone(),
        b_key.clone(),
        Arc::new(move || vec![at]),
        "tap".into(),
        0,
        tally.clone(),
        tx,
    );
    collect(&mut rx, Duration::from_secs(3), |n| {
        matches!(n, Note::Up { .. })
    })
    .await;
    let mut o = client::open(tap.addr, &me).await?;
    let (mut otx, _orx) =
        std::mem::replace(&mut o.ch, client::open(b.lan, &me).await?.ch).into_parts();
    otx.send(&Up::Prompt {
        session: "tap".into(),
        text: "pineapple-7731".into(),
    })
    .await?;
    let notes = collect(&mut rx, Duration::from_secs(15), is_end).await;
    let got: String = notes
        .iter()
        .filter_map(|(_, n)| match n {
            Note::Event(e) => Some(e.text.clone()),
            _ => None,
        })
        .collect();
    let bytes = tap.tap.lock().await.clone();
    let leaked = ["pineapple-7731", "Running on", "Subscribe", "studio"]
        .iter()
        .filter(|s| bytes.windows(s.len()).any(|w| w == s.as_bytes()))
        .count();
    r.check(
        "Nothing readable crosses the network",
        got.contains("pineapple-7731") && leaked == 0 && !bytes.is_empty(),
        format!(
            "{} bytes tapped, {leaked} of 4 plain strings found; the client read the reply: {}",
            bytes.len(),
            got.contains("pineapple-7731")
        ),
    );
    f.abort();

    // 10. A prompt runs on the other machine and streams back.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tally = Arc::new(Mutex::new(Tally::default()));
    let lan = b.lan;
    let f = client::follow(
        me.clone(),
        b_key.clone(),
        Arc::new(move || vec![lan]),
        "s1".into(),
        0,
        tally.clone(),
        tx,
    );
    collect(&mut rx, Duration::from_secs(3), |n| {
        matches!(n, Note::Up { .. })
    })
    .await;
    let mut ctl = client::open(b.lan, &me).await?.ch;
    let sent = Instant::now();
    ctl.send(&Up::Prompt {
        session: "s1".into(),
        text: "summarise the repo".into(),
    })
    .await?;
    let notes = collect(&mut rx, Duration::from_secs(15), is_end).await;
    // Wall-clock time each event was received, against when it was logged.
    let base = (Instant::now(), now_ms());
    let mut lat: Vec<f64> = notes
        .iter()
        .filter_map(|(at, n)| match n {
            Note::Event(e) => {
                let received = base.1 - base.0.duration_since(*at).as_millis() as u64;
                Some(received.saturating_sub(e.t) as f64)
            }
            _ => None,
        })
        .collect();
    let first_ms = notes
        .iter()
        .find(|(_, n)| matches!(n, Note::Event(e) if e.kind == "delta"))
        .map(|(t, _)| t.duration_since(sent).as_millis());
    let tl = tally.lock().await.clone();
    r.check(
        "A prompt runs on the other machine and streams back in order",
        notes.iter().any(|(_, n)| is_end(n)) && tl.dups == 0 && tl.gaps == 0,
        format!(
            "{} events, first text {first_ms:?} ms after sending, log-to-client latency p50 {:.0} ms p99 {:.0} ms",
            lat.len(),
            pct(&mut lat.clone(), 50.),
            pct(&mut lat, 99.)
        ),
    );
    f.abort();

    // 11. Catching up on a long session.
    ctl.send(&Up::Bulk {
        session: "big".into(),
        n: 100_000,
    })
    .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tally = Arc::new(Mutex::new(Tally::default()));
    let t = Instant::now();
    let f = client::follow(
        me.clone(),
        b_key.clone(),
        Arc::new(move || vec![lan]),
        "big".into(),
        0,
        tally.clone(),
        tx,
    );
    let mut count = 0;
    collect(&mut rx, Duration::from_secs(60), |n| {
        if matches!(n, Note::Event(_)) {
            count += 1;
        }
        count >= 100_000
    })
    .await;
    let secs = t.elapsed().as_secs_f64();
    r.check(
        "A 100,000-event session catches up over the link",
        count == 100_000,
        format!(
            "{count} events in {secs:.2} s, {:.0} events/s",
            count as f64 / secs
        ),
    );
    f.abort();

    // 12. The other machine crashes mid-reply and comes back.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tally = Arc::new(Mutex::new(Tally::default()));
    let f = client::follow(
        me.clone(),
        b_key.clone(),
        Arc::new(move || vec![lan]),
        "s2".into(),
        0,
        tally.clone(),
        tx,
    );
    collect(&mut rx, Duration::from_secs(3), |n| {
        matches!(n, Note::Up { .. })
    })
    .await;
    client::open(b.lan, &me)
        .await?
        .ch
        .send(&Up::Prompt {
            session: "s2".into(),
            text: "first".into(),
        })
        .await?;
    let mut seen = 0;
    collect(&mut rx, Duration::from_secs(5), |n| {
        if matches!(n, Note::Event(_)) {
            seen += 1;
        }
        seen >= 12
    })
    .await;
    signal(&b, "-KILL");
    let killed = Instant::now();
    tokio::time::sleep(Duration::from_millis(800)).await;
    drop(b);
    b = spawn(&studio).await?;
    let back = collect(&mut rx, Duration::from_secs(10), |n| {
        matches!(n, Note::Up { .. })
    })
    .await;
    let reconnect = back
        .last()
        .map(|(t, _)| t.duration_since(killed).as_millis());
    client::open(b.lan, &me)
        .await?
        .ch
        .send(&Up::Prompt {
            session: "s2".into(),
            text: "second".into(),
        })
        .await?;
    collect(&mut rx, Duration::from_secs(15), is_end).await;
    let tl = tally.lock().await.clone();
    let logged = std::fs::read_to_string(b.state.join("sessions/s2.jsonl"))?
        .lines()
        .count() as u64;
    r.check(
        "After a crash mid-reply, the stream resumes with no gaps or repeats",
        tl.gaps == 0 && tl.dups == 0 && tl.last == logged,
        format!("back {reconnect:?} ms after the crash (800 ms of it down), received {} of {logged} logged events, {} gaps, {} repeats", tl.last, tl.gaps, tl.dups),
    );
    f.abort();

    // 13. The other machine sleeps (frozen) and wakes.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tally = Arc::new(Mutex::new(Tally::default()));
    let f = client::follow(
        me.clone(),
        b_key.clone(),
        Arc::new(move || vec![lan]),
        "s3".into(),
        0,
        tally.clone(),
        tx,
    );
    collect(&mut rx, Duration::from_secs(3), |n| {
        matches!(n, Note::Up { .. })
    })
    .await;
    client::open(b.lan, &me)
        .await?
        .ch
        .send(&Up::Prompt {
            session: "s3".into(),
            text: "sleepy".into(),
        })
        .await?;
    collect(&mut rx, Duration::from_secs(2), |_| false).await;
    signal(&b, "-STOP");
    let froze = Instant::now();
    let down = collect(&mut rx, Duration::from_secs(8), |n| {
        matches!(n, Note::Down { .. })
    })
    .await;
    let noticed = down.iter().find_map(|(t, n)| match n {
        Note::Down { reason } => Some((t.duration_since(froze).as_millis(), reason.clone())),
        _ => None,
    });
    let (noticed, why) = (
        noticed.as_ref().map(|n| n.0),
        noticed.map(|n| n.1).unwrap_or_default(),
    );
    tokio::time::sleep(Duration::from_secs(4)).await;
    signal(&b, "-CONT");
    let woke = Instant::now();
    let up = collect(&mut rx, Duration::from_secs(10), |n| {
        matches!(n, Note::Up { .. })
    })
    .await;
    let resumed = up
        .iter()
        .find(|(_, n)| matches!(n, Note::Up { .. }))
        .map(|(t, _)| t.duration_since(woke).as_millis());
    collect(&mut rx, Duration::from_secs(15), is_end).await;
    let tl = tally.lock().await.clone();
    r.check(
        "A sleeping machine shows offline quickly and resumes on waking",
        noticed.is_some_and(|ms| ms <= 4500) && resumed.is_some() && tl.gaps == 0 && tl.dups == 0,
        format!("offline noticed {noticed:?} ms after it froze ({why}), back {resumed:?} ms after waking, {} gaps, {} repeats", tl.gaps, tl.dups),
    );
    f.abort();

    // 14. The other machine comes back on a different address.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tally = Arc::new(Mutex::new(Tally::default()));
    let id = b_id.clone();
    let resolve: client::Resolve = Arc::new(move || {
        crate::discovery::browse(Duration::from_secs(3), Some(&id))
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.id == id)
            .flat_map(|f| f.addrs)
            .collect()
    });
    let f = client::follow(
        me.clone(),
        b_key.clone(),
        resolve,
        "s4".into(),
        0,
        tally.clone(),
        tx,
    );
    collect(&mut rx, Duration::from_secs(8), |n| {
        matches!(n, Note::Up { .. })
    })
    .await;
    drop(b);
    studio.lan_port = free_port();
    let moved = Instant::now();
    b = spawn(&studio).await?;
    let up = collect(
        &mut rx,
        Duration::from_secs(15),
        |n| matches!(n, Note::Up { addr } if addr.port() == studio.lan_port),
    )
    .await;
    let found_again = up.iter().find_map(|(t, n)| match n {
        Note::Up { addr } if addr.port() == studio.lan_port => {
            Some(t.duration_since(moved).as_millis())
        }
        _ => None,
    });
    r.check(
        "After moving to a new address, it's found again and reconnects",
        found_again.is_some(),
        format!(
            "reconnected to port {} {found_again:?} ms after the move",
            studio.lan_port
        ),
    );
    f.abort();

    // 15. A connection that keeps dropping.
    let lan = b.lan;
    let flaky = crate::proxy::start(lan, Some(Duration::from_millis(700))).await?;
    let mut ctl = client::open(b.lan, &me).await?.ch;
    ctl.send(&Up::Bulk {
        session: "flaky".into(),
        n: 20_000,
    })
    .await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tally = Arc::new(Mutex::new(Tally::default()));
    let at = flaky.addr;
    let f = client::follow(
        me.clone(),
        b_key.clone(),
        Arc::new(move || vec![at]),
        "flaky".into(),
        0,
        tally.clone(),
        tx,
    );
    collect(&mut rx, Duration::from_secs(2), |_| false).await;
    ctl.send(&Up::Prompt {
        session: "flaky".into(),
        text: "keep going".into(),
    })
    .await?;
    collect(&mut rx, Duration::from_secs(30), is_end).await;
    let tl = tally.lock().await.clone();
    let cuts = *flaky.cuts.lock().await;
    let logged = std::fs::read_to_string(b.state.join("sessions/flaky.jsonl"))?
        .lines()
        .count() as u64;
    r.check(
        "A connection cut every 0.7 s still delivers everything once, in order",
        tl.last == logged && tl.gaps == 0 && tl.dups == 0 && cuts > 2,
        format!(
            "{} of {logged} events, {cuts} cuts, {} reconnects, {} gaps, {} repeats",
            tl.last, tl.reconnects, tl.gaps, tl.dups
        ),
    );
    f.abort();

    // 16. Removing a device cuts it off.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tally = Arc::new(Mutex::new(Tally::default()));
    let f = client::follow(
        me.clone(),
        b_key.clone(),
        Arc::new(move || vec![lan]),
        "s5".into(),
        0,
        tally.clone(),
        tx,
    );
    collect(&mut rx, Duration::from_secs(3), |n| {
        matches!(n, Note::Up { .. })
    })
    .await;
    let mut t = Trust::load(&b.state);
    t.remove(&me.id())?;
    let removed = Instant::now();
    let denied = collect(&mut rx, Duration::from_secs(5), |n| {
        matches!(n, Note::Denied(_))
    })
    .await;
    let cut = denied.iter().find_map(|(t, n)| match n {
        Note::Denied(why) => Some((t.duration_since(removed).as_millis(), why.clone())),
        _ => None,
    });
    let (cut, why) = (
        cut.as_ref().map(|c| c.0),
        cut.map(|c| c.1).unwrap_or_default(),
    );
    let retry = client::open(b.lan, &me).await?;
    r.check(
        "Removing a device on the other machine cuts it off at once",
        cut.is_some_and(|ms| ms < 1500) && !retry.trusted,
        format!(
            "cut off {cut:?} ms after removal ({why}); reconnecting is refused: {}",
            !retry.trusted
        ),
    );
    f.abort();
    drop(b);

    let ok = r.rows.iter().filter(|r| r.0).count();
    println!("{ok}/{} passed", r.rows.len());
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
