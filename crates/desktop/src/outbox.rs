//! Messages for a session on another machine that it hasn't taken yet
//! ([decision 0063]): kept on this device and saved to disk, so they
//! survive the app quitting, shown as the session's last message, waiting,
//! and sent once by their own id when the machine is back. One written
//! more than 12 hours ago waits for a Send now instead of going by itself.
//!
//! [decision 0063]: ../../docs/decisions/0063-queued-prompts.md

use serde::{Deserialize, Serialize};

/// Older than this, a message saved from an earlier run asks before going.
pub const STALE_MS: u64 = 12 * 60 * 60 * 1000;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Outgoing {
    /// The client's session id (qualified with its machine).
    pub session: String,
    /// The message's own id: the daemon takes it once.
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<String>,
    /// When it was written, ms since the epoch.
    pub at: u64,
    /// Waiting for Send now: written long ago, in an earlier run.
    #[serde(default)]
    pub held: bool,
    /// The request it went out as in this run; none while held.
    #[serde(skip)]
    pub req: Option<u64>,
}

fn path() -> std::path::PathBuf {
    dent_remote::store::state_dir().join("outbox.json")
}

pub fn load() -> Vec<Outgoing> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(all: &[Outgoing]) {
    let write = || -> Result<(), String> {
        if all.is_empty() {
            let _ = std::fs::remove_file(path());
            return Ok(());
        }
        std::fs::create_dir_all(dent_remote::store::state_dir()).map_err(|e| e.to_string())?;
        let json = serde_json::to_string(all).map_err(|e| e.to_string())?;
        dent_remote::store::write_private(&path(), json.as_bytes()).map_err(|e| e.to_string())
    };
    if let Err(e) = write() {
        eprintln!("couldn't save the messages waiting for their machines: {e}");
    }
}

/// From an earlier run: which go again now, and which ask first.
pub fn reload(mut all: Vec<Outgoing>, now: u64) -> Vec<Outgoing> {
    for o in &mut all {
        o.held = o.held || now.saturating_sub(o.at) > STALE_MS;
        o.req = None;
    }
    all.sort_by_key(|o| o.at);
    all
}

/// "14 h", for a held message's note.
pub fn age(at: u64, now: u64) -> String {
    let h = now.saturating_sub(at) / 3_600_000;
    if h >= 48 {
        format!("{} days", h / 24)
    } else {
        format!("{h} h")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_from_long_ago_asks_before_it_goes() {
        let o = |id: &str, at: u64| Outgoing {
            session: "ab12/s1".into(),
            id: id.into(),
            text: "go".into(),
            attachments: Vec::new(),
            at,
            held: false,
            req: Some(7),
        };
        let now = 100 * 3_600_000;
        let back = reload(
            vec![o("new", now - 3_600_000), o("old", now - 14 * 3_600_000)],
            now,
        );
        assert_eq!(back[0].id, "old", "oldest first, as they were written");
        assert!(back[0].held && !back[1].held);
        assert!(back.iter().all(|o| o.req.is_none()));
        assert_eq!(age(back[0].at, now), "14 h");
    }
}
