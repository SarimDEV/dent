//! Sessions as conversations. Events from the daemon (each logged with a
//! sequence number) become turns: a user message, then an agent reply built
//! from its deltas. Replies are parsed into blocks lazily, when the host
//! asks for updates, so a burst of 10,000 deltas costs one parse rather than
//! 10,000.
//!
//! Updates are incremental: committed blocks never change, so each update
//! says how many blocks the UI already has that stay (`keep`), then lists
//! the blocks after them.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::blocks::{self, Block};
use crate::proto::Event;
use crate::stream::Streamer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum Role {
    User,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum Turn {
    Streaming,
    Done,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct MessageUpdate {
    pub session: String,
    pub index: u32,
    pub role: Role,
    pub state: Turn,
    /// When the daemon logged the message's first event.
    pub t: u64,
    /// Blocks the UI already has that stay as they are.
    pub keep: u32,
    /// The blocks after those.
    pub blocks: Vec<Block>,
}

struct Message {
    role: Role,
    state: Turn,
    t: u64,
    streamer: Streamer,
    /// Text received but not yet parsed.
    unparsed: String,
    finished: bool,
    /// Streamer blocks already converted into `ui`.
    converted: usize,
    ui: Vec<Block>,
    /// Committed blocks the UI has been sent.
    sent: usize,
}

impl Message {
    fn new(role: Role, t: u64) -> Self {
        Self {
            role,
            state: Turn::Streaming,
            t,
            streamer: Streamer::new(true),
            unparsed: String::new(),
            finished: false,
            converted: 0,
            ui: Vec::new(),
            sent: 0,
        }
    }

    fn update(&mut self, session: &str, index: usize) -> MessageUpdate {
        if !self.unparsed.is_empty() {
            let text = std::mem::take(&mut self.unparsed);
            self.streamer.push(&text);
        }
        if self.state != Turn::Streaming && !self.finished {
            self.streamer.finish();
            self.finished = true;
        }
        for b in &self.streamer.committed[self.converted..] {
            self.ui.extend(blocks::convert(b));
        }
        self.converted = self.streamer.committed.len();
        let mut out = self.ui[self.sent.min(self.ui.len())..].to_vec();
        out.extend(self.streamer.tail.iter().flat_map(blocks::convert));
        let keep = self.sent;
        self.sent = self.ui.len();
        MessageUpdate {
            session: session.to_string(),
            index: index as u32,
            role: self.role,
            state: self.state,
            t: self.t,
            keep: keep as u32,
            blocks: out,
        }
    }
}

#[derive(Default)]
struct Session {
    last: u64,
    messages: Vec<Message>,
    dirty: BTreeSet<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    New,
    Dup,
    /// New, but one or more events before it never arrived.
    Gap,
}

#[derive(Default)]
pub struct Chat {
    sessions: BTreeMap<String, Session>,
}

impl Chat {
    /// The last sequence number held for `session`: resume from here.
    pub fn last(&self, session: &str) -> u64 {
        self.sessions.get(session).map_or(0, |s| s.last)
    }

    pub fn sessions(&self) -> impl Iterator<Item = &String> {
        self.sessions.keys()
    }

    pub fn touch(&mut self, session: &str) {
        self.sessions.entry(session.to_string()).or_default();
    }

    pub fn apply(&mut self, ev: Event) -> Applied {
        let s = self.sessions.entry(ev.session.clone()).or_default();
        if ev.seq <= s.last {
            return Applied::Dup;
        }
        let applied = if ev.seq > s.last + 1 {
            Applied::Gap
        } else {
            Applied::New
        };
        s.last = ev.seq;
        let open = s
            .messages
            .last()
            .is_some_and(|m| m.role == Role::Agent && m.state == Turn::Streaming);
        match ev.kind.as_str() {
            "user" => {
                let mut m = Message::new(Role::User, ev.t);
                m.unparsed = ev.text;
                m.state = Turn::Done;
                s.messages.push(m);
            }
            "delta" => {
                if !open {
                    s.messages.push(Message::new(Role::Agent, ev.t));
                }
                if let Some(m) = s.messages.last_mut() {
                    m.unparsed.push_str(&ev.text);
                }
            }
            "end" | "interrupted" => {
                if open && let Some(m) = s.messages.last_mut() {
                    m.state = if ev.kind == "end" {
                        Turn::Done
                    } else {
                        Turn::Interrupted
                    };
                }
            }
            _ => return applied,
        }
        let i = s.messages.len().saturating_sub(1);
        s.dirty.insert(i);
        applied
    }

    /// Updates for every message changed since the last call.
    pub fn drain(&mut self) -> Vec<MessageUpdate> {
        let mut out = Vec::new();
        for (id, s) in &mut self.sessions {
            for i in std::mem::take(&mut s.dirty) {
                if let Some(m) = s.messages.get_mut(i) {
                    out.push(m.update(id, i));
                }
            }
        }
        out
    }

    /// Everything in `session` again from the start, for a UI that has
    /// just opened it.
    pub fn resend(&mut self, session: &str) {
        if let Some(s) = self.sessions.get_mut(session) {
            for (i, m) in s.messages.iter_mut().enumerate() {
                m.sent = 0;
                s.dirty.insert(i);
            }
        }
    }

    /// The whole text of each message, for checks.
    pub fn texts(&self, session: &str) -> Vec<(Role, Turn, String)> {
        self.sessions.get(session).map_or(Vec::new(), |s| {
            s.messages
                .iter()
                .map(|m| {
                    (
                        m.role,
                        m.state,
                        format!("{}{}", m.streamer.text, m.unparsed),
                    )
                })
                .collect()
        })
    }
}

/// Applies updates the way a UI does, for tests and the harness.
#[derive(Default, Debug)]
pub struct Mirror {
    pub messages: BTreeMap<(String, u32), (Role, Turn, Vec<Block>)>,
}

impl Mirror {
    pub fn apply(&mut self, u: &MessageUpdate) {
        let e = self
            .messages
            .entry((u.session.clone(), u.index))
            .or_insert((u.role, u.state, Vec::new()));
        e.1 = u.state;
        e.2.truncate(u.keep as usize);
        e.2.extend(u.blocks.iter().cloned());
    }

    pub fn blocks(&self, session: &str, index: u32) -> Option<&Vec<Block>> {
        self.messages
            .get(&(session.to_string(), index))
            .map(|m| &m.2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = "Here's what I found.\n\n## The flaky test\n\nThe **retry loop** in `sync.rs` re-reads the cursor *after* the reset.\n\n1. Read the cursor once\n2. Pass it in\n\n```rust\nfn resume(after: u64) {}\n```\n\n| step | ms |\n| - | - |\n| a | 1 |\n| b | 2 |\n\n> Logged before it's sent.\n\nDone.";

    fn ev(seq: u64, kind: &str, text: &str) -> Event {
        Event {
            session: "s".into(),
            seq,
            t: seq,
            kind: kind.into(),
            text: text.into(),
        }
    }

    /// Split the reply at every possible point, draining after each piece:
    /// the UI must end up with exactly the blocks of a one-shot parse, and
    /// blocks it was told to keep must never have changed.
    #[test]
    fn every_split_point() {
        let want = blocks::parse(REPLY);
        for cut in 1..REPLY.len() {
            if !REPLY.is_char_boundary(cut) {
                continue;
            }
            let mut c = Chat::default();
            let mut m = Mirror::default();
            c.apply(ev(1, "delta", &REPLY[..cut]));
            for u in c.drain() {
                m.apply(&u);
            }
            c.apply(ev(2, "delta", &REPLY[cut..]));
            for u in c.drain() {
                // Blocks the UI keeps are final: they're already the
                // finished reply's.
                let k = u.keep as usize;
                assert_eq!(
                    m.blocks("s", 0).unwrap()[..k],
                    want[..k],
                    "kept blocks changed at {cut}"
                );
                m.apply(&u);
            }
            c.apply(ev(3, "end", ""));
            for u in c.drain() {
                m.apply(&u);
            }
            assert_eq!(m.blocks("s", 0).unwrap(), &want, "split at {cut}");
        }
    }

    #[test]
    fn word_by_word_matches() {
        let want = blocks::parse(REPLY);
        let mut c = Chat::default();
        let mut m = Mirror::default();
        let mut seq = 0;
        for w in REPLY.split_inclusive(' ') {
            seq += 1;
            c.apply(ev(seq, "delta", w));
            for u in c.drain() {
                m.apply(&u);
            }
        }
        c.apply(ev(seq + 1, "end", ""));
        for u in c.drain() {
            m.apply(&u);
        }
        assert_eq!(m.blocks("s", 0).unwrap(), &want);
    }

    #[test]
    fn coalesces_and_dedupes() {
        let mut c = Chat::default();
        assert_eq!(c.apply(ev(1, "user", "hi")), Applied::New);
        for i in 2..10_002 {
            c.apply(ev(i, "delta", "word "));
        }
        assert_eq!(c.apply(ev(5, "delta", "again")), Applied::Dup);
        assert_eq!(c.apply(ev(10_005, "end", "")), Applied::Gap);
        let u = c.drain();
        assert_eq!(u.len(), 2, "one update per message, not per delta");
        assert_eq!(u[1].state, Turn::Done);
        assert!(c.drain().is_empty());
        c.resend("s");
        assert_eq!(c.drain().len(), 2);
    }
}
