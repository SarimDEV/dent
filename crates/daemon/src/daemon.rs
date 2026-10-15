//! What every session and connection shares: the log, the live feed, the
//! holder connection and the session tasks.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result, bail};
use dent_protocol::{Body, Event, Model, Permission};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::env::AgentEnv;
use crate::holder::{self, Holder};
use crate::paths::Paths;
use crate::session::{self, Cmd, Op};
use crate::store::{Source, Store};

/// The key for the commands every session starts with, read by the usage
/// probe before any session has.
const EVERY: &str = "";

/// Something every subscribed client should hear about now.
#[derive(Debug, Clone)]
pub enum Live {
    Event(Event),
    Models(Vec<Model>),
    Delta {
        session: String,
        text: String,
        reset: bool,
    },
    Activity {
        session: String,
        activity: Option<dent_protocol::Activity>,
    },
    Commands {
        session: String,
        commands: Vec<dent_protocol::Command>,
    },
    /// Anything else that's live and never logged.
    Out(dent_protocol::Out),
}

pub struct Daemon {
    pub paths: Paths,
    /// Re-read now and then, so tools installed after dent started are found.
    env: Mutex<Arc<AgentEnv>>,
    pub version: String,
    /// This binary's build stamp, taken at start.
    pub build: String,
    pub machine: String,
    store: Mutex<Store>,
    pub live: broadcast::Sender<Arc<Live>>,
    sessions: Mutex<HashMap<String, mpsc::UnboundedSender<Cmd>>>,
    holder: tokio::sync::Mutex<Option<Arc<Holder>>>,
    /// Each session's reply so far, for clients that subscribe mid-stream.
    partials: Mutex<HashMap<String, String>>,
    models: Mutex<Vec<Model>>,
    /// The last usage reading and when it was taken.
    usage: tokio::sync::Mutex<Option<(Vec<dent_protocol::Window>, u64)>>,
    /// Slash commands by session, as each agent reported them.
    commands: Mutex<HashMap<String, Vec<dent_protocol::Command>>>,
    /// What each session's agent knows that fills commands in; `EVERY`
    /// holds what the usage probe read (the `/config` settings).
    catalogs: Mutex<HashMap<String, crate::claude::commands::Catalog>>,
    /// What each session's agent is doing besides talking.
    activities: Mutex<HashMap<String, dent_protocol::Activity>>,
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Daemon {
    pub fn new(paths: Paths, env: AgentEnv, store: Store) -> Arc<Self> {
        let models = store
            .get("models")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let (live, _) = broadcast::channel(4096);
        Arc::new(Self {
            paths,
            env: Mutex::new(Arc::new(env)),
            version: env!("CARGO_PKG_VERSION").into(),
            build: std::env::current_exe()
                .map(|p| dent_protocol::build_stamp(&p))
                .unwrap_or_default(),
            machine: hostname(),
            store: Mutex::new(store),
            live,
            sessions: Mutex::new(HashMap::new()),
            holder: tokio::sync::Mutex::new(None),
            partials: Mutex::new(HashMap::new()),
            models: Mutex::new(models),
            commands: Mutex::new(HashMap::new()),
            catalogs: Mutex::new(HashMap::new()),
            activities: Mutex::new(HashMap::new()),
            usage: tokio::sync::Mutex::new(None),
        })
    }

    pub fn store(&self) -> MutexGuard<'_, Store> {
        lock(&self.store)
    }

    /// Write an event and tell every client.
    pub fn append(&self, session: &str, body: Body, src: Option<Source<'_>>) -> Result<Event> {
        let ev = self.store().append(session, now_ms(), &body, src)?;
        if matches!(ev.body, Body::Assistant { parent: None, .. }) {
            lock(&self.partials).remove(session);
        }
        let _ = self.live.send(Arc::new(Live::Event(ev.clone())));
        Ok(ev)
    }

    pub fn delta(&self, session: &str, text: &str, reset: bool) {
        {
            let mut p = lock(&self.partials);
            let s = p.entry(session.into()).or_default();
            if reset {
                s.clear();
            }
            s.push_str(text);
        }
        let _ = self.live.send(Arc::new(Live::Delta {
            session: session.into(),
            text: text.into(),
            reset,
        }));
    }

