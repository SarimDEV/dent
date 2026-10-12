//! A stand-in daemon: runs fake agents through the holder and copies their
//! output to `copy`, the way the real daemon writes events. The copy's
//! length is the resume point, so a restart picks up exactly where the last
//! write landed. The harness kills and restarts it to check that.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::client::{self, Event, Holder, Start};

#[derive(Default, Clone)]
pub struct Opts {
    pub dir: PathBuf,
    pub copy: PathBuf,
    pub agents: usize,
    /// Arguments for each agent after `agent --id <id>`.
    pub agent_args: Vec<String>,
    /// Lines to send to every agent's stdin once connected.
    pub send: Vec<String>,
    pub close: bool,
    /// Exit once every agent has finished.
    pub until_done: bool,
    /// Holder protocol this daemon wants; an older holder is replaced once
    /// its agents finish.
    pub want: u32,
    /// For the holder this daemon starts.
    pub holder_args: Vec<String>,
    /// Run agents as our own children, without a holder, for comparison.
    pub direct: bool,
    /// Stop every agent this long after connecting, giving it `grace_ms`.
    pub stop_after_ms: Option<u64>,
    pub grace_ms: u64,
}

struct Copy {
    root: PathBuf,
    files: HashMap<(String, bool), std::fs::File>,
    lens: HashMap<(String, bool), u64>,
    partial: HashMap<String, Vec<u8>>,
}

impl Copy {
    fn path(&self, id: &str, ext: &str) -> PathBuf {
        self.root.join(format!("{id}.{ext}"))
    }

    fn len(&mut self, id: &str, err: bool) -> u64 {
        let p = self.path(id, if err { "err" } else { "out" });
        *self
            .lens
            .entry((id.into(), err))
            .or_insert_with(|| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
    }

    fn mark(&self, id: &str, ext: &str, text: &str) {
        let _ = std::fs::write(self.path(id, ext), text);
    }

    fn has(&self, id: &str, ext: &str) -> bool {
        self.path(id, ext).exists()
    }

    /// Append output that starts at byte `off`, skipping anything already
    /// copied and logging any gap.
    fn add(&mut self, id: &str, err: bool, off: u64, bytes: &[u8]) -> Result<()> {
        let have = self.len(id, err);
        let end = off + bytes.len() as u64;
        if end <= have {
            log(&self.root, &format!("dup {id} {off}"));
            return Ok(());
        }
        if off > have {
            log(&self.root, &format!("gap {id} {have}..{off}"));
        }
        let skip = have.saturating_sub(off) as usize;
        let key = (id.to_string(), err);
        if !self.files.contains_key(&key) {
            let p = self.path(id, if err { "err" } else { "out" });
            let f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)?;
            self.files.insert(key.clone(), f);
        }
        if let Some(f) = self.files.get_mut(&key) {
            f.write_all(&bytes[skip..])?;
        }
        self.lens.insert(key, end);
        if !err {
            self.latency(id, &bytes[skip..]);
        }
        Ok(())
    }

    /// For agents that stamp their lines: how long each took to arrive.
    fn latency(&mut self, id: &str, bytes: &[u8]) {
        let buf = self.partial.entry(id.into()).or_default();
        buf.extend_from_slice(bytes);
        let now = crate::agent::now_us();
        let mut lat = Vec::new();
        while let Some(i) = buf.iter().position(|b| *b == b'\n') {
            let l: Vec<u8> = buf.drain(..=i).collect();
            let s = String::from_utf8_lossy(&l);
            let mut it = s.split_whitespace();
            if let (Some(t), Some(_), None) = (it.next(), it.next(), it.next())
                && let Ok(t) = t.parse::<u64>()
            {
                lat.push(now.saturating_sub(t));
            }
        }
        if !lat.is_empty()
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.root.join("lat.txt"))
        {
            for l in lat {
                let _ = writeln!(f, "{l}");
            }
        }
        // Only stamped output is short lines of two fields; cap the rest.
        if buf.len() > 4096 {
            buf.clear();
        }
    }
}

