//! Noise XX between device keys, and the framed transport that follows.
//!
//! The initiator (a client) learns the responder's (a daemon's) static key
//! in message 2 and checks it against the key it expects *before* writing
//! message 3, which would reveal its own key: a stranger answering at a
//! known address learns nothing about who called. Each side's [`Hello`]
//! rides in its encrypted handshake payload (messages 2 and 3), so versions
//! are known before any session starts.
//!
//! After the handshake every plaintext frame starts with one byte: data
//! with more parts to follow, the last part of a data message, or a whole
//! control message. Data messages are split into parts of at most 64 KiB;
//! control messages are single frames and may be sent between the parts
//! of a data message, so pings, stops and approvals never wait behind a
//! large payload.

use serde::{Deserialize, Serialize};

use crate::control::Control;
use crate::keys::Keys;
use crate::{Error, Result, SMALL};

pub const PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

/// Largest Noise message.
const NOISE_MAX: usize = 65535;
/// Room for the AEAD tag.
const TAG: usize = 16;
/// Largest plaintext part of a data message (one header byte, then this).
pub const PART: usize = NOISE_MAX - TAG - 1;

const DATA_MORE: u8 = 0;
const DATA_LAST: u8 = 1;
const CONTROL: u8 = 2;

/// What each side says about itself in the handshake.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Hello {
    pub name: String,
    /// Oldest protocol this side still speaks.
    pub proto_min: u32,
    /// Newest protocol this side speaks.
    pub proto: u32,
    /// Discovery only: never changes what the other side trusts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub probe: bool,
}

impl Hello {
    /// The newest protocol both sides speak, if any.
    pub fn agree(&self, other: &Hello) -> Option<u32> {
        let hi = self.proto.min(other.proto);
        let lo = self.proto_min.max(other.proto_min);
        (hi >= lo).then_some(hi)
    }
}

/// The responder answered with a key other than the one expected. Nothing
/// about this device was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrongKey {
    pub got: Vec<u8>,
}

impl std::fmt::Display for WrongKey {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "answered with a different key ({})",
            crate::fingerprint(&self.got)
        )
    }
}

impl std::error::Error for WrongKey {}

fn builder() -> Result<snow::Builder<'static>> {
    Ok(snow::Builder::new(PATTERN.parse()?))
}

/// The calling side, after message 1.
pub struct Initiator {
    hs: snow::HandshakeState,
}

/// The calling side, after reading message 2: the responder is known, and
/// this side hasn't revealed itself yet.
pub struct Answered {
    hs: snow::HandshakeState,
    /// The responder's static key, proven by message 2.
    pub remote: Vec<u8>,
    pub hello: Hello,
}

impl Initiator {
    pub fn start(keys: &Keys) -> Result<(Self, Vec<u8>)> {
        let mut hs = builder()?
            .local_private_key(&keys.private)
            .build_initiator()?;
        let mut buf = vec![0u8; NOISE_MAX];
        let n = hs.write_message(&[], &mut buf)?;
        buf.truncate(n);
        Ok((Self { hs }, buf))
    }

    /// Reads message 2. With `expect`, a different responder key is a
    /// [`WrongKey`] error and this side writes nothing more.
    pub fn read(mut self, msg2: &[u8], expect: Option<&[u8]>) -> Result<Answered> {
        let mut buf = vec![0u8; NOISE_MAX];
        let n = self.hs.read_message(msg2, &mut buf)?;
        let remote = self
            .hs
            .get_remote_static()
            .ok_or_else(|| Error::Malformed("no static key in message 2".into()))?
            .to_vec();
        if let Some(want) = expect
            && want != remote
        {
            return Err(Error::WrongKey(WrongKey { got: remote }));
        }
        let hello: Hello = parse_small(&buf[..n])?;
        Ok(Answered {
            hs: self.hs,
            remote,
            hello,
        })
    }
}

impl Answered {
    /// Writes message 3 with this side's hello, and the transport is ready.
    pub fn finish(mut self, hello: &Hello) -> Result<(Transport, Vec<u8>)> {
        let mut buf = vec![0u8; NOISE_MAX];
        let n = self
            .hs
            .write_message(&serde_json::to_vec(hello)?, &mut buf)?;
        buf.truncate(n);
        Ok((Transport::new(self.hs, self.remote)?, buf))
    }
}

/// The answering side, after writing message 2.
pub struct Responder {
    hs: snow::HandshakeState,
}

impl Responder {
    /// Reads message 1 and writes message 2, which carries `hello`.
    pub fn accept(keys: &Keys, msg1: &[u8], hello: &Hello) -> Result<(Self, Vec<u8>)> {
        let mut hs = builder()?
            .local_private_key(&keys.private)
            .build_responder()?;
        let mut buf = vec![0u8; NOISE_MAX];
        hs.read_message(msg1, &mut buf)?;
        let n = hs.write_message(&serde_json::to_vec(hello)?, &mut buf)?;
        buf.truncate(n);
        Ok((Self { hs }, buf))
    }

