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
use crate::proto::{Down, Event, Health, Info, PROTO, PROTO_MIN, Up, now_ms};

pub struct Opened {
    pub ch: Channel,
    /// The address that answered.
    pub addr: SocketAddr,
    pub name: String,
    pub id: String,
    pub trusted: bool,
    pub via: String,
    pub proto: u32,
    pub min: u32,
    pub info: Info,
}

impl Opened {
    /// Whether the two versions overlap.
    pub fn compatible(&self) -> bool {
        self.proto >= PROTO_MIN && PROTO >= self.min
    }
}

/// From dialling to knowing who answered. A machine that has gone away
/// often drops packets rather than refusing, and the OS would wait over a
/// minute for that.
pub const OPEN: Duration = Duration::from_secs(3);
/// Each step of pairing, once connected.
const STEP: Duration = Duration::from_secs(5);

pub async fn open(addr: SocketAddr, me: &Identity) -> Result<Opened> {
    open_as(addr, me, false).await
}

/// Connect only to see who's there (discovery). Never changes trust.
pub async fn probe(addr: SocketAddr, me: &Identity) -> Result<Opened> {
    open_as(addr, me, true).await
}

async fn open_as(addr: SocketAddr, me: &Identity, probe: bool) -> Result<Opened> {
    tokio::time::timeout(OPEN, async {
        let url = format!("ws://{addr}/");
        let (ws, _) = tokio_tungstenite::connect_async_with_config(
            url,
            Some(crate::channel::ws_config()),
            true,
        )
        .await?;
        let mut ch = Channel::initiate(Ws::Client(ws), me).await?;
        ch.send(&Up::Hello {
            name: me.name.clone(),
            proto: PROTO,
            probe,
        })
        .await?;
        let Down::Welcome {
            name,
            id,
            trusted,
            via,
            proto,
            min,
            info,
        } = ch.recv().await?
        else {
            bail!("expected welcome");
        };
        Ok(Opened {
            ch,
            addr,
            name,
            id,
            trusted,
            via,
            proto,
            min,
            info: *info,
        })
    })
    .await
    .map_err(|_| anyhow!("no answer within {OPEN:?}"))?
}

/// Dial every address, a quarter second apart, and take the best answer:
/// the first that trusts this device, else the first that answered with
/// `key` (when given). One dead address doesn't cost the others a wait.
pub async fn open_any(addrs: &[SocketAddr], me: &Identity, key: Option<&[u8]>) -> Result<Opened> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut tasks = Vec::new();
    for (i, &addr) in addrs.iter().enumerate() {
        let (me, tx) = (me.clone(), tx.clone());
        tasks.push(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250 * i as u64)).await;
            let _ = tx.send((addr, open(addr, &me).await));
        }));
    }
    drop(tx);
    let mut last = anyhow!("no address");
    let mut fallback = None;
    while let Some((addr, r)) = rx.recv().await {
        match r {
            Ok(o) if key.is_some_and(|k| o.ch.remote != k) => {
                last = anyhow!("{addr} answered with a different key")
            }
            Ok(o) if o.trusted => {
                fallback = Some(o);
                break;
            }
            Ok(o) => {
                fallback.get_or_insert(o);
            }
            Err(e) => last = e,
        }
    }
    for t in tasks {
        t.abort();
    }
    fallback.ok_or(last)
}