pub fn log(root: &Path, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("daemon.log"))
    {
        let _ = writeln!(f, "{} {line}", crate::wire::now_ms());
    }
}

pub async fn run(o: Opts) -> Result<()> {
    std::fs::create_dir_all(&o.copy)?;
    let t0 = Instant::now();
    // One daemon per state directory: a second one (an upgrade started
    // before the old one finished exiting) waits its turn.
    let lockf = std::fs::File::create(o.copy.join("daemon.lock"))?;
    let fd = std::os::fd::AsRawFd::as_raw_fd(&lockf);
    // SAFETY: a valid descriptor, kept open for the life of the process.
    if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        log(&o.copy, "waiting for the other daemon");
        // SAFETY: as above.
        tokio::task::spawn_blocking(move || unsafe { libc::flock(fd, libc::LOCK_EX) }).await?;
        log(&o.copy, "other daemon gone");
    }
    if o.direct {
        return direct(o).await;
    }
    let mut copy = Copy {
        root: o.copy.clone(),
        files: HashMap::new(),
        lens: HashMap::new(),
        partial: HashMap::new(),
    };
    let ids: Vec<String> = (0..o.agents).map(|i| format!("a{i}")).collect();
    let exe = std::env::current_exe()?.to_string_lossy().into_owned();
    let mut holder_argv = vec![
        exe.clone(),
        "holder".into(),
        "--dir".into(),
        o.dir.to_string_lossy().into_owned(),
    ];
    holder_argv.extend(o.holder_args.iter().cloned());
    let start = Start { argv: holder_argv };
    let mut resumed = false;
    loop {
        let (h, mut rx) = client::connect(&o.dir, Some(&start)).await?;
        log(
            &o.copy,
            &format!("holder {} {} {}", h.pid, h.version, h.proto),
        );
        let upgrading = o.want > h.proto;
        if upgrading {
            h.exit_when_idle().await?;
            log(&o.copy, "holder out of date: replacing once idle");
        }
        let procs = h.list().await?;
        let listed: HashSet<String> = procs.iter().map(|p| p.id.clone()).collect();
        salvage(&o.dir, &mut copy, &listed)?;
        let mut live = Vec::new();
        for p in &procs {
            copy.mark(&p.id, "pid", &p.pid.to_string());
            let (out, err) = (copy.len(&p.id, false), copy.len(&p.id, true));
            h.attach(&p.id, out, err).await?;
            if !p.exited {
                live.push(p.id.clone());
            }
        }
        for id in &ids {
            let done = ["started", "exit", "interrupted"]
                .iter()
                .any(|e| copy.has(id, e));
            if listed.contains(id) || done || upgrading {
                continue;
            }
            let mut argv = vec![exe.clone(), "agent".into(), "--id".into(), id.clone()];
            argv.extend(o.agent_args.iter().cloned());
            let pid = h.spawn(id, argv, None, Vec::new()).await?;
            copy.mark(id, "started", "");
            copy.mark(id, "pid", &pid.to_string());
            h.attach(id, 0, 0).await?;
            live.push(id.clone());
        }
        for id in &live {
            for s in &o.send {
                h.write(id, format!("{s}\n").as_bytes()).await?;
            }
            if o.close {
                h.close_stdin(id).await?;
            }
        }
        if let Some(ms) = o.stop_after_ms {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            for id in &live {
                h.stop(id, Duration::from_millis(o.grace_ms)).await?;
            }
            log(&o.copy, "stopped");
        }
        if finished(&copy, &ids) && o.until_done {
            log(&o.copy, "done");
            return Ok(());
        }
        match pump(&o, &h, &mut rx, &mut copy, &ids, t0, &mut resumed).await? {
            Next::Done | Next::Replaced => return Ok(()),
            Next::Reconnect => continue,
        }
    }
}

enum Next {
    Done,
    Replaced,
    Reconnect,
}