    /// A reply was replaced by its final message, or cut off: clients drop
    /// what streamed so far.
    pub fn clear_partial(&self, session: &str) {
        if lock(&self.partials).remove(session).is_some() {
            let _ = self.live.send(Arc::new(Live::Delta {
                session: session.into(),
                text: String::new(),
                reset: true,
            }));
        }
    }

    /// What has streamed of the reply in progress.
    pub fn partial(&self, session: &str) -> Option<String> {
        lock(&self.partials)
            .get(session)
            .filter(|t| !t.trim().is_empty())
            .cloned()
    }

    pub fn partials(&self) -> Vec<(String, String)> {
        lock(&self.partials)
            .iter()
            .filter(|(_, t)| !t.is_empty())
            .map(|(s, t)| (s.clone(), t.clone()))
            .collect()
    }

    pub fn models(&self) -> Vec<Model> {
        let m = lock(&self.models).clone();
        if m.is_empty() {
            crate::claude::default_models()
        } else {
            m
        }
    }

    pub fn set_models(&self, models: Vec<Model>) {
        if models.is_empty() || *lock(&self.models) == models {
            return;
        }
        if let Ok(s) = serde_json::to_string(&models) {
            let _ = self.store().put("models", &s);
        }
        *lock(&self.models) = models.clone();
        let _ = self.live.send(Arc::new(Live::Models(models)));
    }

    pub fn set_commands(&self, session: &str, commands: Vec<dent_protocol::Command>) {
        if !commands.is_empty() {
            lock(&self.commands).insert(session.into(), commands);
        }
    }

    /// A session's commands changed while it ran: clients get the new list.
    pub fn commands_changed(&self, session: &str, commands: Vec<dent_protocol::Command>) {
        self.set_commands(session, commands);
        let _ = self.live.send(Arc::new(Live::Commands {
            session: session.into(),
            commands: self.commands(Some(session)),
        }));
    }

    /// Tell every client something live that isn't logged.
    pub fn tell(&self, out: dent_protocol::Out) {
        let _ = self.live.send(Arc::new(Live::Out(out)));
    }

    /// Change what a session's agent is known to offer.
    pub fn catalog(&self, session: &str, f: impl FnOnce(&mut crate::claude::commands::Catalog)) {
        f(lock(&self.catalogs).entry(session.into()).or_default());
    }

    /// A session's commands, each with what it takes; before it has
    /// started, the ones every session starts with (from the usage probe),
    /// or another session's.
    pub fn commands(&self, session: Option<&str>) -> Vec<dent_protocol::Command> {
        let raw = {
            let all = lock(&self.commands);
            session
                .and_then(|s| all.get(s))
                .or_else(|| all.get(EVERY))
                .or_else(|| all.values().next())
                .cloned()
                .unwrap_or_default()
        };
        let cat = {
            let cats = lock(&self.catalogs);
            let every = cats.get(EVERY).cloned().unwrap_or_default();
            let mut cat = session
                .and_then(|s| cats.get(s))
                .cloned()
                .unwrap_or_else(|| every.clone());
            if cat.settings.is_empty() {
                cat.settings = every.settings;
            }
            if cat.styles.is_empty() {
                cat.styles = every.styles;
                cat.style = cat.style.or(every.style);
            }
            if !cat.effective.is_object() {
                cat.effective = every.effective;
            }
            cat.models = self.models();
            cat
        };
        crate::claude::commands::typed(&raw, &cat)
    }

    /// What a session's agent is doing besides talking, for every client.
    pub fn set_activity(&self, session: &str, activity: Option<dent_protocol::Activity>) {
        let changed = {
            let mut all = lock(&self.activities);
            match &activity {
                Some(a) => all.insert(session.into(), a.clone()).as_ref() != Some(a),
                None => all.remove(session).is_some(),
            }
        };
        if changed {
            let _ = self.live.send(Arc::new(Live::Activity {
                session: session.into(),
                activity,
            }));
        }
    }

    pub fn activities(&self) -> Vec<(String, dent_protocol::Activity)> {
        lock(&self.activities)
            .iter()
            .map(|(s, a)| (s.clone(), a.clone()))
            .collect()
    }