    /// Reads message 3: the caller's proven key and hello.
    pub fn finish(mut self, msg3: &[u8]) -> Result<(Transport, Hello)> {
        let mut buf = vec![0u8; NOISE_MAX];
        let n = self.hs.read_message(msg3, &mut buf)?;
        let hello: Hello = parse_small(&buf[..n])?;
        let remote = self
            .hs
            .get_remote_static()
            .ok_or_else(|| Error::Malformed("no static key in message 3".into()))?
            .to_vec();
        Ok((Transport::new(self.hs, remote)?, hello))
    }
}

fn parse_small<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    if bytes.len() > SMALL {
        return Err(Error::TooLarge { limit: SMALL });
    }
    Ok(serde_json::from_slice(bytes)?)
}

/// What one received frame completed.
#[derive(Debug, PartialEq)]
pub enum Opened {
    /// A whole data message.
    Data(Vec<u8>),
    Control(Control),
}

/// Encrypts and frames in one direction and decrypts and reassembles in the
/// other.
pub struct Transport {
    noise: snow::TransportState,
    /// The other side's static key, proven in the handshake.
    pub remote: Vec<u8>,
    /// Unique to this channel; pairing signs it.
    pub hash: Vec<u8>,
    partial: Vec<u8>,
    /// Largest data message accepted; raised to [`crate::LARGE`] once the
    /// other side is trusted.
    pub limit: usize,
}

impl Transport {
    fn new(hs: snow::HandshakeState, remote: Vec<u8>) -> Result<Self> {
        let hash = hs.get_handshake_hash().to_vec();
        Ok(Self {
            noise: hs.into_transport_mode()?,
            remote,
            hash,
            partial: Vec::new(),
            limit: SMALL,
        })
    }

