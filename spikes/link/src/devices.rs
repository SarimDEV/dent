//! The client's view of other machines, independent of any UI: what's been
//! found on the LAN and the tailnet, what's paired, whether each is online,
//! asleep or gone, what it is, and the replies streaming from prompts run
//! elsewhere. A UI sends commands and redraws when the snapshot changes.
//!
//! Everything a person would want while a machine is away is kept here:
//! last seen survives restarts, replies are cached and resume where they
//! left off, and a prompt is never silently lost.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

use crate::client::{self, Note, PairError, Tally};
use crate::identity::{Identity, Peer, Trust, fingerprint, write_atomic};
use crate::proto::{Health, Info, new_id, now_ms};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Via {
    Lan,
    Tailnet,
}

/// What a paired machine is doing, as honestly as the link can tell.
/// Times are wall-clock ms, so they stay right across restarts and sleep.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Found, not added.
    Available,
    /// Waiting for the code shown on the other machine.
    NeedsCode {
        error: Option<String>,
    },
    Connecting,
    Online,
    /// Lost without warning moments ago; probably a blip.
    Reconnecting {
        since: u64,
    },
    /// It said it was going to sleep.
    Asleep {
        since: u64,
    },
    /// It said dent was quitting, or restarting to update.
    Stopped {
        since: u64,
        restarting: bool,
    },
    /// Gone without a word for longer than a blip, or not reached since
    /// this app started.
    Offline {
        since: u64,
    },
    /// The other machine removed this one.
    Removed,
    /// Its dent and this one can't talk; one needs updating.
    NeedsUpdate {
        theirs: u32,
        min: u32,
    },
}

impl Status {
    pub fn online(&self) -> bool {
        *self == Status::Online
    }

