//! Who this machine trusts ([decision 0061]): devices paired with a code,
//! kept on disk, and the pairing window that lets one more in.
//!
//! Trust through the Tailscale account is checked live on every connection
//! and never stored here.
//!
//! [decision 0061]: ../../../docs/decisions/0061-trust.md

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use dent_link::Refusal;
use serde::{Deserialize, Serialize};

use super::identity::write_private;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// The device's public key, in hex.
    pub key: String,
    pub name: String,
    /// When it was paired, in ms since the epoch.
    pub added: u64,
    /// When it last connected, in ms since the epoch.
    #[serde(default)]
    pub seen: u64,
    /// "full" in v1.
    #[serde(default = "full")]
    pub access: String,
}

fn full() -> String {
    "full".into()
}

/// Paired devices by id (key fingerprint).
pub struct Trust {
    path: PathBuf,
    peers: BTreeMap<String, Peer>,
}

impl Trust {
    /// A damaged file is set aside, and nobody is trusted until paired
    /// again: failing safe, never open.
    pub fn load(state: &Path) -> Self {
        let path = state.join("trusted.json");
        let peers = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(p) => p,
                Err(_) => {
                    let aside = path.with_extension(format!("damaged-{}", now_ms()));
                    let _ = std::fs::rename(&path, aside);
                    BTreeMap::new()
                }
            },
            Err(_) => BTreeMap::new(),
        };
        Self { path, peers }
    }

    pub fn get(&self, public: &[u8]) -> Option<&Peer> {
        self.peers.get(&dent_link::fingerprint(public))
    }

    pub fn has(&self, id: &str) -> bool {
        self.peers.contains_key(id)
    }

    pub fn list(&self) -> Vec<(String, Peer)> {
        self.peers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Written before it's acknowledged. Memory changes only once the
    /// file has, so a failed write trusts nobody new.
    pub fn add(&mut self, public: &[u8], name: &str) -> Result<()> {
        let now = now_ms();
        let mut peers = self.peers.clone();
        peers.insert(
            dent_link::fingerprint(public),
            Peer {
                key: hex::encode(public),
                name: name.into(),
                added: now,
                seen: now,
                access: full(),
            },
        );
        self.commit(peers)
    }

    /// Written before it's acknowledged, as [`Trust::add`].
    pub fn remove(&mut self, id: &str) -> Result<bool> {
        if !self.peers.contains_key(id) {
            return Ok(false);
        }
        let mut peers = self.peers.clone();
        peers.remove(id);
        self.commit(peers)?;
        Ok(true)
    }

    fn commit(&mut self, peers: BTreeMap<String, Peer>) -> Result<()> {
        write_private(&self.path, serde_json::to_string_pretty(&peers)?.as_bytes())?;
        self.peers = peers;
        Ok(())
    }

    /// Notes a connection. Saved at most once a minute per device.
    pub fn seen(&mut self, public: &[u8]) {
        let id = dent_link::fingerprint(public);
        let now = now_ms();
        if let Some(p) = self.peers.get_mut(&id)
            && now.saturating_sub(p.seen) > 60_000
        {
            p.seen = now;
            let _ = self.save();
        }
    }

    fn save(&self) -> Result<()> {
        write_private(
            &self.path,
            serde_json::to_string_pretty(&self.peers)?.as_bytes(),
        )
    }
}

/// How long a shown code works.
pub const CODE_TTL: Duration = Duration::from_secs(600);
/// Wrong codes before the code changes and pairing pauses.
const WRONG_PER_CODE: u32 = 3;
/// Wrong codes before the window closes and must be reopened.
const WRONG_PER_WINDOW: u32 = 10;

/// Pairing is open only while a code is showing ([decision 0033]).
///
/// [decision 0033]: ../../../docs/decisions/0033-direct-websocket-hub-on-hold.md
#[derive(Default)]
pub struct Window {
    open: Option<Open>,
    /// Counts codes shown, so an exchange begun on one code can't finish on
    /// another.
    shown: u64,
}

struct Open {
    code: String,
    /// Which code this is, from [`Window::shown`].
    nth: u64,
    /// An exchange is in flight. One at a time: otherwise many connections
    /// could each start one before any wrong guess is counted, and get far
    /// more than three guesses at a code.
    busy: bool,
    until: Instant,
    wrong_code: u32,
    wrong_window: u32,
    paused_until: Option<Instant>,
    pause: Duration,
}

impl Window {
    /// Shows a fresh code, closing any earlier one.
    pub fn open(&mut self) -> Result<(String, u64)> {
        let code = dent_link::pair::new_code()?;
        let expires_ms = now_ms() + CODE_TTL.as_millis() as u64;
        self.shown += 1;
        self.open = Some(Open {
            code: code.clone(),
            nth: self.shown,
            busy: false,
            until: Instant::now() + CODE_TTL,
            wrong_code: 0,
            wrong_window: 0,
            paused_until: None,
            pause: Duration::from_secs(1),
        });
        Ok((code, expires_ms))
    }

    pub fn is_open(&self) -> bool {
        self.open.as_ref().is_some_and(|o| Instant::now() < o.until)
    }

