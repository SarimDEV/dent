//! The daemon's side of `dent-launcher` ([decision 0065]), when the
//! launcher runs it: two pipes, `DENT_LAUNCHER_OUT` (lines to the launcher)
//! and `DENT_LAUNCHER_IN` (lines from it).
//!
//! - On trial (`DENT_TRIAL=1`) the daemon migrates and binds, says
//!   `prepared`, and parks until told `committed` (with how the update
//!   went): a version that can't get that far is rolled back without ever
//!   touching an agent.
//! - Asked to update, it says `update <version>`; the launcher answers
//!   `accepted` (and then stops it) or `refused <why>`.
//!
//! All of it is taken once, at startup ([`init`]), and taken away from the
//! process: the pipes are made close-on-exec and the variables removed, so
//! the holder, agents and anything they run (a `dent-daemon` in an agent's
//! shell) never take them for their own.
//!
//! Run any other way (by the app, or by hand) there's no launcher and
//! none of this applies.
//!
//! [decision 0065]: ../../../docs/decisions/0065-launcher-and-updates.md

use std::io::{BufRead, BufReader, Write};
use std::os::fd::FromRawFd;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

/// The launcher protocol this daemon needs; a launcher speaking less is
/// told to update itself first (`dent-daemon install`).
pub const PROTOCOL: u32 = 1;

/// What the daemon exits with when another daemon has its state folder:
/// the launcher waits instead of starting it again and again.
pub const LOCKED: i32 = 75;

pub struct Launcher {
    out: Mutex<std::fs::File>,
    heard: Mutex<Receiver<String>>,
    trial: bool,
    version: String,
    outcome: Mutex<Option<dent_protocol::UpdateOutcome>>,
}

static LAUNCHER: OnceLock<Option<Launcher>> = OnceLock::new();

const VARS: [&str; 6] = [
    "DENT_LAUNCHER",
    "DENT_LAUNCHER_OUT",
    "DENT_LAUNCHER_IN",
    "DENT_TRIAL",
    "DENT_VERSION",
    "DENT_UPDATE_OUTCOME",
];

/// `argv` without what the launcher and systemd give their daemon, for an
/// agent: the holder keeps the environment it started with across daemon
/// updates, so one an older daemon started may still have them.
pub fn scrubbed(argv: Vec<String>) -> Vec<String> {
    let mut v = vec!["/usr/bin/env".to_string()];
    for var in VARS.iter().chain(&["INVOCATION_ID"]) {
        v.push("-u".into());
        v.push((*var).into());
    }
    v.extend(argv);
    v
}

/// Take what the launcher gave, if it ran this daemon. Call first thing in
/// `main`, before any thread starts (it changes the environment).
pub fn init() {
    LAUNCHER.get_or_init(|| {
        let launched = std::env::var_os("DENT_LAUNCHER").is_some();
        let fd = |k: &str| {
            std::env::var(k)
                .ok()?
                .parse::<i32>()
                .ok()
                .filter(|n| *n > 2)
        };
        let fds = (fd("DENT_LAUNCHER_OUT"), fd("DENT_LAUNCHER_IN"));
        let trial = std::env::var("DENT_TRIAL").as_deref() == Ok("1");
        let version = std::env::var("DENT_VERSION").unwrap_or_default();
        let outcome = std::env::var("DENT_UPDATE_OUTCOME")
            .ok()
            .and_then(|l| parse_outcome(&l));
        for v in VARS {
            // SAFETY: called before any thread starts.
            unsafe { std::env::remove_var(v) };
        }
        let (Some(out), Some(inp)) = fds else {
            return None;
        };
        // Only pipes the launcher made: descriptors 3 and 4 that something
        // else left open are none of ours.
        if !launched || !is_pipe(out) || !is_pipe(inp) {
            return None;
        }
        for fd in [out, inp] {
            // SAFETY: setting a flag on a descriptor this process holds.
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        // SAFETY: descriptors the launcher opened for this process alone,
        // taken once (behind the OnceLock).
        let (out, inp) = unsafe {
            (
                std::fs::File::from_raw_fd(out),
                std::fs::File::from_raw_fd(inp),
            )
        };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(inp).lines() {
                let Ok(line) = line else { break };
                if tx.send(line.trim().to_string()).is_err() {
                    break;
                }
            }
        });
        Some(Launcher {
            out: Mutex::new(out),
            heard: Mutex::new(rx),
            trial,
            version,
            outcome: Mutex::new(outcome),
        })
    });
}

