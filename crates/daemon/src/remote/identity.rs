//! This machine's identity: its key, bound to the install
//! ([decision 0059]). A copied state folder (Migration Assistant, a restored
//! backup, a cloned VM) is caught at startup rather than answering as the
//! original.
//!
//! [decision 0059]: ../../../docs/decisions/0059-machine-identity.md

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use dent_link::Keys;
use serde::{Deserialize, Serialize};

/// What ties a key to the machine it was made on.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Binding {
    /// `IOPlatformUUID` on macOS, `/etc/machine-id` on Linux.
    pub hardware: String,
    /// Permanent addresses of the physical network cards (Linux), sorted.
    /// A cloned VM keeps its machine id but gets new cards ([spike 14]).
    ///
    /// [spike 14]: ../../../docs/spikes/14-routes-identity-and-boot.md
    #[serde(default)]
    pub cards: Vec<String>,
    /// The state folder's real path.
    pub state: String,
}

impl Binding {
    /// Whether `now` is the machine this binding was made on. One card
    /// changing (a new USB adapter) doesn't count; all of them changing
    /// does. No cards now (not yet up at boot) proves nothing either way.
    pub fn matches(&self, now: &Binding) -> bool {
        if self.hardware != now.hardware || self.state != now.state {
            return false;
        }
        self.cards.is_empty()
            || now.cards.is_empty()
            || self.cards.iter().any(|c| now.cards.contains(c))
    }
}

pub struct Identity {
    pub keys: Keys,
}

/// Why the identity couldn't be used. Local clients still work; only
/// remote access stays off.
#[derive(Debug)]
pub enum Problem {
    /// The binding doesn't match: the state folder was copied here.
    Copied {
        was: Box<Binding>,
        now: Box<Binding>,
    },
    Damaged(String),
    /// This machine's hardware id couldn't be read just now, so a copy
    /// can't be ruled out.
    Unverified(String),
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Problem::Copied { was, now } => write!(
                f,
                "this state folder was copied from another machine or folder (made on {} at {}, now {} at {}); remote access stays off until `dent-daemon reidentify` gives this machine its own identity",
                short(&was.hardware),
                was.state,
                short(&now.hardware),
                now.state
            ),
            Problem::Unverified(why) => write!(
                f,
                "couldn't check this machine's identity ({why}); remote access stays off until the daemon restarts"
            ),
            Problem::Damaged(why) => write!(
                f,
                "the identity key is damaged ({why}); remote access stays off rather than becoming a new machine. `dent-daemon reidentify` makes a new one"
            ),
        }
    }
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn key_path(state: &Path) -> PathBuf {
    state.join("identity.key")
}

fn binding_path(state: &Path) -> PathBuf {
    state.join("identity.binding")
}

/// Loads the identity, making one on first use. Called under the daemon's
/// state lock, so two daemons can't make two keys.
pub fn load(state: &Path) -> Result<Identity, Problem> {
    let now = current(state);
    let kp = key_path(state);
    if !kp.exists() {
        let keys = Keys::generate().map_err(|e| Problem::Damaged(format!("{e:#}")))?;
        write_private(&kp, keys.to_hex().as_bytes())
            .map_err(|e| Problem::Damaged(format!("{e:#}")))?;
        save_binding(state, &now).map_err(|e| Problem::Damaged(format!("{e:#}")))?;
        return Ok(Identity { keys });
    }
    let text = std::fs::read_to_string(&kp).map_err(|e| Problem::Damaged(e.to_string()))?;

    let keys = Keys::from_hex(&text).map_err(|e| Problem::Damaged(format!("{e:#}")))?;
    match std::fs::read_to_string(binding_path(state)) {
        Ok(b) => {
            let was: Binding =
                serde_json::from_str(&b).map_err(|e| Problem::Damaged(format!("binding: {e}")))?;
            if !was.hardware.is_empty() && now.hardware.is_empty() {
                // A failed read isn't a different machine.
                return Err(Problem::Unverified("no hardware id".into()));
            }
            if !was.matches(&now) {
                return Err(Problem::Copied {
                    was: Box::new(was),
                    now: Box::new(now),
                });
            }
            // Keep the card list current, so a slowly changing set of cards
            // never drifts out of reach.
            if was.cards != now.cards {
                let _ = save_binding(state, &now);
            }
        }
        // A key from before bindings: bind it now, once.
        Err(_) => {
            let _ = save_binding(state, &now);
        }
    }
    Ok(Identity { keys })
}

