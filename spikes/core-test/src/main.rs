//! Spike 07: the I/O-free client core, driven over real sockets against
//! spike 05's daemon (a second process standing in for another machine).
//!
//! `cargo build -p spike-link && cargo run -p spike-core-test` runs every
//! scenario; `-- --out FILE` also writes the results as JSON.

mod host;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use host::{Host, now_ms};
use spike_core::chat::Mirror;
use spike_core::{BlockKind, Keys, Note, PairError, Role, Status, Turn};

const PROMPT: &str = "Why does the sync test fail after a reconnect?";

struct Daemon {
    child: Child,
    state: PathBuf,
    port: u16,
    extra: Vec<String>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(7490)
}

fn link_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?
        .parent()
        .context("exe dir")?
        .join("spike-link");
    if !exe.exists() {
        bail!("build spike-link first: cargo build -p spike-link");
    }
    Ok(exe)
}

impl Daemon {
    async fn spawn(state: &Path, port: u16, extra: &[&str]) -> Result<Self> {
        let child = Command::new(link_exe()?)
            .args(["daemon", "--name", "studio", "--state"])
            .arg(state)
            .args(["--lan", &format!("0.0.0.0:{port}"), "--no-mdns"])
            .args(["--agent-style", "markdown", "--agent-ms", "20"])
            .args(extra)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let start = Instant::now();
        while tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
        {
            if start.elapsed() > Duration::from_secs(5) {
                bail!("daemon didn't start");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(Self {
            child,
            state: state.into(),
            port,
            extra: extra.iter().map(|s| s.to_string()).collect(),
        })
    }

    async fn restart(&mut self) -> Result<()> {
        let extra: Vec<&str> = self.extra.iter().map(String::as_str).collect();
        let fresh = Daemon::spawn(&self.state, self.port, &extra).await?;
        let old = std::mem::replace(self, fresh);
        drop(old);
        Ok(())
    }

    fn signal(&self, sig: &str) {
        let _ = Command::new("kill")
            .args([sig, &self.child.id().to_string()])
            .status();
    }

    fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    async fn code(&self) -> Result<String> {
        let _ = std::fs::remove_file(self.state.join("pairing-code"));
        self.signal("-HUP");
        for _ in 0..100 {
            if let Ok(c) = std::fs::read_to_string(self.state.join("pairing-code"))
                && c.len() == 6
            {
                return Ok(c);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        bail!("no pairing code")
    }
}

struct Report {
    rows: Vec<serde_json::Value>,
    failed: usize,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        println!("{} {name} ({detail})", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.failed += 1;
        }
        self.rows
            .push(serde_json::json!({"name": name, "ok": ok, "detail": detail}));
    }
}

/// The text of message `index` in `session`, as the core holds it.
fn text(h: &Host, session: &str, index: usize) -> String {
    h.link
        .chat()
        .texts(session)
        .get(index)
        .map(|t| t.2.clone())
        .unwrap_or_default()
}

fn agent_done(m: &Mirror, session: &str, index: u32) -> bool {
    m.messages
        .get(&(session.to_string(), index))
        .is_some_and(|(r, s, _)| *r == Role::Agent && *s != Turn::Streaming)
}

fn agent_blocks(m: &Mirror, session: &str, index: u32) -> usize {
    m.blocks(session, index).map_or(0, |b| b.len())
}

fn stop_holders(root: &Path) {
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let dir = spike_link::daemon::holder_dir(&e.path());
        if let Ok(pid) = std::fs::read_to_string(dir.join("holder.lock")) {
            let _ = Command::new("kill").args(["-KILL", pid.trim()]).status();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let root = std::env::temp_dir().join(format!("dent-spike07-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    let mut r = Report {
        rows: Vec::new(),
        failed: 0,
    };
    let res = run(&root, &mut r).await;
    stop_holders(&root);
    let _ = std::fs::remove_dir_all(&root);
    if let Err(e) = &res {
        r.check("harness", false, format!("{e:#}"));
    }
    if let Some(path) = out {
        std::fs::write(&path, serde_json::to_string_pretty(&r.rows)?)?;
    }
    println!("\n{} of {} passed", r.rows.len() - r.failed, r.rows.len());
    if r.failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

async fn run(root: &Path, r: &mut Report) -> Result<()> {
    let mut studio = Daemon::spawn(&root.join("studio"), free_port(), &[]).await?;
    let keys = Keys::generate();

    // 1. Pairing with a code: unknown first, a wrong code, the right one.
    let mut h = Host::new(keys.clone(), None, vec![studio.addr()]);
    h.link.start(now_ms());
    h.run_until(Duration::from_secs(5), |h| h.status() == Status::NeedsCode)
        .await;
    let found = h
        .log
        .iter()
        .any(|(_, n)| matches!(n, Note::Welcome { name, .. } if name == "studio"));
    r.check(
        "an unpaired machine asks for its code",
        h.status() == Status::NeedsCode && found,
        format!("{:?}", h.status()),
    );
    let code = studio.code().await?;
    let wrong = if code == "000000" { "111111" } else { "000000" };
    h.link.pair(wrong, now_ms());
    h.run_until(Duration::from_secs(5), |h| {
        h.saw(|n| matches!(n, Note::PairFailed { .. })).is_some()
    })
    .await;
    let failed = h.log.iter().find_map(|(_, n)| match n {
        Note::PairFailed { error } => Some(error.clone()),
        _ => None,
    });
    r.check(
        "a wrong code is refused",
        failed == Some(PairError::Wrong),
        format!("{failed:?}"),
    );
    let t = Instant::now();
    h.link.pair(&code, now_ms());
    h.run_until(Duration::from_secs(5), |h| h.status() == Status::Online)
        .await;
    let known = h.link.known_key();
    r.check(
        "the right code pairs, then it connects trusted",
        h.status() == Status::Online && known.is_some(),
        format!("online {} ms after the code", t.elapsed().as_millis()),
    );
    let known = known.unwrap_or_default();

    // 2. A prompt streams back as Markdown blocks.
    let s = "s-md";
    h.link.follow(s);
    let t = Instant::now();
    h.link.prompt(s, PROMPT);
    let ok = h
        .run_until(Duration::from_secs(15), |h| agent_done(&h.mirror, s, 1))
        .await;
    let first = h
        .log
        .iter()
        .find(|(_, n)| {
            matches!(n, Note::Message { update } if update.role == Role::Agent && !update.blocks.is_empty())
        })
        .map(|(at, _)| at.duration_since(t).as_millis());
    let reply = text(&h, s, 1);
    let want = spike_core::blocks::parse(&reply);
    let got = h.mirror.blocks(s, 1).cloned().unwrap_or_default();
    let kinds = |k: fn(&BlockKind) -> bool| got.iter().filter(|b| k(&b.kind)).count();
    r.check(
        "a prompt streams back and draws as blocks",
        ok && got == want
            && kinds(|k| matches!(k, BlockKind::Heading { .. })) == 1
            && kinds(|k| matches!(k, BlockKind::Item { .. })) == 3
            && kinds(|k| matches!(k, BlockKind::Code { .. })) == 1,
        format!(
            "first text {} ms, {} blocks, same as one parse of the whole reply: {}",
            first.unwrap_or(0),
            got.len(),
            got == want
        ),
    );

    // 3. Two addresses, the first a black hole.
    let mut h2 = Host::new(
        keys.clone(),
        Some(hex(&known)),
        vec!["10.255.255.1:9".into(), studio.addr()],
    );
    let t = Instant::now();
    h2.link.start(now_ms());
    h2.run_until(Duration::from_secs(5), |h| h.status() == Status::Online)
        .await;
    let ms = t.elapsed().as_millis();
    r.check(
        "a dead first address costs a quarter second, not three",
        h2.status() == Status::Online && ms < 600,
        format!("online in {ms} ms through the second address"),
    );
    h2.shutdown();

    // 4. A prompt sent while the machine is down runs once when it's back.
    studio.signal("-KILL");
    h.run_until(Duration::from_secs(5), |h| h.status() == Status::Offline)
        .await;
    let s = "s-queue";
    h.link.follow(s);
    let id = h.link.prompt(s, "Sent while studio was down");
    h.run_until(Duration::from_millis(800), |_| false).await;
    studio.restart().await?;
    let ok = h
        .run_until(Duration::from_secs(15), |h| agent_done(&h.mirror, s, 1))
        .await;
    let users = h
        .link
        .chat()
        .texts(s)
        .iter()
        .filter(|m| m.0 == Role::User)
        .count();
    let accepted = h
        .log
        .iter()
        .filter(|(_, n)| matches!(n, Note::Accepted { id: i, .. } if *i == id))
        .count();
    r.check(
        "a prompt sent offline is delivered once on reconnect",
        ok && users == 1 && accepted == 1,
        format!("{users} user message, acknowledged {accepted} time"),
    );

    // 5. The machine crashes mid-reply and comes back.
    let s = "s-crash";
    h.link.follow(s);
    h.link.prompt(s, PROMPT);
    h.run_until(Duration::from_secs(10), |h| {
        agent_blocks(&h.mirror, s, 1) >= 3
    })
    .await;
    let before = h.link.stats();
    studio.signal("-KILL");
    tokio::time::sleep(Duration::from_millis(300)).await;
    studio.restart().await?;
    let ok = h
        .run_until(Duration::from_secs(15), |h| agent_done(&h.mirror, s, 1))
        .await;
    let after = h.link.stats();
    r.check(
        "the machine crashes mid-reply: the reply finishes whole",
        ok && text(&h, s, 1) == reply && after.gaps == before.gaps && after.dups == before.dups,
        format!(
            "{} reconnects, {} gaps, {} repeats, text identical: {}",
            after.reconnects - before.reconnects,
            after.gaps - before.gaps,
            after.dups - before.dups,
            text(&h, s, 1) == reply
        ),
    );

    // 6. The machine freezes like a sleeping laptop, then wakes.
    let s = "s-sleep";
    h.link.follow(s);
    h.link.prompt(s, PROMPT);
    h.run_until(Duration::from_secs(10), |h| {
        agent_blocks(&h.mirror, s, 1) >= 2
    })
    .await;
    studio.signal("-STOP");
    let t = Instant::now();
    h.run_until(Duration::from_secs(6), |h| h.status() == Status::Offline)
        .await;
    let offline = t.elapsed().as_millis();
    studio.signal("-CONT");
    let t = Instant::now();
    h.run_until(Duration::from_secs(10), |h| h.status() == Status::Online)
        .await;
    let back = t.elapsed().as_millis();
    let ok = h
        .run_until(Duration::from_secs(15), |h| agent_done(&h.mirror, s, 1))
        .await;
    r.check(
        "the machine freezes and wakes: offline at 3 s, then the reply finishes",
        ok && (2900..4000).contains(&offline) && text(&h, s, 1) == reply,
        format!("offline after {offline} ms, back {back} ms after waking"),
    );

    // 7. A flaky network: every connection cut after 0.15 s, while 100,000
    // events stream (over a second), so it's cut many times mid-stream.
    let proxy = spike_link::proxy::start(
        format!("127.0.0.1:{}", studio.port).parse()?,
        Some(Duration::from_millis(150)),
    )
    .await?;
    let mut f = Host::new(
        keys.clone(),
        Some(hex(&known)),
        vec![proxy.addr.to_string()],
    );
    f.link.start(now_ms());
    f.run_until(Duration::from_secs(5), |h| h.status() == Status::Online)
        .await;
    let s = "s-flaky";
    f.link.follow(s);
    f.link.bulk(s, 100_000);
    let ok = f
        .run_until(Duration::from_secs(60), |h| {
            h.link.stats().events >= 100_000
        })
        .await;
    let st = f.link.stats();
    r.check(
        "connection cut every 0.15 s: every event exactly once",
        ok && st.events == 100_000 && st.gaps == 0 && st.reconnects >= 3,
        format!(
            "{} of 100,000, {} reconnects, {} gaps, {} repeats dropped",
            st.events, st.reconnects, st.gaps, st.dups
        ),
    );
    f.shutdown();

    // 8. Catching up 100,000 events.
    let s = "s-100k";
    h.link.follow(s);
    h.link.bulk(s, 100_000);
    h.run_until(Duration::from_secs(60), |h| {
        h.link.chat().last(s) >= 100_000
    })
    .await;
    let mut c = Host::new(keys.clone(), Some(hex(&known)), vec![studio.addr()]);
    c.link.follow(s);
    let t = Instant::now();
    c.link.start(now_ms());
    let ok = c
        .run_until(Duration::from_secs(60), |h| {
            h.link.stats().events >= 100_000
                && h.mirror.blocks(s, 0).is_some_and(|b| b.len() >= 10_000)
        })
        .await;
    let ms = t.elapsed().as_millis().max(1);
    let st = c.link.stats();
    r.check(
        "a 100,000-event session catches up through the core",
        ok && st.gaps == 0,
        format!(
            "{:.2} s, {} events/s, {:.1} MB, {} ms of it parsing and building blocks",
            ms as f64 / 1000.,
            st.events as u128 * 1000 / ms,
            st.bytes as f64 / 1e6,
            c.drain_time.as_millis()
        ),
    );
    c.shutdown();

    // 9. The machine says goodbye before restarting.
    studio.signal("-USR2");
    h.run_until(Duration::from_secs(5), |h| {
        h.saw(|n| matches!(n, Note::Bye { .. })).is_some()
    })
    .await;
    let bye = h.saw(|n| matches!(n, Note::Bye { .. }));
    let n = h.connects.len();
    h.run_until(Duration::from_secs(3), |h| h.connects.len() > n)
        .await;
    let gap = match (bye, h.connects.get(n)) {
        (Some(b), Some((c, _))) => c.duration_since(b).as_millis(),
        _ => 0,
    };
    studio.restart().await?;
    h.run_until(Duration::from_secs(5), |h| h.status() == Status::Online)
        .await;
    r.check(
        "after a goodbye it waits a second before redialling",
        (900..1500).contains(&gap) && h.status() == Status::Online,
        format!("redialled {gap} ms after the goodbye"),
    );

    // 10. A different machine at the same address.
    let impostor = Daemon::spawn(&root.join("impostor"), free_port(), &[]).await?;
    let mut w = Host::new(keys.clone(), Some(hex(&known)), vec![impostor.addr()]);
    w.link.start(now_ms());
    let online = w
        .run_until(Duration::from_secs(4), |h| h.status() == Status::Online)
        .await;
    let detail = w
        .log
        .iter()
        .rev()
        .find_map(|(_, n)| match n {
            Note::Status { detail, .. } if !detail.is_empty() => Some(detail.clone()),
            _ => None,
        })
        .unwrap_or_default();
    r.check(
        "a different machine at the known address is refused",
        !online && detail.contains("different key"),
        detail,
    );
    w.shutdown();
    drop(impostor);

    // 11. Versions that don't overlap.
    let newer = Daemon::spawn(&root.join("newer"), free_port(), &["--proto", "3"]).await?;
    let mut v = Host::new(keys.clone(), None, vec![newer.addr()]);
    v.link.start(now_ms());
    v.run_until(Duration::from_secs(4), |h| {
        h.status() == Status::Incompatible
    })
    .await;
    let wait = v
        .link
        .next_deadline()
        .map_or(0, |d| d.saturating_sub(now_ms()));
    r.check(
        "a machine on a newer protocol says so and is retried slowly",
        v.status() == Status::Incompatible && wait > 25_000,
        format!("next try in {} s", wait / 1000),
    );
    v.shutdown();
    drop(newer);

    // 12. Removed on the other machine.
    let id = keys.id();
    spike_link::identity::Trust::update(&studio.state, |p| {
        if let Some(e) = p.get_mut(&id) {
            e.via = "removed".into();
        }
    })?;
    let t = Instant::now();
    h.run_until(Duration::from_secs(5), |h| h.status() == Status::Removed)
        .await;
    let ms = t.elapsed().as_millis();
    let n = h.connects.len();
    h.run_until(Duration::from_secs(3), |_| false).await;
    r.check(
        "removed on the other machine: cut off, and no more attempts",
        h.status() == Status::Removed && h.connects.len() == n,
        format!(
            "cut off after {ms} ms, {} dials since",
            h.connects.len() - n
        ),
    );
    h.shutdown();
    Ok(())
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
        .collect()
}
