//! The daemon side: listens on the LAN and on the tailnet, decides trust,
//! pairs with a code, runs a fake agent and serves sessions from a log.

mod agents;
mod push;
pub use agents::{fake_agent, holder_dir};

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, broadcast};

use crate::channel::{Channel, Sender, Ws};
use crate::identity::{Identity, Peer, Trust, fingerprint};
use crate::pair::{self, Pairing};
use crate::proto::{Down, Event, Info, Up, now_ms};
use crate::tailnet::Tailnet;

pub struct Config {
    pub name: String,
    pub state: PathBuf,
    pub lan: SocketAddr,
    pub tailnet: Option<SocketAddr>,
    pub tailnet_impl: Box<dyn Tailnet>,
    pub advertise: bool,
    /// Protocol version and oldest supported, overridable to test skew.
    pub proto: (u32, u32),
    /// How long a shown pairing code stays valid.
    pub pair_ttl: Duration,
    /// Base delay between the fake agent's chunks.
    pub agent_ms: u64,
    /// Reported dent version, overridable to show an upgrade.
    pub version: Option<String>,
    /// "markdown" makes the fake agent answer the way a real one formats.
    pub agent_style: String,
    /// dent-push, for phone notifications when a reply finishes.
    pub push_url: Option<String>,
    /// A connection is dropped after this long without hearing from it.
    pub idle: Duration,
}

struct Session {
    events: Vec<Event>,
    tx: broadcast::Sender<Event>,
    log: std::io::BufWriter<std::fs::File>,
}

/// Pairing is open only while a code is showing on this machine (opened by
/// the person there), for `pair_ttl`.
struct Window {
    code: String,
    opened: Instant,
    /// Wrong codes against this code, and in this window.
    strikes: u32,
    total: u32,
    locked_until: Option<Instant>,
    pause: Duration,
}

struct Daemon {
    idle: Duration,
    me: Identity,
    info: std::sync::Mutex<Info>,
    state: PathBuf,
    tailnet: Box<dyn Tailnet>,
    proto: (u32, u32),
    pair_ttl: Duration,
    agent_ms: u64,
    agent_style: String,
    push: push::Push,
    window: Mutex<Option<Window>>,
    sessions: Mutex<HashMap<String, Session>>,
    prompts: Mutex<HashMap<String, String>>,
    /// Sessions with a reply under way.
    busy: std::sync::Mutex<HashSet<String>>,
    agents: agents::Agents,
    /// Fired once on the way out, so every link can say why.
    bye: broadcast::Sender<String>,
    /// Set from goodbye until gone (or awake again): new links are refused,
    /// so nobody reconnects in the moment before it goes.
    leaving: std::sync::atomic::AtomicBool,
    /// Connections that haven't proven anything yet, by source address.
    unauthed: Arc<std::sync::Mutex<HashMap<std::net::IpAddr, usize>>>,
}

/// Holds one unauthenticated slot for an address until dropped.
struct Slot(
    Arc<std::sync::Mutex<HashMap<std::net::IpAddr, usize>>>,
    std::net::IpAddr,
);

impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut m) = self.0.lock()
            && let Some(n) = m.get_mut(&self.1)
        {
            *n -= 1;
            if *n == 0 {
                m.remove(&self.1);
            }
        }
    }
}

/// The default for [`Config::idle`]: spike 05's 10 s.
pub const IDLE: Duration = Duration::from_secs(10);
/// Time allowed from accepting a socket to knowing who it is.
const HANDSHAKE: Duration = Duration::from_secs(5);
/// Unauthenticated connections held at once, from one address and in all;
/// more are closed straight away, so a flood from one machine can't crowd
/// out another.
const UNAUTHED_PER_IP: usize = 8;
const UNAUTHED: usize = 256;
/// Wrong codes before the code changes and pairing pauses.
const TRIES: u32 = 3;
/// Wrong codes before the window closes and must be reopened by hand.
const WINDOW_TRIES: u32 = 10;
const HEALTH_EVERY: Duration = Duration::from_secs(5);