/// Why pairing didn't work, in terms a person can act on.
#[derive(Debug, Clone, PartialEq)]
pub enum PairError {
    Wrong,
    /// Three wrong: the other machine now shows a new code.
    WrongNewCode,
    /// Paused after wrong codes; try again in this many seconds.
    Locked(u64),
    /// No code is showing there, or it expired.
    Closed,
    Unreachable(String),
    Failed(String),
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PairError {}

fn denied(reason: &str) -> PairError {
    match reason {
        "wrong" => PairError::Wrong,
        "wrong-new-code" => PairError::WrongNewCode,
        "closed" => PairError::Closed,
        r => match r.strip_prefix("locked:").and_then(|s| s.parse().ok()) {
            Some(secs) => PairError::Locked(secs),
            None => PairError::Failed(r.into()),
        },
    }
}

/// Pair using the code shown on the other machine. Both sides prove they
/// know it, bound to this channel's keys.
pub async fn pair(ch: &mut Channel, code: &str) -> Result<(), PairError> {
    let step = |e: anyhow::Error| PairError::Unreachable(format!("{e:#}"));
    let mut p = Pairing::start(code);
    ch.send(&Up::Pair {
        spake: hex::encode(&p.outbound),
    })
    .await
    .map_err(step)?;
    let recv = async |ch: &mut Channel| -> Result<Down, PairError> {
        tokio::time::timeout(STEP, ch.recv())
            .await
            .map_err(|_| PairError::Unreachable("stopped answering".into()))?
            .map_err(step)
    };
    let spake = match recv(ch).await? {
        Down::Pair { spake } => spake,
        Down::Denied { reason } => return Err(denied(&reason)),
        other => return Err(PairError::Failed(format!("unexpected {other:?}"))),
    };
    let bad = |e: String| PairError::Failed(e);
    let key = p
        .finish(&hex::decode(spake).map_err(|e| bad(e.to_string()))?)
        .map_err(|e| bad(e.to_string()))?;
    ch.send(&Up::PairConfirm {
        mac: hex::encode(pair::confirm(&key, &ch.hash, b"client")),
    })
    .await
    .map_err(step)?;
    match recv(ch).await? {
        Down::PairConfirm { mac }
            if hex::decode(&mac).is_ok_and(|m| pair::verify(&key, &ch.hash, b"daemon", &m)) => {}
        Down::Denied { reason } => return Err(denied(&reason)),
        _ => {
            return Err(bad(
                "the other machine couldn't prove it knows the code".into()
            ));
        }
    }
    match recv(ch).await? {
        Down::Paired => Ok(()),
        other => Err(bad(format!("unexpected {other:?}"))),
    }
}

/// Send a prompt and wait until the daemon has logged it. Safe to repeat
/// with the same `id`: it runs once.
pub async fn prompt(
    me: &Identity,
    key: &[u8],
    addrs: &[SocketAddr],
    id: &str,
    session: &str,
    text: &str,
) -> Result<(String, bool)> {
    let mut o = open_any(addrs, me, Some(key)).await?;
    if !o.trusted {
        bail!("not trusted there any more");
    }
    o.ch.send(&Up::Prompt {
        id: id.into(),
        session: session.into(),
        text: text.into(),
    })
    .await?;
    let (tx, mut rx) = o.ch.into_parts();
    drop(tx);
    match tokio::time::timeout(STEP, rx.recv::<Down>()).await {
        Ok(Ok(Down::Accepted { session, dup, .. })) => Ok((session, dup)),
        Ok(Ok(other)) => bail!("unexpected {other:?}"),
        Ok(Err(e)) => Err(e),
        Err(_) => bail!("no acknowledgement"),
    }
}

/// Ask the daemon to forget this device, after it was removed here.
pub async fn forget(me: &Identity, key: Option<&[u8]>, addrs: &[SocketAddr]) -> Result<()> {
    let mut o = open_any(addrs, me, key).await?;
    if o.trusted {
        o.ch.send(&Up::Forget).await?;
        // Give it a moment to land before the socket closes.
        let _ = tokio::time::timeout(Duration::from_millis(300), o.ch.recv::<Down>()).await;
    }
    Ok(())
}

#[derive(Debug)]
pub enum Note {
    Up {
        addr: SocketAddr,
        info: Box<Info>,
    },
    Down {
        reason: String,
    },
    Event(Event),
    Denied(String),
    /// The machine said goodbye: "sleep", "quit" or "restart".
    Bye(String),
    Health(Health),
    /// Round trip of a ping, in ms.
    Rtt(u64),
    /// The versions don't overlap: theirs, and the oldest they accept.
    Incompatible {
        proto: u32,
        min: u32,
    },
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
            let mut wait = None;
            let reason = match link(&me, &key, &addrs, &session, &tally, &out, first).await {
                Ok(Ended::Denied) => return,
                Ok(Ended::Incompatible) => {
                    // Nothing will change until one side updates.
                    wait = Some(Duration::from_secs(30));
                    "versions don't match".into()
                }
                Ok(Ended::Lost(r)) => {
                    // After a goodbye it won't be back in a moment.
                    backoff = if r.starts_with("said goodbye") {
                        Duration::from_secs(1)
                    } else {
                        Duration::from_millis(100)
                    };
                    first = false;
                    r
                }
                Err(e) => format!("{e:#}"),
            };
            let _ = out.send(Note::Down { reason });
            tokio::time::sleep(wait.unwrap_or(backoff)).await;
            backoff = (backoff * 2).min(Duration::from_secs(2));
        }
    })
}

enum Ended {
    Lost(String),
    Denied,
    Incompatible,
}

async fn link(
    me: &Identity,
    key: &[u8],
    addrs: &[SocketAddr],
    session: &str,
    tally: &Arc<Mutex<Tally>>,
    out: &mpsc::UnboundedSender<Note>,
    first: bool,
) -> Result<Ended> {
    let o = open_any(addrs, me, Some(key)).await?;
    let addr = o.addr;
    if !o.compatible() {
        let _ = out.send(Note::Incompatible {
            proto: o.proto,
            min: o.min,
        });
        return Ok(Ended::Incompatible);
    }
    if !o.trusted {
        // Trusted through Tailscale, reached only over the LAN this time.
        if o.via == "tailnet-only" {
            bail!("{addr} only trusts this device over Tailscale");
        }
        let _ = out.send(Note::Denied("removed".into()));
        return Ok(Ended::Denied);
    }
    if !first {
        tally.lock().await.reconnects += 1;
    }
    let _ = out.send(Note::Up {
        addr,
        info: Box::new(o.info.clone()),
    });
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
                if tx
                    .lock()
                    .await
                    .send(&Up::Ping { t: now_ms() })
                    .await
                    .is_err()
                {
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
            Ok(Ok(Down::Bye { reason })) => {
                let _ = out.send(Note::Bye(reason.clone()));
                break Ended::Lost(format!("said goodbye: {reason}"));
            }
            Ok(Ok(Down::Health(h))) => {
                let _ = out.send(Note::Health(h));
            }
            Ok(Ok(Down::Pong { t })) if t > 0 => {
                let _ = out.send(Note::Rtt(now_ms().saturating_sub(t)));
            }
            Ok(Ok(_)) => {}
        }
    };
    pinger.abort();
    Ok(ended)
}
