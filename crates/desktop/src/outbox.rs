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
    let Ok(text) = std::fs::read_to_string(path()) else {
        return Vec::new();
    };
    // Each message on its own: one that can't be read (another version's,
    // or damaged) doesn't take the rest with it. What can't be read is
    // kept beside, since the next save writes over the file.
    let entries: Option<Vec<serde_json::Value>> = serde_json::from_str(&text).ok();
    let all: Vec<Outgoing> = entries
        .iter()
        .flatten()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();
    if entries.as_ref().is_none_or(|e| e.len() != all.len()) {
        let kept = path().with_extension("json.unread");
        eprintln!(
            "some messages waiting for their machines couldn't be read; the file is kept at {}",
            kept.display()
        );
        let _ = dent_remote::store::write_private(&kept, text.as_bytes());
    }
    all
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

/// The daemon refused request `req`: a message it went as will never be
/// taken (its session is gone there), so it stops waiting. The one taken
/// out, if any.
pub fn refused(all: &mut Vec<Outgoing>, req: u64) -> Option<Outgoing> {
    let i = all.iter().position(|o| o.req == Some(req))?;
    Some(all.remove(i))
}

/// Machine `machine` was removed: its messages no longer wait for it.
/// Whether any did.
pub fn forget_machine(all: &mut Vec<Outgoing>, machine: &str) -> bool {
    let before = all.len();
    all.retain(|o| dent_client_core::fleet::split(&o.session).0 != machine);
    all.len() != before
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

    fn out(session: &str, id: &str, req: Option<u64>) -> Outgoing {
        Outgoing {
            session: session.into(),
            id: id.into(),
            text: "go".into(),
            attachments: Vec::new(),
            at: 0,
            held: false,
            req,
        }
    }

    #[test]
    fn a_refused_message_stops_waiting_and_others_stay() {
        let mut all = vec![out("ab12/s1", "m1", Some(4)), out("ab12/s1", "m2", None)];
        assert!(refused(&mut all, 9).is_none());
        assert_eq!(refused(&mut all, 4).map(|o| o.id), Some("m1".into()));
        assert_eq!(all.len(), 1);
        // A held one went as no request: nothing answers for it.
        assert!(refused(&mut all, 0).is_none());
    }

    #[test]
    fn a_removed_machines_messages_stop_waiting() {
        let mut all = vec![out("ab12/s1", "m1", Some(4)), out("cd34/s1", "m2", Some(5))];
        assert!(forget_machine(&mut all, "ab12"));
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "m2");
        assert!(!forget_machine(&mut all, "ab12"));
    }

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

#[cfg(test)]
#[path = "outbox_break_b.rs"]
mod break_b;
