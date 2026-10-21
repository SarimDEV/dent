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
    /// Links reopened after a drop. A swap to a replacement that was already
    /// up isn't one.
    pub reconnects: u64,
    /// Moves to a replacement link that was already up (silence or coming
    /// home).
    pub swaps: u64,
    /// Connection attempts, successful or not.
    pub dials: u64,
}

/// How a follower judges liveness and backs off. `spike()` is spike 05's
/// numbers; `plan()` is decision 0062's.
#[derive(Debug, Clone)]
pub struct Rules {
    pub name: &'static str,
    /// Send a ping this often.
    pub ping: Duration,
    /// No frame of any kind for this long tears the link down.
    pub silence: Duration,
    /// The first waits after a drop, in order.
    pub quick: Vec<Duration>,
    /// Then doubling from the last quick wait up to this, each wait random
    /// in the upper half of its step (no jitter when `jitter` is off).
    pub cap: Duration,
    pub jitter: bool,
    /// Only a link that stayed up this long resets the ladder.
    pub reset_after: Duration,
    /// After this much silence, dial a replacement while keeping the old
    /// link, and move to whichever proves alive first.
    pub replace_after: Option<Duration>,
    /// While on an address other than the first (preferred) one, try the
    /// better ones this often and move once one is up.
    pub home_every: Option<Duration>,
}

impl Rules {
    pub fn spike() -> Self {
        Rules {
            name: "spike",
            ping: Duration::from_secs(1),
            silence: SILENCE,
            quick: vec![Duration::from_millis(100)],
            cap: Duration::from_secs(2),
            jitter: false,
            reset_after: Duration::ZERO,
            replace_after: None,
            home_every: None,
        }
    }

    pub fn plan() -> Self {
        Rules {
            name: "plan",
            ping: Duration::from_secs(2),
            silence: Duration::from_secs(15),
            quick: [100, 250, 500, 1000].map(Duration::from_millis).to_vec(),
            cap: Duration::from_secs(60),
            jitter: true,
            reset_after: Duration::from_secs(30),
            replace_after: Some(Duration::from_secs(5)),
            home_every: Some(Duration::from_secs(60)),
        }
    }

    /// The wait before attempt `n` (0-based) after a drop.
    pub fn wait(&self, n: usize) -> Duration {
        if let Some(d) = self.quick.get(n) {
            return *d;
        }
        let last = self.quick.last().copied().unwrap_or(Duration::from_secs(1));
        let steps = (n + 1 - self.quick.len()).min(16) as u32;
        let ceiling = (last * 2u32.pow(steps)).min(self.cap);
        if self.jitter {
            let half = ceiling / 2;
            half + half.mul_f64(rand::random::<f64>())
        } else {
            ceiling
        }
    }
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
    follow_with(Rules::spike(), me, key, resolve, session, after, tally, out)
}

/// [`follow`] under the given rules.
#[allow(clippy::too_many_arguments)]
pub fn follow_with(
    rules: Rules,
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
        let mut attempt = 0usize;
        let mut first = true;
        let mut ready: Option<Opened> = None;
        loop {
            let addrs = {
                let r = resolve.clone();
                tokio::task::spawn_blocking(move || r())
                    .await
                    .unwrap_or_default()
            };
            let mut wait = None;
            let pre = ready.take();
            if pre.is_none() {
                tally.lock().await.dials += 1;
            }
            let mut healthy = Duration::ZERO;
            let result = link(
                &rules,
                &me,
                &key,
                &addrs,
                pre,
                &session,
                &tally,
                &out,
                first,
                &mut healthy,
            )
            .await;
            // A link that stayed healthy long enough starts the ladder again,
            // whether it was lost or handed over to a replacement. Healthy
            // time runs from the link opening to the last frame heard, so
            // neither the dial nor the final silence counts.
            if matches!(result, Ok(Ended::Lost(_) | Ended::Swap(..)))
                && healthy >= rules.reset_after
            {
                attempt = 0;
            }
            let reason = match result {
                Ok(Ended::Swap(o, why)) => {
                    // A replacement is already up: move to it at once. It is
                    // counted as a swap, not a drop or a reconnect.
                    first = false;
                    ready = Some(*o);
                    tally.lock().await.swaps += 1;
                    let _ = out.send(Note::Down { reason: why.into() });
                    continue;
                }
                Ok(Ended::Denied) => return,
                Ok(Ended::Incompatible) => {
                    // Nothing will change until one side updates.
                    wait = Some(Duration::from_secs(30));
                    "versions don't match".into()
                }
                Ok(Ended::Lost(r)) => {
                    // After a goodbye it won't be back in a moment.
                    if r.starts_with("said goodbye") {
                        wait = Some(Duration::from_secs(1));
                    }
                    first = false;
                    r
                }
                Err(e) => format!("{e:#}"),
            };
            let _ = out.send(Note::Down { reason });
            tokio::time::sleep(wait.unwrap_or_else(|| rules.wait(attempt))).await;
            attempt += 1;
        }
    })
}

