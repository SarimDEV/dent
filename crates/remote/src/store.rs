//! This device's key and the machines it saved, in the client state folder:
//! `DENT_CLIENT_STATE`, else `client/` beside the daemon's state
//! (`DENT_STATE`, else the platform's place for it).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dent_link::Keys;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Saved {
    /// What the machine calls itself.
    pub name: String,
    /// Its key's fingerprint ([decision 0059](../../docs/decisions/0059-machine-identity.md)).
    pub id: String,
    /// The machine's public key, hex; empty until the first trusted link.
    pub key: String,
    /// Routes added by hand, best first.
    pub routes: Vec<String>,
    /// Routes the machine reported in its welcome.
    #[serde(default)]
    pub learned: Vec<String>,
    /// How to wake it, as its welcome last said.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wake: Vec<dent_protocol::Wake>,
}

impl Saved {
    /// Every route to try, best first: the ones added by hand, then the
    /// ones the machine reported.
    pub fn all_routes(&self) -> Vec<String> {
        let mut r = self.routes.clone();
        for l in &self.learned {
            if !r.contains(l) {
                r.push(l.clone());
            }
        }
        r
    }

    pub fn known_key(&self) -> Option<Vec<u8>> {
        hex::decode(&self.key).ok().filter(|k| !k.is_empty())
    }
}

/// The state folder of this machine's own daemon (`DENT_STATE`, else the
/// platform's place for it), where its socket is.
pub fn daemon_state() -> PathBuf {
    if let Some(p) = std::env::var_os("DENT_STATE") {
        return PathBuf::from(p);
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/dent")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"))
            .join("dent")
    }
}

pub fn state_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("DENT_CLIENT_STATE") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("DENT_STATE") {
        return PathBuf::from(p).join("client");
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/dent/client")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"))
            .join("dent/client")
    }
}

/// This device's key, made on first use.
pub fn device_keys() -> Result<Keys> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    let p = dir.join("device.key");
    // Made only when there's none: one that's there but can't be read is an
    // error, since a new key in its place would unpair this device from
    // every machine.
    match std::fs::read(&p) {
        Ok(b) => {
            let t = String::from_utf8(b)
                .map_err(|_| anyhow::anyhow!("the device key at {} is damaged", p.display()))?;
            return Keys::from_hex(&t)
                .with_context(|| format!("the device key at {} is damaged", p.display()));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e)
                .with_context(|| format!("couldn't read the device key at {}", p.display()));
        }
    }
    let k = Keys::generate()?;
    write_private(&p, k.to_hex().as_bytes())?;
    Ok(k)
}

/// Written whole or not at all, readable only by this user.
pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    // The mode applies only to a new file: never reuse one a crash left.
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn load() -> Vec<Saved> {
    std::fs::read_to_string(state_dir().join("machines.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(all: &[Saved]) -> Result<()> {
    std::fs::create_dir_all(state_dir())?;
    write_private(
        &state_dir().join("machines.json"),
        serde_json::to_string_pretty(all)?.as_bytes(),
    )
}

/// Save one machine, replacing the one with its id.
pub fn upsert(m: &Saved) -> Result<()> {
    let mut all = load();
    match all.iter_mut().find(|s| s.id == m.id) {
        Some(s) => *s = m.clone(),
        None => all.push(m.clone()),
    }
    save(&all)
}

/// Forget a machine by its id.
pub fn forget(id: &str) -> Result<()> {
    let mut all = load();
    all.retain(|s| s.id != id);
    save(&all)
}

/// What this device is called, for the machines it connects to.
pub fn device_name() -> String {
    std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "dent".into())
}
