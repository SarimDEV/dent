//! Messages inside the encrypted channel: the same wire format as spike 05's
//! daemon. In the real build these live in the `protocol` crate and both
//! sides use them; here they're copied, and the harness proves they match by
//! talking to that daemon.

use serde::{Deserialize, Serialize};

pub const PROTO: u32 = 2;
pub const PROTO_MIN: u32 = 2;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Up {
    Hello {
        name: String,
        proto: u32,
        probe: bool,
    },
    Pair {
        spake: String,
    },
    PairConfirm {
        mac: String,
    },
    Prompt {
        id: String,
        session: String,
        text: String,
    },
    Bulk {
        session: String,
        n: usize,
    },
    Subscribe {
        session: String,
        after: u64,
    },
    /// A phone's dent-push handle and the key its notifications are
    /// encrypted with.
    Push {
        handle: String,
        key: String,
    },
    Forget,
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
        via: String,
        #[serde(default)]
        proto: u32,
        #[serde(default)]
        min: u32,
        #[serde(default)]
        info: Box<Info>,
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
    Accepted {
        id: String,
        session: String,
        dup: bool,
    },
    Event(Event),
    Bye {
        reason: String,
    },
    Health(Health),
    Pong {
        t: u64,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Event {
    pub session: String,
    pub seq: u64,
    pub t: u64,
    /// "user", "delta", "end" or "interrupted".
    pub kind: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Info {
    pub hostname: String,
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub model: String,
    pub cores: u32,
    pub dent_version: String,
    pub agents: Vec<Agent>,
    pub macs: Vec<String>,
    pub timezone: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Agent {
    pub name: String,
    pub version: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Health {
    pub at: u64,
    pub load: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_free: u64,
    pub disk_total: u64,
    pub battery: Option<Battery>,
    pub running: u32,
    pub uptime_s: u64,
    /// Why phone notifications aren't going out, when they aren't:
    /// "limited:<until ms>" or "unavailable".
    pub push: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
    pub on_battery: bool,
}

/// A random id for sessions and prompts.
pub fn new_id() -> String {
    let mut b = [0u8; 8];
    // Without randomness nothing else works either (no keys, no pairing).
    getrandom::getrandom(&mut b).expect("system randomness");
    hex::encode(b)
}
