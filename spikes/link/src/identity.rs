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
            let raw = hex::decode(hex.trim()).context("identity.key")?;
            (raw[..32].to_vec(), raw[32..].to_vec())
        } else {
            let kp = snow::Builder::new(NOISE.parse()?).generate_keypair()?;
            let mut raw = kp.private.clone();
            raw.extend(&kp.public);
            std::fs::write(&path, hex::encode(raw))?;
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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Peer {
    pub name: String,
    pub key: String,
    /// "code" or "tailnet".
    pub via: String,
    /// Last address it was reached at (clients only).
    #[serde(default)]
    pub addr: Option<String>,
}

/// Keys this device trusts, by fingerprint, saved as JSON.
pub struct Trust {
    path: PathBuf,
    pub peers: BTreeMap<String, Peer>,
}

impl Trust {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("trusted.json");
        let peers = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { path, peers }
    }

    pub fn get(&self, public: &[u8]) -> Option<&Peer> {
        self.peers.get(&fingerprint(public))
    }

    pub fn add(&mut self, public: &[u8], peer: Peer) -> Result<()> {
        self.peers.insert(fingerprint(public), peer);
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> Result<()> {
        self.peers.remove(id);
        self.save()
    }

    pub fn save(&self) -> Result<()> {
        std::fs::write(&self.path, serde_json::to_string_pretty(&self.peers)?)?;
        Ok(())
    }
}