fn is_pipe(fd: i32) -> bool {
    // SAFETY: fstat fills plain data for a descriptor number.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        libc::fstat(fd, &mut st) == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFIFO
    }
}

/// The launcher running this daemon, if one does.
pub fn get() -> Option<&'static Launcher> {
    LAUNCHER.get().and_then(Option::as_ref)
}

/// On trial: migrate and bind, then wait to be committed.
pub fn on_trial() -> bool {
    get().is_some_and(|l| l.trial)
}

/// How the last update went, as the launcher told this daemon.
pub fn outcome() -> Option<Box<dent_protocol::UpdateOutcome>> {
    let l = get()?;
    let o = l
        .outcome
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()?;
    Some(Box::new(o))
}

/// The version this daemon runs as, under the launcher.
pub fn version() -> Option<String> {
    get().map(|l| l.version.clone()).filter(|v| !v.is_empty())
}

/// `<id> <from> <to> committed|rolled_back`, as the launcher writes it.
fn parse_outcome(line: &str) -> Option<dent_protocol::UpdateOutcome> {
    let mut w = line.split(' ');
    Some(dent_protocol::UpdateOutcome {
        id: w.next()?.into(),
        from: w.next()?.into(),
        to: w.next()?.into(),
        rolled_back: match w.next()? {
            "committed" => false,
            "rolled_back" => true,
            _ => return None,
        },
    })
}

impl Launcher {
    fn say(&self, line: &str) -> Result<()> {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        out.write_all(format!("{line}\n").as_bytes())?;
        Ok(())
    }

    /// Migrated and bound: ready to be committed. Blocks until it is (the
    /// launcher says how the update went with it); a launcher that goes
    /// away instead ends the trial.
    pub fn prepared_then_wait(&self) -> Result<()> {
        self.say("prepared")?;
        let heard = self.heard.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match heard.recv() {
                Ok(l) if l == "committed" || l.starts_with("committed ") => {
                    let said = l.strip_prefix("committed ").and_then(parse_outcome);
                    *self.outcome.lock().unwrap_or_else(|e| e.into_inner()) = said;
                    return Ok(());
                }
                Ok(_) => {}
                Err(_) => bail!("the launcher went away during the trial"),
            }
        }
    }

    /// Ask the launcher to run `version`, staged under `versions/`. Accepted,
    /// the launcher stops this daemon soon after; refused, it says why.
    pub fn update(&self, version: &str) -> Result<()> {
        let heard = self.heard.lock().unwrap_or_else(|e| e.into_inner());
        // Nothing heard earlier answers this.
        while heard.try_recv().is_ok() {}
        self.say(&format!("update {version}"))?;
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match heard.recv_timeout(left) {
                Ok(l) if l == "accepted" => return Ok(()),
                Ok(l) => {
                    if let Some(why) = l.strip_prefix("refused ") {
                        bail!("{why}");
                    }
                }
                Err(RecvTimeoutError::Timeout) => bail!("the launcher didn't answer"),
                Err(RecvTimeoutError::Disconnected) => bail!("the launcher went away"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_outcome_reads_back() {
        let o = super::parse_outcome("u1 a b rolled_back").unwrap();
        assert!(o.rolled_back && o.from == "a" && o.to == "b");
        assert!(super::parse_outcome("u1 a b not_tried").is_none());
        assert!(super::parse_outcome("u1 a").is_none());
    }
}
