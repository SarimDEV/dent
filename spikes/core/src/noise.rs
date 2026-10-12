//! Noise XX with no socket: the handshake and the framing as functions from
//! bytes to bytes. One WebSocket binary frame carries one Noise message; a
//! JSON message larger than one Noise message is split, with a one-byte
//! header saying whether more parts follow.

use anyhow::{Context, Result, anyhow, bail};

pub const NOISE: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

/// Largest Noise message, less its tag.
const MAX: usize = 65535 - 16;
/// Largest message until the other side is trusted.
pub const SMALL: usize = 256 << 10;
/// Largest message afterwards.
pub const LARGE: usize = 64 << 20;

pub struct Initiator {
    hs: snow::HandshakeState,
}

impl Initiator {
    /// The first handshake message, to send as soon as the socket opens.
    pub fn start(private: &[u8]) -> Result<(Self, Vec<u8>)> {
        let mut hs = snow::Builder::new(NOISE.parse()?)
            .local_private_key(private)
            .build_initiator()?;
        let mut buf = vec![0u8; 1024];
        let n = hs.write_message(&[], &mut buf)?;
        buf.truncate(n);
        Ok((Self { hs }, buf))
    }

    /// The responder's message in; the last handshake message out, and the
    /// channel.
    pub fn finish(mut self, msg: &[u8]) -> Result<(Transport, Vec<u8>)> {
        let mut buf = vec![0u8; 1024.max(msg.len())];
        self.hs
            .read_message(msg, &mut buf)
            .map_err(|e| anyhow!("handshake: {e}"))?;
        let n = self.hs.write_message(&[], &mut buf)?;
        buf.truncate(n);
        let remote = self
            .hs
            .get_remote_static()
            .context("no remote static key")?
            .to_vec();
        let hash = self.hs.get_handshake_hash().to_vec();
        Ok((
            Transport {
                noise: self.hs.into_transport_mode()?,
                remote,
                hash,
                partial: Vec::new(),
                limit: SMALL,
            },
            buf,
        ))
    }
}

pub struct Transport {
    noise: snow::TransportState,
    /// The other side's static key, proven in the handshake.
    pub remote: Vec<u8>,
    /// Unique to this channel; pairing signs it.
    pub hash: Vec<u8>,
    partial: Vec<u8>,
    /// Largest message accepted; raised once the other side is trusted.
    pub limit: usize,
}

impl Transport {
    /// One message in, its frames out.
    pub fn seal(&mut self, plain: &[u8]) -> Result<Vec<Vec<u8>>> {
        let parts: Vec<&[u8]> = if plain.is_empty() {
            vec![&[]]
        } else {
            plain.chunks(MAX - 1).collect()
        };
        let mut frames = Vec::with_capacity(parts.len());
        for (i, part) in parts.iter().enumerate() {
            let mut frame = Vec::with_capacity(part.len() + 1);
            frame.push(u8::from(i + 1 < parts.len()));
            frame.extend_from_slice(part);
            let mut out = vec![0u8; frame.len() + 16];
            let n = self.noise.write_message(&frame, &mut out)?;
            out.truncate(n);
            frames.push(out);
        }
        Ok(frames)
    }

    /// One frame in; a whole message once its last part has arrived.
    pub fn open(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut out = vec![0u8; frame.len()];
        let n = self
            .noise
            .read_message(frame, &mut out)
            .map_err(|e| anyhow!("decrypt: {e}"))?;
        let (more, body) = out[..n].split_first().context("empty frame")?;
        if self.partial.len() + body.len() > self.limit {
            self.partial.clear();
            bail!("message over {} bytes", self.limit);
        }
        self.partial.extend_from_slice(body);
        Ok((*more == 0).then(|| std::mem::take(&mut self.partial)))
    }
}
