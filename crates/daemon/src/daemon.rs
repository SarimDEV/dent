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
        /// Counts every change to the replies in progress, so a client
        /// that took a snapshot ([`Daemon::partials`]) skips the deltas
        /// already in it.
        n: u64,
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
    /// Unique to this start of the daemon.
    pub boot: String,
    store: Mutex<Store>,
    pub live: broadcast::Sender<Arc<Live>>,
    sessions: Mutex<HashMap<String, mpsc::UnboundedSender<Cmd>>>,
    holder: tokio::sync::Mutex<Option<Arc<Holder>>>,
    /// Each session's reply so far, for clients that subscribe mid-stream.
    partials: Mutex<Partials>,
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
    awake: Mutex<Awake>,
    /// Sessions whose new worktree is being set up.
    setting_up: Mutex<std::collections::HashSet<String>>,
}

/// Keeping the machine awake while agents work: the sessions working now,
/// whether it's wanted, and the `caffeinate` holding it.
struct Awake {
    busy: std::collections::HashSet<String>,
    on: bool,
    hold: Option<std::process::Child>,
}

/// The replies streaming now, and how many changes they've had.
#[derive(Default)]
struct Partials {
    text: HashMap<String, String>,
    n: u64,
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where `made_once` keeps its claims in the store's key-value table.
const MADE: &str = "made.";

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
            boot: new_id(),
            store: Mutex::new(store),
            live,
            sessions: Mutex::new(HashMap::new()),
            holder: tokio::sync::Mutex::new(None),
            partials: Mutex::new(Partials::default()),
            models: Mutex::new(models),
            commands: Mutex::new(HashMap::new()),
            catalogs: Mutex::new(HashMap::new()),
            activities: Mutex::new(HashMap::new()),
            usage: tokio::sync::Mutex::new(None),
            awake: Mutex::new(Awake {
                busy: Default::default(),
                on: true,
                hold: None,
            }),
            setting_up: Mutex::new(Default::default()),
        })
    }

    /// A session started or stopped working (a turn, or background work).
    pub fn set_busy(&self, session: &str, busy: bool) {
        let mut a = lock(&self.awake);
        if busy {
            a.busy.insert(session.to_string());
        } else {
            a.busy.remove(session);
        }
        hold_awake(&mut a);
    }

    /// Whether to keep the machine awake while agents work.
    pub fn keep_awake(&self, on: bool) {
        let mut a = lock(&self.awake);
        a.on = on;
        hold_awake(&mut a);
    }

    pub fn store(&self) -> MutexGuard<'_, Store> {
        lock(&self.store)
    }

    /// Write an event and tell every client. The log stays locked until
    /// the event is on the live feed, so the feed carries events in `seq`
    /// order: a subscriber skips anything at or below the last seq it
    /// sent, and an event overtaken by a later one would be lost to it.
    pub fn append(&self, session: &str, body: Body, src: Option<Source<'_>>) -> Result<Event> {
        let mut store = self.store();
        let ev = store.append(session, now_ms(), &body, src)?;
        if matches!(ev.body, Body::Assistant { parent: None, .. }) {
            let mut p = lock(&self.partials);
            if p.text.remove(session).is_some() {
                p.n += 1;
            }
        }
        let _ = self.live.send(Arc::new(Live::Event(ev.clone())));
        drop(store);
        Ok(ev)
    }

    pub fn delta(&self, session: &str, text: &str, reset: bool) {
        // Changed and sent under one lock, so a snapshot's count says
        // exactly which deltas it holds.
        let mut p = lock(&self.partials);
        p.n += 1;
        let n = p.n;
        let s = p.text.entry(session.into()).or_default();
        if reset {
            s.clear();
        }
        s.push_str(text);
        let _ = self.live.send(Arc::new(Live::Delta {
            session: session.into(),
            text: text.into(),
            reset,
            n,
        }));
    }

    /// A reply was replaced by its final message, or cut off: clients drop
    /// what streamed so far.
    pub fn clear_partial(&self, session: &str) {
        let mut p = lock(&self.partials);
        if p.text.remove(session).is_some() {
            p.n += 1;
            let n = p.n;
            let _ = self.live.send(Arc::new(Live::Delta {
                session: session.into(),
                text: String::new(),
                reset: true,
                n,
            }));
        }
    }

    /// What has streamed of the reply in progress.
    pub fn partial(&self, session: &str) -> Option<String> {
        lock(&self.partials)
            .text
            .get(session)
            .filter(|t| !t.trim().is_empty())
            .cloned()
    }

    /// Every reply in progress, and the count of the last delta they
    /// hold: deltas numbered up to it are already in the snapshot.
    pub fn partials(&self) -> (Vec<(String, String)>, u64) {
        let p = lock(&self.partials);
        let text = p
            .text
            .iter()
            .filter(|(_, t)| !t.is_empty())
            .map(|(s, t)| (s.clone(), t.clone()))
            .collect();
        (text, p.n)
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

    /// The session request `key` already made, or a new id for it,
    /// claimed in the same step so a resent request (after a reconnect, or
    /// a restart) never makes a second session.
    fn made_once(&self, key: Option<&str>) -> Result<(String, bool)> {
        let Some(key) = key.filter(|k| !k.is_empty()) else {
            return Ok((new_id(), false));
        };
        let key = format!("{MADE}{key}");
        let store = self.store();
        if let Some(v) = store.get(&key)?
            && let s = v.split_whitespace().next().unwrap_or_default().to_string()
            && store.session(&s)?.is_some()
        {
            return Ok((s, true));
        }
        let session = new_id();
        // When, so a claim no resend can still need is let go (`restore`).
        store.put(&key, &format!("{session} {}", now_ms()))?;
        Ok((session, false))
    }

    /// Claims of `made_once` older than a day go: a request is only ever
    /// resent minutes after it was made, and otherwise there'd be one row
    /// for every session ever made. One kept from before claims were dated
    /// is let go too.
    fn forget_old_claims(&self) {
        let store = self.store();
        let Ok(rows) = store.with_prefix(MADE) else {
            return;
        };
        let day_ago = now_ms().saturating_sub(24 * 60 * 60 * 1000);
        for (k, v) in rows {
            let at = v
                .split_whitespace()
                .nth(1)
                .and_then(|t| t.parse::<u64>().ok())
                .unwrap_or(0);
            if at < day_ago {
                let _ = store.del(&k);
            }
        }
    }

    /// Start a session and send its first message.
    pub async fn create(self: &Arc<Self>, new: NewSession) -> Result<String> {
        let NewSession {
            cwd,
            worktree,
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
        let key = (!id.is_empty()).then(|| format!("create.{id}"));
        let (session, made) = self.made_once(key.as_deref())?;
        if made {
            // Sent again: the session's there; its first message goes if
            // it hadn't, and is passed over if it had.
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
            return Ok(session);
        }
        // A new worktree is made before the session exists, so one that
        // can't be made leaves nothing behind.
        let made = match worktree {
            Some(w) => {
                let (dir, under) = (cwd.clone(), crate::workspace::worktrees_root());
                let made = tokio::task::spawn_blocking(move || {
                    crate::workspace::make(std::path::Path::new(&dir), w.base.as_deref(), &under)
                })
                .await?
                .map_err(|e| anyhow::anyhow!("couldn't make a worktree: {e}"))?;
                Some(made)
            }
            None => None,
        };
        let cwd = match &made {
            Some((path, ..)) => path.display().to_string(),
            None => cwd,
        };
        self.append(
            &session,
            Body::Created {
                agent: "claude".into(),
                cwd: cwd.clone(),
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
        self.note_workspace(&session, &cwd).await;
        // A setup script runs first: meanwhile messages wait in the
        // session's queue, shown, and go in order once it's done.
        let setup = made
            .as_ref()
            .and_then(|(path, _, root)| crate::workspace::setup_script(path, root));
        if setup.is_some() {
            lock(&self.setting_up).insert(session.clone());
        }
        let row = self
            .store()
            .session(&session)?
            .context("session row missing")?;
        session::start(self.clone(), row, Vec::new());
        let first = Op::Send {
            id,
            text: text.clone(),
            steer: false,
            attachments,
        };
        let Some((path, branch, root)) = made else {
            self.command(&session, first).await?;
            return Ok(session);
        };
        // The branch gets a name from the task while the worktree readies.
        {
            let (d, session, path) = (self.clone(), session.clone(), path.clone());
            tokio::spawn(async move {
                if let Err(e) = d.name_branch(&session, &path, &branch, &text).await {
                    eprintln!("dent-daemon: {session}: couldn't name its branch: {e:#}");
                }
            });
        }
        self.command(&session, first).await?;
        // The queue goes when setup's done, whatever came of it, so a
        // failed install never strands the task.
        if let Some(script) = setup {
            let (d, session) = (self.clone(), session.clone());
            tokio::spawn(async move {
                d.set_up(&session, &script, &path, &root).await;
                lock(&d.setting_up).remove(&session);
                if let Err(e) = d.command(&session, Op::SetUp).await {
                    eprintln!("dent-daemon: {session}: couldn't carry on after setup: {e:#}");
                }
            });
        }
        Ok(session)
    }

    /// Session `id`'s new worktree is being set up: what's sent waits.
    pub fn setting_up(&self, id: &str) -> bool {
        lock(&self.setting_up).contains(id)
    }

    /// Run a repository's setup script in a session's new worktree, saying
    /// so while it runs and in the chat when it's done.
    async fn set_up(
        self: &Arc<Self>,
        session: &str,
        script: &std::path::Path,
        worktree: &std::path::Path,
        root: &std::path::Path,
    ) {
        let started = now_ms();
        self.set_activity(
            session,
            Some(dent_protocol::Activity::SettingUp {
                since: started,
                what: format!("Running {}", crate::workspace::SETUP),
            }),
        );
        let (s, w, r) = (
            script.to_path_buf(),
            worktree.to_path_buf(),
            root.to_path_buf(),
        );
        let got = tokio::task::spawn_blocking(move || crate::workspace::run_setup(&s, &w, &r))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        self.set_activity(session, None);
        let secs = now_ms().saturating_sub(started).div_ceil(1000);
        let text = match got {
            Ok(_) => format!(
                "Set up the worktree with {} in {secs}s",
                crate::workspace::SETUP
            ),
            Err(tail) => match tail.lines().last() {
                // Its last words say most; the chat line is plain text.
                Some(last) => format!(
                    "{} failed in {secs}s: {}",
                    crate::workspace::SETUP,
                    last.trim()
                ),
                None => format!("{} failed in {secs}s", crate::workspace::SETUP),
            },
        };
        if let Err(e) = self.append(session, Body::Notice { text }, None) {
            eprintln!("dent-daemon: {session}: {e:#}");
        }
    }

    /// Name a new worktree's placeholder branch after the task.
    async fn name_branch(
        self: &Arc<Self>,
        session: &str,
        worktree: &std::path::Path,
        placeholder: &str,
        task: &str,
    ) -> Result<()> {
        let task: String = task.chars().take(2000).collect();
        let answer = self
            .ask(&format!(
                "Name a Git branch for this task given to a coding agent: two to four \
                 lowercase words joined by hyphens, like fix-footer-on-mobile. Reply with the \
                 name only.\n\nTask: {task}"
            ))
            .await?;
        let Some(wanted) = crate::workspace::branch_name(&answer) else {
            return Ok(());
        };
        let (w, old) = (worktree.to_path_buf(), placeholder.to_string());
        tokio::task::spawn_blocking(move || crate::workspace::rename(&w, &old, &wanted))
            .await?
            .map_err(|e| anyhow::anyhow!(e))?;
        self.note_workspace(session, &worktree.display().to_string())
            .await;
        Ok(())
    }

    /// Say the session's branch, and its main checkout when it's in a
    /// worktree, if either changed since last said.
    pub async fn note_workspace(self: &Arc<Self>, session: &str, cwd: &str) {
        let dir = std::path::PathBuf::from(cwd);
        if !dir.is_dir() {
            return;
        }
        let Ok((branch, root)) = tokio::task::spawn_blocking(move || {
            (
                crate::workspace::branch(&dir),
                crate::workspace::main_checkout(&dir).map(|p| p.display().to_string()),
            )
        })
        .await
        else {
            return;
        };
        let key = format!("{WORKSPACE}{session}");
        let now = serde_json::json!({ "branch": branch, "root": root }).to_string();
        let said = self.store().get(&key).ok().flatten();
        if said.as_deref() == Some(now.as_str()) {
            return;
        }
        // Outside a repository, there's nothing to say until there is.
        if said.is_none() && branch.is_none() && root.is_none() {
            return;
        }
        if let Err(e) = self
            .append(session, Body::Workspace { branch, root }, None)
            .and_then(|_| self.store().put(&key, &now))
        {
            eprintln!("dent-daemon: {session}: {e:#}");
        }
    }

    /// Say the session's pull request if its state, checks or review
    /// changed since last said. Only a pushed branch is asked about.
    pub async fn note_pr(self: &Arc<Self>, session: &str, cwd: &str) {
        let Some(gh) = crate::ship::gh(&self.env().path) else {
            return;
        };
        let dir = std::path::PathBuf::from(cwd);
        if !dir.is_dir() {
            return;
        }
        let key = format!("{PR}{session}");
        let said = self.store().get(&key).ok().flatten();
        let Ok(pr) = tokio::task::spawn_blocking(move || {
            let pushed =
                crate::workspace::run(&dir, &["rev-parse", "--abbrev-ref", "-q", "@{u}"]).is_ok();
            pushed
                .then(|| crate::ship::pull_request(&dir, &gh))
                .flatten()
        })
        .await
        else {
            return;
        };
        // Never had one: nothing to say until there is.
        if said.is_none() && pr.is_none() {
            return;
        }
        let now = serde_json::to_string(&pr).unwrap_or_default();
        if said.as_deref() == Some(now.as_str()) {
            return;
        }
        if let Err(e) = self
            .append(session, Body::PullRequest { pr }, None)
            .and_then(|_| self.store().put(&key, &now))
        {
            eprintln!("dent-daemon: {session}: {e:#}");
        }
    }

    /// Every couple of minutes, look again at the pull requests of the
    /// sessions that are open, so checks, reviews and merges show.
    pub fn watch_prs(self: &Arc<Self>) {
        let d = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(pr_every()).await;
                let open = d.store().open_sessions().unwrap_or_default();
                for (id, cwd) in open {
                    d.note_pr(&id, &cwd).await;
                }
            }
        });
    }

    /// The branch and main checkout last said for a session.
    pub fn workspace_of(&self, session: &str) -> (Option<String>, Option<String>) {
        let v: serde_json::Value = self
            .store()
            .get(&format!("{WORKSPACE}{session}"))
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let s = |k: &str| v[k].as_str().map(str::to_owned);
        (s("branch"), s("root"))
    }

    /// One question to a small, quick model, run in dent's own folder so
    /// it leaves nothing in a project's history: its answer.
    pub async fn ask(self: &Arc<Self>, prompt: &str) -> Result<String> {
        let env = self.env();
        let argv = crate::claude::ask_argv(&env.claude, &env.path, prompt);
        let dir = self.paths.state.join("titles");
        std::fs::create_dir_all(&dir)?;
        let dir = dir.display().to_string();
        let got = tokio::task::spawn_blocking(move || run_aside(&argv, &dir))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(got.0)
    }

    /// Commit, push, open a pull request or pull in a session's folder:
    /// what happened in a line, and the pull request's address.
    pub async fn git(
        self: &Arc<Self>,
        cwd: &str,
        action: dent_protocol::GitAction,
        message: Option<String>,
    ) -> Result<(String, Option<String>)> {
        use crate::ship;
        use dent_protocol::GitAction as A;
        let dir = std::path::PathBuf::from(cwd);
        let gh = ship::gh(&self.env().path);
        let blocking = |f: Box<dyn FnOnce() -> std::result::Result<String, String> + Send>| async {
            tokio::task::spawn_blocking(f)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .map_err(|e| anyhow::anyhow!(e))
        };
        let st = {
            let (dir, gh) = (dir.clone(), gh.clone());
            tokio::task::spawn_blocking(move || ship::status(&dir, gh.as_deref()))
                .await?
                .context("the session's folder isn't in a Git repository")?
        };
        let commits = matches!(action, A::Commit | A::CommitPush | A::CommitPushPr);
        let pushes = matches!(action, A::CommitPush | A::CommitPushPr | A::Push | A::Pr);
        let opens = matches!(action, A::CommitPushPr | A::Pr);
        if (pushes || action == A::Pull) && !st.remote {
            bail!("the repository has no remote called origin");
        }
        if opens && gh.is_none() {
            bail!("pull requests need the GitHub CLI (gh), signed in");
        }
        let mut done = Vec::new();
        if commits {
            if !st.changes {
                bail!("there's nothing to commit");
            }
            let message = match message.filter(|m| !m.trim().is_empty()) {
                Some(m) => m,
                None => {
                    let d = dir.clone();
                    let pending = blocking(Box::new(move || Ok(ship::pending(&d)))).await?;
                    let answer = self
                        .ask(&format!(
                            "Write a Git commit message for these changes: a subject line in \
                             the imperative, at most 60 characters, then, only if it helps, a \
                             blank line and a few short lines on why. No quotes or code fences. \
                             Reply with the message only.\n\n{pending}"
                        ))
                        .await
                        .unwrap_or_default();
                    ship::message(&answer).unwrap_or_else(|| "Update files".into())
                }
            };
            let (d, m) = (dir.clone(), message.clone());
            blocking(Box::new(move || {
                ship::commit(&d, &m).map(|_| String::new())
            }))
            .await?;
            done.push(format!(
                "Committed “{}”",
                message.lines().next().unwrap_or_default()
            ));
        }
        if pushes && (commits || st.ahead > 0 || !st.upstream) {
            let (d, up) = (dir.clone(), st.upstream);
            blocking(Box::new(move || ship::push(&d, up).map(|_| String::new()))).await?;
            done.push("pushed".into());
        }
        let mut url = None;
        if opens {
            if let Some(pr) = st.pr.as_ref().filter(|p| p.open()) {
                url = Some(pr.url.clone());
                done.push(format!("pull request #{} is open", pr.number));
            } else {
                let base = format!("origin/{}", st.default_branch.as_deref().unwrap_or("main"));
                let d = dir.clone();
                let since = blocking(Box::new(move || Ok(ship::since(&d, &base)))).await?;
                let answer = self
                    .ask(&format!(
                        "Write a GitHub pull request for this branch: a title on the first line \
                         (at most 70 characters, no prefix), then a blank line and a short body \
                         in Markdown saying what changed and why. Reply with those only.\n\n{since}"
                    ))
                    .await
                    .unwrap_or_default();
                let (mut title, body) = ship::title_and_body(&answer);
                if title.is_empty() {
                    title = st.branch.clone().unwrap_or_else(|| "Changes".into());
                }
                let (d, gh) = (dir.clone(), gh.clone().context("no gh")?);
                let got = blocking(Box::new(move || ship::open(&d, &gh, &title, &body))).await?;
                done.push("opened a pull request".into());
                url = (!got.is_empty()).then_some(got);
            }
        }
        if action == A::Pull {
            let d = dir.clone();
            blocking(Box::new(move || ship::pull(&d).map(|_| String::new()))).await?;
            done.push("Pulled".into());
        }
        let mut summary = done.join(", ");
        if let Some(c) = summary.get(..1) {
            summary = c.to_uppercase() + &summary[1..];
        }
        Ok((summary, url))
    }

    /// A new session that carries on from `parent`: its history up to just
    /// before message `before` (or all of it) is copied over, and its agent
    /// resumes that conversation as a new agent session.
    /// `id` is the client's for the request: sent again, it gets the same
    /// fork.
    pub async fn fork(
        self: &Arc<Self>,
        parent: &str,
        before: Option<&str>,
        id: Option<&str>,
    ) -> Result<String> {
        let key = id.map(|i| format!("fork.{i}"));
        self.fork_with(parent, before, None, key.as_deref()).await
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
        // One session per side question, however often it's asked for.
        let key = format!("aside.{parent}.{id}");
        self.fork_with(
            parent,
            None,
            Some((id.to_string(), question, answer, agent)),
            Some(&key),
        )
        .await
    }

    /// A fork, optionally ending with a side question's (id, question,
    /// answer, agent conversation).
    /// A new session carrying on from an earlier version of the
    /// conversation: everything before event `until`, the rewind that left
    /// that version behind.
    pub async fn fork_until(
        self: &Arc<Self>,
        parent: &str,
        until: u64,
        id: Option<&str>,
    ) -> Result<String> {
        let key = id.map(|i| format!("fork.{i}"));
        self.fork_cut(parent, Cut::Until(until), None, key.as_deref())
            .await
    }

    async fn fork_with(
        self: &Arc<Self>,
        parent: &str,
        before: Option<&str>,
        aside: Option<(String, String, String, String)>,
        key: Option<&str>,
    ) -> Result<String> {
        let cut = match before {
            Some(b) => Cut::Before(b.to_string()),
            None => Cut::All,
        };
        self.fork_cut(parent, cut, aside, key).await
    }

    async fn fork_cut(
        self: &Arc<Self>,
        parent: &str,
        cut: Cut,
        aside: Option<(String, String, String, String)>,
        key: Option<&str>,
    ) -> Result<String> {
        let row = self.store().session(parent)?.context("no such session")?;
        let events = self.store().session_events(parent)?;
        let cut = match cut {
            Cut::Before(b) => events
                .iter()
                .position(|e| matches!(&e.body, Body::User { id, .. } if *id == b))
                .context("no such message")?,
            Cut::Until(seq) => events
                .iter()
                .position(|e| e.seq >= seq)
                .context("no such version")?,
            Cut::All => events.len(),
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
        let (session, made) = self.made_once(key)?;
        if made {
            return Ok(session);
        }
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
        // A terminal session keeps the title it came with.
        self.store().put(&titled_key(&session), "1")?;
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

    /// Title a session from its conversation, with a small model and no
    /// tools, run in dent's own folder so it leaves nothing in the project's
    /// history. Without `force`, only once and never after you've named it.
    pub async fn title_session(self: &Arc<Self>, session: &str, force: bool) -> Result<()> {
        let key = titled_key(session);
        if !force && self.store().get(&key)?.is_some() {
            return Ok(());
        }
        self.store().put(&key, "1")?;
        let events = self.store().session_events(session)?;
        let clip = |t: &str, n: usize| -> String { t.chars().take(n).collect() };
        let mut said = Vec::new();
        for e in &events {
            match &e.body {
                Body::User { text, .. } => said.push(format!("Person: {}", clip(text, 1500))),
                Body::Assistant {
                    parent: None,
                    blocks,
                    ..
                } => {
                    for b in blocks {
                        if let dent_protocol::Block::Text { text } = b {
                            said.push(format!("Agent: {}", clip(text, 1500)));
                        }
                    }
                }
                _ => {}
            }
        }
        if said.is_empty() {
            return Ok(());
        }
        // The start says what it set out to do; the end, where it went.
        let keep = if said.len() > 8 {
            let mut k = said[..2].to_vec();
            k.push("…".into());
            k.extend_from_slice(&said[said.len() - 6..]);
            k
        } else {
            said
        };
        let answer = self
            .ask(&crate::claude::title_prompt(&keep.join("\n\n")))
            .await?;
        let t = clean_title(&answer);
        if !t.is_empty() {
            self.append(
                session,
                Body::Settings {
                    effort: None,
                    model: None,
                    permission: None,
                    title: Some(t),
                },
                None,
            )?;
        }
        Ok(())
    }

    /// Bring back every session after a start: reattach running agents and
    /// mark turns that died with the old daemon.
    pub async fn restore(self: &Arc<Self>) -> Result<()> {
        self.forget_old_claims();
        let rows = self.store().sessions()?;
        // Sessions from before dent kept track of what you'd seen start out
        // seen, so an upgrade doesn't mark them all as new.
        if self.store().get(SEEN_MARKS)?.is_none() {
            let upto = self.store().last_seq()?;
            for row in &rows {
                self.append(&row.id, Body::Seen { upto }, None)?;
            }
            self.store().put(SEEN_MARKS, "1")?;
            // Their titles stand: dent doesn't retitle them on its own.
            for row in &rows {
                self.store().put(&titled_key(&row.id), "1")?;
            }
        }
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
        // Sessions whose branch was never said (older than branches being
        // noted) get it now, in the background.
        let unsaid: Vec<(String, String)> = rows
            .iter()
            .filter(|r| {
                self.store()
                    .get(&format!("{WORKSPACE}{}", r.id))
                    .ok()
                    .flatten()
                    .is_none()
            })
            .map(|r| (r.id.clone(), r.cwd.clone()))
            .collect();
        if !unsaid.is_empty() {
            let d = self.clone();
            tokio::spawn(async move {
                for (id, cwd) in unsaid {
                    d.note_workspace(&id, &cwd).await;
                }
            });
        }
        for row in rows {
            session::start(self.clone(), row, live.clone());
        }
        Ok(())
    }
}

/// While anything works (and it's wanted), the machine stays awake: idle
/// sleep, and on power a closed lid, as `caffeinate -is` does. The hold
/// ends with the daemon (`-w`), and as soon as nothing works.
fn hold_awake(a: &mut Awake) {
    let want = a.on && !a.busy.is_empty();
    if let Some(c) = a.hold.as_mut()
        && c.try_wait().ok().flatten().is_some()
    {
        a.hold = None;
    }
    match (want, a.hold.is_some()) {
        (true, false) if cfg!(target_os = "macos") => {
            a.hold = std::process::Command::new("/usr/bin/caffeinate")
                .args(["-i", "-s", "-w", &std::process::id().to_string()])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok();
        }
        (false, true) => {
            if let Some(mut c) = a.hold.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        _ => {}
    }
}

/// Where a fork stops copying the conversation.
enum Cut {
    /// Before your message with this id.
    Before(String),
    /// Before the event with this sequence number.
    Until(u64),
    All,
}

/// Set once a session has been titled (by dent or by you), so a session is
/// titled on its own only once.
pub fn titled_key(session: &str) -> String {
    format!("titled:{session}")
}

/// A model's answer as a title: its first line, without quotes, a heading
/// mark, a "Title:" label or a closing full stop, at most 60 characters.
fn clean_title(answer: &str) -> String {
    let line = answer.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut t = line.trim().trim_start_matches('#').trim();
    for label in ["Title:", "title:"] {
        t = t.strip_prefix(label).unwrap_or(t).trim();
    }
    let t = t
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '“' | '”'))
        .trim_end_matches('.')
        .trim();
    // A title with a subtitle keeps the title.
    let t = match t.split_once([':', '—']) {
        Some((head, _)) if head.split_whitespace().count() >= 2 => head.trim(),
        _ => t,
    };
    // Past eight words it's a sentence, not a title.
    let words: Vec<&str> = t.split_whitespace().collect();
    title(&words[..words.len().min(8)].join(" "))
}

/// Each session's branch and main checkout as last said, as JSON.
const WORKSPACE: &str = "workspace.";
/// Each session's pull request as last said, as JSON.
const PR: &str = "pr.";
/// How often open sessions' pull requests are looked at again: two
/// minutes, or `DENT_PR_EVERY` seconds (the walkthroughs').
fn pr_every() -> std::time::Duration {
    let secs = std::env::var("DENT_PR_EVERY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);
    std::time::Duration::from_secs(secs)
}

/// Set once every session older than seen marks has one.
const SEEN_MARKS: &str = "seen-marks";

/// What a new session starts with.
pub struct NewSession {
    pub cwd: String,
    /// Work in a new worktree of `cwd`'s repository.
    pub worktree: Option<dent_protocol::NewWorktree>,
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
    if !std::path::Path::new(cwd).is_dir() {
        return Err(format!("The session's folder {cwd} no longer exists"));
    }
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

#[cfg(test)]
mod tests {
    #[test]
    fn a_models_answer_becomes_a_title() {
        assert_eq!(super::clean_title("\"Fix the footer.\""), "Fix the footer");
        assert_eq!(
            super::clean_title("Title: Cache with Redis\nmore"),
            "Cache with Redis"
        );
        assert_eq!(super::clean_title("# Landing page"), "Landing page");
        assert_eq!(
            super::clean_title("Checking the hero on mobile: the screenshot and more"),
            "Checking the hero on mobile"
        );
    }
}
