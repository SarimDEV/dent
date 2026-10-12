//! The daemon side: listens on the LAN and on the tailnet, decides trust,
//! pairs with a code, runs a fake agent and serves sessions from a log.

use std::collections::HashMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, broadcast};

use crate::channel::{Channel, Sender, Ws};
use crate::identity::{Identity, Peer, Trust, fingerprint};
use crate::pair::{self, Pairing};
use crate::proto::{Down, Event, Up, now_ms};
use crate::tailnet::Tailnet;

pub struct Config {
    pub name: String,
    pub state: PathBuf,
    pub lan: SocketAddr,
    pub tailnet: Option<SocketAddr>,
    pub tailnet_impl: Box<dyn Tailnet>,
    pub advertise: bool,
}

struct Session {
    events: Vec<Event>,
    tx: broadcast::Sender<Event>,
    log: std::io::BufWriter<std::fs::File>,
}

struct Daemon {
    me: Identity,
    state: PathBuf,
    tailnet: Box<dyn Tailnet>,
    code: Mutex<(String, u32)>,
    sessions: Mutex<HashMap<String, Session>>,
}

/// A connection is dropped after this long without hearing from it.
const IDLE: Duration = Duration::from_secs(10);
/// Wrong codes allowed before the code changes.
const TRIES: u32 = 3;

pub async fn run(cfg: Config) -> Result<()> {
    let me = Identity::load(&cfg.state, &cfg.name)?;
    let d = Arc::new(Daemon {
        state: cfg.state.clone(),
        tailnet: cfg.tailnet_impl,
        code: Mutex::new((String::new(), 0)),
        sessions: Mutex::new(HashMap::new()),
        me,
    });
    d.rotate_code().await?;
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
    serve(d, lan, false).await;
    Ok(())
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
    /// The code a person would read off this machine's screen. Written to a
    /// file so the test harness can play that person.
    async fn rotate_code(&self) -> Result<()> {
        let code = pair::new_code();
        std::fs::write(self.state.join("pairing-code"), &code)?;
        *self.code.lock().await = (code, 0);
        Ok(())
    }

    fn trusted(&self, key: &[u8]) -> bool {
        Trust::load(&self.state).get(key).is_some()
    }

    async fn handle(
        self: Arc<Self>,
        stream: TcpStream,
        peer: SocketAddr,
        tailnet: bool,
    ) -> Result<()> {
        stream.set_nodelay(true)?;
        let ws = tokio_tungstenite::accept_async(stream).await?;
        let mut ch = Channel::respond(Ws::Server(ws), &self.me).await?;
        let Up::Hello { name } = ch.recv().await? else {
            bail!("expected hello");
        };
        let (trusted, via) = if self.trusted(&ch.remote) {
            (true, "known")
        } else if tailnet
            && let Some(me) = self.tailnet.me()
            && self.tailnet.owner(peer.ip(), peer.port()).as_deref() == Some(me.as_str())
        {
            // Same Tailscale account: Tailscale has already proven it's yours.
            Trust::load(&self.state).add(
                &ch.remote,
                Peer {
                    name: name.clone(),
                    key: hex::encode(&ch.remote),
                    via: "tailnet".into(),
                    addr: None,
                },
            )?;
            (true, "tailnet")
        } else {
            (false, "none")
        };
        ch.send(&Down::Welcome {
            name: self.me.name.clone(),
            id: self.me.id(),
            trusted,
            via: via.into(),
        })
        .await?;
        if !trusted {
            return self.pair(ch, name).await;
        }
        self.serve_trusted(ch).await
    }

    async fn pair(&self, mut ch: Channel, name: String) -> Result<()> {
        let Up::Pair { spake } = ch.recv().await? else {
            ch.send(&Down::Denied {
                reason: "pair first".into(),
            })
            .await?;
            bail!("untrusted client skipped pairing");
        };
        let code = self.code.lock().await.0.clone();
        let mut p = Pairing::start(&code);
        ch.send(&Down::Pair {
            spake: hex::encode(&p.outbound),
        })
        .await?;
        let key = p.finish(&hex::decode(spake)?)?;
        let Up::PairConfirm { mac } = ch.recv().await? else {
            bail!("expected confirmation");
        };
        if !pair::verify(&key, &ch.hash, b"client", &hex::decode(mac)?) {
            let mut c = self.code.lock().await;
            c.1 += 1;
            let rotate = c.1 >= TRIES;
            drop(c);
            if rotate {
                self.rotate_code().await?;
            }
            ch.send(&Down::Denied {
                reason: "wrong code".into(),
            })
            .await?;
            bail!("wrong code from {}", fingerprint(&ch.remote));
        }
        ch.send(&Down::PairConfirm {
            mac: hex::encode(pair::confirm(&key, &ch.hash, b"daemon")),
        })
        .await?;
        Trust::load(&self.state).add(
            &ch.remote,
            Peer {
                name,
                key: hex::encode(&ch.remote),
                via: "code".into(),
                addr: None,
            },
        )?;
        // A code works once.
        self.rotate_code().await?;
        ch.send(&Down::Paired).await?;
        Ok(())
    }

    async fn serve_trusted(self: Arc<Self>, ch: Channel) -> Result<()> {
        let key = ch.remote.clone();
        let (tx, mut rx) = ch.into_parts();
        let tx = Arc::new(Mutex::new(tx));
        let mut tasks = Vec::new();
        let result = loop {
            let msg = match tokio::time::timeout(IDLE, rx.recv::<Up>()).await {
                Ok(Ok(m)) => m,
                Ok(Err(e)) => break Err(e),
                Err(_) => break Ok(()),
            };
            // Removing a device takes effect on its next message.
            if !self.trusted(&key) {
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
                Up::Prompt { session, text } => {
                    let d = self.clone();
                    tokio::spawn(async move { d.fake_agent(session, text).await });
                }
                Up::Bulk { session, n } => self.bulk(&session, n).await?,
                Up::Subscribe { session, after } => {
                    let d = self.clone();
                    let tx = tx.clone();
                    let key = key.clone();
                    tasks.push(tokio::spawn(async move {
                        if let Err(e) = d.forward(session, after, tx, key).await {
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
        let mut sessions = self.sessions.lock().await;
        let s = self.open(&mut sessions, id).await?;
        let ev = Event {
            session: id.to_string(),
            seq: s.events.len() as u64 + 1,
            t: now_ms(),
            kind: kind.into(),
            text: text.into(),
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
                text: format!("event {i} with a line of text like an agent writes "),
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
                        if !self.trusted(&key) {
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

    /// Stands in for an agent: streams a reply a few words at a time.
    async fn fake_agent(&self, session: String, prompt: String) {
        let reply = format!(
            "Running on {}. You asked: \"{}\". This reply streams from the other \
             machine a few words at a time, the way an agent's text arrives, so the \
             link can be checked without spending any usage. Each piece is logged \
             with a sequence number before it is sent, so a client that drops off \
             picks up exactly where it left off.",
            self.me.name, prompt
        );
        let _ = self.append(&session, "user", &prompt).await;
        let words: Vec<&str> = reply.split_inclusive(' ').collect();
        let mut i = 0;
        let mut step = 0u64;
        while i < words.len() {
            let n = 1 + (step % 4) as usize;
            let chunk: String = words[i..(i + n).min(words.len())].concat();
            if self.append(&session, "delta", &chunk).await.is_err() {
                return;
            }
            i += n;
            step += 1;
            tokio::time::sleep(Duration::from_millis(30 + (step * 17) % 40)).await;
        }
        let _ = self.append(&session, "end", "").await;
    }
}