    /// A short phrase for why it can't take work right now.
    pub fn away(&self, name: &str) -> Option<String> {
        Some(match self {
            Status::Online | Status::Available | Status::NeedsCode { .. } => return None,
            Status::Connecting => format!("Connecting to {name}"),
            Status::Reconnecting { .. } => format!("Reconnecting to {name}"),
            Status::Asleep { .. } => format!("{name} is asleep"),
            Status::Stopped {
                restarting: true, ..
            } => format!("{name} is restarting"),
            Status::Stopped { .. } => format!("dent isn't running on {name}"),
            Status::Offline { .. } => format!("{name} is offline"),
            Status::Removed => format!("{name} removed this Mac"),
            Status::NeedsUpdate { .. } => format!("{name} needs a dent update"),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub via: Via,
    pub addr: SocketAddr,
    /// When discovery last saw it, wall-clock ms.
    pub seen: u64,
}

#[derive(Clone, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// Every way it's been found recently.
    pub routes: Vec<Route>,
    pub status: Status,
    /// How trust was established, once added: "code" or "tailnet".
    pub trusted_via: Option<String>,
    /// The address the live link uses, and its round trip.
    pub route: Option<SocketAddr>,
    pub rtt_ms: Option<u64>,
    pub info: Option<Info>,
    /// Latest health, and when it arrived here.
    pub health: Option<(Health, u64)>,
    pub last_seen: Option<u64>,
    pub paired_at: Option<u64>,
    /// Last address it answered on, kept across restarts.
    pub last_ok: Option<SocketAddr>,
    /// When discovery last saw it at all.
    pub seen: u64,
    /// A message under the row: why adding failed, or that a wake was sent.
    pub note: Option<String>,
    /// An offline paired machine with the same name: perhaps this is it,
    /// reinstalled with a new identity.
    pub same_name_as: Option<String>,
    /// The dent version it last reported, kept across restarts.
    pub version: Option<String>,
    /// The version it ran before, and when it changed.
    pub updated: Option<(String, u64)>,
}

impl Device {
    /// How the live link (or, failing that, the best route) reaches it.
    pub fn via(&self) -> Via {
        let at = self.route.or(self.last_ok);
        self.routes
            .iter()
            .find(|r| Some(r.addr) == at)
            .or(self.routes.first())
            .map(|r| r.via)
            .unwrap_or(if self.trusted_via.as_deref() == Some("tailnet") {
                Via::Tailnet
            } else {
                Via::Lan
            })
    }

    pub fn addr(&self) -> Option<SocketAddr> {
        self.route
            .or(self.last_ok)
            .or(self.routes.first().map(|r| r.addr))
    }

    /// Addresses to try, best first. Trust through Tailscale only holds on
    /// the tailnet, so those devices are only dialled there.
    fn dial(&self, prefer: Via) -> Vec<SocketAddr> {
        let tail_only = self.trusted_via.as_deref() == Some("tailnet");
        let mut out: Vec<SocketAddr> = Vec::new();
        let push = |out: &mut Vec<SocketAddr>, a: SocketAddr| {
            if !out.contains(&a) {
                out.push(a);
            }
        };
        if let Some(a) = self.last_ok
            && (!tail_only
                || self
                    .routes
                    .iter()
                    .any(|r| r.addr == a && r.via == Via::Tailnet))
        {
            push(&mut out, a);
        }
        let mut routes = self.routes.clone();
        routes.sort_by_key(|r| (r.via != prefer, std::cmp::Reverse(r.seen)));
        for r in routes {
            if !tail_only || r.via == Via::Tailnet {
                push(&mut out, r.addr);
            }
        }
        if out.is_empty()
            && let Some(a) = self.last_ok
        {
            push(&mut out, a);
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ReplyState {
    /// Waiting for the other machine to confirm it has the prompt.
    Sending,
    Running,
    /// Running there, but the link is down: shows why, and catches up.
    Paused(String),
    Done,
    /// The machine stopped before the reply finished.
    Interrupted(String),
    /// Not confirmed by the other machine; Retry sends it again, and it
    /// still runs only once.
    NotSent(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    pub id: String,
    pub session: String,
    pub device: String,
    pub device_name: String,
    pub prompt: String,
    pub text: String,
    pub state: ReplyState,
    pub sent: u64,
    /// Time to first text, ms.
    pub first: Option<u64>,
    /// Last event applied, to resume from.
    pub seq: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub me: (String, String),
    pub devices: Vec<Device>,
    pub replies: Vec<Reply>,
}

pub enum Cmd {
    Add(String),
    Code(String, String),
    Remove(String),
    Prompt(String, String),
    /// Send a reply's prompt again, by index.
    Retry(usize),
    Wake(String),
    /// Clear a removed device from the list.
    Dismiss(String),
}

pub struct Devices {
    pub cmd: mpsc::UnboundedSender<Cmd>,
    pub state: Arc<Mutex<Snapshot>>,
    pub changed: watch::Receiver<u64>,
}

pub struct Options {
    /// Machines in the Tailscale account to probe for dent.
    pub tailnet_peers: Vec<SocketAddr>,
    /// Where Wake sends magic packets; normally the LAN's broadcast address.
    pub wake_to: Option<SocketAddr>,
    /// Use Bonjour to find machines on the LAN.
    pub bonjour: bool,
}

/// Lost for longer than this means offline rather than reconnecting.
const GRACE: u64 = 10_000;
/// An unpaired machine not seen for this long leaves the list.
const STALE: u64 = 10_000;
/// Discovered addresses are forgotten after this long unseen.
const ROUTE_TTL: u64 = 60_000;
/// How long a prompt keeps trying to reach the other machine.
const DELIVER: Duration = Duration::from_secs(10);

struct Live {
    devices: BTreeMap<String, Device>,
    keys: BTreeMap<String, Vec<u8>>,
    followers: BTreeMap<String, tokio::task::JoinHandle<()>>,
    replies: Vec<Reply>,
    /// Devices removed here that still need telling.
    forget: BTreeSet<String>,
    dirty: bool,
}

type Shared = Arc<Mutex<Live>>;

struct Core {
    dir: PathBuf,
    me: Identity,
    live: Shared,
    state: Arc<Mutex<Snapshot>>,
    bump: watch::Sender<u64>,
    opts: Options,
    saving: Mutex<()>,
}

impl Core {
    /// Copy the live state into the snapshot the UI reads, with what's
    /// derived from it (paused replies, same-name hints), and wake the UI.
    fn publish(&self) {
        let Ok(mut l) = self.live.lock() else { return };
        let away: BTreeMap<String, String> = l
            .devices
            .values()
            .filter(|d| d.trusted_via.is_some())
            .filter_map(|d| d.status.away(&d.name).map(|a| (d.id.clone(), a)))
            .collect();
        let gone: Vec<(String, String)> = l
            .devices
            .values()
            .filter(|d| d.trusted_via.is_some() && !d.status.online())
            .map(|d| (d.name.clone(), d.id.clone()))
            .collect();
        for d in l.devices.values_mut() {
            d.same_name_as = (d.trusted_via.is_none())
                .then(|| gone.iter().find(|(n, id)| *n == d.name && *id != d.id))
                .flatten()
                .map(|(n, _)| n.clone());
        }
        let replies = l
            .replies
            .iter()
            .cloned()
            .map(|mut r| {
                if r.state == ReplyState::Running
                    && let Some(why) = away.get(&r.device)
                {
                    r.state = ReplyState::Paused(why.clone());
                }
                r
            })
            .collect();
        let devices = l.devices.values().cloned().collect();
        drop(l);
        if let Ok(mut s) = self.state.lock() {
            s.devices = devices;
            s.replies = replies;
        }
        self.bump.send_modify(|n| *n += 1);
    }

    fn with<T>(&self, f: impl FnOnce(&mut Live) -> T) -> Option<T> {
        self.live.lock().ok().map(|mut l| f(&mut l))
    }

    fn device(&self, id: &str) -> Option<Device> {
        self.with(|l| l.devices.get(id).cloned()).flatten()
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Device)) {
        self.with(|l| l.devices.get_mut(id).map(f));
    }

    fn resolver(self: &Arc<Self>, id: &str) -> client::Resolve {
        let (core, id) = (self.clone(), id.to_string());
        Arc::new(move || {
            core.device(&id)
                .map(|d| d.dial(Via::Lan))
                .unwrap_or_default()
        })
    }

    fn save_last_seen(&self, id: &str, at: u64, addr: Option<SocketAddr>) {
        let id = id.to_string();
        let _ = Trust::update(&self.dir, |p| {
            if let Some(peer) = p.get_mut(&id) {
                peer.last_seen = at;
                if let Some(a) = addr {
                    peer.addr = Some(a.to_string());
                }
            }
        });
    }

    fn save_forget(&self) {
        if let Some(ids) = self.with(|l| l.forget.clone()) {
            let _ = write_atomic(
                &self.dir.join("forget.json"),
                serde_json::to_string(&ids).unwrap_or_default().as_bytes(),
            );
        }
    }

    /// Trust a device whose key was just proven, and start watching it.
    fn remember(self: &Arc<Self>, id: &str, key: &[u8], via: &str, at: Option<SocketAddr>) {
        let Some(d) = self.device(id) else { return };
        let now = now_ms();
        let _ = Trust::load(&self.dir).add(
            key,
            Peer {
                name: d.name.clone(),
                key: hex::encode(key),
                via: via.into(),
                addr: at.map(|a| a.to_string()),
                paired_at: now,
                last_seen: now,
                version: d.version.clone().unwrap_or_default(),
            },
        );
        self.with(|l| {
            l.keys.insert(id.to_string(), key.to_vec());
            l.forget.remove(id);
            if let Some(d) = l.devices.get_mut(id) {
                d.status = Status::Connecting;
                d.trusted_via = Some(via.into());
                d.paired_at = Some(now);
                d.last_ok = at;
                d.note = None;
            }
        });
        self.save_forget();
        self.watch(id);
    }

    /// Keep a link open to a paired device: status, health and round trip.
    fn watch(self: &Arc<Self>, id: &str) {
        let Some(key) = self.with(|l| l.keys.get(id).cloned()).flatten() else {
            return;
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let follower = client::follow(
            self.me.clone(),
            key,
            self.resolver(id),
            "_status".into(),
            0,
            Arc::new(tokio::sync::Mutex::new(Tally::default())),
            tx,
        );
        if let Some(Some(old)) = self.with(|l| l.followers.insert(id.to_string(), follower)) {
            old.abort();
        }
        let (core, id) = (self.clone(), id.to_string());
        tokio::spawn(async move {
            let mut saved = 0u64;
            while let Some(n) = rx.recv().await {
                let now = now_ms();
                let mut stop = false;
                let mut version = None;
                core.update(&id, |d| match n {
                    Note::Up { addr, info } => {
                        d.status = Status::Online;
                        d.route = Some(addr);
                        d.last_ok = Some(addr);
                        let v = info.dent_version.clone();
                        if !v.is_empty() && d.version.as_deref() != Some(v.as_str()) {
                            if let Some(old) = d.version.replace(v.clone()) {
                                d.updated = Some((old, now));
                            }
                            version = Some(v);
                        }
                        d.info = Some(*info);
                        d.last_seen = Some(now);
                    }
                    Note::Health(h) => {
                        d.health = Some((h, now));
                        d.last_seen = Some(now);
                    }
                    Note::Rtt(ms) => {
                        d.rtt_ms = Some(ms);
                        d.last_seen = Some(now);
                    }
                    Note::Bye(reason) => {
                        d.route = None;
                        d.last_seen = Some(now);
                        d.status = match reason.as_str() {
                            "sleep" => Status::Asleep { since: now },
                            r => Status::Stopped {
                                since: now,
                                restarting: r == "restart",
                            },
                        };
                    }
                    Note::Down { .. } => {
                        d.route = None;
                        let last = d.last_seen.unwrap_or(0);
                        d.status = match d.status.clone() {
                            Status::Online => Status::Reconnecting { since: last },
                            Status::Reconnecting { since } if now.saturating_sub(since) > GRACE => {
                                Status::Offline { since }
                            }
                            Status::Connecting => Status::Offline { since: last },
                            s => s,
                        };
                    }
                    Note::Denied(_) => {
                        d.status = Status::Removed;
                        d.trusted_via = None;
                        d.route = None;
                        stop = true;
                    }
                    Note::Incompatible { proto, min } => {
                        d.status = Status::NeedsUpdate { theirs: proto, min };
                    }
                    Note::Event(_) => {}
                });
                if let Some(v) = version {
                    let id = id.clone();
                    let _ = Trust::update(&core.dir, |p| {
                        if let Some(peer) = p.get_mut(&id) {
                            peer.version = v;
                        }
                    });
                }
                let d = core.device(&id);
                if stop {
                    // The other side removed us: forget its key here too.
                    let _ = Trust::load(&core.dir).remove(&id);
                    core.with(|l| l.keys.remove(&id));
                    core.publish();
                    return;
                }
                // Last seen is saved on the way down, and every 30 s while up.
                if let Some(d) = d {
                    let going = !d.status.online();
                    if let Some(at) = d.last_seen
                        && (going && at > saved || at > saved + 30_000)
                    {
                        saved = at;
                        core.save_last_seen(&id, at, d.last_ok);
                    }
                }
                core.publish();
            }
        });
    }

    fn mark_dirty(&self) {
        self.with(|l| l.dirty = true);
    }

    /// Change reply `i`. A new state is saved at once; streamed text is
    /// saved in batches (it resumes from the saved position either way).
    fn reply(&self, i: usize, f: impl FnOnce(&mut Reply)) {
        let changed = self
            .with(|l| {
                let r = l.replies.get_mut(i)?;
                let before = r.state.clone();
                f(r);
                l.dirty = true;
                Some(r.state != before)
            })
            .flatten()
            .unwrap_or(false);
        if changed {
            self.save_replies();
        }
    }

    /// Get prompt `i` to its machine, retrying for a while; then follow
    /// the reply from wherever it got to.
    fn deliver(self: &Arc<Self>, i: usize) {
        let core = self.clone();
        tokio::spawn(async move {
            let Some(r) = core.with(|l| l.replies.get(i).cloned()).flatten() else {
                return;
            };
            let key = core.with(|l| l.keys.get(&r.device).cloned()).flatten();
            let Some(key) = key else {
                core.reply(i, |r| {
                    r.state = ReplyState::NotSent("Not paired any more.".into())
                });
                core.publish();
                return;
            };
            let deadline = tokio::time::Instant::now() + DELIVER;
            let result = loop {
                let addrs = core
                    .device(&r.device)
                    .map(|d| d.dial(Via::Lan))
                    .unwrap_or_default();
                match client::prompt(&core.me, &key, &addrs, &r.id, &r.session, &r.prompt).await {
                    Ok(ok) => break Ok(ok),
                    Err(e) if tokio::time::Instant::now() >= deadline => break Err(e),
                    Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
                }
            };
            match result {
                Ok((session, _dup)) => {
                    core.reply(i, |r| {
                        r.session = session;
                        r.state = ReplyState::Running;
                    });
                    core.publish();
                    core.follow_reply(i);
                }
                Err(_) => {
                    let name = r.device_name.clone();
                    core.reply(i, |r| {
                        r.state = ReplyState::NotSent(format!(
                            "{name} didn't confirm it got this. Retry is safe: it runs once."
                        ))
                    });
                    core.publish();
                }
            }
        });
    }

    /// Stream reply `i` from its last event to the end.
    fn follow_reply(self: &Arc<Self>, i: usize) {
        let Some((r, key)) = self
            .with(|l| {
                let r = l.replies.get(i)?.clone();
                Some((l.keys.get(&r.device)?.clone(), r))
            })
            .flatten()
            .map(|(k, r)| (r, k))
        else {
            return;
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let follower = client::follow(
            self.me.clone(),
            key,
            self.resolver(&r.device),
            r.session.clone(),
            r.seq,
            Arc::new(tokio::sync::Mutex::new(Tally::default())),
            tx,
        );
        let core = self.clone();
        tokio::spawn(async move {
            while let Some(n) = rx.recv().await {
                let Note::Event(e) = n else { continue };
                let mut finished = false;
                core.reply(i, |r| {
                    r.seq = e.seq;
                    match e.kind.as_str() {
                        "delta" => {
                            r.first.get_or_insert(now_ms().saturating_sub(r.sent));
                            r.text.push_str(&e.text);
                        }
                        "end" => {
                            r.state = ReplyState::Done;
                            finished = true;
                        }
                        "interrupted" => {
                            r.state = ReplyState::Interrupted(e.text.clone());
                            finished = true;
                        }
                        _ => {}
                    }
                });
                core.publish();
                if finished {
                    break;
                }
            }
            follower.abort();
        });
    }

    fn save_replies(&self) {
        // One writer at a time, so an older copy never lands after a newer.
        let _g = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        let Some(replies) = self.with(|l| {
            let dirty = std::mem::take(&mut l.dirty);
            dirty.then(|| l.replies.clone())
        }) else {
            return;
        };
        if let Some(r) = replies {
            let _ = write_atomic(
                &self.dir.join("replies.json"),
                serde_json::to_string(&r).unwrap_or_default().as_bytes(),
            );
        }
    }

    async fn command(self: &Arc<Self>, cmd: Cmd) {
        match cmd {
            Cmd::Add(id) => self.add(&id).await,
            Cmd::Code(id, code) => self.code(&id, &code).await,
            Cmd::Remove(id) => self.remove(&id),
            Cmd::Dismiss(id) => {
                self.with(|l| {
                    if l.devices
                        .get(&id)
                        .is_some_and(|d| d.status == Status::Removed)
                    {
                        l.devices.remove(&id);
                    }
                });
            }
            Cmd::Prompt(id, text) => {
                let Some(d) = self.device(&id) else { return };
                if d.trusted_via.is_none() {
                    return;
                }
                let state = match d.status.away(&d.name) {
                    // Nothing is queued for later: a prompt starting hours
                    // after it was typed would be a surprise.
                    Some(why) => ReplyState::NotSent(format!("{why}. Nothing was sent.")),
                    None => ReplyState::Sending,
                };
                let send = state == ReplyState::Sending;
                let i = self
                    .with(|l| {
                        l.replies.push(Reply {
                            id: new_id(),
                            session: new_id(),
                            device: id.clone(),
                            device_name: d.name.clone(),
                            prompt: text,
                            text: String::new(),
                            state,
                            sent: now_ms(),
                            first: None,
                            seq: 0,
                        });
                        l.dirty = true;
                        l.replies.len() - 1
                    })
                    .unwrap_or(0);
                // A prompt someone typed is on disk before it's sent.
                self.save_replies();
                if send {
                    self.deliver(i);
                }
            }
            Cmd::Retry(i) => {
                let ok = self
                    .with(|l| {
                        let r = l.replies.get_mut(i)?;
                        matches!(r.state, ReplyState::NotSent(_)).then(|| {
                            r.state = ReplyState::Sending;
                            r.sent = now_ms();
                        })
                    })
                    .flatten()
                    .is_some();
                if ok {
                    self.mark_dirty();
                    self.save_replies();
                    self.deliver(i);
                }
            }
            Cmd::Wake(id) => {
                let Some(d) = self.device(&id) else { return };
                let macs = d.info.as_ref().map(|i| i.macs.clone()).unwrap_or_default();
                let mut targets: Vec<SocketAddr> = self.opts.wake_to.into_iter().collect();
                if targets.is_empty() {
                    targets.push(SocketAddr::from(([255, 255, 255, 255], 9)));
                    // The subnet's broadcast address, assuming a /24.
                    if let Some(SocketAddr::V4(a)) =
                        d.routes.iter().find(|r| r.via == Via::Lan).map(|r| r.addr)
                    {
                        let o = a.ip().octets();
                        targets.push(SocketAddr::from(([o[0], o[1], o[2], 255], 9)));
                    }
                }
                let sent = crate::info::wake(&macs, &targets);
                let name = d.name.clone();
                self.update(&id, |d| {
                    d.note = Some(if sent > 0 {
                        format!("Sent a wake request to {name}. Macs wake this way when Wake for network access is on.")
                    } else {
                        format!("{name} hasn't shared a network address to wake.")
                    })
                });
            }
        }
        self.publish();
    }

    async fn add(self: &Arc<Self>, id: &str) {
        let Some(d) = self.device(id) else { return };
        // Try the tailnet first: your own account needs no code.
        let addrs = d.dial(Via::Tailnet);
        self.update(id, |d| d.note = None);
        match client::open_any(&addrs, &self.me, None).await {
            Ok(o) if fingerprint(&o.ch.remote) != id => self.update(id, |d| {
                d.note = Some(format!(
                    "{} answered with a different identity than it announced. Not adding it.",
                    d.name
                ))
            }),
            Ok(o) if !o.compatible() => self.update(id, |d| {
                d.status = Status::NeedsUpdate {
                    theirs: o.proto,
                    min: o.min,
                }
            }),
            // Already trusted there (your Tailscale account, or added
            // before): no code needed.
            Ok(o) if o.trusted => {
                let via = if o.via == "tailnet" {
                    "tailnet"
                } else {
                    "code"
                };
                let key = o.ch.remote.clone();
                self.remember(id, &key, via, Some(o.addr));
            }
            Ok(_) => self.update(id, |d| d.status = Status::NeedsCode { error: None }),
            Err(_) => self.update(id, |d| {
                d.note = Some(format!(
                    "Couldn't reach {}. Check it's awake and on this network.",
                    d.name
                ))
            }),
        }
    }

    async fn code(self: &Arc<Self>, id: &str, code: &str) {
        let Some(d) = self.device(id) else { return };
        let name = d.name.clone();
        let fail = |e: String| Status::NeedsCode { error: Some(e) };
        // People paste "123 456" or "123-456".
        let code: String = code
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect();
        if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
            self.update(id, |d| d.status = fail("Codes are 6 digits.".into()));
            return;
        }
        let addrs = d.dial(Via::Lan);
        let mut o = match client::open_any(&addrs, &self.me, None).await {
            Ok(o) => o,
            Err(_) => {
                self.update(id, |d| {
                    d.status = fail(format!(
                        "Couldn't reach {name}. Check it's awake and on this network."
                    ))
                });
                return;
            }
        };
        if fingerprint(&o.ch.remote) != id {
            self.update(id, |d| {
                d.status = fail(format!(
                    "{name} answered with a different identity than it announced."
                ))
            });
            return;
        }
        let at = Some(o.addr);
        if o.trusted {
            let key = o.ch.remote.clone();
            self.remember(id, &key, "code", at);
            return;
        }
        match client::pair(&mut o.ch, &code).await {
            Ok(()) => {
                let key = o.ch.remote.clone();
                self.remember(id, &key, "code", at);
            }
            Err(e) => {
                let msg = match e {
                    PairError::Wrong => {
                        format!("That code didn't match. Check the code on {name}.")
                    }
                    PairError::WrongNewCode => {
                        format!("That code didn't match, so {name} now shows a new one.")
                    }
                    PairError::Locked(s) => format!(
                        "Too many wrong codes. Try again in {s} s with the code now on {name}."
                    ),
                    PairError::Closed => format!(
                        "No code is showing on {name}. Open Settings › Devices there to show one."
                    ),
                    PairError::Unreachable(_) => {
                        format!("Lost {name} while pairing. Check it's awake and try again.")
                    }
                    PairError::Failed(e) => format!("Pairing failed: {e}"),
                };
                self.update(id, |d| d.status = fail(msg));
            }
        }
    }

    fn remove(self: &Arc<Self>, id: &str) {
        let _ = Trust::load(&self.dir).remove(id);
        let (key, d) = self
            .with(|l| {
                if let Some(f) = l.followers.remove(id) {
                    f.abort();
                }
                l.forget.insert(id.to_string());
                let key = l.keys.remove(id);
                if let Some(d) = l.devices.get_mut(id) {
                    d.status = Status::Available;
                    d.trusted_via = None;
                    d.route = None;
                    d.note = None;
                }
                (key, l.devices.get(id).cloned())
            })
            .unwrap_or_default();
        self.save_forget();
        // Tell the other side to forget this device too. If it's away,
        // discovery tries again when it's next seen.
        if let Some(d) = d {
            let core = self.clone();
            let id = id.to_string();
            tokio::spawn(async move {
                let addrs = d.dial(Via::Lan);
                if client::forget(&core.me, key.as_deref(), &addrs)
                    .await
                    .is_ok()
                {
                    core.with(|l| l.forget.remove(&id));
                    core.save_forget();
                }
            });
        }
    }

    /// One round of discovery: Bonjour on the LAN, and a probe of each
    /// tailnet machine, all at once.
    async fn discover(self: &Arc<Self>) {
        let bonjour = self.opts.bonjour;
        let lan = tokio::task::spawn_blocking(move || {
            if bonjour {
                crate::discovery::browse(Duration::from_millis(1500), None).unwrap_or_default()
            } else {
                Vec::new()
            }
        });
        let probes = self.opts.tailnet_peers.iter().map(|&addr| {
            let me = self.me.clone();
            async move { client::probe(addr, &me).await.ok().map(|o| (addr, o)) }
        });
        let tail = futures_util::future::join_all(probes).await;
        let lan = lan.await.unwrap_or_default();
        let now = now_ms();
        let mine = self.me.id();
        let mut found: Vec<(String, String, Via, SocketAddr, bool)> = Vec::new();
        for f in lan.into_iter().filter(|f| f.id != mine) {
            for a in f.addrs {
                found.push((f.id.clone(), f.name.clone(), Via::Lan, a, false));
            }
        }
        // A tailnet probe proved the key; Bonjour's id and name are only
        // what was announced.
        for (addr, o) in tail.into_iter().flatten() {
            found.push((fingerprint(&o.ch.remote), o.name, Via::Tailnet, addr, true));
        }
        let forget = self.with(|l| {
            for (id, name, via, addr, proven) in found {
                let d = l.devices.entry(id.clone()).or_insert_with(|| Device {
                    id: id.clone(),
                    name: name.clone(),
                    routes: Vec::new(),
                    status: Status::Available,
                    trusted_via: None,
                    route: None,
                    rtt_ms: None,
                    info: None,
                    health: None,
                    last_seen: None,
                    paired_at: None,
                    last_ok: None,
                    seen: now,
                    note: None,
                    same_name_as: None,
                    version: None,
                    updated: None,
                });
                // A paired machine's name comes from its own authenticated
                // welcome, not from whoever announces its id.
                if d.trusted_via.is_none() || proven {
                    d.name = name;
                }
                d.seen = now;
                match d.routes.iter_mut().find(|r| r.addr == addr) {
                    Some(r) => {
                        r.seen = now;
                        r.via = via;
                    }
                    None => d.routes.push(Route {
                        via,
                        addr,
                        seen: now,
                    }),
                }
            }
            for d in l.devices.values_mut() {
                d.routes.retain(|r| now.saturating_sub(r.seen) < ROUTE_TTL);
            }
            // Unpaired machines that have gone quiet leave the list.
            l.devices.retain(|_, d| {
                d.trusted_via.is_some()
                    || matches!(d.status, Status::NeedsCode { .. } | Status::Removed)
                    || now.saturating_sub(d.seen) < STALE
            });
            l.forget
                .iter()
                .filter_map(|id| l.devices.get(id).cloned())
                .filter(|d| now.saturating_sub(d.seen) < 5_000)
                .collect::<Vec<_>>()
        });
        for d in forget.unwrap_or_default() {
            let core = self.clone();
            tokio::spawn(async move {
                if client::forget(&core.me, None, &d.dial(Via::Lan))
                    .await
                    .is_ok()
                {
                    core.with(|l| l.forget.remove(&d.id));
                    core.save_forget();
                }
            });
        }
        self.publish();
    }
}

/// Start managing devices on the current tokio runtime. `tailnet_peers` are
/// the addresses of machines in the Tailscale account to probe for dent.
pub fn start(dir: PathBuf, name: &str, tailnet_peers: Vec<SocketAddr>) -> anyhow::Result<Devices> {
    start_with(
        dir,
        name,
        Options {
            tailnet_peers,
            wake_to: None,
            bonjour: true,
        },
    )
}

pub fn start_with(dir: PathBuf, name: &str, opts: Options) -> anyhow::Result<Devices> {
    let me = Identity::load(&dir, name)?;
    let state = Arc::new(Mutex::new(Snapshot {
        me: (me.name.clone(), me.id()),
        ..Default::default()
    }));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (bump, changed) = watch::channel(0u64);
    let forget: BTreeSet<String> = std::fs::read_to_string(dir.join("forget.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let replies = load_replies(&dir);
    let core = Arc::new(Core {
        dir: dir.clone(),
        me,
        live: Arc::new(Mutex::new(Live {
            devices: BTreeMap::new(),
            keys: BTreeMap::new(),
            followers: BTreeMap::new(),
            replies,
            forget,
            dirty: false,
        })),
        state: state.clone(),
        bump,
        opts,
        saving: Mutex::new(()),
    });

    // Paired machines from last time start out connecting, with what was
    // known about them.
    for (id, peer) in Trust::load(&dir).peers {
        let Ok(key) = hex::decode(&peer.key) else {
            continue;
        };
        let last_ok = peer.addr.as_deref().and_then(|a| a.parse().ok());
        let via = if peer.via == "tailnet" {
            Via::Tailnet
        } else {
            Via::Lan
        };
        let opt = |v: u64| (v > 0).then_some(v);
        let opt_str = |v: String| (!v.is_empty()).then_some(v);
        core.with(|l| {
            l.keys.insert(id.clone(), key);
            l.devices.insert(
                id.clone(),
                Device {
                    id: id.clone(),
                    name: peer.name,
                    routes: last_ok
                        .map(|addr| {
                            vec![Route {
                                via,
                                addr,
                                seen: now_ms(),
                            }]
                        })
                        .unwrap_or_default(),
                    status: Status::Connecting,
                    trusted_via: Some(peer.via),
                    route: None,
                    rtt_ms: None,
                    info: None,
                    health: None,
                    last_seen: opt(peer.last_seen),
                    paired_at: opt(peer.paired_at),
                    last_ok,
                    seen: 0,
                    note: None,
                    same_name_as: None,
                    version: opt_str(peer.version),
                    updated: None,
                },
            );
        });
        core.watch(&id);
    }
    // Replies that were streaming carry on from where they got to; ones
    // never confirmed are offered to send again.
    let n = core.with(|l| l.replies.len()).unwrap_or(0);
    for i in 0..n {
        let state = core.with(|l| l.replies[i].state.clone());
        match state {
            Some(ReplyState::Running) | Some(ReplyState::Paused(_)) => core.follow_reply(i),
            Some(ReplyState::Sending) => core.reply(i, |r| {
                r.state = ReplyState::NotSent(
                    "dent closed before the other machine confirmed it. Retry is safe: it runs once."
                        .into(),
                )
            }),
            _ => {}
        }
    }
    core.publish();

    {
        let core = core.clone();
        tokio::spawn(async move {
            loop {
                core.discover().await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }
    // Offline is reached by the clock as well as by notes.
    {
        let core = core.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                core.save_replies();
                let now = now_ms();
                let mut changed = false;
                core.with(|l| {
                    for d in l.devices.values_mut() {
                        if let Status::Reconnecting { since } = d.status
                            && now.saturating_sub(since) > GRACE
                        {
                            d.status = Status::Offline { since };
                            changed = true;
                        }
                    }
                });
                if changed {
                    core.publish();
                }
            }
        });
    }
    {
        let core = core.clone();
        tokio::spawn(async move {
            // Each command runs on its own, so a slow Add never holds up a
            // Remove or a prompt.
            while let Some(cmd) = rx.recv().await {
                let core = core.clone();
                tokio::spawn(async move { core.command(cmd).await });
            }
        });
    }
    Ok(Devices {
        cmd: tx,
        state,
        changed,
    })
}

fn load_replies(dir: &Path) -> Vec<Reply> {
    std::fs::read_to_string(dir.join("replies.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
