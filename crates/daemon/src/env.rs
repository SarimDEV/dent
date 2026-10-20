//! The environment agents run in. A daemon started by an app doesn't get the
//! person's shell PATH, so ask their login shell once, the way a terminal
//! would, and give agents that.

use std::process::{Command, Stdio};
use std::time::Duration;

/// What every agent runs with; which command each runs is its driver's
/// ([`crate::agent::Driver::command`]).
pub struct AgentEnv {
    pub path: String,
}

impl AgentEnv {
    pub fn detect() -> Self {
        Self {
            path: login_path().unwrap_or_else(fallback_path),
        }
    }
}

fn login_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mut child = Command::new(shell)
        .args(["-l", "-c", "printf '%s' \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // A slow or broken shell profile mustn't hold up the daemon.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut out = child.stdout.take()?;
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut out, &mut s);
        let _ = tx.send(s);
    });
    let got = rx.recv_timeout(Duration::from_secs(5)).ok();
    let _ = child.kill();
    let _ = child.wait();
    got.filter(|p| p.contains('/'))
}

fn fallback_path() -> String {
    let home = crate::paths::home();
    let mut dirs: Vec<String> = [".local/bin", ".claude/local", ".bun/bin", ".cargo/bin"]
        .iter()
        .map(|d| home.join(d).display().to_string())
        .collect();
    dirs.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ]
        .map(String::from),
    );
    if let Ok(p) = std::env::var("PATH") {
        dirs.push(p);
    }
    dirs.join(":")
}
