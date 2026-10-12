//! The client side (the desktop app's role): connect, pair, and follow a
//! session across drops, restarts, sleep and address changes.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use tokio::sync::{Mutex, mpsc};

use crate::channel::{Channel, Ws};
use crate::identity::Identity;
use crate::pair::{self, Pairing};
use crate::proto::{Down, Event, Up};

pub struct Opened {
    pub ch: Channel,
    pub name: String,
    pub id: String,
    pub trusted: bool,
    pub via: String,
}

pub async fn open(addr: SocketAddr, me: &Identity) -> Result<Opened> {
    let url = format!("ws://{addr}/");
    let (ws, _) = tokio::time::timeout(
        Duration::from_secs(2),
        tokio_tungstenite::connect_async(url),
    )
    .await
    .map_err(|_| anyhow!("connect timed out"))??;
    let mut ch = Channel::initiate(Ws::Client(ws), me).await?;
    ch.send(&Up::Hello {
        name: me.name.clone(),
    })
    .await?;
    let Down::Welcome {
        name,
        id,
        trusted,
        via,
    } = ch.recv().await?
    else {
        bail!("expected welcome");
    };
    Ok(Opened {
        ch,
        name,
        id,
        trusted,
        via,
    })
}

/// Pair using the code shown on the other machine. Both sides prove they
/// know it, bound to this channel's keys.
pub async fn pair(ch: &mut Channel, code: &str) -> Result<()> {
    let mut p = Pairing::start(code);
    ch.send(&Up::Pair {
        spake: hex::encode(&p.outbound),
    })
    .await?;
    let spake = match ch.recv().await? {
        Down::Pair { spake } => spake,
        Down::Denied { reason } => bail!("denied: {reason}"),
        other => bail!("unexpected {other:?}"),
    };
    let key = p.finish(&hex::decode(spake)?)?;
    ch.send(&Up::PairConfirm {
        mac: hex::encode(pair::confirm(&key, &ch.hash, b"client")),
    })
    .await?;
    match ch.recv().await? {
        Down::PairConfirm { mac }
            if pair::verify(&key, &ch.hash, b"daemon", &hex::decode(&mac)?) => {}
        Down::Denied { reason } => bail!("denied: {reason}"),
        _ => bail!("the other machine couldn't prove it knows the code"),
    }
    match ch.recv().await? {
        Down::Paired => Ok(()),
        other => bail!("unexpected {other:?}"),
    }
}

#[derive(Debug)]
pub enum Note {
    Up { addr: SocketAddr },
    Down { reason: String },
    Event(Event),
    Denied(String),
}

#[derive(Default, Debug, Clone)]
pub struct Tally {
    pub last: u64,
    pub dups: u64,
    pub gaps: u64,
    pub reconnects: u64,
}

pub type Resolve = Arc<dyn Fn() -> Vec<SocketAddr> + Send + Sync>;

/// No frame for this long (pings are answered every second) means the link
/// is dead, as with a sleeping machine or a pulled cable.
const SILENCE: Duration = Duration::from_secs(3);

/// Follow `session` on the daemon whose key is `key`, from `after`, for
/// good: reconnect with backoff, re-resolve its address each time, refuse
/// any other key, and resume from the last event received.
pub fn follow(
    me: Identity,
    key: Vec<u8>,
    resolve: Resolve,
    session: String,
    after: u64,
    tally: Arc<Mutex<Tally>>,
    out: mpsc::UnboundedSender<Note>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tally.lock().await.last = after;
        let mut backoff = Duration::from_millis(100);
        let mut first = true;
        loop {
            let addrs = {
                let r = resolve.clone();
                tokio::task::spawn_blocking(move || r())
                    .await
                    .unwrap_or_default()
            };
            let mut reason = "no address".to_string();
            for addr in addrs {
                match link(&me, &key, addr, &session, &tally, &out, first).await {
                    Ok(Ended::Denied) => return,
                    Ok(Ended::Lost(r)) => {
                        reason = r;
                        backoff = Duration::from_millis(100);
                        first = false;
                        break;
                    }
                    Err(e) => reason = format!("{e:#}"),
                }
            }
            let _ = out.send(Note::Down { reason });
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(2));
        }
    })
}

enum Ended {
    Lost(String),
    Denied,
}

async fn link(
    me: &Identity,
    key: &[u8],
    addr: SocketAddr,
    session: &str,
    tally: &Arc<Mutex<Tally>>,
    out: &mpsc::UnboundedSender<Note>,
    first: bool,
) -> Result<Ended> {
    let o = open(addr, me).await?;
    if o.ch.remote != key {
        bail!("{addr} answered with a different key; not connecting");
    }
    if !o.trusted {
        let _ = out.send(Note::Denied("no longer trusted".into()));
        return Ok(Ended::Denied);
    }
    if !first {
        tally.lock().await.reconnects += 1;
    }
    let _ = out.send(Note::Up { addr });
    let (tx, mut rx) = o.ch.into_parts();
    let tx = Arc::new(Mutex::new(tx));
    let after = tally.lock().await.last;
    tx.lock()
        .await
        .send(&Up::Subscribe {
            session: session.into(),
            after,
        })
        .await?;
    let pinger = {
        let tx = tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if tx.lock().await.send(&Up::Ping { t: 0 }).await.is_err() {
                    return;
                }
            }
        })
    };
    let ended = loop {
        match tokio::time::timeout(SILENCE, rx.recv::<Down>()).await {
            Err(_) => break Ended::Lost("silent for 3 s".into()),
            Ok(Err(e)) => break Ended::Lost(format!("{e:#}")),
            Ok(Ok(Down::Event(ev))) => {
                let mut t = tally.lock().await;
                if ev.seq <= t.last {
                    t.dups += 1;
                    continue;
                }
                if ev.seq > t.last + 1 {
                    t.gaps += 1;
                }
                t.last = ev.seq;
                drop(t);
                let _ = out.send(Note::Event(ev));
            }
            Ok(Ok(Down::Denied { reason })) => {
                let _ = out.send(Note::Denied(reason));
                break Ended::Denied;
            }
            Ok(Ok(_)) => {}
        }
    };
    pinger.abort();
    Ok(ended)
}
