//! Messages inside the encrypted channel. JSON, one per frame.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Up {
    Hello {
        name: String,
    },
    /// SPAKE2 message for pairing with a code.
    Pair {
        spake: String,
    },
    /// HMAC over the channel's handshake hash, keyed by the SPAKE2 key.
    PairConfirm {
        mac: String,
    },
    Prompt {
        session: String,
        text: String,
    },
    /// Append `n` events at once, for throughput checks.
    Bulk {
        session: String,
        n: usize,
    },
    /// Everything in `session` after `after`, then live events.
    Subscribe {
        session: String,
        after: u64,
    },
    Ping {
        t: u64,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Down {
    Welcome {
        name: String,
        id: String,
        trusted: bool,
        /// How trust was decided: "known", "tailnet" or "none".
        via: String,
    },
    Pair {
        spake: String,
    },
    PairConfirm {
        mac: String,
    },
    Paired,
    Denied {
        reason: String,
    },
    Event(Event),
    Pong {
        t: u64,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Event {
    pub session: String,
    pub seq: u64,
    /// Milliseconds since the Unix epoch when the daemon logged it.
    pub t: u64,
    pub kind: String,
    pub text: String,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