    /// Account usage, read at most once a minute however many ask.
    pub async fn usage(self: &Arc<Self>) -> (Vec<dent_protocol::Window>, u64) {
        let mut cached = self.usage.lock().await;
        if let Some((w, at)) = cached.as_ref()
            && now_ms().saturating_sub(*at) < 60_000
        {
            return (w.clone(), *at);
        }
        let me = self.clone();
        let read = tokio::task::spawn_blocking({
            let me = me.clone();
            move || crate::usage::probe(&me.env())
        })
        .await
        .ok();
        let (windows, models, commands, catalog) = read
            .map(|p| (p.windows, p.models, p.commands, p.catalog))
            .unwrap_or_default();
        me.set_models(models);
        me.set_commands(EVERY, commands);
        if !catalog.settings.is_empty() {
            me.catalog(EVERY, |c| *c = catalog);
        }
        let reading = (windows.unwrap_or_default(), now_ms());
        *cached = Some(reading.clone());
        reading
    }

    /// The holder, connecting (and starting it) on first use.
    pub fn env(&self) -> Arc<AgentEnv> {
        lock(&self.env).clone()
    }

    /// Ask the login shell again, in the background; the next agent started
    /// gets the answer.
    pub fn refresh_env(self: &Arc<Self>) {
        let me = self.clone();
        tokio::task::spawn_blocking(move || {
            *lock(&me.env) = Arc::new(AgentEnv::detect());
        });
    }

