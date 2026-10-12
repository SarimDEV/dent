//! A device's long-lived key pair (X25519, the Noise static key) and the
//! keys it trusts. Each daemon and each client has one, in its state folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const NOISE: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

#[derive(Clone)]
pub struct Identity {
    pub name: String,
    pub private: Vec<u8>,
    pub public: Vec<u8>,
}

impl Identity {
    /// Load the key pair from `dir`, creating it on first run.
    pub fn load(dir: &Path, name: &str) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("identity.key");
        let (private, public) = if let Ok(hex) = std::fs::read_to_string(&path) {
            // A damaged key is an error, never a silent new identity: every
            // paired machine would stop recognising this one.
            let raw = hex::decode(hex.trim()).context("identity.key is not hex")?;
            anyhow::ensure!(
                raw.len() == 64,
                "identity.key has {} bytes, not 64",
                raw.len()
            );
            private_file(&path);
            (raw[..32].to_vec(), raw[32..].to_vec())
        } else {
            let kp = snow::Builder::new(NOISE.parse()?).generate_keypair()?;
            let mut raw = kp.private.clone();
            raw.extend(&kp.public);
            write_atomic(&path, hex::encode(raw).as_bytes())?;
            private_file(&path);
            (kp.private, kp.public)
        };
        Ok(Self {
            name: name.to_string(),
            private,
            public,
        })
    }

    pub fn id(&self) -> String {
        fingerprint(&self.public)
    }
}

/// A short, stable name for a public key.
pub fn fingerprint(public: &[u8]) -> String {
    hex::encode(&Sha256::digest(public)[..6])
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Peer {
    pub name: String,
    pub key: String,
    /// "code" or "tailnet".
    pub via: String,
    /// Last address it was reached at (clients only).
    #[serde(default)]
    pub addr: Option<String>,
    /// When it was paired, and last connected (ms since the Unix epoch).
    #[serde(default)]
    pub paired_at: u64,
    #[serde(default)]
    pub last_seen: u64,
    /// The dent version it last reported (clients only).
    #[serde(default)]
    pub version: String,
}

/// Only this user can read the private key.
fn private_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Write to a temporary file and rename it over the old one, so a crash
/// mid-write leaves the old file or the new one, never half of each.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp{}-{n}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Read-modify-write of a trust file happens under this, so two pairings
/// finishing at once can't drop one.
static TRUST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Keys this device trusts, by fingerprint, saved as JSON.
pub struct Trust {
    path: PathBuf,
    pub peers: BTreeMap<String, Peer>,
}

impl Trust {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("trusted.json");
        let peers = match std::fs::read_to_string(&path) {
            Err(_) => BTreeMap::new(),
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                // Keep the damaged file for inspection; start with no
                // trust, which fails safe (machines ask to pair again).
                let aside = path.with_extension(format!("corrupt-{}", crate::proto::now_ms()));
                eprintln!(
                    "trusted.json unreadable ({e}); moved to {}",
                    aside.display()
                );
                let _ = std::fs::rename(&path, &aside);
                BTreeMap::new()
            }),
        };
        Self { path, peers }
    }

    /// Change the trust file under the process-wide lock.
    pub fn update<T>(dir: &Path, f: impl FnOnce(&mut BTreeMap<String, Peer>) -> T) -> Result<T> {
        let _g = TRUST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut t = Self::load(dir);
        let out = f(&mut t.peers);
        t.save()?;
        Ok(out)
    }

    pub fn get(&self, public: &[u8]) -> Option<&Peer> {
        self.peers.get(&fingerprint(public))
    }

    pub fn add(&mut self, public: &[u8], peer: Peer) -> Result<()> {
        let dir = self
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let id = fingerprint(public);
        self.peers = Self::update(&dir, |p| {
            p.insert(id, peer);
            p.clone()
        })?;
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<()> {
        let dir = self
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        self.peers = Self::update(&dir, |p| {
            p.remove(id);
            p.clone()
        })?;
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        write_atomic(
            &self.path,
            serde_json::to_string_pretty(&self.peers)?.as_bytes(),
        )
    }
}
