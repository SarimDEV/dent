//! An encrypted, mutually authenticated channel: Noise XX over a WebSocket.
//! Both sides prove their static key in the handshake; the handshake hash
//! binds pairing to this exact channel. Every connection is encrypted, on
//! Tailscale too, so the LAN and Tailscale differ only in how a device is
//! found and first trusted.

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde::{Serialize, de::DeserializeOwned};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::identity::{Identity, NOISE};

/// Largest Noise message; payloads above it are split.
const MAX: usize = 65535 - 16;

pub enum Ws {
    Server(WebSocketStream<TcpStream>),
    Client(WebSocketStream<MaybeTlsStream<TcpStream>>),
}

impl Ws {
    async fn send(&mut self, data: Vec<u8>) -> Result<()> {
        let msg = Message::Binary(data.into());
        match self {
            Ws::Server(s) => s.send(msg).await?,
            Ws::Client(s) => s.send(msg).await?,
        }
        Ok(())
    }

    async fn recv(&mut self) -> Result<Vec<u8>> {
        loop {
            let msg = match self {
                Ws::Server(s) => s.next().await,
                Ws::Client(s) => s.next().await,
            };
            match msg {
                Some(Ok(Message::Binary(b))) => return Ok(b.to_vec()),
                Some(Ok(Message::Close(_))) | None => bail!("closed"),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(e.into()),
            }
        }
    }
}

pub struct Channel {
    ws: Ws,
    noise: snow::TransportState,
    /// The other side's static public key, proven in the handshake.
    pub remote: Vec<u8>,
    /// Unique to this channel; pairing signs it.
    pub hash: Vec<u8>,
}

impl Channel {
    pub async fn initiate(mut ws: Ws, me: &Identity) -> Result<Self> {
        let mut hs = snow::Builder::new(NOISE.parse()?)
            .local_private_key(&me.private)
            .build_initiator()?;
        let mut buf = vec![0u8; 65535];
        let n = hs.write_message(&[], &mut buf)?;
        ws.send(buf[..n].to_vec()).await?;
        let msg = ws.recv().await?;
        hs.read_message(&msg, &mut buf)?;
        let n = hs.write_message(&[], &mut buf)?;
        ws.send(buf[..n].to_vec()).await?;
        Self::finish(ws, hs)
    }

    pub async fn respond(mut ws: Ws, me: &Identity) -> Result<Self> {
        let mut hs = snow::Builder::new(NOISE.parse()?)
            .local_private_key(&me.private)
            .build_responder()?;
        let mut buf = vec![0u8; 65535];
        let msg = ws.recv().await?;
        hs.read_message(&msg, &mut buf)?;
        let n = hs.write_message(&[], &mut buf)?;
        ws.send(buf[..n].to_vec()).await?;
        let msg = ws.recv().await?;
        hs.read_message(&msg, &mut buf)?;
        Self::finish(ws, hs)
    }

    fn finish(ws: Ws, hs: snow::HandshakeState) -> Result<Self> {
        let remote = hs
            .get_remote_static()
            .context("no remote static key")?
            .to_vec();
        let hash = hs.get_handshake_hash().to_vec();
        Ok(Self {
            ws,
            noise: hs.into_transport_mode()?,
            remote,
            hash,
        })
    }

    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<()> {
        let plain = serde_json::to_vec(msg)?;
        // A one-byte header says whether more parts follow.
        let parts: Vec<&[u8]> = plain.chunks(MAX - 1).collect();
        for (i, part) in parts.iter().enumerate() {
            let mut frame = vec![u8::from(i + 1 < parts.len())];
            frame.extend_from_slice(part);
            let mut out = vec![0u8; frame.len() + 16];
            let n = self.noise.write_message(&frame, &mut out)?;
            self.ws.send(out[..n].to_vec()).await?;
        }
        Ok(())
    }

    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<T> {
        let mut plain = Vec::new();
        loop {
            let frame = self.ws.recv().await?;
            let mut out = vec![0u8; frame.len()];
            let n = self
                .noise
                .read_message(&frame, &mut out)
                .map_err(|e| anyhow!("decrypt: {e}"))?;
            let (more, body) = out[..n].split_first().context("empty frame")?;
            plain.extend_from_slice(body);
            if *more == 0 {
                break;
            }
        }
        Ok(serde_json::from_slice(&plain)?)
    }

    /// Split into halves that can be used from two tasks at once.
    pub fn into_parts(self) -> (Sender, Receiver) {
        let shared = std::sync::Arc::new(tokio::sync::Mutex::new(self.noise));
        let (tx, rx) = match self.ws {
            Ws::Server(s) => {
                let (tx, rx) = s.split();
                (Half::Server(tx), Rx::Server(rx))
            }
            Ws::Client(s) => {
                let (tx, rx) = s.split();
                (Half::Client(tx), Rx::Client(rx))
            }
        };
        (
            Sender {
                tx,
                noise: shared.clone(),
            },
            Receiver { rx, noise: shared },
        )
    }
}

type Noise = std::sync::Arc<tokio::sync::Mutex<snow::TransportState>>;

enum Half {
    Server(futures_util::stream::SplitSink<WebSocketStream<TcpStream>, Message>),
    Client(futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>),
}

enum Rx {
    Server(futures_util::stream::SplitStream<WebSocketStream<TcpStream>>),
    Client(futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>),
}

pub struct Sender {
    tx: Half,
    noise: Noise,
}

impl Sender {
    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<()> {
        let plain = serde_json::to_vec(msg)?;
        let parts: Vec<&[u8]> = plain.chunks(MAX - 1).collect();
        for (i, part) in parts.iter().enumerate() {
            let mut frame = vec![u8::from(i + 1 < parts.len())];
            frame.extend_from_slice(part);
            let mut out = vec![0u8; frame.len() + 16];
            let n = self.noise.lock().await.write_message(&frame, &mut out)?;
            let msg = Message::Binary(out[..n].to_vec().into());
            match &mut self.tx {
                Half::Server(s) => s.send(msg).await?,
                Half::Client(s) => s.send(msg).await?,
            }
        }
        Ok(())
    }
}

pub struct Receiver {
    rx: Rx,
    noise: Noise,
}

impl Receiver {
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<T> {
        let mut plain = Vec::new();
        loop {
            let msg = match &mut self.rx {
                Rx::Server(s) => s.next().await,
                Rx::Client(s) => s.next().await,
            };
            let frame = match msg {
                Some(Ok(Message::Binary(b))) => b.to_vec(),
                Some(Ok(Message::Close(_))) | None => bail!("closed"),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(e.into()),
            };
            let mut out = vec![0u8; frame.len()];
            let n = self
                .noise
                .lock()
                .await
                .read_message(&frame, &mut out)
                .map_err(|e| anyhow!("decrypt: {e}"))?;
            let (more, body) = out[..n].split_first().context("empty frame")?;
            plain.extend_from_slice(body);
            if *more == 0 {
                break;
            }
        }
        Ok(serde_json::from_slice(&plain)?)
    }
}
