//! The client's view of other machines, independent of any UI: what's been
//! found on the LAN and the tailnet, what's paired, what's online, and the
//! replies streaming from prompts run elsewhere. A UI sends commands and
//! redraws when the snapshot changes.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};

use crate::client::{self, Note, Tally};
use crate::identity::{Identity, Peer, Trust};
use crate::proto::Up;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Via {
    Lan,
    Tailnet,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Found, not added.
    Available,
    /// Added on the LAN; waiting for the code shown on the other machine.
    NeedsCode {
        error: Option<String>,
    },
    Connecting,
    Online,
    Offline {
        since: Instant,
    },
}

#[derive(Clone, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub via: Via,
    pub addrs: Vec<SocketAddr>,
    pub status: Status,
    /// How trust was established, once added: "code" or "tailnet".
    pub trusted_via: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Reply {
    pub device: String,
    pub prompt: String,
    pub text: String,
    pub done: bool,
    pub sent: Instant,
    pub first: Option<Duration>,
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
}

pub struct Devices {
    pub cmd: mpsc::UnboundedSender<Cmd>,
    pub state: Arc<Mutex<Snapshot>>,
    pub changed: watch::Receiver<u64>,
}

struct Live {
    devices: BTreeMap<String, Device>,
    keys: BTreeMap<String, Vec<u8>>,
    followers: BTreeMap<String, tokio::task::JoinHandle<()>>,
}

