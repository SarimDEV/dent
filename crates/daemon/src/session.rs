//! One task per session. It owns the session's agent process (through the
//! holder), turns the agent's output into events, and runs the person's
//! commands. Everything it knows can be rebuilt from the log, so a
//! restarted daemon carries on where the last one stopped.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use dent_protocol::{Answer, Body, Outcome, Permission, Status, Task};
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
}

struct Pending {
    tool: String,
    input: Value,
    suggestions: Value,
}

#[derive(Clone, Copy)]
enum Ctl {
    Initialize,
    Interrupt,
    Other,
}

struct Session {
    d: Arc<Daemon>,
    id: String,
    cwd: String,
    model: Option<String>,
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
        permission: row.permission,
        agent_session: row.agent_session.clone(),
        status: row.status,
        settled: row.status,
        in_turn: false,
        agent_started: false,
        resume_at: None,
        held: VecDeque::new(),
        steered: false,
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
    };
    s.rebuild();
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
                Body::Asked {
                    id, tool, input, ..
                } => {
                    let input = serde_json::from_str(&input).unwrap_or(Value::Null);
                    self.asked.insert(
                        id,
                        Pending {
                            tool,
                            input,
                            suggestions: Value::Null,
                        },
                    );
                }
                Body::Answered { id, .. } => {
                    self.asked.remove(&id);
                }
                Body::TurnStarted => {
                    self.in_turn = true;
                    self.agent_started = true;
                    rewound_to = None;
                }
                Body::Rewound { to, .. } => {
                    rewound_to = Some(to);
                    self.in_turn = false;
                }
                Body::TurnEnded { outcome } => {
                    self.in_turn = false;
                    self.settled = settled(&outcome);
                }
                Body::Background { tasks } => self.background = tasks,
                Body::Context { used, max } => {
                    self.context = Some(used);
                    self.max_context = max.or(self.max_context);
                }
                _ => {}
            }
        }
        if matches!(self.status, Status::Working | Status::NeedsYou) {
            self.in_turn = true;
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
            });
            if !self.in_turn {
                self.idle_at = Some(Instant::now() + idle());
            }
        } else {
            let _ = self.d.store().clear_proc(&self.id);
            if self.in_turn {
                self.end_turn(Outcome::Interrupted {
                    reason: Some("dent restarted while the agent was working".into()),
                });
            }
        }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        loop {
            let deadline = [self.idle_at, self.stop_at].into_iter().flatten().min();
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
            Op::Interrupt => self.interrupt().await,
            Op::Answer { id, answer } => self.answer(id, answer).await,
            Op::SetModel(model) => {
                if self.proc.is_some() {
                    self.control("set_model", json!({ "model": model }), Ctl::Other)
                        .await?;
                }
                self.model = Some(model.clone());
                self.log(Body::Settings {
                    model: Some(model),
                    permission: None,
                    title: None,
                });
                Ok(())
            }
            Op::SetPermission(p) => {
                if self.proc.is_some() {
                    let mode = claude::mode_name(p);
                    self.control("set_permission_mode", json!({ "mode": mode }), Ctl::Other)
                        .await?;
                }
                self.permission = p;
                self.log(Body::Settings {
                    model: None,
                    permission: Some(p),
                    title: None,
                });
                Ok(())
            }
            Op::Rename(title) => {
                self.log(Body::Settings {
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
        self.idle_at = None;
        Ok(())
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
                bail!("there's no snapshot of the files from then (is the folder a git repository?)");
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
        if let Some(p) = self.proc.take() {
            let _ = self.d.store().clear_proc(&self.id);
            if let Ok(h) = self.d.holder().await {
                let _ = h.stop(&p.id, Duration::from_secs(1)).await;
            }
        }
        self.idle_at = None;
        for (id, _, _) in std::mem::take(&mut self.held) {
            self.log(Body::Unqueued { id });
        }
        self.settled = Status::Done;
        self.log(Body::Rewound { to, files });
        Ok(())
    }

    async fn ensure_proc(&mut self) -> Result<()> {
        if self.proc.is_some() {
            return Ok(());
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
        let proc = self.d.store().next_proc(&self.id)?;
        let argv = claude::argv(
            &self.d.env.claude,
            &self.d.env.path,
            self.model.as_deref(),
            self.permission,
            &start,
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
        });
        self.control("initialize", json!({}), Ctl::Initialize)
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

    async fn control(&mut self, subtype: &str, extra: Value, kind: Ctl) -> Result<()> {
        self.next_ctl += 1;
        let id = format!("dent-{}", self.next_ctl);
        self.controls.insert(id.clone(), kind);
        self.write(&wire::control_request(&id, subtype, extra))
            .await
    }

    async fn interrupt(&mut self) -> Result<()> {
        if !self.in_turn || self.proc.is_none() {
            return Ok(());
        }
        // Stop means nothing more arrives: held messages come back.
        for (id, _, _) in std::mem::take(&mut self.held) {
            self.log(Body::Unqueued { id });
        }
        self.steered = false;
        self.control("interrupt", json!({}), Ctl::Interrupt).await?;
        // The agent doesn't always end the turn after an interrupt.
        self.stop_at = Some(Instant::now() + STOP_GRACE);
        Ok(())
    }

    async fn answer(&mut self, id: String, answer: Answer) -> Result<()> {
        let Some(p) = self.asked.get(&id) else {
            bail!("already answered");
        };
        let msg = match &answer {
            Answer::Allow { always } => {
                wire::allow(&id, &p.tool, &p.input, *always, &p.suggestions)
            }
            Answer::Deny { message } => wire::deny(&id, message),
            Answer::Choose { answers } => wire::allow(
                &id,
                &p.tool,
                &claude::answers(&p.input, answers),
                false,
                &Value::Null,
            ),
            Answer::Expired => bail!("can't answer with expired"),
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
                p.stopping = true;
                let id = p.id.clone();
                if let Ok(h) = self.d.holder().await {
                    let _ = h.stop(&id, Duration::from_secs(1)).await;
                }
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
                self.log_from(Body::TurnStarted, end);
            }
            Inbound::MessageStart { parent: None } => self.d.clear_partial(&self.id),
            Inbound::TextDelta { parent: None, text } => self.d.delta(&self.id, &text, false),
            Inbound::Assistant {
                parent,
                uuid,
                blocks,
                context,
            } => {
                if parent.is_none()
                    && let Some(c) = context
                {
                    self.context = Some(c);
                }
                let blocks = claude::blocks(blocks);
                if !blocks.is_empty() {
                    self.log_from(
                        Body::Assistant {
                            parent,
                            blocks,
                            uuid,
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
                    },
                    end,
                );
                self.asked.insert(
                    request_id,
                    Pending {
                        tool: tool_name,
                        input,
                        suggestions,
                    },
                );
            }
            Inbound::ControlResponse {
                request_id,
                ok,
                body,
            } => match (self.controls.remove(&request_id), ok) {
                (Some(Ctl::Initialize), true) => {
                    self.d.set_models(claude::models(&body));
                    self.d.set_commands(&self.id, claude::commands(&body));
                }
                (Some(Ctl::Interrupt), _) | (Some(Ctl::Other), true) | (None, _) => {}
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
                        message: text.unwrap_or_else(|| "The turn failed".into()),
                    }
                } else {
                    Outcome::Done
                };
                if let Some(used) = self.context {
                    self.log_from(
                        Body::Context {
                            used,
                            max: self.max_context,
                        },
                        end,
                    );
                }
                self.end_turn_at(outcome, Some(end));
            }
            Inbound::MessageStart { .. } | Inbound::TextDelta { .. } | Inbound::Other => {}
        }
    }

    fn end_turn(&mut self, outcome: Outcome) {
        self.end_turn_at(outcome, None);
    }

    fn end_turn_at(&mut self, outcome: Outcome, end: Option<u64>) {
        self.stop_at = None;
        // Approvals still open belonged to the turn that just ended.
        for id in std::mem::take(&mut self.asked).into_keys() {
            self.log(Body::Answered {
                id,
                answer: Answer::Expired,
            });
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
        if let Ok(h) = self.d.holder().await {
            let _ = h.release(proc).await;
        }
        self.idle_at = None;
        if p.stopping && !self.in_turn {
            return;
        }
        let why = match (code, signal) {
            (_, Some(s)) => format!("signal {s}"),
            (Some(c), _) => format!("exit code {c}"),
            _ => "no exit status".into(),
        };
        let tail = String::from_utf8_lossy(&p.err).trim().to_string();
        let last = tail.lines().last().unwrap_or_default().to_string();
        if self.in_turn {
            let reason = if p.stopping {
                None
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
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect()
}

fn settled(outcome: &Outcome) -> Status {
    match outcome {
        Outcome::Done => Status::Done,
        Outcome::Interrupted { .. } => Status::Interrupted,
        Outcome::Error { .. } => Status::Error,
    }
}
