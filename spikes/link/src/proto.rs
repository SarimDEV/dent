//! Messages inside the encrypted channel. JSON, one per frame.

use serde::{Deserialize, Serialize};

/// This build's protocol version, and the oldest it still talks to. A
/// newer client keeps working with an older daemon inside that range.
pub const PROTO: u32 = 2;
pub const PROTO_MIN: u32 = 2;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Up {
    Hello {
        name: String,
        #[serde(default)]
        proto: u32,
        /// Only looking (discovery): never changes what the daemon trusts.
        #[serde(default)]
        probe: bool,
    },
    /// SPAKE2 message for pairing with a code.
    Pair {
        spake: String,
    },
    /// HMAC over the channel's handshake hash, keyed by the SPAKE2 key.
    PairConfirm {
        mac: String,
    },
    /// `id` makes a retried prompt run once.
    Prompt {
        id: String,
        session: String,
        text: String,
    },
    /// Append `n` events at once, for throughput checks.
    Bulk {
        session: String,
        n: usize,
    },
    /// Everything in `session` after `after`, then live events. The session
    /// `_status` is the machine's health instead.
    Subscribe {
        session: String,
        after: u64,
    },
    /// This device removed the daemon: forget it on that side too.
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
        /// How trust was decided: "known", "tailnet", "tailnet-only" (trusted
        /// through Tailscale, so only on the tailnet) or "none".
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
    /// "pair first", "closed", "wrong", "wrong-new-code", "locked:<secs>",
    /// "removed" or "version".
    Denied {
        reason: String,
    },
    /// The prompt is logged and will run; `dup` if this id already ran.
    Accepted {
        id: String,
        session: String,
        dup: bool,
    },
    Event(Event),
    /// Sent on the way out: "sleep", "quit" or "restart".
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
    /// Milliseconds since the Unix epoch when the daemon logged it.
    pub t: u64,
    /// "user", "delta", "end" or "interrupted".
    pub kind: String,
    pub text: String,
    /// For agent output: which process and where in its output this
    /// ended, so a restarted daemon resumes from exactly there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<Source>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Source {
    pub proc: String,
    pub off: u64,
}

/// What a machine is, sent once when a connection opens.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Info {
    pub hostname: String,
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub model: String,
    pub cores: u32,
    pub dent_version: String,
    /// Agent CLIs installed there, with versions.
    pub agents: Vec<Agent>,
    /// Hardware addresses of its network interfaces, for Wake-on-LAN.
    pub macs: Vec<String>,
    pub timezone: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Agent {
    pub name: String,
    pub version: Option<String>,
}

/// How a machine is doing right now, sent every few seconds to watchers.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Health {
    /// The daemon's clock when measured; only used to spot skew.
    pub at: u64,
    pub load: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_free: u64,
    pub disk_total: u64,
    /// `None` on machines without a battery.
    pub battery: Option<Battery>,
    pub running: u32,
    pub uptime_s: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
    pub on_battery: bool,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A random id for sessions and prompts: unique across clients and restarts.
pub fn new_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}