/// Replaces the identity with a new one: this machine becomes a new machine
/// to every device. Its sessions stay as they are.
pub fn reidentify(state: &Path) -> Result<Identity> {
    let keys = Keys::generate()?;
    write_private(&key_path(state), keys.to_hex().as_bytes())?;
    save_binding(state, &current(state))?;
    Ok(Identity { keys })
}

fn save_binding(state: &Path, b: &Binding) -> Result<()> {
    write_private(
        &binding_path(state),
        serde_json::to_string_pretty(b)?.as_bytes(),
    )
}

/// Written whole or not at all, readable only by this user.
pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("write {}", tmp.display()))?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent()
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

/// This machine as it is now.
pub fn current(state: &Path) -> Binding {
    Binding {
        hardware: hardware_id().unwrap_or_default(),
        cards: physical_cards(),
        state: std::fs::canonicalize(state)
            .unwrap_or_else(|_| state.to_path_buf())
            .display()
            .to_string(),
    }
}

#[cfg(target_os = "macos")]
fn hardware_id() -> Result<String> {
    let out = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(rest) = line.split("\"IOPlatformUUID\" = \"").nth(1) {
            return Ok(rest.trim_end_matches('"').to_string());
        }
    }
    bail!("no IOPlatformUUID")
}

#[cfg(not(target_os = "macos"))]
fn hardware_id() -> Result<String> {
    for p in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(id) = std::fs::read_to_string(p) {
            let id = id.trim();
            if !id.is_empty() {
                return Ok(id.to_string());
            }
        }
    }
    bail!("no machine id")
}

/// Addresses of network cards backed by a device (not bridges, tunnels or
/// virtual interfaces). Empty on macOS, where the hardware id already
/// differs per machine.
fn physical_cards() -> Vec<String> {
    let mut cards = Vec::new();
    if cfg!(target_os = "linux")
        && let Ok(dir) = std::fs::read_dir("/sys/class/net")
    {
        for e in dir.flatten() {
            let p = e.path();
            if !p.join("device").exists() {
                continue;
            }
            // Only addresses burned into the card. A random or set one
            // (Wi-Fi privacy addresses) changes on its own and would look
            // like a new machine.
            let permanent =
                std::fs::read_to_string(p.join("addr_assign_type")).is_ok_and(|t| t.trim() == "0");
            if !permanent {
                continue;
            }
            if let Ok(a) = std::fs::read_to_string(p.join("address")) {
                let a = a.trim().to_string();
                if !a.is_empty() && a != "00:00:00:00:00:00" {
                    cards.push(a);
                }
            }
        }
    }
    cards.sort();
    cards
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(hw: &str, cards: &[&str], state: &str) -> Binding {
        Binding {
            hardware: hw.into(),
            cards: cards.iter().map(|c| c.to_string()).collect(),
            state: state.into(),
        }
    }

    #[test]
    fn a_clone_is_caught_and_one_new_card_is_not() {
        let orig = b("m1", &["aa", "bb"], "/s");
        assert!(orig.matches(&b("m1", &["aa", "bb"], "/s")));
        assert!(
            orig.matches(&b("m1", &["aa", "cc"], "/s")),
            "one card changed"
        );
        assert!(
            !orig.matches(&b("m1", &["cc", "dd"], "/s")),
            "cloned VM: same id, new cards"
        );
        assert!(
            !orig.matches(&b("m2", &["aa", "bb"], "/s")),
            "another machine"
        );
        assert!(
            !orig.matches(&b("m1", &["aa", "bb"], "/elsewhere")),
            "folder copied"
        );
        assert!(
            b("m1", &[], "/s").matches(&b("m1", &["zz"], "/s")),
            "no cards recorded"
        );
        assert!(
            orig.matches(&b("m1", &[], "/s")),
            "cards not up yet at boot"
        );
    }

    #[test]
    fn identity_is_stable_and_a_copy_is_refused() {
        let dir = std::env::temp_dir().join(format!("dent-id-{}", crate::daemon::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = load(&dir).unwrap();
        let again = load(&dir).unwrap();
        assert_eq!(a.keys.public, again.keys.public);
        // Moving the folder is what a copy looks like from here.
        let moved = dir.with_extension("moved");
        std::fs::rename(&dir, &moved).unwrap();
        assert!(matches!(load(&moved), Err(Problem::Copied { .. })));
        let fresh = reidentify(&moved).unwrap();
        assert_ne!(fresh.keys.public, a.keys.public);
        assert!(load(&moved).is_ok());
        std::fs::write(key_path(&moved), "nonsense").unwrap();
        assert!(matches!(load(&moved), Err(Problem::Damaged(_))));
        let _ = std::fs::remove_dir_all(&moved);
    }
}