/// Start managing devices on the current tokio runtime. `tailnet_peers` are
/// the addresses of machines in the Tailscale account to probe for dent.
pub fn start(dir: PathBuf, name: &str, tailnet_peers: Vec<SocketAddr>) -> anyhow::Result<Devices> {
    let me = Identity::load(&dir, name)?;
    let state = Arc::new(Mutex::new(Snapshot {
        me: (me.name.clone(), me.id()),
        ..Default::default()
    }));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (bump, changed) = watch::channel(0u64);
    let live = Arc::new(Mutex::new(Live {
        devices: BTreeMap::new(),
        keys: BTreeMap::new(),
        followers: BTreeMap::new(),
    }));
    let publish = {
        let (state, live) = (state.clone(), live.clone());
        let bump = Arc::new(bump);
        move || {
            let devices = live
                .lock()
                .map(|l| l.devices.values().cloned().collect())
                .unwrap_or_default();
            if let Ok(mut s) = state.lock() {
                s.devices = devices;
            }
            bump.send_modify(|n| *n += 1);
        }
    };
    let publish = Arc::new(publish);

    // Paired machines from last time start out connecting.
    for (id, peer) in Trust::load(&dir).peers {
        let Ok(key) = hex::decode(&peer.key) else {
            continue;
        };
        let addrs = peer.addr.iter().filter_map(|a| a.parse().ok()).collect();
        let via = if peer.via == "tailnet" {
            Via::Tailnet
        } else {
            Via::Lan
        };
        let mut l = live.lock().expect("fresh lock");
        l.keys.insert(id.clone(), key);
        l.devices.insert(
            id.clone(),
            Device {
                id: id.clone(),
                name: peer.name,
                via,
                addrs,
                status: Status::Connecting,
                trusted_via: Some(peer.via),
            },
        );
        drop(l);
        watch_status(&me, &live, &publish, &id);
    }
    publish();

    // Discovery: Bonjour on the LAN, and a probe of each tailnet machine.
    {
        let (live, publish, me) = (live.clone(), publish.clone(), me.clone());
        tokio::spawn(async move {
            loop {
                let lan = tokio::task::spawn_blocking(|| {
                    crate::discovery::browse(Duration::from_millis(1500), None).unwrap_or_default()
                })
                .await
                .unwrap_or_default();
                let mut found: Vec<(String, String, Via, Vec<SocketAddr>)> = lan
                    .into_iter()
                    .filter(|f| f.id != me.id() && !f.addrs.is_empty())
                    .map(|f| (f.id, f.name, Via::Lan, f.addrs))
                    .collect();
                for addr in &tailnet_peers {
                    if let Ok(o) = client::open(*addr, &me).await {
                        found.retain(|f| f.0 != o.id);
                        found.push((o.id, o.name, Via::Tailnet, vec![*addr]));
                    }
                }
                {
                    let mut l = live.lock().expect("lock");
                    for (id, name, via, addrs) in found {
                        let d = l.devices.entry(id.clone()).or_insert(Device {
                            id,
                            name: name.clone(),
                            via,
                            addrs: addrs.clone(),
                            status: Status::Available,
                            trusted_via: None,
                        });
                        d.name = name;
                        d.addrs = addrs;
                        d.via = via;
                    }
                }
                publish();
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    // Commands from the UI.
    {
        let (live, publish, state) = (live.clone(), publish.clone(), state.clone());
        tokio::spawn(async move {
            let mut prompts = 0u64;
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    Cmd::Add(id) => {
                        let Some(d) = live.lock().ok().and_then(|l| l.devices.get(&id).cloned())
                        else {
                            continue;
                        };
                        if d.via == Via::Lan {
                            set(&live, &id, Status::NeedsCode { error: None });
                            publish();
                            continue;
                        }
                        // On the tailnet the other side checks the account; if
                        // it's ours we're trusted straight away.
                        let Some(addr) = d.addrs.first().copied() else {
                            continue;
                        };
                        match client::open(addr, &me).await {
                            Ok(o) if o.trusted => {
                                remember(&dir, &live, &id, &o.ch.remote, &d, "tailnet");
                                watch_status(&me, &live, &publish, &id);
                            }
                            _ => set(&live, &id, Status::NeedsCode { error: None }),
                        }
                        publish();
                    }
                    Cmd::Code(id, code) => {
                        let Some(d) = live.lock().ok().and_then(|l| l.devices.get(&id).cloned())
                        else {
                            continue;
                        };
                        let Some(addr) = d.addrs.first().copied() else {
                            continue;
                        };
                        let result = async {
                            let mut o = client::open(addr, &me).await?;
                            client::pair(&mut o.ch, &code).await?;
                            anyhow::Ok(o.ch.remote)
                        }
                        .await;
                        match result {
                            Ok(key) => {
                                remember(&dir, &live, &id, &key, &d, "code");
                                watch_status(&me, &live, &publish, &id);
                            }
                            Err(_) => set(&live, &id, Status::NeedsCode {
                                error: Some("That code didn't match. Check the code on the other machine.".into()),
                            }),
                        }
                        publish();
                    }
                    Cmd::Remove(id) => {
                        let _ = Trust::load(&dir).remove(&id);
                        if let Ok(mut l) = live.lock() {
                            if let Some(f) = l.followers.remove(&id) {
                                f.abort();
                            }
                            l.keys.remove(&id);
                            if let Some(d) = l.devices.get_mut(&id) {
                                d.status = Status::Available;
                                d.trusted_via = None;
                            }
                        }
                        publish();
                    }
                    Cmd::Prompt(id, text) => {
                        prompts += 1;
                        let session = format!("p{}-{prompts}", std::process::id());
                        let (key, d) = match live.lock() {
                            Ok(l) => (l.keys.get(&id).cloned(), l.devices.get(&id).cloned()),
                            Err(_) => continue,
                        };
                        let (Some(key), Some(d)) = (key, d) else {
                            continue;
                        };
                        let index = {
                            let mut s = state.lock().expect("lock");
                            s.replies.push(Reply {
                                device: d.name.clone(),
                                prompt: text.clone(),
                                text: String::new(),
                                done: false,
                                sent: Instant::now(),
                                first: None,
                            });
                            s.replies.len() - 1
                        };
                        publish();
                        let (ntx, mut nrx) = mpsc::unbounded_channel();
                        let resolve = resolver(&live, &id);
                        let follower = client::follow(
                            me.clone(),
                            key,
                            resolve,
                            session.clone(),
                            0,
                            Arc::new(tokio::sync::Mutex::new(Tally::default())),
                            ntx,
                        );
                        if let Some(addr) = d.addrs.first().copied()
                            && let Ok(mut o) = client::open(addr, &me).await
                        {
                            let _ = o.ch.send(&Up::Prompt { session, text }).await;
                        }
                        let (state, publish) = (state.clone(), publish.clone());
                        tokio::spawn(async move {
                            while let Some(n) = nrx.recv().await {
                                let Note::Event(e) = n else { continue };
                                let mut s = state.lock().expect("lock");
                                let r = &mut s.replies[index];
                                match e.kind.as_str() {
                                    "delta" => {
                                        r.first.get_or_insert(r.sent.elapsed());
                                        r.text.push_str(&e.text);
                                    }
                                    "end" => r.done = true,
                                    _ => {}
                                }
                                let done = r.done;
                                drop(s);
                                publish();
                                if done {
                                    break;
                                }
                            }
                            follower.abort();
                        });
                    }
                }
            }
        });
    }
    Ok(Devices {
        cmd: tx,
        state,
        changed,
    })
}

fn set(live: &Arc<Mutex<Live>>, id: &str, status: Status) {
    if let Ok(mut l) = live.lock()
        && let Some(d) = l.devices.get_mut(id)
    {
        d.status = status;
    }
}

fn remember(
    dir: &std::path::Path,
    live: &Arc<Mutex<Live>>,
    id: &str,
    key: &[u8],
    d: &Device,
    via: &str,
) {
    let _ = Trust::load(dir).add(
        key,
        Peer {
            name: d.name.clone(),
            key: hex::encode(key),
            via: via.into(),
            addr: d.addrs.first().map(|a| a.to_string()),
        },
    );
    if let Ok(mut l) = live.lock() {
        l.keys.insert(id.to_string(), key.to_vec());
        if let Some(d) = l.devices.get_mut(id) {
            d.status = Status::Connecting;
            d.trusted_via = Some(via.into());
        }
    }
}

/// The latest known addresses for a device, as discovery updates them.
fn resolver(live: &Arc<Mutex<Live>>, id: &str) -> client::Resolve {
    let (live, id) = (live.clone(), id.to_string());
    Arc::new(move || {
        live.lock()
            .ok()
            .and_then(|l| l.devices.get(&id).map(|d| d.addrs.clone()))
            .unwrap_or_default()
    })
}

/// Keep a link open to a paired device to show whether it's online.
fn watch_status(
    me: &Identity,
    live: &Arc<Mutex<Live>>,
    publish: &Arc<impl Fn() + Send + Sync + 'static>,
    id: &str,
) {
    let Some(key) = live.lock().ok().and_then(|l| l.keys.get(id).cloned()) else {
        return;
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let follower = client::follow(
        me.clone(),
        key,
        resolver(live, id),
        "_status".into(),
        0,
        Arc::new(tokio::sync::Mutex::new(Tally::default())),
        tx,
    );
    if let Ok(mut l) = live.lock()
        && let Some(old) = l.followers.insert(id.to_string(), follower)
    {
        old.abort();
    }
    let (live, publish, id) = (live.clone(), publish.clone(), id.to_string());
    tokio::spawn(async move {
        while let Some(n) = rx.recv().await {
            let status = match n {
                Note::Up { .. } => Status::Online,
                Note::Down { .. } => {
                    let since = match live
                        .lock()
                        .ok()
                        .and_then(|l| l.devices.get(&id).map(|d| d.status.clone()))
                    {
                        Some(Status::Offline { since }) => since,
                        _ => Instant::now(),
                    };
                    Status::Offline { since }
                }
                Note::Denied(_) => Status::Available,
                Note::Event(_) => continue,
            };
            set(&live, &id, status);
            publish();
        }
    });
}
