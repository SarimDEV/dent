//! A device's long-lived key pair: X25519, the Noise static key. The host
//! stores it (Keychain on iOS, IndexedDB on the web); the core only uses it.

use sha2::{Digest, Sha256};

use crate::noise::NOISE;

#[derive(Clone, Debug)]
pub struct Keys {
    pub private: Vec<u8>,
    pub public: Vec<u8>,
}

impl Keys {
    pub fn generate() -> Self {
        let kp = snow::Builder::new(NOISE.parse().expect("valid pattern"))
            .generate_keypair()
            .expect("system randomness");
        Self {
            private: kp.private,
            public: kp.public,
        }
    }

    /// 64 bytes: private then public, as stored.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = self.private.clone();
        b.extend(&self.public);
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        (b.len() == 64).then(|| Self {
            private: b[..32].to_vec(),
            public: b[32..].to_vec(),
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
