//! Pairing with a short code, safely. A 6-digit code is too short to use as
//! a key, so both sides run SPAKE2 with it: an eavesdropper learns nothing
//! and an attacker gets one guess per attempt. Each side then sends an HMAC
//! of the channel's handshake hash under the agreed key, which ties the
//! code to the keys of this exact channel and so rules out a man in the
//! middle.

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

/// Which side a confirmation comes from, so one can't be reflected back.
#[derive(Clone, Copy)]
pub enum Role {
    Client,
    Daemon,
}

impl Role {
    fn label(self) -> &'static [u8] {
        match self {
            Role::Client => b"client",
            Role::Daemon => b"daemon",
        }
    }
}

fn mac(key: &[u8], hash: &[u8], role: Role) -> Hmac<Sha256> {
    // HMAC takes keys of any length, so this can't fail.
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).unwrap_or_else(|_| unreachable!());
    m.update(role.label());
    m.update(hash);
    m
}

pub fn confirm(key: &[u8], hash: &[u8], role: Role) -> Vec<u8> {
    mac(key, hash, role).finalize().into_bytes().to_vec()
}

pub fn verify(key: &[u8], hash: &[u8], role: Role, got: &[u8]) -> bool {
    mac(key, hash, role).verify_slice(got).is_ok()
}

/// A fresh random 6-digit code, uniform over 000000–999999.
pub fn new_code() -> Result<String> {
    // Rejection sampling keeps every code equally likely.
    loop {
        let mut b = [0u8; 4];
        getrandom::getrandom(&mut b).map_err(|e| anyhow!("randomness: {e}"))?;
        let n = u32::from_le_bytes(b);
        if n < 4_294_000_000 {
            return Ok(format!("{:06}", n % 1_000_000));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_code_agrees_and_confirms() {
        let mut a = Pairing::start("123456");
        let mut b = Pairing::start("123456");
        let (oa, ob) = (a.outbound.clone(), b.outbound.clone());
        let ka = a.finish(&ob).unwrap();
        let kb = b.finish(&oa).unwrap();
        assert_eq!(ka, kb);
        let hash = [7u8; 32];
        let m = confirm(&ka, &hash, Role::Client);
        assert!(verify(&kb, &hash, Role::Client, &m));
        // A reflected confirmation doesn't pass as the other role's.
        assert!(!verify(&kb, &hash, Role::Daemon, &m));
        // Nor on another channel.
        assert!(!verify(&kb, &[8u8; 32], Role::Client, &m));
    }

    #[test]
    fn different_codes_disagree() {
        let mut a = Pairing::start("123456");
        let mut b = Pairing::start("654321");
        let (oa, ob) = (a.outbound.clone(), b.outbound.clone());
        let ka = a.finish(&ob).unwrap();
        let kb = b.finish(&oa).unwrap();
        assert_ne!(ka, kb);
    }

    #[test]
    fn codes_are_six_digits() {
        for _ in 0..200 {
            let c = new_code().unwrap();
            assert_eq!(c.len(), 6);
            assert!(c.bytes().all(|b| b.is_ascii_digit()));
        }
    }
}
