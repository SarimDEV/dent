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

    pub fn list(&self) -> Vec<(String, Peer)> {
        self.peers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Written before it's acknowledged.
    pub fn add(&mut self, public: &[u8], name: &str) -> Result<()> {
        let now = now_ms();
        self.peers.insert(
            dent_link::fingerprint(public),
            Peer {
                key: hex::encode(public),
                name: name.into(),
                added: now,
                seen: now,
                access: full(),
            },
        );
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> Result<bool> {
        let had = self.peers.remove(id).is_some();
        if had {
            self.save()?;
        }
        Ok(had)
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
}

struct Open {
    code: String,
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
        self.open = Some(Open {
            code: code.clone(),
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

    /// The code to pair against now, or why not.
    pub fn code(&mut self) -> Result<String, Refusal> {
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
        Ok(o.code.clone())
    }

    /// A pairing finished: the code is spent.
    pub fn succeeded(&mut self) {
        self.open = None;
    }

    /// A completed exchange with the wrong code. Returns the refusal to send.
    pub fn wrong(&mut self) -> Refusal {
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
            o.wrong_code = 0;
            o.paused_until = Some(Instant::now() + o.pause);
            o.pause = (o.pause * 2).min(Duration::from_secs(300));
            return Refusal::PairWrongNewCode;
        }
        Refusal::PairWrong
    }
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

    #[test]
    fn guesses_are_capped() {
        let mut w = Window::default();
        assert_eq!(w.code(), Err(Refusal::PairClosed));
        let (first, _) = w.open().unwrap();
        assert_eq!(w.code(), Ok(first.clone()));
        assert_eq!(w.wrong(), Refusal::PairWrong);
        assert_eq!(w.wrong(), Refusal::PairWrong);
        assert_eq!(w.wrong(), Refusal::PairWrongNewCode);
        // Paused: even the right code waits.
        assert!(matches!(w.code(), Err(Refusal::PairLocked { .. })));
        // Ten wrong in one window closes it.
        if let Some(o) = w.open.as_mut() {
            o.paused_until = None;
        }
        for _ in 0..6 {
            if let Some(o) = w.open.as_mut() {
                o.paused_until = None;
            }
            let _ = w.wrong();
        }
        assert_eq!(w.wrong(), Refusal::PairClosed);
        assert!(!w.is_open());
    }

    #[test]
    fn a_code_works_once() {
        let mut w = Window::default();
        w.open().unwrap();
        w.succeeded();
        assert_eq!(w.code(), Err(Refusal::PairClosed));
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