    /// Starts an exchange: the code to pair against now and a ticket for
    /// it, or why not. Every ticket ends in [`Window::wrong`],
    /// [`Window::succeeded`] or [`Window::abandon`].
    pub fn begin(&mut self) -> Result<(String, Ticket), Refusal> {
        let now = Instant::now();
        let Some(o) = self.open.as_mut() else {
            return Err(Refusal::PairClosed);
        };
        if now >= o.until {
            self.open = None;
            return Err(Refusal::PairClosed);
        }
        if let Some(p) = o.paused_until
            && now < p
        {
            return Err(Refusal::PairLocked {
                secs: (p - now).as_secs().max(1),
            });
        }
        if o.busy {
            return Err(Refusal::Busy { retry_ms: 1000 });
        }
        o.busy = true;
        Ok((o.code.clone(), Ticket { nth: o.nth }))
    }

    /// The open window, if `t` was begun on its current code.
    fn current(&mut self, t: &Ticket) -> Option<&mut Open> {
        self.open.as_mut().filter(|o| o.nth == t.nth)
    }

    /// An exchange ended without a guess (the connection dropped).
    pub fn abandon(&mut self, t: Ticket) {
        if let Some(o) = self.current(&t) {
            o.busy = false;
        }
    }

    /// A completed exchange with the right code. Whether it still counts:
    /// the code must still be showing, unexpired. If so, it's spent.
    pub fn succeeded(&mut self, t: Ticket) -> bool {
        let live = self.current(&t).is_some_and(|o| Instant::now() < o.until);
        if live {
            self.open = None;
        }
        live
    }

    /// A completed exchange with the wrong code. Returns the refusal to send.
    pub fn wrong(&mut self, t: Ticket) -> Refusal {
        if let Some(o) = self.current(&t) {
            o.busy = false;
        }
        let Some(o) = self.open.as_mut() else {
            return Refusal::PairClosed;
        };
        o.wrong_code += 1;
        o.wrong_window += 1;
        if o.wrong_window >= WRONG_PER_WINDOW {
            self.open = None;
            return Refusal::PairClosed;
        }
        if o.wrong_code >= WRONG_PER_CODE {
            // A new code shows, and pairing pauses, longer each time.
            if let Ok(code) = dent_link::pair::new_code() {
                o.code = code;
            }
            self.shown += 1;
            o.nth = self.shown;
            o.wrong_code = 0;
            o.paused_until = Some(Instant::now() + o.pause);
            o.pause = (o.pause * 2).min(Duration::from_secs(300));
            return Refusal::PairWrongNewCode;
        }
        Refusal::PairWrong
    }
}

/// One pairing exchange in flight, from [`Window::begin`].
#[derive(Debug)]
pub struct Ticket {
    nth: u64,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guess(w: &mut Window) -> Refusal {
        if let Some(o) = w.open.as_mut() {
            o.paused_until = None;
        }
        let (_, t) = w.begin().unwrap();
        w.wrong(t)
    }

    #[test]
    fn guesses_are_capped() {
        let mut w = Window::default();
        assert_eq!(w.begin().unwrap_err(), Refusal::PairClosed);
        let (first, _) = w.open().unwrap();
        let (code, t) = w.begin().unwrap();
        assert_eq!(code, first);
        assert_eq!(w.wrong(t), Refusal::PairWrong);
        assert_eq!(guess(&mut w), Refusal::PairWrong);
        let (_, t) = w.begin().unwrap();
        assert_eq!(w.wrong(t), Refusal::PairWrongNewCode);
        // Paused: even the right code waits.
        assert!(matches!(w.begin(), Err(Refusal::PairLocked { .. })));
        // Ten wrong in one window closes it.
        for _ in 0..6 {
            guess(&mut w);
        }
        assert_eq!(guess(&mut w), Refusal::PairClosed);
        assert!(!w.is_open());
    }

    #[test]
    fn a_code_works_once() {
        let mut w = Window::default();
        w.open().unwrap();
        let (_, t) = w.begin().unwrap();
        assert!(w.succeeded(t));
        assert_eq!(w.begin().unwrap_err(), Refusal::PairClosed);
    }

    #[test]
    fn exchanges_run_one_at_a_time_on_the_code_they_began_with() {
        let mut w = Window::default();
        w.open().unwrap();
        let (_, first) = w.begin().unwrap();
        // A second connection can't start a guess while one is in flight.
        assert!(matches!(w.begin(), Err(Refusal::Busy { .. })));
        // A dropped exchange frees the window without counting.
        w.abandon(first);
        let (_, t) = w.begin().unwrap();
        // A code shown again meanwhile: the exchange begun on the old one
        // no longer pairs, and its end doesn't free the new code's slot.
        w.open().unwrap();
        let (_, newer) = w.begin().unwrap();
        assert!(!w.succeeded(t));
        assert!(matches!(w.begin(), Err(Refusal::Busy { .. })));
        assert!(w.succeeded(newer));
    }

    #[test]
    fn trust_survives_damage_by_failing_safe() {
        let dir = std::env::temp_dir().join(format!("dent-trust-{}", crate::daemon::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut t = Trust::load(&dir);
        t.add(&[1; 32], "phone").unwrap();
        assert!(Trust::load(&dir).get(&[1; 32]).is_some());
        std::fs::write(dir.join("trusted.json"), "{ not json").unwrap();
        let t = Trust::load(&dir);
        assert!(t.get(&[1; 32]).is_none());
        assert!(
            std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().contains("damaged"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