pub async fn run(cfg: Config) -> Result<()> {
    let me = Identity::load(&cfg.state, &cfg.name)?;
    let d = Arc::new(Daemon {
        state: cfg.state.clone(),
        tailnet: cfg.tailnet_impl,
        info: std::sync::Mutex::new(Info::default()),
        proto: cfg.proto,
        pair_ttl: cfg.pair_ttl,
        agent_ms: cfg.agent_ms,
        idle: cfg.idle,
        agent_style: cfg.agent_style.clone(),
        push: push::Push::new(cfg.push_url.clone(), &cfg.state),
        window: Mutex::new(None),
        sessions: Mutex::new(HashMap::new()),
        prompts: Mutex::new(HashMap::new()),
        busy: std::sync::Mutex::new(HashSet::new()),
        agents: agents::Agents::default(),
        bye: broadcast::channel(4).0,
        leaving: std::sync::atomic::AtomicBool::new(false),
        unauthed: Arc::new(std::sync::Mutex::new(HashMap::new())),
        me,
    });
    d.load_prompts();
    // Agents that kept running through a restart carry on; anything else
    // that was mid-reply is marked interrupted.
    let resumed = d.resume_agents().await;
    d.mark_interrupted(&resumed).await;
    // The spike has no screen: a window opens at start, and SIGHUP stands
    // in for the person opening Settings › Devices to show a new code.
    d.open_window().await?;
    {
        let d = d.clone();
        let version = cfg.version.clone();
        tokio::task::spawn_blocking(move || {
            let mut info = crate::info::info();
            if let Some(v) = version {
                info.dent_version = v;
            }
            if let Ok(mut i) = d.info.lock() {
                *i = info;
            }
        });
    }
    let lan = TcpListener::bind(cfg.lan).await?;
    let lan_port = lan.local_addr()?.port();
    eprintln!(
        "daemon {} ({}) on {}",
        d.me.name,
        d.me.id(),
        lan.local_addr()?
    );
    let _mdns = if cfg.advertise {
        Some(crate::discovery::advertise(
            &d.me.name,
            &d.me.id(),
            lan_port,
        )?)
    } else {
        None
    };
    if let Some(addr) = cfg.tailnet {
        let tl = TcpListener::bind(addr).await?;
        eprintln!("tailnet listener on {}", tl.local_addr()?);
        tokio::spawn(serve(d.clone(), tl, true));
    }
    tokio::spawn(serve(d.clone(), lan, false));
    signals(d).await
}

/// SIGTERM: quitting. SIGUSR1: going to sleep (the spike then freezes
/// itself, as a sleeping machine does). SIGUSR2: restarting to upgrade.
/// SIGHUP: show a new pairing code.
async fn signals(d: Arc<Daemon>) -> Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    let mut sleep = signal(SignalKind::user_defined1())?;
    let mut restart = signal(SignalKind::user_defined2())?;
    let mut hup = signal(SignalKind::hangup())?;
    loop {
        let reason = tokio::select! {
            _ = term.recv() => "quit",
            _ = int.recv() => "quit",
            _ = restart.recv() => "restart",
            _ = sleep.recv() => "sleep",
            _ = hup.recv() => {
                d.open_window().await?;
                continue;
            }
        };
        d.leaving.store(true, Ordering::SeqCst);
        let _ = d.bye.send(reason.into());
        // Let each link flush its goodbye.
        tokio::time::sleep(Duration::from_millis(150)).await;
        if reason == "sleep" {
            let _ = std::process::Command::new("kill")
                .args(["-STOP", &std::process::id().to_string()])
                .status();
            // Awake again.
            d.leaving.store(false, Ordering::SeqCst);
            continue;
        }
        std::process::exit(0);
    }
}

async fn serve(d: Arc<Daemon>, listener: TcpListener, tailnet: bool) {
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            continue;
        };
        let d = d.clone();
        tokio::spawn(async move {
            if let Err(e) = d.handle(stream, peer, tailnet).await {
                eprintln!("connection from {peer} ended: {e:#}");
            }
        });
    }
}

impl Daemon {
    /// Show a fresh code (written to a file so the test harness can play the
    /// person reading it off this machine's screen).
    async fn open_window(&self) -> Result<()> {
        let code = pair::new_code();
        std::fs::write(self.state.join("pairing-code"), &code)?;
        *self.window.lock().await = Some(Window {
            code,
            opened: Instant::now(),
            strikes: 0,
            total: 0,
            locked_until: None,
            pause: Duration::from_secs(1),
        });
        Ok(())
    }

    async fn close_window(&self) {
        let _ = std::fs::remove_file(self.state.join("pairing-code"));
        *self.window.lock().await = None;
    }

