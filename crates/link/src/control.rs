//! Control messages: the small, single-frame lane that never waits behind
//! data. Liveness, goodbyes, refusals and pairing travel here.
//!
//! Kinds are only ever added; a side that doesn't know a kind skips it.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    /// Answered with a [`Control::Pong`] carrying the same `t`. Any frame
    /// counts as life, not only a pong ([decision 0062]).
    ///
    /// [decision 0062]: ../../docs/decisions/0062-liveness-and-retry.md
    Ping {
        t: u64,
    },
    Pong {
        t: u64,
    },
    /// The machine is going away on purpose.
    Goodbye(Goodbye),
    /// Why the other side won't go on, and whether trying again can help.
    Refuse(Refusal),
    /// The daemon's verdict on this device once the handshake is done.
    Trust(Trust),
    /// SPAKE2 message for pairing with a code.
    Pair {
        spake: String,
    },
    /// HMAC over the channel's handshake hash, keyed by the SPAKE2 key.
    PairConfirm {
        mac: String,
    },
    /// Pairing finished: this device is now trusted on every route.
    Paired {
        access: Access,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Goodbye {
    Sleep,
    Quit,
    /// Back after about `expected_down_ms`; clients redial quickly.
    Restart {
        expected_down_ms: u64,
    },
    Update {
        to: String,
        expected_down_ms: u64,
    },
    /// Awake again: clients waiting on backoff redial now.
    Awake,
}

/// How far a device is trusted ([decision 0061]). v1 has one level.
///
/// [decision 0061]: ../../docs/decisions/0061-trust.md
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    #[default]
    Full,
    /// A level a newer daemon granted that this side doesn't know.
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "how", rename_all = "snake_case")]
pub enum Trust {
    /// Paired with a code: trusted on every route.
    Paired { access: Access },
    /// Same Tailscale account, checked on this connection only.
    Tailscale { access: Access },
    /// Not trusted. Pairing is open on this machine while a code shows.
    Untrusted { pairing_open: bool },
}

/// Each refusal says whether retrying can help ([decision 0060]).
///
/// [decision 0060]: ../../docs/decisions/0060-remote-wire.md
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Refusal {
    /// No protocol in common: blocked until one side updates.
    Incompatible {
        daemon: (u32, u32),
        client: (u32, u32),
    },
    /// Pairing is needed.
    NotTrusted,
    /// Trusted, but not over this route (for example, Tailscale-only trust
    /// reached over the LAN). Other routes still work.
    TrustedOn {
        routes: Vec<String>,
    },
    /// This device was removed. Only believed inside an authenticated
    /// channel, which every control message is.
    Removed,
    /// Still loading; try again shortly.
    Starting {
        retry_ms: u64,
    },
    Busy {
        retry_ms: u64,
    },
    RateLimited {
        retry_ms: u64,
    },
    /// Pairing outcomes.
    PairWrong,
    /// Three wrong codes: a new code is showing there.
    PairWrongNewCode,
    /// No code is showing there, or it expired.
    PairClosed,
    /// Paused after wrong codes; try again in this many seconds.
    PairLocked {
        secs: u64,
    },
    /// LAN connections are off on that machine.
    LanOff,
}

impl Refusal {
    /// Whether waiting on a timer can help, or only a change (an update,
    /// pairing, another route, the person) can.
    pub fn transient(&self) -> bool {
        matches!(
            self,
            Refusal::Starting { .. } | Refusal::Busy { .. } | Refusal::RateLimited { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shapes_are_stable() {
        let s = serde_json::to_string(&Control::Refuse(Refusal::TrustedOn {
            routes: vec!["tailscale".into()],
        }))
        .unwrap();
        assert_eq!(
            s,
            r#"{"type":"refuse","code":"trusted_on","routes":["tailscale"]}"#
        );
        let s = serde_json::to_string(&Control::Goodbye(Goodbye::Restart {
            expected_down_ms: 3000,
        }))
        .unwrap();
        assert_eq!(
            s,
            r#"{"type":"goodbye","reason":"restart","expected_down_ms":3000}"#
        );
        let s = serde_json::to_string(&Control::Trust(Trust::Paired {
            access: Access::Full,
        }))
        .unwrap();
        assert_eq!(s, r#"{"type":"trust","how":"paired","access":"full"}"#);
    }

    #[test]
    fn unknown_access_levels_decode() {
        let t: Trust = serde_json::from_str(r#"{"how":"paired","access":"read_only"}"#).unwrap();
        assert_eq!(
            t,
            Trust::Paired {
                access: Access::Unknown
            }
        );
    }

    #[test]
    fn only_load_refusals_are_transient() {
        assert!(Refusal::Starting { retry_ms: 1 }.transient());
        assert!(!Refusal::NotTrusted.transient());
        assert!(!Refusal::Removed.transient());
        assert!(
            !Refusal::Incompatible {
                daemon: (1, 1),
                client: (2, 2)
            }
            .transient()
        );
    }
}
