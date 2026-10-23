//! A daemon the launcher runs, and the two pipes between them.
//!
//! The daemon finds them as descriptors 3 (its lines to the launcher:
//! `prepared`, `update <version>`) and 4 (the launcher's to it:
//! `committed`, `accepted`, `refused <why>`), named by `DENT_LAUNCHER_OUT`
//! and `DENT_LAUNCHER_IN`. The launcher keeps its ends open while the
//! daemon runs, so the daemon never reads an end of file as the launcher
//! going away, and closes them when it stops, so restarts leak nothing.

use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The daemon exits with this when another daemon already runs for the
/// state folder (`EX_TEMPFAIL`): the launcher waits, it doesn't loop.
pub const LOCKED: i32 = 75;

pub struct Daemon {
    child: Child,
    to: io::PipeWriter,
    from: io::PipeReader,
    buf: Vec<u8>,
    pub started: Instant,
}

/// How a daemon ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// It chose to (asked to restart, or another daemon took over).
    Clean,
    /// Another daemon has the state folder.
    Locked,
    /// It crashed, or was killed.
    Crashed(String),
}

impl Daemon {
    /// Start `versions/<version>/dent-daemon`. On `trial` it migrates and
    /// binds, says `prepared` and waits to be told `committed`.
    pub fn start(
        state: &Path,
        version: &str,
        trial: bool,
        outcome: Option<&str>,
    ) -> io::Result<Self> {
        let exe = state.join("versions").join(version).join("dent-daemon");
        // Ours, and the daemon's, of each pipe. Made close-on-exec; the
        // daemon's ends are moved to 3 and 4 just before it runs.
        let (from, their_out) = io::pipe()?;
        let (their_in, to) = io::pipe()?;
        set_nonblocking(&from)?;
        let (out_fd, in_fd) = (their_out.as_raw_fd(), their_in.as_raw_fd());
        let mut cmd = Command::new(&exe);
        cmd.arg("--state")
            .arg(state)
            .stdin(Stdio::null())
            .env("DENT_LAUNCHER", "1")
            .env("DENT_LAUNCHER_OUT", "3")
            .env("DENT_LAUNCHER_IN", "4")
            .env("DENT_TRIAL", if trial { "1" } else { "0" })
            .env("DENT_VERSION", version)
            .env("DENT_UPDATE_OUTCOME", outcome.unwrap_or(""));
        // SAFETY: only async-signal-safe calls between fork and exec. The
        // ends are first copied above 10, so moving one onto 3 can't
        // overwrite the other.
        unsafe {
            cmd.pre_exec(move || {
                let o = libc::fcntl(out_fd, libc::F_DUPFD_CLOEXEC, 10);
                let i = libc::fcntl(in_fd, libc::F_DUPFD_CLOEXEC, 10);
                if o < 0 || i < 0 || libc::dup2(o, 3) < 0 || libc::dup2(i, 4) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        drop((their_out, their_in));
        Ok(Daemon {
            child,
            to,
            from,
            buf: Vec::new(),
            started: Instant::now(),
        })
    }

    /// Say a line to the daemon. One that's gone hears nothing.
    pub fn say(&mut self, line: &str) {
        let _ = self.to.write_all(format!("{line}\n").as_bytes());
    }

    /// The whole lines the daemon has said since last asked.
    pub fn lines(&mut self) -> Vec<String> {
        let mut chunk = [0u8; 512];
        loop {
            match self.from.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
            // A daemon that never ends a line can't grow this forever.
            if self.buf.len() > 64 * 1024 {
                self.buf.clear();
            }
        }
        let mut out = Vec::new();
        while let Some(at) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=at).collect();
            out.push(String::from_utf8_lossy(&line).trim().to_string());
        }
        out
    }

    /// Whether it has ended, and how.
    pub fn ended(&mut self) -> Option<Ended> {
        let status = self.child.try_wait().ok()??;
        Some(classify(status))
    }

    /// Stop it, saying why first (`stop-reason`, read by the daemon as it
    /// goes, so devices hear "restarting" or "updating" rather than "quit").
    /// Asked politely, then made to.
    pub fn stop(mut self, state: &Path, reason: Option<&str>) -> Ended {
        if let Some(why) = reason {
            let _ = crate::disk::write_atomic(&state.join("stop-reason"), why.as_bytes());
        }
        if let Some(e) = self.ended() {
            return e;
        }
        // SAFETY: signalling our own child by its pid.
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if let Some(e) = self.ended() {
                return e;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        self.child
            .wait()
            .map(classify)
            .unwrap_or(Ended::Crashed("lost".into()))
    }
}

fn classify(status: std::process::ExitStatus) -> Ended {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(0), _) => Ended::Clean,
        (Some(LOCKED), _) => Ended::Locked,
        (Some(c), _) => Ended::Crashed(format!("exited {c}")),
        (None, Some(s)) => Ended::Crashed(format!("killed by signal {s}")),
        _ => Ended::Crashed("ended".into()),
    }
}

fn set_nonblocking(r: &io::PipeReader) -> io::Result<()> {
    let fd = r.as_raw_fd();
    // SAFETY: changing flags on a descriptor we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
