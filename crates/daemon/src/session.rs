//! One task per session. It owns the session's agent process (through the
//! holder), turns the agent's output into events, and runs the person's
//! commands. Everything it knows can be rebuilt from the log, so a
//! restarted daemon carries on where the last one stopped.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use dent_protocol::{
    Act, Activity, Answer, Body, CommandOutcome, Out, Outcome, Permission, Section, Status, Task,
};
use dent_session_holder::wire::Proc as HeldProc;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::claude::{self, wire, wire::Inbound};
use crate::daemon::Daemon;
use crate::store::{Row, Source};

/// An agent with nothing to do is stopped after this long, and resumed from
/// its transcript on the next message.
fn idle() -> Duration {
    std::env::var("DENT_IDLE_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(300))
}

/// After asking the agent to stop, how long before stopping its process.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// A new agent process that hasn't answered in this long is stuck starting.
fn start_limit() -> Duration {
    std::env::var("DENT_START_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(60))
}

/// What Claude Code says when the conversation it was asked to resume has
/// no transcript (it was stopped before writing one, or the file is gone).
const FORGOTTEN: &str = "No conversation found with session ID";

pub struct Cmd {
    pub op: Op,
    pub reply: Option<oneshot::Sender<Result<(), String>>>,
}

impl Cmd {
    pub fn new(op: Op) -> Self {
        Self { op, reply: None }
    }
}

pub enum Op {
    Send {
        id: String,
        text: String,
        steer: bool,
        attachments: Vec<String>,
    },
    Unqueue {
        id: String,
    },
    Requeue {
        id: String,
        before: Option<String>,
    },
    Rewind {
        to: String,
        files: bool,
    },
    Interrupt,
    Answer {
        id: String,
        answer: Answer,
    },
    SetModel(String),
    SetPermission(Permission),
    /// Do what a command's sheet offers.
    Act {
        command: String,
        act: Act,
    },
    SetEffort(String),
    Rename(String),
    Close,
    Reopen,
    Output {
        proc: String,
        err: bool,
        off: u64,
        bytes: Vec<u8>,
    },
    Exit {
        proc: String,
        code: Option<i32>,
        signal: Option<i32>,
    },
    HolderLost,
}

struct Proc {
    id: String,
    /// Output not yet split into lines, starting at byte `off`.
    buf: Vec<u8>,
    off: u64,
    /// The end of stderr, for saying why an agent died.
    err: Vec<u8>,
    err_off: u64,
    /// We asked it to stop, so its exit isn't a crash.
    stopping: bool,
    /// It was started to carry on an earlier conversation.
    resumed: bool,
}

struct Pending {
    tool: String,
    input: Value,
    suggestions: Value,
    /// A question from Claude Code itself (`request_user_dialog`), shown
    /// as an agent question: each option's label and the answer it gives.
    dialog: Option<Vec<(String, String)>>,
}

#[derive(Clone)]
enum Ctl {
    Initialize,
    Interrupt,
    /// How full the context is, by the agent's own count.
    ContextUsage,
    /// What's in effect (`get_settings`), to say which choice is current.
    Settings,
    /// A command's sheet, to answer it with.
    Sheet {
        command: String,
        args: String,
        text: String,
        outcome: CommandOutcome,
    },
    /// A command's sheet again, after something on it changed.
    Refresh {
        command: String,
    },
    /// Something a sheet offered.
    Act {
        command: String,
        act: Act,
    },
    Other,
}

struct Session {
    d: Arc<Daemon>,
    id: String,
    cwd: String,
    model: Option<String>,
    effort: Option<String>,
    /// A setting that only takes effect in a new agent process changed
    /// while a turn ran: start a new one once it ends.
    respawn: bool,
    permission: Permission,
    agent_session: Option<String>,
    status: Status,
    /// How the last turn ended, when nothing is running.
    settled: Status,
    in_turn: bool,
    /// The agent has run at least once, so its transcript exists to resume.
    agent_started: bool,
    /// After a rewind, the next agent process picks up the conversation at
    /// this assistant message (or starts afresh with `Some(None)`).
    resume_at: Option<Option<String>>,
    /// Messages waiting for the turn to end.
    held: VecDeque<(String, String, Vec<String>)>,
    /// A message was steered into the running turn; the abort the agent
    /// reports for it isn't the turn ending.
    steered: bool,
    /// The agent replied in this turn. A slash command the agent runs
    /// itself (`/context`, `/usage`) replies only in the turn's result.
    said: bool,
    /// The call the agent is writing out: its name, and its input so far
    /// until its target shows (`None` after).
    preparing: Option<(String, Option<String>)>,
    background: Vec<Task>,
    asked: HashMap<String, Pending>,
    /// Client message ids already logged, so a resend never runs twice.
    seen: HashSet<String>,
    proc: Option<Proc>,
    controls: HashMap<String, Ctl>,
    next_ctl: u64,
    context: Option<u64>,
    max_context: Option<u64>,
    idle_at: Option<Instant>,
    stop_at: Option<Instant>,
    /// When a starting agent must have answered by.
    start_by: Option<Instant>,
    /// Replies owed to the agent, written once its output is handled.
    outbox: Vec<Value>,
    /// The last message sent, to send again if the agent has to start over.
    last: Option<(String, String, Vec<String>)>,
    /// The agent couldn't find the conversation it was asked to resume.
    forgot: bool,
    /// Each command's sheet as clients last got it, so a refresh that
    /// changes nothing says nothing.
    sheets: HashMap<String, Vec<Section>>,
    /// A sign-in finishing in the browser: refresh this command's sheet
    /// now and then until it's done or the time's up.
    watch: Option<Watch>,
}

struct Watch {
    command: String,
    next: Instant,
    until: Instant,
}

/// Start a session's task from its row. `held` is what the holder is
/// running right now, for reattaching after a restart.
pub fn start(d: Arc<Daemon>, row: Row, held: Vec<HeldProc>) {
    let (tx, rx) = mpsc::unbounded_channel();
    d.register(&row.id, tx);
    let mut s = Session {
        d,
        id: row.id.clone(),
        cwd: row.cwd.clone(),
        model: row.model.clone(),
        effort: None,
        respawn: false,
        permission: row.permission,
        agent_session: row.agent_session.clone(),
        status: row.status,
        settled: row.status,
        in_turn: false,
        agent_started: false,
        resume_at: None,
        held: VecDeque::new(),
        steered: false,
        said: false,
        preparing: None,
        background: Vec::new(),
        asked: HashMap::new(),
        seen: HashSet::new(),
        proc: None,
        controls: HashMap::new(),
        next_ctl: 0,
        context: None,
        max_context: None,
        idle_at: None,
        stop_at: None,
        start_by: None,
        outbox: Vec::new(),
        last: None,
        forgot: false,
        sheets: HashMap::new(),
        watch: None,
    };
    s.rebuild();
    // A side question this daemon never saw finish won't be answered now.
    if let Ok(events) = s.d.store().session_events(&s.id) {
        let mut open: Vec<String> = Vec::new();
        for e in events {
            match e.body {
                Body::Aside { id, .. } => open.push(id),
                Body::AsideReply { id, .. } => open.retain(|a| *a != id),
                _ => {}
            }
        }
        for id in open {
            s.log(Body::AsideReply {
                id,
                text: "dent restarted before it was answered. Ask again.".into(),
                error: true,
                agent_session: None,
            });
        }
    }
    tokio::spawn(async move {
        s.reattach(&row, &held).await;
        s.run(rx).await;
    });
}

impl Session {
    /// Recover what's in memory from the log.
    fn rebuild(&mut self) {
        let Ok(events) = self.d.store().session_events(&self.id) else {
            return;
        };
        let mut rewound_to: Option<String> = None;
        let mut forked_at: Option<Option<String>> = None;
        for e in events.iter().cloned() {
            match e.body {
                Body::User {
                    id,
                    text,
                    queued,
                    attachments,
                    ..
                } => {
                    if queued {
                        self.held.push_back((id.clone(), text, attachments));
                    }
                    self.seen.insert(id);
                }
                Body::Delivered { id } | Body::Unqueued { id } => {
                    self.held.retain(|(h, _, _)| *h != id);
                }
                Body::Requeued { id, before } => {
                    requeue(&mut self.held, &id, before.as_deref());
                }
                Body::Asked {
                    id,
                    tool,
                    input,
                    suggestions,
                    ..
                } => {
                    let input = serde_json::from_str(&input).unwrap_or(Value::Null);
                    self.asked.insert(
                        id,
                        Pending {
                            tool,
                            dialog: claude::dialog_values(&input),
                            input,
                            // Unknown in older logs: "always" then allows
                            // once rather than more than was offered.
                            suggestions: suggestions
                                .and_then(|s| serde_json::from_str(&s).ok())
                                .unwrap_or(Value::Null),
                        },
                    );
                }
                Body::Answered { id, .. } => {
                    self.asked.remove(&id);
                }
                Body::Assistant { parent: None, .. } => self.said = true,
                Body::TurnStarted => {
                    self.in_turn = true;
                    self.said = false;
                    self.agent_started = true;
                    rewound_to = None;
                    forked_at = None;
                }
                Body::Rewound { to, .. } => {
                    rewound_to = Some(to);
                    self.in_turn = false;
                }
                Body::Forked { upto, .. } => {
                    forked_at = Some(upto);
                    self.in_turn = false;
                }
                Body::TurnEnded { outcome } => {
                    self.in_turn = false;
                    self.settled = settled(&outcome);
                }
                Body::Background { tasks } => self.background = tasks,
                Body::Settings {
                    effort: Some(e), ..
                } => self.effort = (!e.is_empty()).then_some(e),
                Body::Context { used, max, .. } => {
                    self.context = Some(used);
                    self.max_context = max.or(self.max_context);
                }
                _ => {}
            }
        }
        if matches!(self.status, Status::Working | Status::NeedsYou) {
            self.in_turn = true;
        }
        // A fork the agent hasn't picked up yet: it starts from the copy.
        if let Some(at) = forked_at {
            self.resume_at = Some(at);
        }
        // A rewind the agent hasn't picked up yet still applies.
        if let Some(to) = rewound_to
            && let Some(at) = events
                .iter()
                .position(|e| matches!(&e.body, Body::User { id, .. } if *id == to))
        {
            self.resume_at = Some(resume_point(&events[..at]));
        }
    }

    async fn reattach(&mut self, row: &Row, held: &[HeldProc]) {
        let Some(proc) = row.proc.clone() else {
            // No agent, so nothing of its runs in the background.
            self.background_gone();
            return;
        };
        let alive = held.iter().find(|p| p.id == proc);
        let attached = match alive {
            Some(p) => match self.d.holder().await {
                Ok(h) => h.attach(&proc, row.proc_off, p.err).await.is_ok(),
                Err(_) => false,
            },
            None => false,
        };
        if attached {
            self.proc = Some(Proc {
                id: proc,
                buf: Vec::new(),
                off: row.proc_off,
                err: Vec::new(),
                err_off: alive.map(|p| p.err).unwrap_or(0),
                stopping: false,
                resumed: true,
            });
            if !self.in_turn {
                self.idle_at = Some(Instant::now() + idle());
            }
        } else {
            let _ = self.d.store().clear_proc(&self.id);
            self.background_gone();
            if self.in_turn {
                self.end_turn(Outcome::Interrupted {
                    reason: Some("dent restarted while the agent was working".into()),
                });
            }
        }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        loop {
            let deadline = [
                self.idle_at,
                self.stop_at,
                self.start_by,
                self.watch.as_ref().map(|w| w.next),
            ]
            .into_iter()
            .flatten()
            .min();
            let cmd = match deadline {
                Some(at) => tokio::select! {
                    c = rx.recv() => c,
                    _ = tokio::time::sleep_until(at) => { self.timers().await; continue }
                },
                None => rx.recv().await,
            };
            let Some(Cmd { op, reply }) = cmd else { return };
            let res = self.op(op).await.map_err(|e| format!("{e:#}"));
            if let Some(r) = reply {
                let _ = r.send(res);
            }
            self.update_status();
        }
    }

    async fn op(&mut self, op: Op) -> Result<()> {
        match op {
            Op::Send {
                id,
                text,
                steer,
                attachments,
            } => self.send(id, text, steer, attachments).await,
            Op::Rewind { to, files } => self.rewind(to, files).await,
            Op::Unqueue { id } => {
                let before = self.held.len();
                self.held.retain(|(h, _, _)| *h != id);
                if self.held.len() == before {
                    bail!("that message has already gone to the agent");
                }
                self.log(Body::Unqueued { id });
                Ok(())
            }
            Op::Requeue { id, before } => {
                if !requeue(&mut self.held, &id, before.as_deref()) {
                    bail!("that message has already gone to the agent");
                }
                self.log(Body::Requeued { id, before });
                Ok(())
            }
            Op::Interrupt => self.interrupt().await,
            Op::Answer { id, answer } => self.answer(id, answer).await,
            Op::SetModel(model) => {
                if self.model.as_deref() == Some(model.as_str()) {
                    return Ok(());
                }
                if self.proc.is_some() {
                    self.control("set_model", json!({ "model": model }), Ctl::Other)
                        .await?;
                }
                self.model = Some(model.clone());
                self.log(Body::Settings {
                    effort: None,
                    model: Some(model),
                    permission: None,
                    title: None,
                });
                Ok(())
            }
            Op::SetPermission(p) => {
                if p == self.permission {
                    return Ok(());
                }
                if self.proc.is_some() {
                    let mode = claude::mode_name(p);
                    self.control("set_permission_mode", json!({ "mode": mode }), Ctl::Other)
                        .await?;
                }
                self.permission = p;
                self.log(Body::Settings {
                    effort: None,
                    model: None,
                    permission: Some(p),
                    title: None,
                });
                Ok(())
            }
            Op::Act { command, act } => {
                let Some((subtype, extra)) = claude::sheets::request(&act) else {
                    bail!("that's for the client to do");
                };
                self.ensure_proc().await?;
                self.control(subtype, extra, Ctl::Act { command, act })
                    .await
            }
            Op::SetEffort(effort) => {
                // The same effort again mustn't restart the agent.
                if self.effort.as_deref().unwrap_or("") == effort {
                    return Ok(());
                }
                self.effort = (!effort.is_empty()).then(|| effort.clone());
                self.log(Body::Settings {
                    effort: Some(effort),
                    model: None,
                    permission: None,
                    title: None,
                });
                // Effort is fixed when the agent starts: start a fresh process
                // that resumes the same conversation, now, or with the next
                // message once this turn and any background work are done.
                if self.in_turn || !self.background.is_empty() {
                    self.respawn = true;
                } else {
                    self.retire_proc().await;
                }
                Ok(())
            }
            Op::Rename(title) => {
                self.log(Body::Settings {
                    effort: None,
                    model: None,
                    permission: None,
                    title: Some(title),
                });
                Ok(())
            }
            Op::Close => {
                if !self.asked.is_empty() {
                    bail!("it's waiting for an answer");
                }
                self.log(Body::Closed);
                Ok(())
            }
            Op::Reopen => {
                self.log(Body::Reopened);
                Ok(())
            }
            Op::Output {
                proc,
                err,
                off,
                bytes,
            } => {
                self.output(&proc, err, off, &bytes);
                for reply in std::mem::take(&mut self.outbox) {
                    let _ = self.write(&reply).await;
                }
                if std::mem::take(&mut self.forgot) {
                    self.start_over().await;
                }
                if !self.in_turn && self.asked.is_empty() && !self.held.is_empty() {
                    self.deliver_held().await;
                }
                Ok(())
            }
            Op::Exit { proc, code, signal } => {
                self.exited(&proc, code, signal).await;
                if !self.in_turn && !self.held.is_empty() {
                    self.deliver_held().await;
                }
                Ok(())
            }
            Op::HolderLost => {
                if let Some(p) = self.proc.take() {
                    let _ = self.d.store().clear_proc(&self.id);
                    self.background_gone();
                    if self.in_turn {
                        self.end_turn(Outcome::Interrupted {
                            reason: Some("the process holding the agent stopped".into()),
                        });
                    }
                    drop(p);
                }
                Ok(())
            }
        }
    }

    fn log(&self, body: Body) {
        if let Err(e) = self.d.append(&self.id, body, None) {
            eprintln!("dent-daemon: {}: can't write event: {e:#}", self.id);
        }
    }

    fn log_from(&self, body: Body, off: u64) {
        let proc = self.proc.as_ref().map(|p| p.id.clone()).unwrap_or_default();
        let src = Source { proc: &proc, off };
        if let Err(e) = self.d.append(&self.id, body, Some(src)) {
            eprintln!("dent-daemon: {}: can't write event: {e:#}", self.id);
        }
    }

    fn update_status(&mut self) {
        let s = if !self.asked.is_empty() {
            Status::NeedsYou
        } else if self.in_turn {
            Status::Working
        } else if !self.background.is_empty() {
            Status::Background
        } else {
            self.settled
        };
        if s != self.status {
            self.status = s;
            self.log(Body::Status { status: s });
        }
    }

    async fn send(
        &mut self,
        id: String,
        text: String,
        steer: bool,
        attachments: Vec<String>,
    ) -> Result<()> {
        if !self.seen.insert(id.clone()) {
            return Ok(());
        }
        let working = self.in_turn && self.proc.is_some();
        let queued = working && !steer;
        self.log(Body::User {
            id: id.clone(),
            text: text.clone(),
            queued,
            steer: working && steer,
            attachments: attachments.clone(),
        });
        if queued {
            self.held.push_back((id, text, attachments));
            return Ok(());
        }
        if working {
            self.steered = true;
        }
        self.deliver(&id, &text, working, &attachments).await
    }

    /// Write a message to the agent, starting it if it isn't running.
    async fn deliver(
        &mut self,
        id: &str,
        text: &str,
        steer: bool,
        attachments: &[String],
    ) -> Result<()> {
        self.checkpoint(id).await;
        self.last = (!steer).then(|| (id.to_string(), text.to_string(), attachments.to_vec()));
        self.hand_over(text, steer, attachments).await
    }

    /// Write a message to the agent process, starting one if needed.
    async fn hand_over(&mut self, text: &str, steer: bool, attachments: &[String]) -> Result<()> {
        // A setting that needs a new process changed: this message starts
        // one, unless background work is still running in the old one.
        if self.respawn && !steer && self.background.is_empty() {
            self.retire_proc().await;
        }
        if let Err(e) = self.ensure_proc().await {
            self.log(Body::Notice {
                text: format!("Couldn't start Claude Code: {e:#}"),
            });
            self.end_turn(Outcome::Error {
                message: format!("{e:#}"),
            });
            return Err(e);
        }
        self.write(&wire::user_message(
            text,
            &crate::daemon::uuid(),
            steer,
            attachments,
        ))
        .await?;
        self.in_turn = true;
        self.said = false;
        self.idle_at = None;
        Ok(())
    }

    /// The agent had no record of the conversation it was resuming: carry
    /// on in a new one rather than failing every message from now on.
    async fn start_over(&mut self) {
        self.retire_proc().await;
        self.agent_started = false;
        self.resume_at = Some(None);
        self.log(Body::Notice {
            text: "Claude Code no longer had this conversation's history, so it started a new \
                   one. It won't remember what was said before."
                .into(),
        });
        match self.last.clone() {
            Some((_, text, attachments)) => {
                let _ = self.hand_over(&text, false, &attachments).await;
            }
            None => self.end_turn(Outcome::Interrupted { reason: None }),
        }
    }

    /// After a turn, the next held message goes.
    async fn deliver_held(&mut self) {
        let Some((id, text, attachments)) = self.held.pop_front() else {
            return;
        };
        self.log(Body::Delivered { id: id.clone() });
        let _ = self.deliver(&id, &text, false, &attachments).await;
        self.update_status();
    }

    /// Snapshot the working tree before message `id` goes, so the session
    /// can go back to it.
    async fn checkpoint(&mut self, id: &str) {
        let (cwd, name) = (self.cwd.clone(), format!("{}/{}", self.id, safe_ref(id)));
        let commit = tokio::task::spawn_blocking(move || {
            crate::git::checkpoint(std::path::Path::new(&cwd), &name)
        })
        .await
        .ok()
        .flatten();
        if let Some(commit) = commit {
            self.log(Body::Checkpoint {
                id: id.to_string(),
                commit,
            });
        }
    }

    /// Go back to just before message `to`: the conversation, and with
    /// `files` the working tree too.
    async fn rewind(&mut self, to: String, files: bool) -> Result<()> {
        if self.in_turn {
            bail!("stop the current turn first");
        }
        let events = self.d.store().session_events(&self.id)?;
        let Some(at) = events
            .iter()
            .position(|e| matches!(&e.body, Body::User { id, .. } if *id == to))
        else {
            bail!("no such message");
        };
        if files {
            let commit = events.iter().find_map(|e| match &e.body {
                Body::Checkpoint { id, commit } if *id == to => Some(commit.clone()),
                _ => None,
            });
            let Some(commit) = commit else {
                bail!(
                    "there's no snapshot of the files from then (is the folder a git repository?)"
                );
            };
            let cwd = self.cwd.clone();
            tokio::task::spawn_blocking(move || {
                crate::git::restore(std::path::Path::new(&cwd), &commit)
            })
            .await?
            .map_err(|e| anyhow::anyhow!(e))?;
        }
        self.resume_at = Some(resume_point(&events[..at]));
        // The running process holds the whole conversation; the next message
        // starts one that picks up at the rewind.
        self.retire_proc().await;
        self.idle_at = None;
        for (id, _, _) in std::mem::take(&mut self.held) {
            self.log(Body::Unqueued { id });
        }
        self.settled = Status::Done;
        self.log(Body::Rewound { to, files });
        Ok(())
    }

    /// Let go of the agent process; the next message starts a new one.
    async fn retire_proc(&mut self) {
        self.respawn = false;
        if let Some(p) = self.proc.take() {
            let _ = self.d.store().clear_proc(&self.id);
            self.background_gone();
            if let Ok(h) = self.d.holder().await {
                let _ = h.stop(&p.id, Duration::from_secs(1)).await;
            }
        }
    }

    /// The agent process has gone, and the work it ran in the background
    /// with it.
    fn background_gone(&mut self) {
        if self.background.is_empty() {
            return;
        }
        self.background.clear();
        self.log(Body::Background { tasks: Vec::new() });
        self.log(Body::Notice {
            text: "Background work stopped with the agent.".into(),
        });
    }

    async fn ensure_proc(&mut self) -> Result<()> {
        if self.proc.is_some() {
            return Ok(());
        }
        if !std::path::Path::new(&self.cwd).is_dir() {
            bail!("the session's folder {} no longer exists", self.cwd);
        }
        let h = self.d.holder().await?;
        // dent names the agent's session up front, so it can be resumed
        // even if the first process dies before saying anything.
        let agent_session = match self.agent_session.clone() {
            Some(a) => a,
            None => {
                let a = crate::daemon::uuid();
                self.agent_session = Some(a.clone());
                self.log(Body::Agent {
                    agent_session: Some(a.clone()),
                    version: None,
                });
                a
            }
        };
        let start = match self.resume_at.take() {
            Some(Some(at)) => claude::Start::ResumeAt {
                id: agent_session,
                at,
            },
            // Rewound to before anything was said: a new conversation.
            Some(None) => {
                let a = crate::daemon::uuid();
                self.agent_session = Some(a.clone());
                self.log(Body::Agent {
                    agent_session: Some(a.clone()),
                    version: None,
                });
                claude::Start::Fresh(a)
            }
            None if self.agent_started => claude::Start::Resume(agent_session),
            None => claude::Start::Fresh(agent_session),
        };
        let resumed = !matches!(start, claude::Start::Fresh(_));
        let proc = self.d.store().next_proc(&self.id)?;
        let env = self.d.env();
        // Each new agent also refreshes the PATH for the one after it.
        self.d.refresh_env();
        let argv = claude::argv(
            &env.claude,
            &env.path,
            self.model.as_deref(),
            self.permission,
            &start,
            self.effort.as_deref(),
        );
        h.spawn(&proc, argv, Some(self.cwd.clone().into()), Vec::new())
            .await?;
        h.attach(&proc, 0, 0).await?;
        self.proc = Some(Proc {
            id: proc,
            buf: Vec::new(),
            off: 0,
            err: Vec::new(),
            err_off: 0,
            stopping: false,
            resumed,
        });
        self.start_by = Some(Instant::now() + start_limit());
        self.control(
            "initialize",
            json!({ "supportedDialogKinds": claude::DIALOGS }),
            Ctl::Initialize,
        )
        .await?;
        Ok(())
    }

    async fn write(&self, v: &Value) -> Result<()> {
        let Some(p) = &self.proc else {
            bail!("the agent isn't running")
        };
        let mut line = serde_json::to_vec(v)?;
        line.push(b'\n');
        self.d.holder().await?.write(&p.id, &line).await
    }

    /// A control request sent once the current output is handled, for
    /// places that can't wait on the write.
    fn control_later(&mut self, subtype: &str, kind: Ctl) {
        self.next_ctl += 1;
        let id = format!("dent-{}", self.next_ctl);
        self.controls.insert(id.clone(), kind);
        self.outbox
            .push(wire::control_request(&id, subtype, json!({})));
    }

    async fn control(&mut self, subtype: &str, extra: Value, kind: Ctl) -> Result<()> {
        self.next_ctl += 1;
        let id = format!("dent-{}", self.next_ctl);
        self.controls.insert(id.clone(), kind);
        self.write(&wire::control_request(&id, subtype, extra))
            .await
    }

    /// Stop always ends the turn: by asking, then by stopping the process,
    /// then by letting go of it.
    async fn interrupt(&mut self) -> Result<()> {
        if !self.in_turn {
            return Ok(());
        }
        // Stop means nothing more arrives: held messages come back.
        for (id, _, _) in std::mem::take(&mut self.held) {
            self.log(Body::Unqueued { id });
        }
        self.steered = false;
        if self.proc.is_none() {
            self.end_turn(Outcome::Interrupted { reason: None });
            return Ok(());
        }
        // The agent doesn't always end the turn after an interrupt, and one
        // that can't be reached gets its process stopped straight away.
        let asked = self.control("interrupt", json!({}), Ctl::Interrupt).await;
        let grace = if asked.is_ok() {
            STOP_GRACE
        } else {
            Duration::ZERO
        };
        self.stop_at = Some(Instant::now() + grace);
        Ok(())
    }

    async fn answer(&mut self, id: String, answer: Answer) -> Result<()> {
        let Some(p) = self.asked.get(&id) else {
            bail!("already answered");
        };
        let msg = match (&answer, &p.dialog) {
            (Answer::Choose { answers }, Some(values)) => {
                let picked = answers.first().map(String::as_str).unwrap_or_default();
                let value = values
                    .iter()
                    .find(|(label, _)| label == picked)
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("cancelled");
                wire::dialog_answer(&id, Some(value))
            }
            (Answer::Deny { .. }, Some(_)) => wire::dialog_answer(&id, None),
            (answer, _) => match answer {
                Answer::Allow { always, project } => {
                    wire::allow(&id, &p.tool, &p.input, *always, *project, &p.suggestions)
                }
                Answer::Deny { message } => wire::deny(&id, message),
                Answer::Choose { answers } => wire::allow(
                    &id,
                    &p.tool,
                    &claude::answers(&p.input, answers),
                    false,
                    false,
                    &Value::Null,
                ),
                Answer::Expired => bail!("can't answer with expired"),
            },
        };
        self.write(&msg).await?;
        self.asked.remove(&id);
        self.log(Body::Answered { id, answer });
        Ok(())
    }

    async fn timers(&mut self) {
        let now = Instant::now();
        if self.stop_at.is_some_and(|t| t <= now) {
            self.stop_at = None;
            if self.in_turn
                && let Some(p) = self.proc.as_mut()
            {
                if p.stopping {
                    // Stopped, and still no exit: let it go.
                    self.retire_proc().await;
                    self.end_turn(Outcome::Interrupted { reason: None });
                } else {
                    p.stopping = true;
                    let id = p.id.clone();
                    let stopped = match self.d.holder().await {
                        Ok(h) => h.stop(&id, Duration::from_secs(1)).await.is_ok(),
                        Err(_) => false,
                    };
                    if stopped {
                        self.stop_at = Some(now + Duration::from_secs(5));
                    } else {
                        self.retire_proc().await;
                        self.end_turn(Outcome::Interrupted { reason: None });
                    }
                }
            }
        }
        if self.start_by.is_some_and(|t| t <= now) {
            self.start_by = None;
            if let Some(p) = self.proc.as_ref() {
                let tail = String::from_utf8_lossy(&p.err).trim().to_string();
                let last = tail.lines().last().unwrap_or_default().to_string();
                self.retire_proc().await;
                let mut message = "Claude Code didn't start within a minute".to_string();
                if !last.is_empty() {
                    message = format!("{message}: {last}");
                }
                self.end_turn(Outcome::Error { message });
            }
        }
        if let Some(w) = self.watch.as_mut().filter(|w| w.next <= now) {
            w.next = now + Duration::from_secs(2);
            let command = w.command.clone();
            if w.until <= now {
                self.watch = None;
            }
            if let Some(source) = claude::sheets::source(&command)
                && self.proc.is_some()
            {
                let _ = self
                    .control(source, json!({}), Ctl::Refresh { command })
                    .await;
            }
        }
        if self.idle_at.is_some_and(|t| t <= now) {
            self.idle_at = None;
            let quiet = !self.in_turn && self.asked.is_empty() && self.background.is_empty();
            if quiet && let Some(p) = self.proc.as_mut() {
                p.stopping = true;
                let id = p.id.clone();
                if let Ok(h) = self.d.holder().await {
                    let _ = h.close_stdin(&id).await;
                }
            }
        }
        self.update_status();
    }

    fn output(&mut self, proc: &str, err: bool, off: u64, bytes: &[u8]) {
        let Some(p) = self.proc.as_mut().filter(|p| p.id == proc) else {
            return;
        };
        if err {
            let end = off + bytes.len() as u64;
            if end > p.err_off {
                let skip = p.err_off.saturating_sub(off) as usize;
                p.err.extend_from_slice(&bytes[skip.min(bytes.len())..]);
                let n = p.err.len().saturating_sub(4096);
                p.err.drain(..n);
                p.err_off = end;
            }
            return;
        }
        let have = p.off + p.buf.len() as u64;
        let end = off + bytes.len() as u64;
        if end <= have {
            return;
        }
        let skip = have.saturating_sub(off) as usize;
        p.buf.extend_from_slice(&bytes[skip..]);
        while let Some(p) = self.proc.as_mut()
            && let Some(i) = p.buf.iter().position(|b| *b == b'\n')
        {
            let line: Vec<u8> = p.buf.drain(..=i).collect();
            p.off += line.len() as u64;
            let end = p.off;
            match wire::parse(&line[..line.len() - 1]) {
                Ok(ev) => self.inbound(ev, end),
                Err(_) if line.iter().all(u8::is_ascii_whitespace) => {}
                Err(e) => eprintln!("dent-daemon: {}: unreadable agent line: {e}", self.id),
            }
        }
        self.update_status();
    }

    /// One line from the agent, which ended at byte `end` of its output.
    fn inbound(&mut self, ev: Inbound, end: u64) {
        match ev {
            Inbound::Init {
                session_id,
                version,
                permission_mode,
                ..
            } => {
                if self.agent_session.as_deref() != Some(session_id.as_str())
                    && !session_id.is_empty()
                {
                    self.agent_session = Some(session_id.clone());
                    self.log_from(
                        Body::Agent {
                            agent_session: Some(session_id),
                            version: Some(version),
                        },
                        end,
                    );
                }
                if let Some(p) = claude::permission_from(&permission_mode)
                    && p != self.permission
                {
                    self.permission = p;
                    self.log_from(
                        Body::Settings {
                            effort: None,
                            model: None,
                            permission: Some(p),
                            title: None,
                        },
                        end,
                    );
                }
                self.in_turn = true;
                self.agent_started = true;
                self.idle_at = None;
                self.start_by = None;
                self.log_from(Body::TurnStarted, end);
            }
            Inbound::MessageStart { parent: None } => {
                // A reply is coming: any retry worked.
                self.preparing = None;
                self.d.set_activity(&self.id, None);
                self.d.clear_partial(&self.id);
            }
            Inbound::TextDelta { parent: None, text } => self.d.delta(&self.id, &text, false),
            // Writing a long file streams nothing else for minutes: say so.
            Inbound::ToolStart { parent: None, name } => {
                self.d.set_activity(
                    &self.id,
                    Some(Activity::Preparing {
                        tool: name.clone(),
                        what: None,
                    }),
                );
                self.preparing = Some((name, Some(String::new())));
            }
            Inbound::InputDelta { parent: None, json } => {
                if let Some((tool, input)) = &mut self.preparing
                    && let Some(head) = input
                {
                    head.push_str(&json);
                    // The target comes first, if at all.
                    let what = claude::target(tool, head);
                    if what.is_some() || head.len() > 4096 {
                        *input = None;
                    }
                    if let Some(what) = what {
                        let tool = tool.clone();
                        self.d.set_activity(
                            &self.id,
                            Some(Activity::Preparing {
                                tool,
                                what: Some(what),
                            }),
                        );
                    }
                }
            }
            // The turn's result says the same thing, as an error.
            Inbound::Assistant {
                synthetic: true, ..
            } => {}
            Inbound::Assistant {
                parent,
                uuid,
                blocks,
                context,
                results,
                ..
            } => {
                if parent.is_none()
                    && let Some(c) = context
                {
                    self.context = Some(c);
                }
                let blocks = claude::blocks(blocks);
                if parent.is_none() {
                    self.said = true;
                    if self.preparing.take().is_some() {
                        self.d.set_activity(&self.id, None);
                    }
                }
                if !blocks.is_empty() {
                    self.log_from(
                        Body::Assistant {
                            parent: parent.clone(),
                            blocks,
                            uuid,
                        },
                        end,
                    );
                }
                // A server tool's result comes with its call.
                for r in results {
                    self.log_from(
                        Body::ToolResult {
                            tool: r.tool_use_id,
                            error: r.is_error,
                            output: r.output,
                            parent: parent.clone(),
                        },
                        end,
                    );
                }
            }
            Inbound::ToolResults { parent, results } => {
                for r in results {
                    self.log_from(
                        Body::ToolResult {
                            tool: r.tool_use_id,
                            error: r.is_error,
                            output: r.output,
                            parent: parent.clone(),
                        },
                        end,
                    );
                }
            }
            Inbound::RateLimit(info) => self.log_from(claude::limits(&info), end),
            Inbound::BackgroundTasks(tasks) => {
                self.background = claude::tasks(&tasks);
                self.log_from(
                    Body::Background {
                        tasks: self.background.clone(),
                    },
                    end,
                );
                // Nothing left to wait for: an agent that stays quiet now
                // is stopped in time, as after a turn.
                if self.background.is_empty() && !self.in_turn && self.proc.is_some() {
                    self.idle_at = Some(Instant::now() + idle());
                }
            }
            Inbound::CanUseTool {
                request_id,
                tool_name,
                input,
                description,
                suggestions,
            } => {
                self.log_from(
                    Body::Asked {
                        id: request_id.clone(),
                        tool: tool_name.clone(),
                        input: input.to_string(),
                        description,
                        keeps: wire::keeps(&suggestions),
                        suggestions: (!suggestions.is_null()).then(|| suggestions.to_string()),
                    },
                    end,
                );
                self.asked.insert(
                    request_id,
                    Pending {
                        tool: tool_name,
                        input,
                        suggestions,
                        dialog: None,
                    },
                );
            }
            Inbound::ControlResponse {
                request_id,
                ok,
                body,
            } => match (self.controls.remove(&request_id), ok) {
                (Some(Ctl::Initialize), _) if !ok => {
                    self.start_by = None;
                    self.log_from(
                        Body::Notice {
                            text: format!(
                                "Claude Code refused: {}",
                                body.as_str().unwrap_or("unknown request")
                            ),
                        },
                        end,
                    );
                }
                (Some(Ctl::Initialize), true) => {
                    self.start_by = None;
                    self.d.set_models(claude::models(&body));
                    self.d.set_commands(&self.id, claude::commands(&body));
                    let (styles, style) = claude::styles(&body);
                    self.d.catalog(&self.id, |c| {
                        c.styles = styles;
                        c.style = style;
                    });
                    self.control_later("get_settings", Ctl::Settings);
                    // A resumed conversation is as full as it was: say so
                    // before the first reply rather than after it.
                    if self.agent_started {
                        self.control_later("get_context_usage", Ctl::ContextUsage);
                    }
                }
                (Some(Ctl::ContextUsage), true) => {
                    if let Some(used) = body["totalTokens"].as_u64() {
                        self.context = Some(used);
                        self.max_context = body["maxTokens"].as_u64().or(self.max_context);
                        self.log_from(
                            Body::Context {
                                used,
                                max: self.max_context,
                                compacts_at: body["autoCompactThreshold"]
                                    .as_u64()
                                    .filter(|_| body["isAutoCompactEnabled"] != false),
                                parts: wire::parts(&body["categories"]),
                            },
                            end,
                        );
                    }
                }
                (Some(Ctl::Settings), true) => {
                    self.d.catalog(&self.id, |c| {
                        c.effective = body["effective"].clone();
                        c.applied = body["applied"].clone();
                    });
                }
                (
                    Some(Ctl::Sheet {
                        command,
                        args,
                        text,
                        outcome,
                    }),
                    ok,
                ) => {
                    let sections = ok
                        .then(|| claude::sheets::sheet(&command, &body, &self.d.models()))
                        .flatten();
                    let body = match sections {
                        Some(sections) => {
                            self.sheets.insert(command.clone(), sections.clone());
                            Body::Command {
                                text: match outcome {
                                    // Its own words, when it had some.
                                    CommandOutcome::Done if !text.trim().is_empty() => text,
                                    _ => claude::sheets::text(&sections),
                                },
                                name: command,
                                args,
                                outcome: CommandOutcome::Done,
                                view: Some(claude::sheets::view(sections)),
                            }
                        }
                        None => Body::Command {
                            name: command,
                            args,
                            text,
                            outcome,
                            view: None,
                        },
                    };
                    self.log_from(body, end);
                }
                (Some(Ctl::Refresh { command }), true) => {
                    if let Some(sections) = claude::sheets::sheet(&command, &body, &self.d.models())
                        && self.sheets.get(&command) != Some(&sections)
                    {
                        self.sheets.insert(command.clone(), sections.clone());
                        self.d.tell(Out::Sheet {
                            session: self.id.clone(),
                            command,
                            sections,
                        });
                    }
                }
                (Some(Ctl::Act { command, act }), ok) => {
                    let url = body["authUrl"]
                        .as_str()
                        .filter(|_| ok && body["requiresUserAction"] == true)
                        .map(str::to_string);
                    // Signing in finishes in the browser: watch for it.
                    if url.is_some() {
                        let now = Instant::now();
                        self.watch = Some(Watch {
                            command: command.clone(),
                            next: now + Duration::from_secs(2),
                            until: now + Duration::from_secs(180),
                        });
                    }
                    self.d.tell(Out::Acted {
                        session: self.id.clone(),
                        command: command.clone(),
                        act,
                        url,
                        error: (!ok).then(|| {
                            body.as_str()
                                .unwrap_or("Claude Code couldn't do that")
                                .to_string()
                        }),
                    });
                    if let Some(source) = claude::sheets::source(&command) {
                        self.control_later(source, Ctl::Refresh { command });
                    }
                }
                (
                    Some(Ctl::Interrupt | Ctl::ContextUsage | Ctl::Settings | Ctl::Refresh { .. }),
                    _,
                )
                | (Some(Ctl::Other), true)
                | (None, _) => {}
                (Some(_), false) => self.log_from(
                    Body::Notice {
                        text: format!(
                            "Claude Code refused: {}",
                            body.as_str().unwrap_or("unknown request")
                        ),
                    },
                    end,
                ),
            },
            Inbound::Request {
                request_id,
                subtype,
                server,
            } => {
                if subtype == "elicitation" {
                    let who = server.unwrap_or_else(|| "An MCP server".into());
                    self.log_from(
                        Body::Notice {
                            text: format!(
                                "{who} asked for input dent can't show yet, so it was declined"
                            ),
                        },
                        end,
                    );
                }
                self.outbox.push(wire::refuse(&request_id, &subtype));
            }
            Inbound::CancelRequest { request_id } => {
                if self.asked.remove(&request_id).is_some() {
                    self.log_from(
                        Body::Answered {
                            id: request_id,
                            answer: Answer::Expired,
                        },
                        end,
                    );
                }
            }
            Inbound::Result {
                is_error,
                text,
                terminal_reason,
            } => {
                let aborted = matches!(
                    terminal_reason.as_deref(),
                    Some("aborted_streaming" | "aborted_tools")
                );
                if is_error
                    && text.as_deref().is_some_and(|t| t.starts_with(FORGOTTEN))
                    && self.proc.as_ref().is_some_and(|p| p.resumed)
                {
                    self.forgot = true;
                    return;
                }
                if !self.in_turn {
                    // Results with no turn running are the agent's own
                    // bookkeeping (after an interrupt, say).
                    return;
                }
                if aborted && self.steered && self.stop_at.is_none() {
                    // A steered message cut the request short; the turn
                    // carries on with it.
                    self.steered = false;
                    return;
                }
                self.steered = false;
                let outcome = if aborted {
                    Outcome::Interrupted { reason: None }
                } else if is_error {
                    Outcome::Error {
                        message: text.clone().unwrap_or_else(|| "The turn failed".into()),
                    }
                } else {
                    Outcome::Done
                };
                if matches!(outcome, Outcome::Done)
                    && !self.said
                    && let Some(text) = text
                {
                    self.log_from(
                        Body::Assistant {
                            parent: None,
                            blocks: vec![dent_protocol::Block::Text { text }],
                            uuid: None,
                        },
                        end,
                    );
                }
                self.d.set_activity(&self.id, None);
                if let Some(used) = self.context {
                    self.log_from(
                        Body::Context {
                            used,
                            max: self.max_context,
                            compacts_at: None,
                            parts: Vec::new(),
                        },
                        end,
                    );
                }
                // The agent's own count, with its real window size; after
                // a command that changed the context (/compact, /clear)
                // it's the only one.
                self.control_later("get_context_usage", Ctl::ContextUsage);
                self.end_turn_at(outcome, Some(end));
            }
            Inbound::Compacted {
                auto,
                before,
                after,
            } => {
                // What was counted before is gone; the turn's end asks
                // for the new count.
                self.context = None;
                self.d.set_activity(&self.id, None);
                self.log_from(
                    Body::Compacted {
                        auto,
                        before,
                        after,
                    },
                    end,
                )
            }
            Inbound::Compacting { on: true, .. } => self.d.set_activity(
                &self.id,
                Some(Activity::Compacting {
                    since: crate::daemon::now_ms(),
                }),
            ),
            Inbound::Compacting { on: false, failed } => {
                self.d.set_activity(&self.id, None);
                if let Some(why) = failed {
                    self.log_from(
                        Body::Notice {
                            text: format!("Couldn't compact: {why}"),
                        },
                        end,
                    );
                }
            }
            Inbound::Retrying {
                attempt,
                max,
                delay_ms,
                error,
            } => self.d.set_activity(
                &self.id,
                Some(Activity::Retrying {
                    attempt,
                    max,
                    at: crate::daemon::now_ms() + delay_ms,
                    error,
                }),
            ),
            Inbound::CommandsChanged(v) => {
                self.d.commands_changed(&self.id, claude::commands(&v));
            }
            Inbound::LocalCommand {
                name,
                args,
                text,
                outcome,
                view,
            } => {
                // A refusal doesn't say which command; it's the one sent.
                let sent = self
                    .last
                    .as_ref()
                    .and_then(|(_, t, _)| t.strip_prefix('/'))
                    .map(|t| {
                        let (n, a) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
                        (n.to_string(), a.trim().to_string())
                    });
                let (name, args) = match (name, sent) {
                    (Some(n), _) => (n, args),
                    (None, Some((n, a))) => (n, a),
                    (None, None) => (String::new(), args),
                };
                self.said = true;
                // A command whose answer is a screen gets its data as a
                // sheet, even where the agent refused to draw it.
                let sheet = claude::sheets::source(&name).filter(|_| {
                    args.is_empty() && !matches!(outcome, CommandOutcome::Unknown { .. })
                });
                match sheet {
                    Some(source) => self.control_later(
                        source,
                        Ctl::Sheet {
                            command: name,
                            args,
                            text,
                            outcome,
                        },
                    ),
                    None => self.log_from(
                        Body::Command {
                            name,
                            args,
                            text,
                            outcome,
                            view,
                        },
                        end,
                    ),
                }
                // It may have changed a setting a command offers.
                self.control_later("get_settings", Ctl::Settings);
            }
            Inbound::Notice(text) => self.log_from(Body::Notice { text }, end),
            Inbound::Dialog {
                request_id,
                kind,
                payload,
            } => {
                // A kind dent didn't declare is never answered: Claude Code
                // takes its default.
                if let Some((input, values)) = claude::dialog(&kind, &payload) {
                    self.log_from(
                        Body::Asked {
                            id: request_id.clone(),
                            tool: "AskUserQuestion".into(),
                            input: input.to_string(),
                            description: None,
                            keeps: false,
                            suggestions: None,
                        },
                        end,
                    );
                    self.asked.insert(
                        request_id,
                        Pending {
                            tool: "AskUserQuestion".into(),
                            input,
                            suggestions: Value::Null,
                            dialog: Some(values),
                        },
                    );
                }
            }
            Inbound::MessageStart { .. }
            | Inbound::TextDelta { .. }
            | Inbound::ToolStart { .. }
            | Inbound::InputDelta { .. }
            | Inbound::Other => {}
        }
    }

    fn end_turn(&mut self, outcome: Outcome) {
        self.end_turn_at(outcome, None);
    }

    fn end_turn_at(&mut self, outcome: Outcome, end: Option<u64>) {
        if self.preparing.take().is_some() {
            self.d.set_activity(&self.id, None);
        }
        self.stop_at = None;
        self.start_by = None;
        // Approvals still open belonged to the turn that just ended.
        for id in std::mem::take(&mut self.asked).into_keys() {
            self.log(Body::Answered {
                id,
                answer: Answer::Expired,
            });
        }
        // Stopped mid-reply: what you'd already read stays, as the reply,
        // rather than vanishing. A final message, if one came, has already
        // taken its place.
        if matches!(outcome, Outcome::Interrupted { .. })
            && let Some(text) = self.d.partial(&self.id)
        {
            // Logged while the stream still stands, then the stream goes
            // (below): clients never show the words twice or not at all.
            self.log(Body::Assistant {
                parent: None,
                blocks: vec![dent_protocol::Block::Text { text }],
                uuid: None,
            });
            // The event took the stream's place; clients drop it too.
            self.d.delta(&self.id, "", true);
        }
        self.d.clear_partial(&self.id);
        self.settled = settled(&outcome);
        let was = self.in_turn;
        self.in_turn = false;
        let body = Body::TurnEnded { outcome };
        match end {
            Some(off) => self.log_from(body, off),
            None if was || self.status != self.settled => self.log(body),
            None => {}
        }
        if self.proc.is_some() {
            self.idle_at = Some(Instant::now() + idle());
        }
        self.update_status();
    }

    async fn exited(&mut self, proc: &str, code: Option<i32>, signal: Option<i32>) {
        let Some(p) = self.proc.take_if(|p| p.id == proc) else {
            // A process left behind by a rewind: let the holder forget it.
            if let Ok(h) = self.d.holder().await {
                let _ = h.release(proc).await;
            }
            return;
        };
        let _ = self.d.store().clear_proc(&self.id);
        self.background_gone();
        if let Ok(h) = self.d.holder().await {
            let _ = h.release(proc).await;
        }
        self.idle_at = None;
        self.start_by = None;
        if p.stopping && !self.in_turn {
            return;
        }
        let tail = String::from_utf8_lossy(&p.err).trim().to_string();
        if self.in_turn && p.resumed && !p.stopping && tail.contains(FORGOTTEN) {
            self.start_over().await;
            return;
        }
        let why = match (code, signal) {
            (_, Some(s)) => format!("signal {s}"),
            (Some(c), _) => format!("exit code {c}"),
            _ => "no exit status".into(),
        };
        let last = tail.lines().last().unwrap_or_default().to_string();
        if code == Some(127) {
            // `env` couldn't find it: maybe installed since dent started.
            self.d.refresh_env();
        }
        if self.in_turn {
            let reason = if p.stopping {
                None
            } else if code == Some(127) {
                Some(format!(
                    "Couldn't find Claude Code (`{}`). Install it, then send again.",
                    self.d.env().claude
                ))
            } else if last.is_empty() {
                Some(format!("Claude Code stopped unexpectedly ({why})"))
            } else {
                Some(format!("Claude Code stopped unexpectedly ({why}): {last}"))
            };
            self.end_turn(Outcome::Interrupted { reason });
        } else if code != Some(0) && !last.is_empty() {
            self.log(Body::Notice {
                text: format!("Claude Code exited ({why}): {last}"),
            });
        }
    }
}

/// Move held message `id` to just before `before`, or to the end. False
/// when `id` isn't waiting any more. A `before` that has gone means the end.
fn requeue(
    held: &mut VecDeque<(String, String, Vec<String>)>,
    id: &str,
    before: Option<&str>,
) -> bool {
    let Some(at) = held.iter().position(|(h, _, _)| h == id) else {
        return false;
    };
    let Some(m) = held.remove(at) else {
        return false;
    };
    let to = before
        .and_then(|b| held.iter().position(|(h, _, _)| h == b))
        .unwrap_or(held.len());
    held.insert(to, m);
    true
}

/// The last thing the agent said before `events` end: where a rewound
/// conversation picks up. `None` when it hadn't said anything yet.
fn resume_point(events: &[dent_protocol::Event]) -> Option<String> {
    events.iter().rev().find_map(|e| match &e.body {
        Body::Assistant {
            parent: None,
            uuid: Some(u),
            ..
        } => Some(u.clone()),
        _ => None,
    })
}

/// A message id as part of a ref name.
fn safe_ref(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn settled(outcome: &Outcome) -> Status {
    match outcome {
        Outcome::Done => Status::Done,
        Outcome::Interrupted { .. } => Status::Interrupted,
        Outcome::Error { .. } => Status::Error,
    }
}

#[cfg(test)]
mod requeue_tests {
    use super::*;

    fn q(ids: &[&str]) -> VecDeque<(String, String, Vec<String>)> {
        ids.iter()
            .map(|i| (i.to_string(), String::new(), Vec::new()))
            .collect()
    }

    fn ids(h: &VecDeque<(String, String, Vec<String>)>) -> Vec<&str> {
        h.iter().map(|(i, _, _)| i.as_str()).collect()
    }

    #[test]
    fn a_waiting_message_moves_before_another_or_to_the_end() {
        let mut h = q(&["a", "b", "c"]);
        assert!(requeue(&mut h, "c", Some("a")));
        assert_eq!(ids(&h), ["c", "a", "b"]);
        assert!(requeue(&mut h, "c", None));
        assert_eq!(ids(&h), ["a", "b", "c"]);
        // One already gone can't move; one it's put before that's gone
        // means the end.
        assert!(!requeue(&mut h, "x", None));
        assert!(requeue(&mut h, "a", Some("x")));
        assert_eq!(ids(&h), ["b", "c", "a"]);
    }
}
