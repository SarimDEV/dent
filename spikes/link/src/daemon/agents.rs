//! Agents run through the session holder, so restarting or upgrading the
//! daemon never stops a reply mid-way. Each event an agent's output becomes
//! records where in that output it ended, so a new daemon resumes exactly.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use spike_holder::client::{self, Event as HolderEvent, Holder, Start};
use tokio::sync::mpsc;

use super::Daemon;
use crate::proto::Source;

/// An agent with no reply under way is stopped after this long (its stdin
/// closed); the next prompt starts a new one, as the real daemon resumes
/// an agent session from its transcript.
const AGENT_IDLE: std::time::Duration = std::time::Duration::from_secs(30);

/// What the fake agent writes, one JSON object per line.
#[derive(Serialize, Deserialize, Default)]
pub struct Line {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub end: bool,
}

#[derive(Default)]
pub(super) struct Agents {
    holder: tokio::sync::Mutex<Option<Arc<Holder>>>,
    /// Running agent processes by holder id.
    live: std::sync::Mutex<HashMap<String, Live>>,
}

struct Live {
    session: String,
    pid: u32,
    /// Bytes of output turned into events so far; a part line waits in
    /// `buf` for the rest.
    off: u64,
    buf: Vec<u8>,
}

/// The holder's directory for a daemon's state: short, because socket paths
/// are limited to about 100 bytes, and the same across daemon versions, so
/// an upgraded daemon finds the holder its predecessor used.
pub fn holder_dir(state: &Path) -> PathBuf {
    let path = std::fs::canonicalize(state).unwrap_or_else(|_| state.to_path_buf());
    let hash = sha2::Sha256::digest(path.to_string_lossy().as_bytes());
    std::env::temp_dir().join(format!("dent-{}", &hex::encode(hash)[..12]))
}

impl Daemon {
    async fn holder(self: &Arc<Self>) -> Result<Arc<Holder>> {
        let mut g = self.agents.holder.lock().await;
        if let Some(h) = g.as_ref() {
            return Ok(h.clone());
        }
        let exe = std::env::current_exe()?.to_string_lossy().into_owned();
        let dir = holder_dir(&self.state);
        let start = Start {
            argv: vec![
                exe,
                "holder".into(),
                "--dir".into(),
                dir.display().to_string(),
            ],
        };
        let (h, rx) = client::connect(&dir, Some(&start)).await?;
        eprintln!("holder {} (dent {})", h.pid, h.version);
        let h = Arc::new(h);
        *g = Some(h.clone());
        tokio::spawn(self.clone().holder_events(rx));
        Ok(h)
    }

    /// At startup: pick up every agent still running (or that finished
    /// while no daemon was here) from where its events stop.
    pub(super) async fn resume_agents(self: &Arc<Self>) -> HashSet<String> {
        let mut resumed = HashSet::new();
        let Ok(h) = self.holder().await else {
            return resumed;
        };
        for p in h.list().await.unwrap_or_default() {
            let Some(session) = p
                .argv
                .iter()
                .position(|a| a == "--session")
                .and_then(|i| p.argv.get(i + 1))
                .cloned()
            else {
                continue;
            };
            let off = self.resume_point(&session, &p.id).await;
            if let Ok(mut live) = self.agents.live.lock() {
                live.insert(
                    p.id.clone(),
                    Live {
                        session: session.clone(),
                        pid: p.pid,
                        off,
                        buf: Vec::new(),
                    },
                );
            }
            if h.attach(&p.id, off, p.err).await.is_ok() {
                resumed.insert(session);
            }
        }
        resumed
    }

    async fn resume_point(&self, session: &str, proc_id: &str) -> u64 {
        let mut sessions = self.sessions.lock().await;
        let Ok(s) = self.open(&mut sessions, session).await else {
            return 0;
        };
        s.events
            .iter()
            .rev()
            .find_map(|e| e.src.as_ref().filter(|s| s.proc == proc_id).map(|s| s.off))
            .unwrap_or(0)
    }

    /// Hand a prompt to the session's agent, starting one if needed.
    pub(super) async fn run_agent(self: &Arc<Self>, session: &str, text: &str) -> Result<()> {
        let h = self.holder().await?;
        let existing = self.agents.live.lock().ok().and_then(|l| {
            l.iter()
                .find(|(_, v)| v.session == session)
                .map(|(k, _)| k.clone())
        });
        let id = match existing {
            Some(id) => id,
            None => {
                let safe: String = session
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .take(64)
                    .collect();
                let id = format!("{safe}.{}", crate::proto::new_id());
                let exe = std::env::current_exe()?.to_string_lossy().into_owned();
                let argv = vec![
                    exe,
                    "agent".into(),
                    "--name".into(),
                    self.me.name.clone(),
                    "--ms".into(),
                    self.agent_ms.to_string(),
                    "--session".into(),
                    session.into(),
                ];
                let pid = h.spawn(&id, argv, None, Vec::new()).await?;
                if let Ok(mut live) = self.agents.live.lock() {
                    live.insert(
                        id.clone(),
                        Live {
                            session: session.into(),
                            pid,
                            off: 0,
                            buf: Vec::new(),
                        },
                    );
                }
                h.attach(&id, 0, 0).await?;
                id
            }
        };
        let mut line = serde_json::to_vec(&serde_json::json!({ "prompt": text }))?;
        line.push(b'\n');
        h.write(&id, &line).await
    }

