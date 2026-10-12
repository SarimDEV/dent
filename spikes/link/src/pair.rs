//! Pairing with a short code, safely. A 6-digit code is too short to use as
//! a key directly, so both sides run SPAKE2 with it: an eavesdropper learns
//! nothing and an attacker gets one guess per attempt. Each side then sends
//! an HMAC of the channel's handshake hash under the agreed key, which ties
//! the code to the exact keys in this Noise channel and so rules out a
//! man in the middle.

use anyhow::{Result, anyhow};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

pub struct Pairing {
    state: Option<Spake2<Ed25519Group>>,
    pub outbound: Vec<u8>,
}

impl Pairing {
    pub fn start(code: &str) -> Self {
        let (state, outbound) = Spake2::<Ed25519Group>::start_symmetric(
            &Password::new(code.as_bytes()),
            &Identity::new(b"dent pairing v1"),
        );
        Self {
            state: Some(state),
            outbound,
        }
    }

    /// The shared key, from the other side's SPAKE2 message.
    pub fn finish(&mut self, inbound: &[u8]) -> Result<Vec<u8>> {
        let state = self.state.take().ok_or_else(|| anyhow!("pairing reused"))?;
        state.finish(inbound).map_err(|e| anyhow!("spake2: {e:?}"))
    }
}

/// Each side's confirmation differs by role so one can't be reflected back.
pub fn confirm(key: &[u8], hash: &[u8], role: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac takes any key length");
    mac.update(role);
    mac.update(hash);
    mac.finalize().into_bytes().to_vec()
}

pub fn verify(key: &[u8], hash: &[u8], role: &[u8], got: &[u8]) -> bool {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac takes any key length");
    mac.update(role);
    mac.update(hash);
    mac.verify_slice(got).is_ok()
}

/// A fresh random 6-digit code.
pub fn new_code() -> String {
    use rand::Rng;
    format!("{:06}", rand::thread_rng().gen_range(0..1_000_000))
}
