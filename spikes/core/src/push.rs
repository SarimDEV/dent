//! Notification payloads, encrypted end to end. The phone makes a key and
//! gives it to each daemon it pairs with; the daemon seals each notification
//! with it, dent-push forwards the sealed bytes, and the phone's
//! notification service extension opens them. dent-push never sees the
//! text.
//!
//! The format is CryptoKit's `ChaChaPoly.SealedBox` combined form (nonce,
//! ciphertext, tag), so Swift opens it with one call.

use anyhow::{Result, anyhow};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};

/// What the phone shows. Kept short: APNs allows 4 KB for the whole push.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Notice {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub session: String,
}

pub fn new_key() -> Vec<u8> {
    let mut k = vec![0u8; 32];
    getrandom::getrandom(&mut k).expect("system randomness");
    k
}

pub fn seal(key: &[u8], notice: &Notice) -> Result<Vec<u8>> {
    let mut nonce = [0u8; 12];
    getrandom::getrandom(&mut nonce)?;
    seal_with(key, &nonce, &serde_json::to_vec(notice)?)
}

pub fn seal_with(key: &[u8], nonce: &[u8; 12], plain: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(key.len() == 32, "key must be 32 bytes");
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let ct = cipher
        .encrypt(Nonce::from_slice(nonce), plain)
        .map_err(|_| anyhow!("seal failed"))?;
    let mut out = nonce.to_vec();
    out.extend(ct);
    Ok(out)
}

pub fn open(key: &[u8], sealed: &[u8]) -> Result<Notice> {
    anyhow::ensure!(key.len() == 32, "key must be 32 bytes");
    anyhow::ensure!(sealed.len() >= 12 + 16, "too short");
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let plain = cipher
        .decrypt(Nonce::from_slice(&sealed[..12]), &sealed[12..])
        .map_err(|_| anyhow!("not sealed with this key"))?;
    Ok(serde_json::from_slice(&plain)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same vector is checked in Swift (spikes/core-ffi/ios), so both
    /// sides agree on the format byte for byte.
    #[test]
    fn vector() {
        let key = [7u8; 32];
        let nonce = [1u8; 12];
        let sealed = seal_with(&key, &nonce, br#"{"title":"t","body":"b"}"#).unwrap();
        assert_eq!(
            hex::encode(&sealed),
            include_str!("../push-vector.hex").trim()
        );
        assert_eq!(open(&key, &sealed).unwrap().body, "b");
        let mut bad = sealed.clone();
        bad[20] ^= 1;
        assert!(open(&key, &bad).is_err());
        assert!(open(&[8u8; 32], &sealed).is_err());
    }
}