    pub async fn holder(self: &Arc<Self>) -> Result<Arc<Holder>> {
        let mut h = self.holder.lock().await;
        if let Some(h) = h.as_ref() {
            return Ok(h.clone());
        }
        let bin = sibling("dent-session-holder")?;
        let dir = self.paths.holder();
        let start = holder::Start {
            argv: vec![
                bin.display().to_string(),
                "--dir".into(),
                dir.display().to_string(),
            ],
        };
        let (conn, mut rx) = holder::connect(&dir, Some(&start)).await?;
        let conn = Arc::new(conn);
        *h = Some(conn.clone());
        let me = self.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                match ev {
                    holder::Event::Data {
                        id,
                        err,
                        off,
                        bytes,
                    } => me.route(
                        &id,
                        Op::Output {
                            proc: id.clone(),
                            err,
                            off,
                            bytes,
                        },
                    ),
                    holder::Event::Exit { id, code, signal } => me.route(
                        &id,
                        Op::Exit {
                            proc: id.clone(),
                            code,
                            signal,
                        },
                    ),
                    holder::Event::Replaced => {
                        eprintln!("dent-daemon: another daemon took over the holder; exiting");
                        std::process::exit(0);
                    }
                    holder::Event::Closed => {
                        *me.holder.lock().await = None;
                        let all: Vec<_> = lock(&me.sessions).values().cloned().collect();
                        for tx in all {
                            let _ = tx.send(Cmd::new(Op::HolderLost));
                        }
                        return;
                    }
                }
            }
        });
        Ok(conn)
    }

    /// Hand holder output to the session whose process it is.
    fn route(&self, proc: &str, op: Op) {
        let Some((session, _)) = proc.rsplit_once('-') else {
            return;
        };
        if let Some(tx) = lock(&self.sessions).get(session) {
            let _ = tx.send(Cmd::new(op));
        }
    }

    pub fn register(&self, id: &str, tx: mpsc::UnboundedSender<Cmd>) {
        lock(&self.sessions).insert(id.into(), tx);
    }

    /// Run `op` on a session and wait for it to be accepted.
    pub async fn command(&self, session: &str, op: Op) -> Result<()> {
        let tx = lock(&self.sessions)
            .get(session)
            .cloned()
            .with_context(|| format!("no session {session}"))?;
        let (reply, rx) = oneshot::channel();
        tx.send(Cmd {
            op,
            reply: Some(reply),
        })
        .map_err(|_| anyhow::anyhow!("session {session} has stopped"))?;
        match rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => bail!("{e}"),
            Err(_) => bail!("session {session} has stopped"),
        }
    }

    /// Start a session and send its first message.
    pub async fn create(self: &Arc<Self>, new: NewSession) -> Result<String> {
        let NewSession {
            cwd,
            model,
            permission,
            effort,
            id,
            text,
            attachments,
        } = new;
        if !std::path::Path::new(&cwd).is_dir() {
            bail!("{cwd} isn't a folder");
        }
        let session = new_id();
        self.append(
            &session,
            Body::Created {
                agent: "claude".into(),
                cwd,
                model,
                permission,
                // Started with files alone: named after the first.
                title: match attachments.first() {
                    Some(f) if text.trim().is_empty() => std::path::Path::new(f)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| title(&text)),
                    _ => title(&text),
                },
            },
            None,
        )?;
        if let Some(e) = effort.filter(|e| !e.is_empty()) {
            self.append(
                &session,
                Body::Settings {
                    model: None,
                    permission: None,
                    title: None,
                    effort: Some(e),
                },
                None,
            )?;
        }
        let row = self
            .store()
            .session(&session)?
            .context("session row missing")?;
        session::start(self.clone(), row, Vec::new());
        self.command(
            &session,
            Op::Send {
                id,
                text,
                steer: false,
                attachments,
            },
        )
        .await?;
        Ok(session)
    }

    /// A new session that carries on from `parent`: its history up to just
    /// before message `before` (or all of it) is copied over, and its agent
    /// resumes that conversation as a new agent session.
    pub async fn fork(self: &Arc<Self>, parent: &str, before: Option<&str>) -> Result<String> {
        self.fork_with(parent, before, None).await
    }

    /// Make side question `id` and its answer into a new session: the
    /// conversation so far, then the question and answer, carrying on from
    /// the forked agent conversation that answered it.
    pub async fn promote_aside(self: &Arc<Self>, parent: &str, id: &str) -> Result<String> {
        let events = self.store().session_events(parent)?;
        let question = events
            .iter()
            .find_map(|e| match &e.body {
                Body::Aside { id: a, question } if a == id => Some(question.clone()),
                _ => None,
            })
            .context("no such side question")?;
        let (answer, agent) = events
            .iter()
            .find_map(|e| match &e.body {
                Body::AsideReply {
                    id: a,
                    text,
                    error: false,
                    agent_session: Some(agent),
                } if a == id => Some((text.clone(), agent.clone())),
                _ => None,
            })
            .context("it hasn't been answered yet")?;
        self.fork_with(
            parent,
            None,
            Some((id.to_string(), question, answer, agent)),
        )
        .await
    }

    /// A fork, optionally ending with a side question's (id, question,
    /// answer, agent conversation).
    async fn fork_with(
        self: &Arc<Self>,
        parent: &str,
        before: Option<&str>,
        aside: Option<(String, String, String, String)>,
    ) -> Result<String> {
        let row = self.store().session(parent)?.context("no such session")?;
        let events = self.store().session_events(parent)?;
        let cut = match before {
            Some(b) => events
                .iter()
                .position(|e| matches!(&e.body, Body::User { id, .. } if id == b))
                .context("no such message")?,
            None => events.len(),
        };
        let kept = &events[..cut];
        let title = events
            .iter()
            .rev()
            .find_map(|e| match &e.body {
                Body::Settings { title: Some(t), .. } => Some(t.clone()),
                Body::Created { title, .. } => Some(title.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let session = new_id();
        self.append(
            &session,
            Body::Created {
                agent: "claude".into(),
                cwd: row.cwd.clone(),
                model: row.model.clone(),
                permission: row.permission,
                title: format!("Fork of {title}"),
            },
            None,
        )?;
        for e in kept {
            let copy = match &e.body {
                // What the conversation said, not the old session's state.
                Body::User { queued: true, .. }
                | Body::Created { .. }
                | Body::Status { .. }
                | Body::Closed
                | Body::Reopened
                | Body::Agent { .. }
                | Body::Checkpoint { .. }
                | Body::Delivered { .. }
                | Body::Unqueued { .. }
                | Body::Requeued { .. }
                | Body::Background { .. }
                | Body::Limits { .. }
                | Body::Aside { .. }
                | Body::AsideReply { .. }
                | Body::Forked { .. } => continue,
                Body::Asked { id, .. } => {
                    // Only questions that were answered.
                    if !kept
                        .iter()
                        .any(|x| matches!(&x.body, Body::Answered { id: a, .. } if a == id))
                    {
                        continue;
                    }
                    e.body.clone()
                }
                Body::Settings { title: Some(_), .. } => continue,
                b => b.clone(),
            };
            self.append(&session, copy, None)?;
        }
        let at = kept.iter().rev().find_map(|e| match &e.body {
            Body::Assistant {
                parent: None,
                uuid: Some(u),
                ..
            } => Some(u.clone()),
            _ => None,
        });
        if let Some((id, question, answer, agent)) = aside {
            // The side conversation already holds the question and answer:
            // carry it on as it is.
            for b in [
                Body::User {
                    id,
                    text: question,
                    queued: false,
                    steer: false,
                    attachments: Vec::new(),
                },
                Body::TurnStarted,
                Body::Assistant {
                    parent: None,
                    blocks: vec![dent_protocol::Block::Text { text: answer }],
                    uuid: None,
                },
                Body::TurnEnded {
                    outcome: dent_protocol::Outcome::Done,
                },
                Body::Agent {
                    agent_session: Some(agent),
                    version: None,
                },
            ] {
                self.append(&session, b, None)?;
            }
        } else {
            if let Some(agent) = &row.agent_session {
                self.append(
                    &session,
                    Body::Agent {
                        agent_session: Some(agent.clone()),
                        version: None,
                    },
                    None,
                )?;
            }
            self.append(
                &session,
                Body::Forked {
                    from: parent.to_string(),
                    upto: at,
                },
                None,
            )?;
        }
        self.append(
            &session,
            Body::Notice {
                text: format!("Forked from {title}"),
            },
            None,
        )?;
        self.append(
            &session,
            Body::Status {
                status: dent_protocol::Status::Done,
            },
            None,
        )?;
        let row = self
            .store()
            .session(&session)?
            .context("session row missing")?;
        session::start(self.clone(), row, Vec::new());
        Ok(session)
    }

    /// Ask a side question about session `id`'s conversation. It runs in the
    /// background; the answer lands in the session's log.
    pub async fn aside(self: &Arc<Self>, session: &str, id: &str, question: &str) -> Result<()> {
        let row = self.store().session(session)?.context("no such session")?;
        let events = self.store().session_events(session)?;
        if events
            .iter()
            .any(|e| matches!(&e.body, Body::Aside { id: a, .. } if a == id))
        {
            return Ok(());
        }
        let started = events.iter().any(|e| matches!(e.body, Body::TurnStarted));
        let Some(agent) = row.agent_session.clone().filter(|_| started) else {
            bail!("ask something in the conversation first");
        };
        self.append(
            session,
            Body::Aside {
                id: id.into(),
                question: question.into(),
            },
            None,
        )?;
        let env = self.env();
        let argv = crate::claude::aside_argv(
            &env.claude,
            &env.path,
            row.model.as_deref(),
            &agent,
            question,
        );
        let (me, session, id, cwd) = (self.clone(), session.to_string(), id.to_string(), row.cwd);
        tokio::spawn(async move {
            let got = tokio::task::spawn_blocking(move || run_aside(&argv, &cwd))
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            let body = match got {
                Ok((text, agent_session)) => Body::AsideReply {
                    id,
                    text,
                    error: false,
                    agent_session,
                },
                Err(why) => Body::AsideReply {
                    id,
                    text: why,
                    error: true,
                    agent_session: None,
                },
            };
            let _ = me.append(&session, body, None);
        });
        Ok(())
    }

    /// Carry on a session started in a terminal: its history comes in, and
    /// its agent resumes it.
    pub async fn adopt(self: &Arc<Self>, agent_session: &str) -> Result<String> {
        if self
            .store()
            .sessions()?
            .iter()
            .any(|r| r.agent_session.as_deref() == Some(agent_session))
        {
            bail!("that session is already in dent");
        }
        if crate::terminal::is_open(agent_session) {
            bail!("it's still open in a terminal; close it there first, so it doesn't run twice");
        }
        let session = new_id();
        let id = agent_session.to_string();
        let media = self.paths.state.join("attachments").join(&session);
        let crate::terminal::Imported { cwd, title, bodies } =
            tokio::task::spawn_blocking(move || {
                crate::terminal::import(&crate::terminal::find(&id)?, &media)
            })
            .await?
            .context("couldn't read that session")?;
        self.append(
            &session,
            Body::Created {
                agent: "claude".into(),
                cwd,
                model: None,
                permission: Permission::Ask,
                title,
            },
            None,
        )?;
        self.append(
            &session,
            Body::Agent {
                agent_session: Some(agent_session.to_string()),
                version: None,
            },
            None,
        )?;
        let ran = bodies.iter().any(|b| matches!(b, Body::TurnStarted));
        for b in bodies {
            self.append(&session, b, None)?;
        }
        // It has run before, so the agent resumes rather than starts.
        if !ran {
            self.append(&session, Body::TurnStarted, None)?;
            self.append(
                &session,
                Body::TurnEnded {
                    outcome: dent_protocol::Outcome::Done,
                },
                None,
            )?;
        }
        self.append(
            &session,
            Body::Notice {
                text: "Picked up from the terminal".into(),
            },
            None,
        )?;
        self.append(
            &session,
            Body::Status {
                status: dent_protocol::Status::Done,
            },
            None,
        )?;
        let row = self
            .store()
            .session(&session)?
            .context("session row missing")?;
        session::start(self.clone(), row, Vec::new());
        Ok(session)
    }

    /// Bring back every session after a start: reattach running agents and
    /// mark turns that died with the old daemon.
    pub async fn restore(self: &Arc<Self>) -> Result<()> {
        let rows = self.store().sessions()?;
        let live = if rows.iter().any(|r| r.proc.is_some()) {
            match self.holder().await {
                Ok(h) => h.list().await.unwrap_or_default(),
                Err(e) => {
                    eprintln!("dent-daemon: holder unavailable: {e:#}");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        for row in rows {
            session::start(self.clone(), row, live.clone());
        }
        Ok(())
    }
}

/// What a new session starts with.
pub struct NewSession {
    pub cwd: String,
    pub model: Option<String>,
    pub permission: Permission,
    pub effort: Option<String>,
    /// The first message's id and text, and the files that go with it.
    pub id: String,
    pub text: String,
    pub attachments: Vec<String>,
}

/// A short title from the first message.
pub fn title(text: &str) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("New session");
    let mut t: String = line.trim().chars().take(60).collect();
    if line.trim().chars().count() > 60 {
        t.push('…');
    }
    t
}

/// Session ids: time then randomness, letters and digits only (process
/// names append `-n`).
/// Run a side question to the end: its answer and the agent conversation
/// that holds it. Gives up after three minutes.
fn run_aside(argv: &[String], cwd: &str) -> std::result::Result<(String, Option<String>), String> {
    use std::io::BufRead;
    let mut child = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start Claude Code: {e}"))?;
    // Stopped after three minutes, unless it has finished (and been waited
    // for, so its pid can't belong to anything else yet) by then.
    let pid = child.id();
    let (done, finished) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if finished
            .recv_timeout(std::time::Duration::from_secs(180))
            .is_err_and(|e| e == std::sync::mpsc::RecvTimeoutError::Timeout)
        {
            // SAFETY: kill has no memory preconditions; the child is unreaped.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
    });
    let out = child.stdout.take().ok_or("no output")?;
    let (mut texts, mut agent, mut failed) = (Vec::new(), None, None);
    for line in std::io::BufReader::new(out).lines().map_while(|l| l.ok()) {
        match crate::claude::wire::parse(line.as_bytes()) {
            Ok(crate::claude::wire::Inbound::Init { session_id, .. }) => agent = Some(session_id),
            Ok(crate::claude::wire::Inbound::Assistant {
                parent: None,
                blocks,
                synthetic: false,
                ..
            }) => {
                for b in blocks {
                    if let crate::claude::wire::AssistantBlock::Text(t) = b
                        && !t.trim().is_empty()
                    {
                        texts.push(t);
                    }
                }
            }
            Ok(crate::claude::wire::Inbound::Result {
                is_error: true,
                text,
                ..
            }) => failed = Some(text.unwrap_or_else(|| "The side question failed".into())),
            _ => {}
        }
    }
    // The watchdog must not fire once the child is reaped.
    let _ = done.send(());
    let status = child.wait().map_err(|e| e.to_string())?;
    if let Some(why) = failed {
        return Err(why);
    }
    if texts.is_empty() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut e, &mut err);
        }
        let last = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        return Err(if last.is_empty() {
            format!("Claude Code gave no answer ({status})")
        } else {
            format!("Claude Code gave no answer: {last}")
        });
    }
    Ok((texts.join("\n\n"), agent))
}

pub fn new_id() -> String {
    let mut rnd = [0u8; 6];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut rnd);
    }
    let r = rnd.iter().fold(0u64, |a, b| (a << 8) | u64::from(*b));
    format!("s{:x}{:012x}", now_ms(), r)
}

/// A random UUID (version 4).
pub fn uuid() -> String {
    let mut b = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut b);
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// A binary installed next to this one.
pub fn sibling(name: &str) -> Result<std::path::PathBuf> {
    let me = std::env::current_exe()?;
    let p = me.with_file_name(name);
    if !p.exists() {
        bail!("{} not found next to {}", name, me.display());
    }
    Ok(p)
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this computer".into())
}
