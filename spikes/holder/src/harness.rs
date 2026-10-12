//! Every scenario runs real processes: the stand-in daemon, the holder and
//! fake agents, killed and restarted the way upgrades and crashes do.

use std::io::{BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::agent;
use crate::wire::{Msg, PROTO, Req, read_frame, write_frame};

struct Env {
    root: PathBuf,
    dir: PathBuf,
    copy: PathBuf,
    exe: PathBuf,
    kids: Vec<Child>,
}

impl Env {
    fn new(name: &str) -> Result<Env> {
        // Short, because socket paths are limited to about 100 bytes.
        let root = std::env::temp_dir().join(format!("dh{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root)?;
        Ok(Env {
            dir: root.join("h"),
            copy: root.join("c"),
            exe: std::env::current_exe()?,
            root,
            kids: Vec::new(),
        })
    }

    fn daemon(&mut self, args: &[&str]) -> Result<u32> {
        self.daemon_in(&self.copy.clone(), args)
    }

    fn daemon_in(&mut self, copy: &Path, args: &[&str]) -> Result<u32> {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("daemon.out"))?;
        let c = Command::new(&self.exe)
            .arg("daemon")
            .arg("--dir")
            .arg(&self.dir)
            .arg("--copy")
            .arg(copy)
            .args(args)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        let pid = c.id();
        self.kids.push(c);
        Ok(pid)
    }

    /// Wait for a daemon we started to exit.
    fn exited(&mut self, pid: u32, within: Duration) -> bool {
        until(within, || {
            self.kids
                .iter_mut()
                .find(|c| c.id() == pid)
                .is_some_and(|c| matches!(c.try_wait(), Ok(Some(_))))
        })
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.copy.join("daemon.log")).unwrap_or_default()
    }

    fn logged(&self, what: &str) -> usize {
        self.log().lines().filter(|l| l.contains(what)).count()
    }

    fn holder(&self) -> Option<i32> {
        self.log()
            .lines()
            .rev()
            .find_map(|l| l.split(" holder ").nth(1)?.split(' ').next()?.parse().ok())
    }

    fn read(&self, id: &str, ext: &str) -> Vec<u8> {
        std::fs::read(self.copy.join(format!("{id}.{ext}"))).unwrap_or_default()
    }

    fn text(&self, id: &str, ext: &str) -> String {
        String::from_utf8_lossy(&self.read(id, ext))
            .trim()
            .to_string()
    }

    fn pid(&self, id: &str) -> Option<i32> {
        self.text(id, "pid").parse().ok()
    }

    fn has(&self, id: &str, ext: &str) -> bool {
        self.copy.join(format!("{id}.{ext}")).exists()
    }

    fn done(&self) -> bool {
        self.logged(" done") > 0
    }

    /// Microseconds from each daemon start to its first byte of output.
    fn resumes(&self) -> Vec<u64> {
        self.log()
            .lines()
            .filter_map(|l| l.split(" resumed ").nth(1)?.parse().ok())
            .collect()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for c in &mut self.kids {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Ok(rd) = std::fs::read_dir(&self.copy) {
            for e in rd.flatten() {
                if e.path().extension().is_some_and(|x| x == "pid")
                    && let Some(pid) = std::fs::read_to_string(e.path())
                        .ok()
                        .and_then(|s| s.trim().parse::<i32>().ok())
                        .filter(|p| *p > 1)
                {
                    sig(-pid, libc::SIGKILL);
                }
            }
        }
        let logs = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|d| std::fs::read_to_string(d.path().join("daemon.log")).ok())
            .collect::<Vec<_>>()
            .join("\n");
        for l in logs.lines() {
            if let Some(pid) = l
                .split(" holder ")
                .nth(1)
                .and_then(|r| r.split(' ').next()?.parse::<i32>().ok())
            {
                sig(pid, libc::SIGKILL);
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn sig(pid: i32, s: i32) {
    // SAFETY: kill has no memory preconditions.
    unsafe { libc::kill(pid, s) };
}

fn alive(pid: i32) -> bool {
    // SAFETY: as above; signal 0 only checks.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn until(within: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + within;
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    f()
}

fn ms(d: Duration) -> String {
    format!("{} ms", d.as_millis())
}

/// A deterministic spread of waits, so runs are comparable.
fn spread(i: u64, lo: u64, hi: u64) -> Duration {
    Duration::from_millis(lo + (i * 7919 + 13) % (hi - lo))
}

fn pct(v: &mut [u64], p: f64) -> u64 {
    v.sort_unstable();
    v.get(((v.len() as f64 - 1.) * p) as usize)
        .copied()
        .unwrap_or(0)
}

/// A connection made by hand, for what the daemon never sends.
struct Raw {
    r: BufReader<UnixStream>,
    w: UnixStream,
    n: u64,
}

impl Raw {
    fn connect(dir: &Path) -> Result<Raw> {
        let s = UnixStream::connect(crate::holder::socket(dir))?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        Ok(Raw {
            w: s.try_clone()?,
            r: BufReader::new(s),
            n: 0,
        })
    }

    fn call(&mut self, req: Req, payload: &[u8]) -> Result<Msg> {
        self.n += 1;
        write_frame(&mut self.w, self.n, req, payload)?;
        loop {
            match read_frame::<Msg>(&mut self.r)? {
                Some((env, _)) if env.req == self.n => return Ok(env.body),
                Some(_) => continue,
                None => bail!("closed"),
            }
        }
    }
}

fn start_holder(e: &mut Env, extra: &[&str]) -> Result<Child> {
    let c = Command::new(&e.exe)
        .arg("holder")
        .arg("--dir")
        .arg(&e.dir)
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    until(Duration::from_secs(3), || {
        UnixStream::connect(crate::holder::socket(&e.dir)).is_ok()
    });
    Ok(c)
}

type Scenario = fn() -> Result<String>;

pub fn run(only: Option<&str>) -> Result<()> {
    if only == Some("launchd") {
        return launchd();
    }
    let all: Vec<(&str, Scenario)> = vec![
        ("streams", streams),
        ("restarts", restarts),
        ("finished-while-away", finished_while_away),
        ("stdin-across-restart", stdin_across_restart),
        ("half-written-stdin", half_written_stdin),
        ("two-daemons", two_daemons),
        ("takeover", takeover),
        ("holder-killed", holder_killed),
        ("stale-socket", stale_socket),
        ("one-holder", one_holder),
        ("stop", stop),
        ("stuck-daemon", stuck_daemon),
        ("idle-exit", idle_exit),
        ("holder-upgrade", holder_upgrade),
        ("binary", binary),
        ("big", big),
        ("many", many),
        ("latency", latency),
        ("bad-requests", bad_requests),
        ("garbage", garbage),
        ("permissions", permissions),
        ("long-path", long_path),
        ("linger", linger),
    ];
    let mut failed = 0;
    let mut ran = 0;
    for (name, f) in all {
        if only.is_some_and(|o| o != name) {
            continue;
        }
        ran += 1;
        let t = Instant::now();
        match f() {
            Ok(detail) => println!("PASS {name:<22} {detail} ({})", ms(t.elapsed())),
            Err(e) => {
                failed += 1;
                println!("FAIL {name:<22} {e:#}");
            }
        }
    }
    println!("{}/{ran} passed", ran - failed);
    if failed > 0 {
        bail!("{failed} failed");
    }
    Ok(())
}

macro_rules! check {
    ($cond:expr, $($fmt:tt)*) => {
        if !$cond {
            bail!($($fmt)*);
        }
    };
}

fn exact(e: &Env, id: &str, lines: usize, size: usize) -> Result<()> {
    let got = e.read(id, "out");
    let want = agent::expected(id, lines, size);
    if got != want {
        let at = got.iter().zip(&want).position(|(a, b)| a != b);
        bail!(
            "{id}: copy differs ({} of {} bytes, first difference at {at:?})",
            got.len(),
            want.len()
        );
    }
    Ok(())
}

fn no_gaps(e: &Env) -> Result<()> {
    check!(e.logged("gap ") == 0, "gaps: {}", e.log());
    Ok(())
}

fn streams() -> Result<String> {
    let mut e = Env::new("s1")?;
    let args = "--lines 5000 --every-us 200 --size 80";
    e.daemon(&["--agents", "4", "--agent-args", args, "--until-done"])?;
    check!(until(Duration::from_secs(15), || e.done()), "not done");
    for i in 0..4 {
        exact(&e, &format!("a{i}"), 5000, 80)?;
        check!(e.text(&format!("a{i}"), "exit") == "Some(0) None", "exit");
    }
    no_gaps(&e)?;
    Ok("4 agents × 5,000 lines, byte for byte".into())
}

fn restarts() -> Result<String> {
    let mut e = Env::new("s2")?;
    let args = "--lines 6000 --every-us 500 --size 80";
    let base = ["--agents", "4", "--agent-args", args, "--until-done"];
    let mut d = e.daemon(&base)?;
    check!(
        until(Duration::from_secs(3), || (0..4)
            .all(|i| e.pid(&format!("a{i}")).is_some())),
        "agents didn't start"
    );
    let pids: Vec<_> = (0..4).map(|i| e.pid(&format!("a{i}"))).collect();
    for i in 0..10 {
        std::thread::sleep(spread(i, 150, 350));
        sig(
            d as i32,
            if i % 2 == 0 {
                libc::SIGKILL
            } else {
                libc::SIGTERM
            },
        );
        check!(e.exited(d, Duration::from_secs(2)), "daemon didn't exit");
        d = e.daemon(&base)?;
    }
    check!(until(Duration::from_secs(15), || e.done()), "not done");
    for (i, pid) in pids.iter().enumerate() {
        let id = format!("a{i}");
        exact(&e, &id, 6000, 80)?;
        check!(e.pid(&id) == *pid, "{id} was restarted");
    }
    no_gaps(&e)?;
    let mut r = e.resumes();
    let n = r.len();
    Ok(format!(
        "killed 10 times mid-stream, same agents, no gaps; output again {} ms after each start (median of {n}, worst {} ms)",
        pct(&mut r, 0.5) / 1000,
        pct(&mut r, 1.) / 1000
    ))
}

fn finished_while_away() -> Result<String> {
    let mut e = Env::new("s3")?;
    let args = "--lines 300 --every-us 1000 --exit 7";
    let d = e.daemon(&["--agents", "2", "--agent-args", args])?;
    check!(
        until(Duration::from_secs(3), || e.has("a1", "pid")),
        "no start"
    );
    sig(d as i32, libc::SIGKILL);
    let pid = e.pid("a0").unwrap_or(0);
    check!(
        until(Duration::from_secs(3), || !alive(pid)),
        "agent still running"
    );
    std::thread::sleep(Duration::from_millis(200));
    e.daemon(&["--agents", "2", "--agent-args", args, "--until-done"])?;
    check!(until(Duration::from_secs(5), || e.done()), "not done");
    for id in ["a0", "a1"] {
        check!(
            e.text(id, "exit") == "Some(7) None",
            "{id} exit {}",
            e.text(id, "exit")
        );
        let got = e.read(id, "out");
        check!(got == agent::expected(id, 300, 64), "{id} output");
    }
    Ok("exit code 7 and all output recovered after the agents ended unwatched".into())
}

fn stdin_across_restart() -> Result<String> {
    let mut e = Env::new("s4")?;
    let d = e.daemon(&["--agent-args", "--echo", "--send", "one"])?;
    check!(
        until(Duration::from_secs(3), || !e.read("a0", "out").is_empty()),
        "no echo"
    );
    sig(d as i32, libc::SIGKILL);
    e.daemon(&[
        "--agent-args",
        "--echo",
        "--send",
        "two",
        "--close",
        "--until-done",
    ])?;
    check!(until(Duration::from_secs(5), || e.done()), "not done");
    let got = String::from_utf8_lossy(&e.read("a0", "out")).to_string();
    check!(got == "got 4: one\ngot 4: two\n", "got {got:?}");
    check!(e.text("a0", "exit") == "Some(0) None", "exit");
    Ok("the same agent heard both daemons, and ended when stdin closed".into())
}

fn half_written_stdin() -> Result<String> {
    let mut e = Env::new("s5")?;
    let d = e.daemon(&["--agent-args", "--echo"])?;
    check!(
        until(Duration::from_secs(3), || e.has("a0", "pid")),
        "no start"
    );
    sig(d as i32, libc::SIGKILL);
    e.exited(d, Duration::from_secs(1));
    // A daemon dying halfway through a 1 MB write.
    let mut raw = Raw::connect(&e.dir)?;
    raw.call(Req::Hello { proto: PROTO }, &[])?;
    let mut head = serde_json::to_vec(&serde_json::json!({
        "req": 9, "n": 1_000_000, "op": "write", "id": "a0"
    }))?;
    head.push(b'\n');
    raw.w.write_all(&head)?;
    raw.w.write_all(&vec![b'x'; 500_000])?;
    drop(raw);
    std::thread::sleep(Duration::from_millis(100));
    e.daemon(&[
        "--agent-args",
        "--echo",
        "--send",
        "done",
        "--close",
        "--until-done",
    ])?;
    check!(until(Duration::from_secs(5), || e.done()), "not done");
    let got = String::from_utf8_lossy(&e.read("a0", "out")).to_string();
    check!(got == "got 5: done\n", "agent saw {got:?}");
    Ok("a write cut off halfway never reached the agent".into())
}

fn two_daemons() -> Result<String> {
    let mut e = Env::new("s6")?;
    let args = "--lines 3000 --every-us 1000";
    let base = ["--agents", "2", "--agent-args", args, "--until-done"];
    let a = e.daemon(&base)?;
    std::thread::sleep(Duration::from_millis(500));
    let b = e.daemon(&base)?;
    check!(
        until(Duration::from_secs(2), || e.logged("waiting for the other")
            == 1),
        "second daemon didn't wait"
    );
    std::thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    sig(a as i32, libc::SIGTERM);
    check!(
        until(Duration::from_secs(2), || e.logged("other daemon gone")
            == 1),
        "second daemon didn't take over"
    );
    let waited = t.elapsed();
    check!(until(Duration::from_secs(10), || e.done()), "not done");
    check!(e.exited(b, Duration::from_secs(2)), "b didn't exit");
    for id in ["a0", "a1"] {
        exact(&e, id, 3000, 64)?;
    }
    no_gaps(&e)?;
    Ok(format!(
        "the new daemon waited for the old one, then took over {} after it exited",
        ms(waited)
    ))
}

fn takeover() -> Result<String> {
    let mut e = Env::new("s7")?;
    let a = e.daemon(&["--agent-args", "--lines 100000 --every-us 1000"])?;
    check!(
        until(Duration::from_secs(3), || e.has("a0", "pid")),
        "no start"
    );
    // Another daemon with a different state directory but the same holder.
    let mut raw = Raw::connect(&e.dir)?;
    raw.call(Req::Hello { proto: PROTO }, &[])?;
    check!(
        e.exited(a, Duration::from_secs(2)),
        "old daemon kept running"
    );
    std::thread::sleep(Duration::from_millis(300));
    check!(e.logged("replaced") == 1, "no replaced notice");
    check!(
        e.logged("holder ") == 1,
        "old daemon reconnected and fought back"
    );
    Ok("a replaced daemon stops instead of taking the holder back".into())
}

fn holder_killed() -> Result<String> {
    let mut e = Env::new("s8")?;
    let args = "--lines 100000 --every-us 100";
    e.daemon(&["--agents", "3", "--agent-args", args])?;
    check!(
        until(Duration::from_secs(3), || e.holder().is_some()
            && e.has("a2", "pid")),
        "no start"
    );
    std::thread::sleep(Duration::from_millis(500));
    let old = e.holder().unwrap_or(0);
    let pids: Vec<i32> = (0..3).filter_map(|i| e.pid(&format!("a{i}"))).collect();
    let t = Instant::now();
    sig(old, libc::SIGKILL);
    check!(
        until(Duration::from_secs(3), || pids.iter().all(|p| !alive(*p))),
        "agents left running"
    );
    let gone = t.elapsed();
    check!(
        until(Duration::from_secs(3), || e
            .holder()
            .is_some_and(|h| h != old)),
        "no new holder"
    );
    check!(
        until(Duration::from_secs(3), || e.logged("salvaged") == 3),
        "not salvaged"
    );
    for i in 0..3 {
        let id = format!("a{i}");
        let got = e.read(&id, "out");
        let want = agent::expected(&id, 100000, 64);
        check!(
            !got.is_empty() && want.starts_with(&got),
            "{id}: salvaged output corrupt"
        );
        check!(e.has(&id, "interrupted"), "{id} not marked");
    }
    Ok(format!(
        "agents stopped {} after the holder died; output up to the crash kept; marked interrupted",
        ms(gone)
    ))
}

fn stale_socket() -> Result<String> {
    let mut e = Env::new("s9")?;
    e.daemon(&["--agent-args", "--lines 10", "--until-done"])?;
    check!(until(Duration::from_secs(5), || e.done()), "not done");
    let old = e.holder().unwrap_or(0);
    sig(old, libc::SIGKILL);
    std::thread::sleep(Duration::from_millis(100));
    check!(
        crate::holder::socket(&e.dir).exists(),
        "socket file was cleaned up (nothing to test)"
    );
    let copy2 = e.root.join("c2");
    let t = Instant::now();
    e.daemon_in(&copy2, &["--agent-args", "--lines 10", "--until-done"])?;
    let log2 = || std::fs::read_to_string(copy2.join("daemon.log")).unwrap_or_default();
    check!(
        until(Duration::from_secs(5), || log2().contains(" done")),
        "second daemon stuck: {}",
        log2()
    );
    Ok(format!(
        "a dead holder's socket file is replaced; new holder and agent done in {}",
        ms(t.elapsed())
    ))
}

fn one_holder() -> Result<String> {
    let mut e = Env::new("s10")?;
    std::fs::create_dir_all(&e.dir)?;
    let mut hs: Vec<Child> = (0..3)
        .map(|_| {
            Command::new(&e.exe)
                .arg("holder")
                .arg("--dir")
                .arg(&e.dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
        })
        .collect::<std::io::Result<_>>()?;
    std::thread::sleep(Duration::from_millis(500));
    let running: Vec<u32> = hs
        .iter_mut()
        .filter_map(|c| matches!(c.try_wait(), Ok(None)).then(|| c.id()))
        .collect();
    let ok = running.len() == 1 && Raw::connect(&e.dir).is_ok();
    for mut c in hs {
        let _ = c.kill();
        let _ = c.wait();
    }
    e.kids.clear();
    check!(ok, "{} holders running", running.len());
    Ok("three started at once; one serves, two exit".into())
}

fn stop() -> Result<String> {
    let mut e = Env::new("s11")?;
    let args = "--lines 1000000 --every-us 1000 --child --ignore-term";
    e.daemon(&[
        "--agent-args",
        args,
        "--stop-after-ms",
        "300",
        "--grace-ms",
        "500",
        "--until-done",
    ])?;
    check!(
        until(Duration::from_secs(3), || e.has("a0", "pid")),
        "no start"
    );
    let child: i32 = until(Duration::from_secs(2), || {
        e.text("a0", "err").starts_with("child")
    })
    .then(|| e.text("a0", "err").split(' ').nth(1)?.parse().ok())
    .flatten()
    .unwrap_or(0);
    check!(child > 0, "no grandchild");
    check!(until(Duration::from_secs(5), || e.done()), "not done");
    let stopped = Instant::now();
    check!(
        e.text("a0", "exit") == "None Some(9)",
        "exit {}",
        e.text("a0", "exit")
    );
    check!(
        until(Duration::from_millis(500), || !alive(child)),
        "grandchild survived"
    );
    let mut e2 = Env::new("s11b")?;
    e2.daemon(&[
        "--agent-args",
        "--lines 1000000 --every-us 1000",
        "--stop-after-ms",
        "300",
        "--until-done",
    ])?;
    check!(until(Duration::from_secs(5), || e2.done()), "not done");
    check!(
        e2.text("a0", "exit") == "None Some(15)",
        "exit {}",
        e2.text("a0", "exit")
    );
    let _ = stopped;
    Ok("SIGTERM stops an agent; one that ignores it is killed after the grace period, with what it started".into())
}

fn stuck_daemon() -> Result<String> {
    let mut e = Env::new("s12")?;
    let args = "--lines 200000 --size 100";
    let d = e.daemon(&["--agent-args", args, "--until-done"])?;
    check!(
        until(Duration::from_secs(3), || e.has("a0", "pid")),
        "no start"
    );
    sig(d as i32, libc::SIGSTOP);
    let pid = e.pid("a0").unwrap_or(0);
    let t = Instant::now();
    let finished = until(Duration::from_secs(5), || !alive(pid));
    let took = t.elapsed();
    sig(d as i32, libc::SIGCONT);
    check!(finished, "agent blocked by a frozen daemon");
    check!(until(Duration::from_secs(10), || e.done()), "not done");
    exact(&e, "a0", 200000, 100)?;
    // Without a holder, the same agent stalls once the pipe fills.
    let mut e2 = Env::new("s12b")?;
    let d2 = e2.daemon(&["--agent-args", args, "--direct"])?;
    check!(
        until(Duration::from_secs(3), || e2.has("a0", "pid")),
        "no start"
    );
    sig(d2 as i32, libc::SIGSTOP);
    let pid2 = e2.pid("a0").unwrap_or(0);
    let blocked = !until(Duration::from_secs(2), || !alive(pid2));
    sig(d2 as i32, libc::SIGCONT);
    Ok(format!(
        "20 MB agent finished in {} with the daemon frozen{}",
        ms(took),
        if blocked {
            "; as a direct child it stalled"
        } else {
            ""
        }
    ))
}

fn idle_exit() -> Result<String> {
    let mut e = Env::new("s13")?;
    let ha = "--holder-args";
    let d = e.daemon(&[
        "--agent-args",
        "--lines 10",
        ha,
        "--idle-ms 800",
        "--until-done",
    ])?;
    check!(until(Duration::from_secs(5), || e.done()), "not done");
    check!(e.exited(d, Duration::from_secs(2)), "daemon didn't exit");
    let h = e.holder().unwrap_or(0);
    let t = Instant::now();
    check!(until(Duration::from_secs(3), || !alive(h)), "holder stayed");
    let took = t.elapsed();
    check!(
        !crate::holder::socket(&e.dir).exists(),
        "socket left behind"
    );
    // With an agent still running and no daemon, it stays.
    let mut e2 = Env::new("s13b")?;
    let d2 = e2.daemon(&[
        "--agent-args",
        "--lines 3000 --every-us 1000",
        ha,
        "--idle-ms 300",
    ])?;
    check!(
        until(Duration::from_secs(3), || e2.has("a0", "pid")),
        "no start"
    );
    sig(d2 as i32, libc::SIGKILL);
    std::thread::sleep(Duration::from_millis(1200));
    check!(
        alive(e2.holder().unwrap_or(0)),
        "holder left a running agent"
    );
    Ok(format!(
        "gone {} after the last agent and daemon; stays while an agent runs",
        ms(took)
    ))
}

fn holder_upgrade() -> Result<String> {
    let mut e = Env::new("s14")?;
    let args = "--lines 1500 --every-us 1000";
    let d = e.daemon(&[
        "--agents",
        "2",
        "--agent-args",
        args,
        "--holder-args",
        "--version 0.0.1",
    ])?;
    check!(
        until(Duration::from_secs(3), || e.has("a1", "pid")),
        "no start"
    );
    let old = e.holder().unwrap_or(0);
    sig(d as i32, libc::SIGTERM);
    e.daemon(&[
        "--agents",
        "4",
        "--agent-args",
        args,
        "--want",
        "2",
        "--holder-args",
        "--proto 2 --version 0.0.2",
        "--until-done",
    ])?;
    check!(
        until(Duration::from_secs(10), || e.done()),
        "not done: {}",
        e.log()
    );
    let new = e.holder().unwrap_or(0);
    check!(new != old && !alive(old), "old holder still in charge");
    check!(
        e.log().contains(&format!("holder {new} 0.0.2 2")),
        "{}",
        e.log()
    );
    for i in 0..4 {
        exact(&e, &format!("a{i}"), 1500, 64)?;
    }
    Ok("old holder kept its two agents to the end, then a new one took the next two".into())
}

fn binary() -> Result<String> {
    let mut e = Env::new("s15")?;
    e.daemon(&["--agent-args", "--binary 3000001", "--until-done"])?;
    check!(until(Duration::from_secs(5), || e.done()), "not done");
    check!(
        e.read("a0", "out") == agent::binary(3_000_001),
        "bytes differ"
    );
    Ok("3 MB of every byte value, no final newline, exact".into())
}

fn big() -> Result<String> {
    let lines = 2_000_000;
    let mut e = Env::new("s16")?;
    let args = format!("--lines {lines} --size 128");
    let base = ["--agent-args", &args, "--until-done"];
    let t = Instant::now();
    let mut d = e.daemon(&base)?;
    for i in 0..3 {
        std::thread::sleep(spread(i, 200, 400));
        sig(d as i32, libc::SIGKILL);
        e.exited(d, Duration::from_secs(1));
        d = e.daemon(&base)?;
    }
    check!(until(Duration::from_secs(60), || e.done()), "not done");
    let took = t.elapsed();
    exact(&e, "a0", lines, 128)?;
    let mut e2 = Env::new("s16b")?;
    let t2 = Instant::now();
    e2.daemon(&["--agent-args", &args, "--direct"])?;
    check!(
        until(Duration::from_secs(60), || e2.done()),
        "direct not done"
    );
    let direct = t2.elapsed();
    let mb = (lines * 128) as f64 / 1e6;
    Ok(format!(
        "256 MB with 3 daemon kills, exact: {:.0} MB/s through the holder, {:.0} MB/s as a direct child",
        mb / took.as_secs_f64(),
        mb / direct.as_secs_f64()
    ))
}

fn many() -> Result<String> {
    let mut e = Env::new("s17")?;
    let args = "--lines 1500 --every-us 2000";
    let base = ["--agents", "50", "--agent-args", args, "--until-done"];
    let mut d = e.daemon(&base)?;
    check!(
        until(Duration::from_secs(5), || e.has("a49", "pid")),
        "no start"
    );
    std::thread::sleep(Duration::from_millis(500));
    let h = e.holder().unwrap_or(0);
    let rss = Command::new("ps")
        .args(["-o", "rss=", "-p", &h.to_string()])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(0);
    for i in 0..2 {
        sig(d as i32, libc::SIGKILL);
        e.exited(d, Duration::from_secs(1));
        d = e.daemon(&base)?;
        std::thread::sleep(spread(i, 300, 600));
    }
    check!(until(Duration::from_secs(20), || e.done()), "not done");
    for i in 0..50 {
        exact(&e, &format!("a{i}"), 1500, 64)?;
    }
    no_gaps(&e)?;
    let mut r = e.resumes();
    Ok(format!(
        "50 agents, 2 kills, all exact; holder uses {:.1} MB; output again {} ms after a start",
        rss as f64 / 1024.,
        pct(&mut r[1..], 1.) / 1000
    ))
}

fn latency() -> Result<String> {
    let args = "--stamp --lines 2000 --every-us 1000";
    let lat = |direct: bool| -> Result<(u64, u64)> {
        let mut e = Env::new(if direct { "s18b" } else { "s18" })?;
        let mut a = vec!["--agent-args", args];
        a.push(if direct { "--direct" } else { "--until-done" });
        e.daemon(&a)?;
        check!(until(Duration::from_secs(10), || e.done()), "not done");
        let mut v: Vec<u64> = std::fs::read_to_string(e.copy.join("lat.txt"))?
            .lines()
            .filter_map(|l| l.parse().ok())
            .collect();
        Ok((pct(&mut v, 0.5), pct(&mut v, 0.99)))
    };
    let (h50, h99) = lat(false)?;
    let (d50, d99) = lat(true)?;
    Ok(format!(
        "a line reaches the daemon in {h50} µs (p99 {h99} µs) through the holder, {d50} µs (p99 {d99} µs) direct"
    ))
}

fn bad_requests() -> Result<String> {
    let mut e = Env::new("s19")?;
    let h = start_holder(&mut e, &[])?;
    e.kids.push(h);
    let mut raw = Raw::connect(&e.dir)?;
    let before = raw.call(Req::List, &[])?;
    check!(
        matches!(&before, Msg::Error { msg } if msg == "hello first"),
        "acted before hello: {before:?}"
    );
    raw.call(Req::Hello { proto: PROTO }, &[])?;
    let spawn = |raw: &mut Raw, id: &str, argv: &[&str]| {
        raw.call(
            Req::Spawn {
                id: id.into(),
                argv: argv.iter().map(|s| s.to_string()).collect(),
                cwd: None,
                env: Vec::new(),
            },
            &[],
        )
    };
    let err = |m: Result<Msg>| match m {
        Ok(Msg::Error { msg }) => msg,
        other => format!("{other:?}"),
    };
    let missing = err(spawn(&mut raw, "x", &["/nonexistent/agent"]));
    check!(missing.starts_with("spawn"), "{missing}");
    for id in ["", "../x", "a/b", ".hidden"] {
        check!(
            err(spawn(&mut raw, id, &["true"])) == "bad id",
            "id {id:?} accepted"
        );
    }
    check!(err(spawn(&mut raw, "y", &[])) == "empty argv", "empty argv");
    let exe = e.exe.to_string_lossy().to_string();
    spawn(&mut raw, "z", &[&exe, "agent", "--echo"])?;
    check!(
        err(spawn(&mut raw, "z", &["true"])) == "exists",
        "duplicate id"
    );
    check!(
        err(raw.call(Req::Release { id: "z".into() }, &[])) == "still running",
        "released a running agent"
    );
    check!(
        err(raw.call(Req::Write { id: "nope".into() }, b"x")).starts_with("no process"),
        "write to nobody"
    );
    let left: Vec<_> = std::fs::read_dir(e.dir.join("procs"))?
        .flatten()
        .map(|d| d.file_name().to_string_lossy().to_string())
        .collect();
    check!(left == ["z"], "leftover dirs {left:?}");
    raw.call(
        Req::Stop {
            id: "z".into(),
            grace_ms: 100,
        },
        &[],
    )?;
    Ok("no hello, missing program, bad ids, empty argv, duplicates and unknown ids all refused cleanly".into())
}

fn garbage() -> Result<String> {
    let mut e = Env::new("s20")?;
    e.daemon(&[
        "--agent-args",
        "--lines 2000 --every-us 1000",
        "--until-done",
    ])?;
    check!(
        until(Duration::from_secs(3), || e.holder().is_some()),
        "no holder"
    );
    let h = e.holder().unwrap_or(0);
    let sock = crate::holder::socket(&e.dir);
    let mut a = UnixStream::connect(&sock)?;
    a.write_all(&vec![b'z'; 70_000])?;
    let mut b = UnixStream::connect(&sock)?;
    b.write_all(b"{not json}\n")?;
    let mut c = UnixStream::connect(&sock)?;
    c.write_all(b"{\"req\":1,\"n\":999999999999,\"op\":\"list\"}\n")?;
    check!(until(Duration::from_secs(10), || e.done()), "not done");
    exact(&e, "a0", 2000, 64)?;
    check!(alive(h) && e.holder() == Some(h), "holder restarted");
    Ok("oversized, malformed and huge frames close only that connection".into())
}

fn permissions() -> Result<String> {
    let mut e = Env::new("s21")?;
    std::fs::create_dir_all(&e.dir)?;
    std::fs::set_permissions(&e.dir, std::fs::Permissions::from_mode(0o755))?;
    let h = start_holder(&mut e, &[])?;
    e.kids.push(h);
    let dir = std::fs::metadata(&e.dir)?.permissions().mode() & 0o777;
    let sock = std::fs::metadata(crate::holder::socket(&e.dir))?
        .permissions()
        .mode()
        & 0o777;
    check!(dir == 0o700 && sock == 0o600, "dir {dir:o} socket {sock:o}");
    Ok("directory tightened to 0700, socket 0600".into())
}

fn long_path() -> Result<String> {
    let e = Env::new("s22")?;
    let deep = e.root.join("x".repeat(120));
    let out = Command::new(&e.exe)
        .arg("holder")
        .arg("--dir")
        .arg(&deep)
        .output()?;
    let msg = String::from_utf8_lossy(&out.stderr);
    check!(
        !out.status.success() && msg.contains("socket path too long"),
        "{msg}"
    );
    Ok("refused with a clear error instead of a confusing bind failure".into())
}

fn linger() -> Result<String> {
    let mut e = Env::new("s23")?;
    let t = Instant::now();
    e.daemon(&["--agent-args", "--lines 10 --linger", "--until-done"])?;
    check!(
        until(Duration::from_secs(6), || e.done()),
        "exit never reported"
    );
    let took = t.elapsed();
    check!(
        e.read("a0", "out") == agent::expected("a0", 10, 64),
        "output"
    );
    Ok(format!(
        "exit reported {} after start although a leftover process held the output open",
        ms(took)
    ))
}

/// Under launchd: restart a daemon job with `kickstart -k`, once with agents
/// as its own children and once through the holder.
fn launchd() -> Result<()> {
    let uid = unsafe { libc::getuid() };
    for direct in [true, false] {
        let mut e = Env::new(if direct { "ld" } else { "lh" })?;
        // launchd jobs may not read ~/Documents without asking, so run a
        // copy of this binary from the temp directory instead.
        let exe = e.root.join("spike-holder");
        std::fs::copy(&e.exe, &exe)?;
        e.exe = exe;
        let label = format!(
            "dev.dent.spike06.{}",
            if direct { "direct" } else { "holder" }
        );
        let plist = e.root.join(format!("{label}.plist"));
        let mut args = vec![
            e.exe.to_string_lossy().to_string(),
            "daemon".into(),
            "--dir".into(),
            e.dir.to_string_lossy().to_string(),
            "--copy".into(),
            e.copy.to_string_lossy().to_string(),
            "--agent-args".into(),
            "--lines 1000000 --every-us 1000".into(),
        ];
        if direct {
            args.push("--direct".into());
        }
        let args_xml: String = args
            .iter()
            .map(|a| format!("<string>{a}</string>"))
            .collect();
        std::fs::write(
            &plist,
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{label}</string>
<key>ProgramArguments</key><array>{args_xml}</array>
<key>KeepAlive</key><true/>
<key>StandardErrorPath</key><string>{err}</string>
</dict></plist>"#,
                err = e.root.join("launchd.err").display()
            ),
        )?;
        let domain = format!("gui/{uid}");
        let target = format!("{domain}/{label}");
        let _ = Command::new("launchctl")
            .args(["bootout", &target])
            .output();
        Command::new("launchctl")
            .args(["bootstrap", &domain])
            .arg(&plist)
            .status()?;
        let started = until(Duration::from_secs(5), || e.has("a0", "pid"));
        let pid = e.pid("a0").unwrap_or(0);
        let daemon = || {
            Command::new("launchctl")
                .args(["print", &target])
                .output()
                .ok()
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .find_map(|l| l.trim().strip_prefix("pid = ")?.parse::<i32>().ok())
                })
        };
        let d1 = daemon();
        Command::new("launchctl")
            .args(["kickstart", "-k", &target])
            .status()?;
        std::thread::sleep(Duration::from_millis(1000));
        let mid = e.read("a0", "out").len();
        std::thread::sleep(Duration::from_millis(1000));
        let after = e.read("a0", "out").len();
        let survived = alive(pid);
        let d2 = daemon();
        let _ = Command::new("launchctl")
            .args(["bootout", &target])
            .output();
        std::thread::sleep(Duration::from_millis(300));
        println!(
            "{}: started {started}, daemon {d1:?} → {d2:?}, agent {pid} {} the restart; after it, output kept arriving: {} ({mid} → {after} bytes)",
            if direct { "direct child" } else { "via holder" },
            if survived {
                "survived"
            } else {
                "was killed by"
            },
            after > mid,
        );
        if !started {
            println!(
                "{}",
                std::fs::read_to_string(e.root.join("launchd.err")).unwrap_or_default()
            );
        }
        if let Some(h) = e.holder() {
            sig(h, libc::SIGKILL);
        }
    }
    Ok(())
}