    /// Whether `key` may connect over this listener right now. Trust that
    /// came from Tailscale is checked against Tailscale every time, and only
    /// holds on the tailnet: leaving the account ends it.
    fn trust(&self, key: &[u8], peer: SocketAddr, tailnet: bool) -> (bool, &'static str) {
        let same_account = || {
            tailnet
                && self.tailnet.me().is_some_and(|me| {
                    self.tailnet.owner(peer.ip(), peer.port()).as_deref() == Some(me.as_str())
                })
        };
        match Trust::load(&self.state).get(key).map(|p| p.via.clone()) {
            // Removed here: needs a code again, even on your own account.
            Some(via) if via == "removed" => (false, "none"),
            Some(via) if via != "tailnet" => (true, "known"),
            Some(_) if same_account() => (true, "known"),
            Some(_) if !tailnet => (false, "tailnet-only"),
            _ if same_account() => (true, "tailnet"),
            _ => (false, "none"),
        }
    }

    async fn handle(
        self: Arc<Self>,
        stream: TcpStream,
        peer: SocketAddr,
        tailnet: bool,
    ) -> Result<()> {
        if self.leaving.load(Ordering::SeqCst) {
            bail!("on the way out");
        }
        stream.set_nodelay(true)?;
        // Until it proves who it is, a connection holds one of a few slots
        // and has a few seconds.
        let slot = {
            let mut m = self
                .unauthed
                .lock()
                .map_err(|_| anyhow::anyhow!("poisoned"))?;
            let total: usize = m.values().sum();
            let mine = m.entry(peer.ip()).or_insert(0);
            if *mine >= UNAUTHED_PER_IP || total >= UNAUTHED {
                bail!("too many unauthenticated connections");
            }
            *mine += 1;
            Slot(self.unauthed.clone(), peer.ip())
        };
        let opened = tokio::time::timeout(HANDSHAKE, async {
            let ws = tokio_tungstenite::accept_async_with_config(
                stream,
                Some(crate::channel::ws_config()),
            )
            .await?;
            let mut ch = Channel::respond(Ws::Server(ws), &self.me).await?;
            let Up::Hello { name, proto, probe } = ch.recv().await? else {
                bail!("expected hello");
            };
            anyhow::Ok((ch, name, proto, probe))
        })
        .await;
        let (mut ch, name, proto, probe) = match opened {
            Ok(r) => r?,
            Err(_) => bail!("no handshake within {HANDSHAKE:?}"),
        };
        let (trusted, via) = self.trust(&ch.remote, peer, tailnet);
        let compatible = proto >= self.proto.1;
        ch.send(&Down::Welcome {
            name: self.me.name.clone(),
            id: self.me.id(),
            trusted,
            via: via.into(),
            proto: self.proto.0,
            min: self.proto.1,
            // Only paired devices learn what this machine is.
            info: Box::new(if trusted {
                self.info.lock().map(|i| i.clone()).unwrap_or_default()
            } else {
                Info::default()
            }),
        })
        .await?;
        if !compatible {
            ch.send(&Down::Denied {
                reason: "version".into(),
            })
            .await?;
            bail!(
                "client speaks protocol {proto}, oldest supported {}",
                self.proto.1
            );
        }
        if !trusted {
            let r = self.pair(ch, name).await;
            drop(slot);
            return r;
        }
        drop(slot);
        // A probe only wanted to know who's here.
        if probe {
            return Ok(());
        }
        self.seen(&ch.remote, &name, via);
        self.serve_trusted(ch, peer, tailnet).await
    }

    /// Note a trusted device's visit: keeps its name and last-seen time, and
    /// records devices trusted through Tailscale so they're listed.
    fn seen(&self, key: &[u8], name: &str, via: &str) {
        let id = fingerprint(key);
        let now = now_ms();
        // At most once a minute, and only for a device still listed (or one
        // just trusted through Tailscale), so a visit never brings back a
        // device removed a moment ago.
        let fresh = Trust::load(&self.state)
            .peers
            .get(&id)
            .is_some_and(|p| now.saturating_sub(p.last_seen) < 60_000 && p.name == name);
        if fresh {
            return;
        }
        let key = hex::encode(key);
        let _ = Trust::update(&self.state, |peers| {
            if via == "tailnet" && !peers.contains_key(&id) {
                peers.insert(
                    id.clone(),
                    Peer {
                        name: name.into(),
                        key,
                        via: "tailnet".into(),
                        paired_at: now,
                        ..Default::default()
                    },
                );
            }
            if let Some(p) = peers.get_mut(&id) {
                p.name = name.into();
                p.last_seen = now;
            }
        });
    }