    fn seal_frame(&mut self, kind: u8, body: &[u8]) -> Result<Vec<u8>> {
        let mut frame = Vec::with_capacity(body.len() + 1);
        frame.push(kind);
        frame.extend_from_slice(body);
        let mut out = vec![0u8; frame.len() + TAG];
        let n = self.noise.write_message(&frame, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    /// One data message in, its frames out, in order. Control frames sealed
    /// later may be sent between them.
    ///
    /// Sealing advances the cipher's nonce, so frames must be sent in the
    /// order they were sealed, control ones included.
    pub fn seal_data(&mut self, plain: &[u8]) -> Result<Vec<Vec<u8>>> {
        let parts: Vec<&[u8]> = if plain.is_empty() {
            vec![&[]]
        } else {
            plain.chunks(PART).collect()
        };
        let last = parts.len() - 1;
        parts
            .iter()
            .enumerate()
            .map(|(i, p)| self.seal_frame(if i == last { DATA_LAST } else { DATA_MORE }, p))
            .collect()
    }

    /// One data part at a time, for a sender that interleaves control frames
    /// between parts: seals part `i` of `plain`, or returns `None` past the
    /// end.
    pub fn seal_part(&mut self, plain: &[u8], i: usize) -> Result<Option<Vec<u8>>> {
        let count = plain.len().div_ceil(PART).max(1);
        if i >= count {
            return Ok(None);
        }
        let body = &plain[(i * PART).min(plain.len())..((i + 1) * PART).min(plain.len())];
        let kind = if i + 1 == count { DATA_LAST } else { DATA_MORE };
        self.seal_frame(kind, body).map(Some)
    }

    pub fn seal_control(&mut self, c: &Control) -> Result<Vec<u8>> {
        let body = serde_json::to_vec(c)?;
        if body.len() > PART {
            return Err(Error::TooLarge { limit: PART });
        }
        self.seal_frame(CONTROL, &body)
    }

    /// One frame in; a whole message once it's complete.
    pub fn open(&mut self, frame: &[u8]) -> Result<Option<Opened>> {
        let mut out = vec![0u8; frame.len()];
        let n = self.noise.read_message(frame, &mut out)?;
        let (kind, body) = out[..n]
            .split_first()
            .ok_or_else(|| Error::Malformed("empty frame".into()))?;
        match *kind {
            CONTROL => {
                // An unknown control message is skipped, not fatal: a newer
                // peer may send kinds this side doesn't know.
                Ok(serde_json::from_slice::<Control>(body)
                    .ok()
                    .map(Opened::Control))
            }
            DATA_MORE | DATA_LAST => {
                if self.partial.len() + body.len() > self.limit {
                    self.partial.clear();
                    return Err(Error::TooLarge { limit: self.limit });
                }
                self.partial.extend_from_slice(body);
                Ok((*kind == DATA_LAST).then(|| Opened::Data(std::mem::take(&mut self.partial))))
            }
            k => Err(Error::Malformed(format!("unknown frame kind {k}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{Control, Refusal};

    fn hello(name: &str, lo: u32, hi: u32) -> Hello {
        Hello {
            name: name.into(),
            proto_min: lo,
            proto: hi,
            probe: false,
        }
    }

    fn pair() -> (Transport, Transport, Keys, Keys) {
        let (c, d) = (Keys::generate().unwrap(), Keys::generate().unwrap());
        let (ini, m1) = Initiator::start(&c).unwrap();
        let (res, m2) = Responder::accept(&d, &m1, &hello("daemon", 2, 3)).unwrap();
        let ans = ini.read(&m2, Some(&d.public)).unwrap();
        assert_eq!(ans.hello.name, "daemon");
        assert_eq!(ans.remote, d.public);
        let (ct, m3) = ans.finish(&hello("phone", 1, 2)).unwrap();
        let (dt, h) = res.finish(&m3).unwrap();
        assert_eq!(h.name, "phone");
        assert_eq!(dt.remote, c.public);
        assert_eq!(ct.hash, dt.hash);
        (ct, dt, c, d)
    }

    #[test]
    fn handshake_proves_both_keys_and_carries_hellos() {
        pair();
    }

    #[test]
    fn wrong_responder_key_stops_before_revealing_ours() {
        let (c, d, other) = (
            Keys::generate().unwrap(),
            Keys::generate().unwrap(),
            Keys::generate().unwrap(),
        );
        let (ini, m1) = Initiator::start(&c).unwrap();
        let (_res, m2) = Responder::accept(&d, &m1, &hello("d", 1, 1)).unwrap();
        let err = match ini.read(&m2, Some(&other.public)) {
            Err(e) => e,
            Ok(_) => panic!("accepted the wrong key"),
        };
        let Error::WrongKey(wrong) = err else {
            panic!("expected a WrongKey error, got {err}")
        };
        assert_eq!(wrong.got, d.public);
        // Message 1 carries only an ephemeral key, never our static one.
        assert!(!m1.windows(32).any(|w| w == c.public.as_slice()));
    }

    #[test]
    fn versions_agree_on_the_newest_shared() {
        assert_eq!(hello("a", 2, 4).agree(&hello("b", 1, 3)), Some(3));
        assert_eq!(hello("a", 4, 5).agree(&hello("b", 1, 3)), None);
        assert_eq!(hello("a", 1, 1).agree(&hello("b", 1, 1)), Some(1));
    }

    #[test]
    fn large_messages_split_and_control_slips_between_parts() {
        let (mut ct, mut dt, ..) = pair();
        dt.limit = crate::LARGE;
        let big: Vec<u8> = (0..300_000u32).map(|i| i as u8).collect();
        let mut frames = Vec::new();
        let mut i = 0;
        while let Some(f) = ct.seal_part(&big, i).unwrap() {
            frames.push(f);
            if i == 1 {
                frames.push(ct.seal_control(&Control::Ping { t: 7 }).unwrap());
            }
            i += 1;
        }
        assert_eq!(frames.len(), 6, "5 parts and a ping");
        let mut got = Vec::new();
        for f in &frames {
            if let Some(o) = dt.open(f).unwrap() {
                got.push(o);
            }
        }
        assert_eq!(
            got,
            vec![Opened::Control(Control::Ping { t: 7 }), Opened::Data(big)]
        );
    }

    #[test]
    fn seal_data_and_seal_part_agree() {
        let (mut ct, mut dt, ..) = pair();
        for msg in [vec![], vec![1, 2, 3], vec![9; PART], vec![9; PART + 1]] {
            for f in ct.seal_data(&msg).unwrap() {
                if let Some(Opened::Data(d)) = dt.open(&f).unwrap() {
                    assert_eq!(d, msg);
                }
            }
        }
    }

    #[test]
    fn untrusted_limit_refuses_big_messages() {
        let (mut ct, mut dt, ..) = pair();
        let big = vec![0u8; SMALL + 1];
        let mut failed = false;
        for f in ct.seal_data(&big).unwrap() {
            if dt.open(&f).is_err() {
                failed = true;
                break;
            }
        }
        assert!(failed);
    }

    #[test]
    fn unknown_control_kinds_are_skipped() {
        let (mut ct, mut dt, ..) = pair();
        let f = ct
            .seal_frame(CONTROL, br#"{"type":"from_the_future","x":1}"#)
            .unwrap();
        assert_eq!(dt.open(&f).unwrap(), None);
        let f = ct
            .seal_control(&Control::Refuse(Refusal::Starting { retry_ms: 500 }))
            .unwrap();
        assert_eq!(
            dt.open(&f).unwrap(),
            Some(Opened::Control(Control::Refuse(Refusal::Starting {
                retry_ms: 500
            })))
        );
    }

    #[test]
    fn tampered_frames_fail() {
        let (mut ct, mut dt, ..) = pair();
        let mut f = ct.seal_control(&Control::Ping { t: 1 }).unwrap();
        f[3] ^= 1;
        assert!(dt.open(&f).is_err());
    }
}
