//! The daemon's side of `dent-launcher` ([decision 0065]), when the
//! launcher runs it: two pipes, `DENT_LAUNCHER_OUT` (lines to the launcher)
//! and `DENT_LAUNCHER_IN` (lines from it).
//!
//! - On trial (`DENT_TRIAL=1`) the daemon migrates and binds, says
//!   `prepared`, and parks until told `committed`: a version that can't get
//!   that far is rolled back without ever touching an agent.
//! - Asked to update, it says `update <version>`; the launcher answers
//!   `accepted` (and then stops it) or `refused <why>`.
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
}

/// The launcher running this daemon, if one does.
pub fn get() -> Option<&'static Launcher> {
    static L: OnceLock<Option<Launcher>> = OnceLock::new();
    L.get_or_init(|| {
        std::env::var_os("DENT_LAUNCHER")?;
        let fd = |k: &str| {
            std::env::var(k)
                .ok()?
                .parse::<i32>()
                .ok()
                .filter(|n| *n > 2)
        };
        let (out, inp) = (fd("DENT_LAUNCHER_OUT")?, fd("DENT_LAUNCHER_IN")?);
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
        })
    })
    .as_ref()
}

/// On trial: migrate and bind, then wait to be committed.
pub fn on_trial() -> bool {
    get().is_some() && std::env::var("DENT_TRIAL").as_deref() == Ok("1")
}

/// How the last update went, as the launcher told this daemon.
pub fn outcome() -> Option<Box<dent_protocol::UpdateOutcome>> {
    let line = std::env::var("DENT_UPDATE_OUTCOME").ok()?;
    let mut w = line.split(' ');
    let o = dent_protocol::UpdateOutcome {
        id: w.next()?.into(),
        from: w.next()?.into(),
        to: w.next()?.into(),
        rolled_back: match w.next()? {
            "committed" => false,
            "rolled_back" => true,
            _ => return None,
        },
    };
    get().map(|_| Box::new(o))
}

/// The version this daemon runs as, under the launcher.
pub fn version() -> Option<String> {
    get()?;
    std::env::var("DENT_VERSION").ok().filter(|v| !v.is_empty())
}

impl Launcher {
    fn say(&self, line: &str) -> Result<()> {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        out.write_all(format!("{line}\n").as_bytes())?;
        Ok(())
    }

    /// Migrated and bound: ready to be committed. Blocks until it is; a
    /// launcher that goes away instead ends the trial.
    pub fn prepared_then_wait(&self) -> Result<()> {
        self.say("prepared")?;
        let heard = self.heard.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match heard.recv() {
                Ok(l) if l == "committed" => return Ok(()),
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