    async fn pair(&self, mut ch: Channel, name: String) -> Result<()> {
        let deny = |reason: String| Down::Denied { reason };
        let msg = tokio::time::timeout(HANDSHAKE * 6, ch.recv()).await??;
        let Up::Pair { spake } = msg else {
            ch.send(&deny("pair first".into())).await?;
            bail!("untrusted client skipped pairing");
        };
        // Is a code showing, unexpired and not paused after wrong guesses?
        let code = {
            let mut w = self.window.lock().await;
            match w.as_mut() {
                None => None,
                Some(win) if win.opened.elapsed() > self.pair_ttl => {
                    drop(w);
                    self.close_window().await;
                    None
                }
                Some(win) => match win.locked_until {
                    Some(t) if t > Instant::now() => {
                        let secs = (t - Instant::now()).as_secs_f32().ceil() as u64;
                        drop(w);
                        ch.send(&deny(format!("locked:{secs}"))).await?;
                        bail!("pairing paused");
                    }
                    _ => Some(win.code.clone()),
                },
            }
        };
        let Some(code) = code else {
            ch.send(&deny("closed".into())).await?;
            bail!("no pairing code showing");
        };
        let mut p = Pairing::start(&code);
        ch.send(&Down::Pair {
            spake: hex::encode(&p.outbound),
        })
        .await?;
        let key = p.finish(&hex::decode(spake)?)?;
        let Up::PairConfirm { mac } = tokio::time::timeout(HANDSHAKE, ch.recv()).await?? else {
            bail!("expected confirmation");
        };
        if !pair::verify(&key, &ch.hash, b"client", &hex::decode(mac)?) {
            let reason = self.strike().await?;
            ch.send(&deny(reason)).await?;
            bail!("wrong code from {}", fingerprint(&ch.remote));
        }
        ch.send(&Down::PairConfirm {
            mac: hex::encode(pair::confirm(&key, &ch.hash, b"daemon")),
        })
        .await?;
        let now = now_ms();
        Trust::load(&self.state).add(
            &ch.remote,
            Peer {
                name,
                key: hex::encode(&ch.remote),
                via: "code".into(),
                paired_at: now,
                last_seen: now,
                ..Default::default()
            },
        )?;
        // A code works once; pairing closes until someone shows a new one.
        self.close_window().await;
        ch.send(&Down::Paired).await?;
        Ok(())
    }

    /// A wrong code. Three change the code and pause pairing, doubling each
    /// time; ten close the window. Returns the reason to send back.
    async fn strike(&self) -> Result<String> {
        let mut w = self.window.lock().await;
        let Some(win) = w.as_mut() else {
            return Ok("closed".into());
        };
        win.strikes += 1;
        win.total += 1;
        if win.total >= WINDOW_TRIES {
            drop(w);
            self.close_window().await;
            return Ok("closed".into());
        }
        if win.strikes < TRIES {
            return Ok("wrong".into());
        }
        let code = pair::new_code();
        std::fs::write(self.state.join("pairing-code"), &code)?;
        win.code = code;
        win.strikes = 0;
        win.locked_until = Some(Instant::now() + win.pause);
        win.pause = (win.pause * 2).min(Duration::from_secs(3600));
        Ok("wrong-new-code".into())
    }

