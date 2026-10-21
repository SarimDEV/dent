//! A device's long-lived key pair. Its fingerprint is the device's id.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::noise::PATTERN;

#[derive(Clone)]
pub struct Keys {
    pub private: Vec<u8>,
    pub public: Vec<u8>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // Never print the private half.
        write!(f, "Keys({})", fingerprint(&self.public))
    }
}

impl Keys {
    pub fn generate() -> Result<Self> {
        let kp = snow::Builder::new(PATTERN.parse()?).generate_keypair()?;
        Ok(Self {
            private: kp.private,
            public: kp.public,
        })
    }

    /// Hex of private then public, as kept on disk.
    pub fn to_hex(&self) -> String {
        let mut raw = self.private.clone();
        raw.extend_from_slice(&self.public);
        hex::encode(raw)
    }

    /// Reads [`Keys::to_hex`] back. A damaged key is an error, never a new
    /// identity.
    pub fn from_hex(text: &str) -> Result<Self> {
        let raw = hex::decode(text.trim()).context("key isn't hex")?;
        if raw.len() != 64 {
            bail!("key has {} bytes, not 64", raw.len());
        }
        let keys = Self {
            private: raw[..32].to_vec(),
            public: raw[32..].to_vec(),
        };
        if derive_public(&keys.private)? != keys.public {
            bail!("key's halves don't match");
        }
        Ok(keys)
    }

    pub fn id(&self) -> String {
        fingerprint(&self.public)
    }
}

/// The public half of a private key. snow doesn't expose X25519 directly,
/// so a throwaway XX exchange reveals the static key it derived.
fn derive_public(private: &[u8]) -> Result<Vec<u8>> {
    let throwaway = snow::Builder::new(PATTERN.parse()?).generate_keypair()?;
    let mut ini = snow::Builder::new(PATTERN.parse()?)
        .local_private_key(&throwaway.private)
        .build_initiator()?;
    let mut res = snow::Builder::new(PATTERN.parse()?)
        .local_private_key(private)
        .build_responder()?;
    let mut buf = vec![0u8; 1024];
    let mut out = vec![0u8; 1024];
    let n = ini.write_message(&[], &mut buf)?;
    res.read_message(&buf[..n], &mut out)?;
    let n = res.write_message(&[], &mut buf)?;
    ini.read_message(&buf[..n], &mut out)?;
    Ok(ini
        .get_remote_static()
        .context("no static key in message 2")?
        .to_vec())
}

/// A short id for a public key: the first 6 bytes of its SHA-256, in hex.
pub fn fingerprint(public: &[u8]) -> String {
    hex::encode(&Sha256::digest(public)[..6])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_and_damage_is_refused() {
        let k = Keys::generate().unwrap();
        let back = Keys::from_hex(&k.to_hex()).unwrap();
        assert_eq!(back.public, k.public);
        assert_eq!(back.id(), k.id());
        let mut bad = k.to_hex();
        bad.replace_range(70..72, if &bad[70..72] == "00" { "11" } else { "00" });
        assert!(Keys::from_hex(&bad).is_err());
        assert!(Keys::from_hex("abcd").is_err());
    }

    #[test]
    fn debug_never_shows_the_private_key() {
        let k = Keys::generate().unwrap();
        let shown = format!("{k:?}");
        assert!(!shown.contains(&hex::encode(&k.private)));
    }
}