async fn pump(
    o: &Opts,
    h: &Holder,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    copy: &mut Copy,
    ids: &[String],
    t0: Instant,
    resumed: &mut bool,
) -> Result<Next> {
    while let Some(ev) = rx.recv().await {
        match ev {
            Event::Data {
                id,
                err,
                off,
                bytes,
            } => {
                if !*resumed {
                    *resumed = true;
                    log(&o.copy, &format!("resumed {}", t0.elapsed().as_micros()));
                }
                copy.add(&id, err, off, &bytes)?;
            }
            Event::Exit { id, code, signal } => {
                copy.mark(&id, "exit", &format!("{code:?} {signal:?}"));
                h.release(&id).await?;
                if o.until_done && finished(copy, ids) {
                    log(&o.copy, "done");
                    return Ok(Next::Done);
                }
            }
            Event::Replaced => {
                log(&o.copy, "replaced");
                return Ok(Next::Replaced);
            }
            Event::Closed => {
                log(&o.copy, "holder closed");
                // Its agents lost their output pipes; stop them now rather
                // than when they next write, so none runs on unwatched.
                for id in ids {
                    if copy.has(id, "exit") || copy.has(id, "interrupted") {
                        continue;
                    }
                    let pid = std::fs::read_to_string(copy.path(id, "pid"))
                        .ok()
                        .and_then(|s| s.trim().parse::<i32>().ok());
                    if let Some(pid) = pid.filter(|p| *p > 1) {
                        // SAFETY: kill has no memory preconditions.
                        unsafe { libc::kill(-pid, libc::SIGTERM) };
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
                return Ok(Next::Reconnect);
            }
        }
    }
    Ok(Next::Reconnect)
}

fn finished(copy: &Copy, ids: &[String]) -> bool {
    ids.iter()
        .all(|id| copy.has(id, "exit") || copy.has(id, "interrupted"))
}

/// Output a holder wrote before it died: copy what's left, and mark the
/// agent interrupted.
fn salvage(dir: &Path, copy: &mut Copy, listed: &HashSet<String>) -> Result<()> {
    let Ok(rd) = std::fs::read_dir(dir.join("procs")) else {
        return Ok(());
    };
    for e in rd.flatten() {
        let id = e.file_name().to_string_lossy().into_owned();
        if listed.contains(&id) {
            continue;
        }
        for err in [false, true] {
            let data =
                std::fs::read(e.path().join(if err { "err" } else { "out" })).unwrap_or_default();
            let have = copy.len(&id, err) as usize;
            if data.len() > have {
                copy.add(&id, err, have as u64, &data[have..])?;
            }
        }
        copy.mark(&id, "interrupted", "");
        log(&copy.root, &format!("salvaged {id}"));
        let _ = std::fs::remove_dir_all(e.path());
    }
    Ok(())
}

/// Agents as our own children: what happens without a holder.
async fn direct(o: Opts) -> Result<()> {
    use tokio::io::AsyncReadExt;
    let exe = std::env::current_exe()?;
    let mut copy = Copy {
        root: o.copy.clone(),
        files: HashMap::new(),
        lens: HashMap::new(),
        partial: HashMap::new(),
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    for i in 0..o.agents {
        let id = format!("a{i}");
        let mut child = tokio::process::Command::new(&exe)
            .arg("agent")
            .arg("--id")
            .arg(&id)
            .args(&o.agent_args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        copy.mark(&id, "pid", &child.id().unwrap_or(0).to_string());
        let mut out = child.stdout.take().ok_or(anyhow::anyhow!("no stdout"))?;
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut buf = vec![0; 256 * 1024];
            let mut off = 0;
            while let Ok(n) = out.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let _ = tx.send((id.clone(), off, buf[..n].to_vec()));
                off += n as u64;
            }
            let _ = child.wait().await;
            let _ = tx.send((id, u64::MAX, Vec::new()));
        });
    }
    drop(tx);
    while let Some((id, off, bytes)) = rx.recv().await {
        if off == u64::MAX {
            copy.mark(&id, "exit", "");
        } else {
            copy.add(&id, false, off, &bytes)?;
        }
    }
    log(&o.copy, "done");
    Ok(())
}