    async fn serve_trusted(
        self: Arc<Self>,
        ch: Channel,
        peer: SocketAddr,
        tailnet: bool,
    ) -> Result<()> {
        let key = ch.remote.clone();
        // Tailscale is asked again every 30 s, not on every message.
        let mut checked = Instant::now();
        let (tx, mut rx) = ch.into_parts();
        let tx = Arc::new(Mutex::new(tx));
        let mut bye = self.bye.subscribe();
        let mut tasks = Vec::new();
        let result = loop {
            let msg = tokio::select! {
                m = tokio::time::timeout(self.idle, rx.recv::<Up>()) => match m {
                    Ok(Ok(m)) => m,
                    Ok(Err(e)) => break Err(e),
                    Err(_) => break Ok(()),
                },
                Ok(reason) = bye.recv() => {
                    let _ = tx.lock().await.send(&Down::Bye { reason }).await;
                    break Ok(());
                }
            };
            // Removing a device takes effect on its next message, and so does
            // leaving the Tailscale account.
            let stored = Trust::load(&self.state).get(&key).map(|p| p.via.clone());
            let mut ok = stored.as_deref().is_some_and(|v| v != "removed");
            if ok
                && stored.as_deref() == Some("tailnet")
                && checked.elapsed() > Duration::from_secs(30)
            {
                checked = Instant::now();
                ok = self.trust(&key, peer, tailnet).0;
            }
            if !ok {
                let _ = tx
                    .lock()
                    .await
                    .send(&Down::Denied {
                        reason: "removed".into(),
                    })
                    .await;
                break Ok(());
            }
            match msg {
                Up::Ping { t } => tx.lock().await.send(&Down::Pong { t }).await?,
                Up::Prompt { id, session, text } => {
                    let (session, dup) = self.accept(id.clone(), session, text).await?;
                    tx.lock()
                        .await
                        .send(&Down::Accepted { id, session, dup })
                        .await?;
                }
                Up::Bulk { session, n } => self.bulk(&session, n).await?,
                Up::Push { handle, key: k } => self.push.register(&fingerprint(&key), handle, k),
                Up::Forget => {
                    Trust::load(&self.state).remove(&fingerprint(&key))?;
                    break Ok(());
                }
                Up::Subscribe { session, after } => {
                    let d = self.clone();
                    let tx = tx.clone();
                    let key = key.clone();
                    tasks.push(tokio::spawn(async move {
                        let r = if session == "_status" {
                            d.health(tx).await
                        } else {
                            d.forward(session, after, tx, key).await
                        };
                        if let Err(e) = r {
                            eprintln!("subscription ended: {e:#}");
                        }
                    }));
                }
                _ => {}
            }
        };
        for t in tasks {
            t.abort();
        }
        result
    }