    async fn holder_events(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<HolderEvent>) {
        while let Some(ev) = rx.recv().await {
            match ev {
                HolderEvent::Data {
                    id,
                    err: false,
                    off,
                    bytes,
                } => {
                    if let Err(e) = self.agent_output(&id, off, &bytes).await {
                        eprintln!("agent {id}: {e}");
                    }
                }
                HolderEvent::Data { .. } => {}
                HolderEvent::Exit { id, .. } => {
                    let live = self.agents.live.lock().ok().and_then(|mut l| l.remove(&id));
                    if let Some(l) = live {
                        self.interrupt_if_mid_reply(
                            &l.session,
                            "the agent stopped before finishing",
                        )
                        .await;
                    }
                    let h = self.agents.holder.lock().await.clone();
                    if let Some(h) = h {
                        let _ = h.release(&id).await;
                    }
                }
                HolderEvent::Replaced => {
                    eprintln!("another daemon took over the holder; quitting");
                    std::process::exit(0);
                }
                HolderEvent::Closed => {
                    *self.agents.holder.lock().await = None;
                    let lost: Vec<Live> = self
                        .agents
                        .live
                        .lock()
                        .map(|mut l| l.drain().map(|(_, v)| v).collect())
                        .unwrap_or_default();
                    for l in lost {
                        // Its output pipe is gone: stop it now rather than
                        // when it next writes.
                        // SAFETY: kill has no memory preconditions.
                        unsafe { libc::kill(-(l.pid as i32), libc::SIGTERM) };
                        self.interrupt_if_mid_reply(
                            &l.session,
                            "the agent was lost on this machine",
                        )
                        .await;
                    }
                    return;
                }
            }
        }
    }

    fn stop_when_idle(self: &Arc<Self>, id: String, session: String) {
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(AGENT_IDLE).await;
            // Under the lock, so a prompt either sees this agent or starts
            // a new one, never writes to one that's closing.
            let idle = me.agents.live.lock().is_ok_and(|mut l| {
                let busy = me.busy.lock().is_ok_and(|b| b.contains(&session));
                !busy && l.remove(&id).is_some()
            });
            let h = me.agents.holder.lock().await.clone();
            if let (true, Some(h)) = (idle, h) {
                let _ = h.close_stdin(&id).await;
            }
        });
    }

    async fn interrupt_if_mid_reply(&self, session: &str, why: &str) {
        let mid = {
            let mut sessions = self.sessions.lock().await;
            self.open(&mut sessions, session)
                .await
                .ok()
                .and_then(|s| {
                    s.events
                        .last()
                        .map(|e| e.kind == "user" || e.kind == "delta")
                })
                .unwrap_or(false)
        };
        if mid {
            let _ = self.append(session, "interrupted", why).await;
        }
    }

    /// Turn output into events, one per complete line, each recording
    /// where in the output it ended.
    async fn agent_output(self: &Arc<Self>, id: &str, off: u64, bytes: &[u8]) -> Result<()> {
        let mut lines = Vec::new();
        let session;
        {
            let Ok(mut live) = self.agents.live.lock() else {
                bail!("poisoned")
            };
            let Some(l) = live.get_mut(id) else {
                return Ok(());
            };
            session = l.session.clone();
            let have = l.off + l.buf.len() as u64;
            let end = off + bytes.len() as u64;
            if end <= have {
                return Ok(());
            }
            if off > have {
                bail!("output skipped from {have} to {off}");
            }
            l.buf.extend_from_slice(&bytes[(have - off) as usize..]);
            while let Some(i) = l.buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = l.buf.drain(..=i).collect();
                l.off += line.len() as u64;
                lines.push((line, l.off));
            }
        }
        for (line, off) in lines {
            let Ok(l) = serde_json::from_slice::<Line>(&line) else {
                continue;
            };
            let src = Some(Source {
                proc: id.into(),
                off,
            });
            if let Some(d) = l.delta {
                self.append_from(&session, "delta", &d, src.clone()).await?;
            }
            if l.end {
                self.append_from(&session, "end", "", src).await?;
                self.stop_when_idle(id.to_string(), session.clone());
            }
        }
        Ok(())
    }
}

/// The fake agent process: reads prompts on stdin, streams each reply a few
/// words at a time, and exits when stdin closes.
pub fn fake_agent(name: &str, ms: u64) -> Result<()> {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        let prompt = serde_json::from_str::<serde_json::Value>(&line)
            .ok()
            .and_then(|v| v["prompt"].as_str().map(str::to_string))
            .unwrap_or_default();
        let reply = format!(
            "Running on {name}. You asked: \"{prompt}\". This reply streams from the other \
             machine a few words at a time, the way an agent's text arrives, so the \
             link can be checked without spending any usage. Each piece is logged \
             with a sequence number before it is sent, so a client that drops off \
             picks up exactly where it left off."
        );
        let words: Vec<&str> = reply.split_inclusive(' ').collect();
        let (mut i, mut step) = (0, 0u64);
        while i < words.len() {
            let n = 1 + (step % 4) as usize;
            let chunk: String = words[i..(i + n).min(words.len())].concat();
            let l = Line {
                delta: Some(chunk),
                end: false,
            };
            writeln!(out, "{}", serde_json::to_string(&l)?)?;
            out.flush()?;
            i += n;
            step += 1;
            std::thread::sleep(std::time::Duration::from_millis(
                ms + (step * 17) % (ms + 10),
            ));
        }
        let l = Line {
            delta: None,
            end: true,
        };
        writeln!(out, "{}", serde_json::to_string(&l)?)?;
        out.flush()?;
    }
    Ok(())
}