enum Ended {
    Lost(String),
    /// A replacement is up and ready: dialled during silence, or a faster
    /// route found by the home check. The reason is one of [`SWAP_REASONS`].
    Swap(Box<Opened>, &'static str),
    Denied,
    Incompatible,
}

/// The [`Note::Down`] reasons that mean "moved to a replacement link that is
/// already up", not a drop.
pub const SWAP_REASONS: [&str; 2] = ["replaced a silent link", "moved to a faster route"];

/// Takes a ready replacement from `slot` once its dial has finished, if it
/// is trusted and compatible. A failed dial empties the slot.
async fn take_ready(slot: &mut Option<tokio::task::JoinHandle<Result<Opened>>>) -> Option<Opened> {
    if !slot.as_ref().is_some_and(|h| h.is_finished()) {
        return None;
    }
    match slot.take()?.await {
        Ok(Ok(o)) if o.trusted && o.compatible() => Some(o),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn link(
    rules: &Rules,
    me: &Identity,
    key: &[u8],
    addrs: &[SocketAddr],
    pre: Option<Opened>,
    session: &str,
    tally: &Arc<Mutex<Tally>>,
    out: &mpsc::UnboundedSender<Note>,
    first: bool,
    healthy: &mut Duration,
) -> Result<Ended> {
    let swapped = pre.is_some();
    let o = match pre {
        Some(o) => o,
        None => open_any(addrs, me, Some(key)).await?,
    };
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
    if !first && !swapped {
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
        let every = rules.ping;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
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
    // The reader never waits on anything but the socket, so a slow consumer
    // can't hide a live link, and a half-read message is never cancelled.
    let (ftx, mut frx) = mpsc::unbounded_channel();
    let reader = tokio::spawn(async move {
        loop {
            let r = rx.recv::<Down>().await;
            let end = r.is_err();
            if ftx.send(r).is_err() || end {
                return;
            }
        }
    });
    let up = std::time::Instant::now();
    let mut heard = up;
    let mut swap: Option<tokio::task::JoinHandle<Result<Opened>>> = None;
    // Addresses ahead of the one in use are better routes to come home to.
    let better: Vec<SocketAddr> = addrs.iter().take_while(|a| **a != addr).copied().collect();
    let mut home_checked = std::time::Instant::now();
    let mut home: Option<tokio::task::JoinHandle<Result<Opened>>> = None;
    // One steady tick, so a busy stream can't starve the silence, swap and
    // home checks (a sleep made afresh each turn would never fire while
    // frames arrive faster than it).
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let ended = loop {
        let m = tokio::select! {
            m = frx.recv() => m,
            _ = tick.tick() => {
                let quiet = heard.elapsed();
                if let Some(o) = take_ready(&mut swap).await {
                    break Ended::Swap(Box::new(o), SWAP_REASONS[0]);
                }
                if let Some(o) = take_ready(&mut home).await {
                    break Ended::Swap(Box::new(o), SWAP_REASONS[1]);
                }
                if let Some(every) = rules.home_every
                    && !better.is_empty()
                    && home.is_none()
                    && home_checked.elapsed() >= every
                {
                    home_checked = std::time::Instant::now();
                    tally.lock().await.dials += 1;
                    let (me, key, b) = (me.clone(), key.to_vec(), better.clone());
                    home = Some(tokio::spawn(async move { open_any(&b, &me, Some(&key)).await }));
                }
                if quiet >= rules.silence {
                    break Ended::Lost(format!("silent for {} s", rules.silence.as_secs()));
                }
                if let Some(after) = rules.replace_after
                    && quiet >= after
                    && swap.is_none()
                {
                    tally.lock().await.dials += 1;
                    let (me, key, addrs) = (me.clone(), key.to_vec(), addrs.to_vec());
                    swap = Some(tokio::spawn(async move { open_any(&addrs, &me, Some(&key)).await }));
                }
                continue;
            }
        };
        let Some(m) = m else {
            break Ended::Lost("reader ended".into());
        };
        heard = std::time::Instant::now();
        *healthy = heard - up;
        if let Some(h) = swap.take() {
            h.abort();
        }
        match m {
            Err(e) => break Ended::Lost(format!("{e:#}")),
            Ok(Down::Event(ev)) => {
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
            Ok(Down::Denied { reason }) => {
                let _ = out.send(Note::Denied(reason));
                break Ended::Denied;
            }
            Ok(Down::Bye { reason }) => {
                let _ = out.send(Note::Bye(reason.clone()));
                break Ended::Lost(format!("said goodbye: {reason}"));
            }
            Ok(Down::Health(h)) => {
                let _ = out.send(Note::Health(h));
            }
            Ok(Down::Pong { t }) if t > 0 => {
                let _ = out.send(Note::Rtt(now_ms().saturating_sub(t)));
            }
            Ok(_) => {}
        }
    };
    pinger.abort();
    reader.abort();
    for h in [swap, home].into_iter().flatten() {
        h.abort();
    }
    Ok(ended)
}