    /// Log a prompt and start the agent, once per prompt id however many
    /// times it's sent. Returns the session it runs in.
    async fn accept(
        self: &Arc<Self>,
        id: String,
        session: String,
        text: String,
    ) -> Result<(String, bool)> {
        let mut prompts = self.prompts.lock().await;
        if let Some(s) = prompts.get(&id) {
            return Ok((s.clone(), true));
        }
        self.append(&session, "user", &text).await?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.state.join("prompts"))?;
        writeln!(f, "{id} {session}")?;
        prompts.insert(id, session.clone());
        drop(prompts);
        let d = self.clone();
        let s = session.clone();
        tokio::spawn(async move {
            if let Err(e) = d.run_agent(&s, &text).await {
                let why = format!("dent couldn't start the agent: {e}");
                let _ = d.append(&s, "interrupted", &why).await;
            }
        });
        Ok((session, false))
    }

    fn load_prompts(&self) {
        let map = std::fs::read_to_string(self.state.join("prompts"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let (id, s) = l.split_once(' ')?;
                Some((id.to_string(), s.to_string()))
            })
            .collect();
        if let Ok(mut p) = self.prompts.try_lock() {
            *p = map;
        }
    }

    /// After a crash or power loss, replies that were mid-way are marked so
    /// every client shows them as interrupted, not still working.
    async fn mark_interrupted(&self, resumed: &HashSet<String>) {
        let Ok(dir) = std::fs::read_dir(self.state.join("sessions")) else {
            return;
        };
        for entry in dir.flatten() {
            let path = entry.path();
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let last = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .lines()
                .rev()
                .find_map(|l| serde_json::from_str::<Event>(l).ok());
            let mid = last.is_some_and(|e| e.kind == "user" || e.kind == "delta");
            if mid && resumed.contains(&id) {
                if let Ok(mut b) = self.busy.lock() {
                    b.insert(id);
                }
            } else if mid {
                let _ = self
                    .append(
                        &id,
                        "interrupted",
                        "dent stopped on this machine before the reply finished",
                    )
                    .await;
            }
        }
    }

    /// Health every few seconds to a watcher, and at once when the number
    /// of agents running changes.
    async fn health(&self, tx: Arc<Mutex<Sender>>) -> Result<()> {
        let mut sent: Option<(u32, Option<String>, Instant)> = None;
        loop {
            let running = self.busy.lock().map(|b| b.len() as u32).unwrap_or(0);
            let push = self.push.state();
            let due = sent.as_ref().is_none_or(|(n, p, at)| {
                *n != running || *p != push || at.elapsed() >= HEALTH_EVERY
            });
            if due {
                let mut h =
                    tokio::task::spawn_blocking(move || crate::info::health(running)).await?;
                h.push = push.clone();
                tx.lock().await.send(&Down::Health(h)).await?;
                sent = Some((running, push, Instant::now()));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn open<'a>(
        &self,
        sessions: &'a mut HashMap<String, Session>,
        id: &str,
    ) -> Result<&'a mut Session> {
        if !sessions.contains_key(id) {
            let dir = self.state.join("sessions");
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(format!("{id}.jsonl"));
            let events: Vec<Event> = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            sessions.insert(
                id.to_string(),
                Session {
                    events,
                    tx: broadcast::channel(4096).0,
                    log: std::io::BufWriter::new(file),
                },
            );
        }
        Ok(sessions.get_mut(id).expect("just inserted"))
    }

    async fn append(&self, id: &str, kind: &str, text: &str) -> Result<()> {
        self.append_from(id, kind, text, None).await
    }

    async fn append_from(
        &self,
        id: &str,
        kind: &str,
        text: &str,
        src: Option<crate::proto::Source>,
    ) -> Result<()> {
        if let Ok(mut b) = self.busy.lock() {
            match kind {
                "user" => {
                    b.insert(id.to_string());
                }
                "end" | "interrupted" => {
                    b.remove(id);
                }
                _ => {}
            }
        }
        let mut sessions = self.sessions.lock().await;
        let s = self.open(&mut sessions, id).await?;
        let ev = Event {
            session: id.to_string(),
            seq: s.events.len() as u64 + 1,
            t: now_ms(),
            kind: kind.into(),
            text: text.into(),
            src,
        };
        writeln!(s.log, "{}", serde_json::to_string(&ev)?)?;
        s.log.flush()?;
        s.events.push(ev.clone());
        let _ = s.tx.send(ev);
        Ok(())
    }

    async fn bulk(&self, id: &str, n: usize) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let s = self.open(&mut sessions, id).await?;
        for i in 0..n {
            let ev = Event {
                session: id.to_string(),
                seq: s.events.len() as u64 + 1,
                t: now_ms(),
                kind: "delta".into(),
                // A paragraph every ten events, as real replies have.
                text: if i % 10 == 9 {
                    format!("event {i} with a line of text like an agent writes.\n\n")
                } else {
                    format!("event {i} with a line of text like an agent writes ")
                },
                src: None,
            };
            writeln!(s.log, "{}", serde_json::to_string(&ev)?)?;
            s.events.push(ev.clone());
            let _ = s.tx.send(ev);
        }
        s.log.flush()?;
        Ok(())
    }

    /// Send everything after `after`, then live events, in order, once each.
    async fn forward(
        &self,
        id: String,
        after: u64,
        tx: Arc<Mutex<Sender>>,
        key: Vec<u8>,
    ) -> Result<()> {
        let mut live = {
            let mut sessions = self.sessions.lock().await;
            self.open(&mut sessions, &id).await?.tx.subscribe()
        };
        let mut last = after;
        loop {
            // Catch up from the log: the start, or after falling behind.
            let backlog: Vec<Event> = {
                let mut sessions = self.sessions.lock().await;
                let s = self.open(&mut sessions, &id).await?;
                s.events.iter().skip(last as usize).cloned().collect()
            };
            for ev in backlog {
                last = ev.seq;
                tx.lock().await.send(&Down::Event(ev)).await?;
            }
            loop {
                match tokio::time::timeout(Duration::from_secs(1), live.recv()).await {
                    Ok(Ok(ev)) if ev.seq <= last => {}
                    Ok(Ok(ev)) if ev.seq == last + 1 => {
                        last = ev.seq;
                        tx.lock().await.send(&Down::Event(ev)).await?;
                    }
                    // A gap or a lag: go back to the log.
                    Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => break,
                    Ok(Err(broadcast::error::RecvError::Closed)) => return Ok(()),
                    Err(_) => {
                        if Trust::load(&self.state)
                            .get(&key)
                            .is_none_or(|p| p.via == "removed")
                        {
                            let _ = tx
                                .lock()
                                .await
                                .send(&Down::Denied {
                                    reason: "removed".into(),
                                })
                                .await;
                            bail!("device removed");
                        }
                    }
                }
            }
        }
    }
}
