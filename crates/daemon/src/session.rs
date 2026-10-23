//! One task per session. It owns the session's agent process (through the
//! holder), turns the agent's output into events, and runs the person's
//! commands. Everything it knows can be rebuilt from the log, so a
//! restarted daemon carries on where the last one stopped.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use dent_protocol::{
    Act, Activity, Answer, Body, CommandOutcome, Out, Outcome, Permission, SecretPlace, Section,
    Status, Task, View,
};
use dent_session_holder::wire::Proc as HeldProc;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::agent::{self, Asked, AssistantBlock, Driver, Event, Launch, Reply, Req, Start, log};
use crate::daemon::Daemon;
use crate::store::{Row, Source};

/// How long messages left waiting across a restart wait for reconnecting
/// clients before they go.
const RELEASE_GRACE: Duration = Duration::from_secs(5);
/// A turn cut off by a reboot or the holder dying carries on by itself,
/// but not after this many times in [`CARRY_ON_WINDOW`] seconds: something
/// in it may be what keeps bringing the machine down.
const CARRY_ON_MAX: usize = 3;
const CARRY_ON_WINDOW: u64 = 300;

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
    /// The session's new worktree is set up.
    SetUp,
    Send {
        id: String,
        text: String,
        steer: bool,
        attachments: Vec<String>,
    },
    Unqueue {
        id: String,
    },
    /// Every held message goes back to whoever sent it.
    GiveBack,
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
    /// A desktop's picture of a page the agent asked to see.
    Looked {
        key: String,
        look: dent_protocol::Look,
    },
    SetModel(String),
    SetPermission(Permission),
    /// Do what a command's sheet offers.
    Act {
        command: String,
        act: Act,
    },
    SetEffort(String),
    Restart,
    Rename(String),
    Close,
    Reopen,
    Seen(u64),
    Resume(Option<u64>),
    StopTask(String),
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
    /// Its agent's process group uses this much memory, past the warning
    /// limit ([`crate::memory`]).
    Memory {
        proc: String,
        bytes: u64,
    },
    /// A step in moving it to another machine.
    Move(crate::moving::state::Step),
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
    /// The agents' slice's out-of-memory kills when it started (under
    /// systemd), to tell running out of memory from a plain kill.
    ooms: Option<u64>,
    /// Said once that it uses a lot of memory.
    warned: bool,
    /// Measured past the stopping limit once: twice in a row stops it.
    high: bool,
    /// Stopped by dent for using this much memory.
    over: Option<u64>,
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
    /// A model or permission mode, kept once the agent takes it.
    Model(String),
    Permission(Permission),
    StopTask(String),
}

impl Ctl {
    /// What it asks the agent.
    fn req(&self) -> Req {
        match self {
            Ctl::Initialize => Req::Initialize,
            Ctl::Interrupt => Req::Interrupt,
            Ctl::ContextUsage => Req::ContextUsage,
            Ctl::Settings => Req::Settings,
            Ctl::Sheet { command, .. } | Ctl::Refresh { command } => Req::Sheet(command.clone()),
            Ctl::Act { act, .. } => Req::Act(act.clone()),
            Ctl::Model(m) => Req::Model(m.clone()),
            Ctl::Permission(p) => Req::Permission(*p),
            Ctl::StopTask(t) => Req::StopTask(t.clone()),
        }
    }
}

struct Session {
    d: Arc<Daemon>,
    /// The agent this session runs, for good.
    agent: &'static dyn Driver,
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
    /// Its move to another machine, while there is one, or once it moved.
    moving: Option<crate::moving::state::Move>,
    /// Which tool call started each background task.
    task_calls: HashMap<String, String>,
    /// The thinking streaming now, and the sentence of it on the working
    /// line.
    thinking: String,
    thought: Option<String>,
    /// MCP servers already said to be in trouble, as name/status.
    mcp_told: HashSet<String>,
    /// When to carry on by itself (Unix seconds): a used-up limit resets.
    wake: Option<u64>,
    asked: HashMap<String, Asked>,
    /// Client message ids already logged, so a resend never runs twice.
    seen: HashSet<String>,
    proc: Option<Proc>,
    controls: HashMap<String, Ctl>,
    next_ctl: u64,
    context: Option<u64>,
    max_context: Option<u64>,
    idle_at: Option<Instant>,
    /// When messages left waiting across a restart go, if nothing has
    /// taken them back by then.
    release_at: Option<Instant>,
    /// When a turn dent's own restart cut off carries on by itself, unless
    /// you do something first. What it sends is kept in the store.
    carry_on_at: Option<Instant>,
    stop_at: Option<Instant>,
    /// When a starting agent must have answered by.
    start_by: Option<Instant>,
    /// After a restart: the turn is in progress only because a waiting
    /// message was handed over whose turn hadn't started.
    owed: bool,
    /// Replies owed to the agent, written once its output is handled.
    outbox: Vec<Value>,
    /// The last message sent, to send again if the agent has to start over.
    last: Option<(String, String, Vec<String>)>,
    /// The agent couldn't find the conversation it was asked to resume.
    forgot: bool,
    /// Stop was asked for the running turn. Kept in the store, so a
    /// restarted daemon still sees it through.
    stop_asked: bool,
    /// Each command's sheet as clients last got it, so a refresh that
    /// changes nothing says nothing.
    sheets: HashMap<String, Vec<Section>>,
    /// Calls that run a subagent, whose result is its report and is kept
    /// far longer than other output ([`log::keep`]).
    reports: log::Reports,
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
    // A session from a newer daemon, with an agent this one can't drive,
    // stays as it is: its log still reads, and nothing else runs it.
    let Some(agent) = agent::driver(&row.agent) else {
        eprintln!(
            "dent-daemon: {}: no driver for agent {:?}, leaving it be",
            row.id, row.agent
        );
        return;
    };
    let (tx, rx) = mpsc::unbounded_channel();
    d.register(&row.id, tx, agent);
    let mut s = Session {
        d,
        agent,
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
        moving: None,
        task_calls: HashMap::new(),
        mcp_told: HashSet::new(),
        thinking: String::new(),
        thought: None,
        wake: None,
        asked: HashMap::new(),
        seen: HashSet::new(),
        proc: None,
        controls: HashMap::new(),
        next_ctl: 0,
        context: None,
        max_context: None,
        idle_at: None,
        release_at: None,
        carry_on_at: None,
        stop_at: None,
        start_by: None,
        owed: false,
        outbox: Vec::new(),
        last: None,
        forgot: false,
        stop_asked: false,
        sheets: HashMap::new(),
        reports: log::Reports::default(),
        watch: None,
    };
    s.rebuild();
    // Carrying on was due when the last daemon went: still is.
    if let Some((id, _)) = s.carry_on() {
        if s.seen.contains(&id) {
            let _ = s.d.store().del(&s.carry_on_key());
        } else {
            s.carry_on_at = Some(Instant::now() + RELEASE_GRACE);
        }
    }
    s.moving =
        s.d.store()
            .get(&crate::moving::move_key(&s.id))
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok());
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
    /// Where a stop asked for this session is kept.
    fn stop_key(&self) -> String {
        format!("stop.{}", self.id)
    }

    /// Its agent, if it outlived the holder that ran it.
    fn stop_left_behind(&self, proc: &str) {
        let paths = &self.d.paths;
        for dir in [paths.holder(), paths.holder_in_tmp()] {
            crate::holder::stop_left_behind(&dir, proc);
        }
    }

    /// Where the message it carries on with is kept, until it's sent.
    fn carry_on_key(&self) -> String {
        format!("carry_on.{}", self.id)
    }

    /// The message it carries on with, as (id, text), if one is due.
    fn carry_on(&self) -> Option<(String, String)> {
        let v = self.d.store().get(&self.carry_on_key()).ok().flatten()?;
        serde_json::from_str(&v).ok()
    }

    fn carry_ons_key(&self) -> String {
        format!("carry_ons.{}", self.id)
    }

    /// When it carried on by itself lately, in Unix seconds.
    fn carry_ons(&self) -> Vec<u64> {
        let now = crate::daemon::now_ms() / 1000;
        let mut times: Vec<u64> = self
            .d
            .store()
            .get(&self.carry_ons_key())
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        times.retain(|t| t + CARRY_ON_WINDOW > now);
        times
    }

    /// Counts carrying on now. Not counted (the store failing), it doesn't
    /// go: it waits for you rather than risk a loop.
    fn count_carry_on(&self) -> bool {
        let mut times = self.carry_ons();
        times.push(crate::daemon::now_ms() / 1000);
        serde_json::to_string(&times)
            .ok()
            .is_some_and(|v| self.d.store().put(&self.carry_ons_key(), &v).is_ok())
    }

    /// You did something first: it doesn't carry on by itself.
    fn call_off_carry_on(&mut self) {
        if self.carry_on_at.take().is_some() {
            let _ = self.d.store().del(&self.carry_on_key());
        }
    }

    /// The agent's process went with something that was neither it nor
    /// you: the machine restarting, or the holder dying. The turn ends, and
    /// carries on by itself in a moment, unless it was waiting on you,
    /// messages wait to go instead, or it's been cut off too often lately.
    fn cut_off(&mut self, reason: &str) {
        let lost = self.background_gone();
        if !self.in_turn {
            return;
        }
        // Waiting on you, or being stopped by you: yours to carry on.
        let waiting = !self.asked.is_empty() || self.stop_asked;
        let carry = !waiting
            && self.held.is_empty()
            && crate::moving::state::fence(self.moving.as_ref()).is_none();
        // Only times it really carried on count: those you took over don't.
        let too_often = carry && self.carry_ons().len() >= CARRY_ON_MAX;
        // Kept before the turn is logged as ended: a daemon that goes in
        // between finds the turn still running, and comes here again,
        // keeping the message it kept the first time.
        let kept = carry && !too_often && {
            let due = self
                .carry_on()
                .unwrap_or_else(|| (crate::daemon::new_id(), dent_protocol::carry_on_text(&lost)));
            serde_json::to_string(&due)
                .ok()
                .is_some_and(|v| self.d.store().put(&self.carry_on_key(), &v).is_ok())
        };
        self.end_turn(Outcome::Interrupted {
            reason: Some(reason.into()),
        });
        if too_often {
            self.log(Body::Notice {
                text: format!(
                    "Cut off {CARRY_ON_MAX} times in {} minutes, so it waits for you to continue.",
                    CARRY_ON_WINDOW / 60
                ),
            });
        }
        if kept {
            self.carry_on_at = Some(Instant::now() + RELEASE_GRACE);
        }
    }

    /// Recover what's in memory from the log.
    fn rebuild(&mut self) {
        self.stop_asked = matches!(self.d.store().get(&self.stop_key()), Ok(Some(_)));
        let Ok(events) = self.d.store().session_events(&self.id) else {
            return;
        };
        let mut rewound_to: Option<String> = None;
        let mut forked_at: Option<Option<String>> = None;
        // A turn taken back the agent hasn't started again from, and
        // whether its end is still to come (it leaves the outcome before).
        let mut withdrew: Option<usize> = None;
        let mut quiet_end = false;
        // A waiting message handed to the agent whose turn hadn't started:
        // the last daemon went in between, so that turn is still owed.
        let mut handed = false;
        for (ix, e) in events.iter().cloned().enumerate() {
            match &e.body {
                Body::Delivered { .. } => handed = true,
                Body::TurnStarted
                | Body::TurnEnded { .. }
                | Body::Rewound { .. }
                | Body::Forked { .. } => handed = false,
                _ => {}
            }
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
                Body::Delivered { id } | Body::Unqueued { id, .. } => {
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
                        Asked {
                            tool,
                            dialog: agent::dialog_values(&input),
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
                // A subagent still running across a restart keeps its
                // report whole when it comes.
                Body::Assistant { parent, blocks, .. } => {
                    self.reports.note(&blocks);
                    if parent.is_none() {
                        self.said = true;
                    }
                }
                Body::ToolResult { tool, .. } => {
                    self.reports.answered(&tool);
                }
                Body::TurnStarted => {
                    self.in_turn = true;
                    self.said = false;
                    self.agent_started = true;
                    rewound_to = None;
                    forked_at = None;
                    withdrew = None;
                }
                Body::Rewound { to, .. } => {
                    rewound_to = Some(to);
                    withdrew = None;
                    self.in_turn = false;
                }
                Body::Forked { upto, .. } => {
                    forked_at = Some(upto);
                    withdrew = None;
                    self.in_turn = false;
                }
                // Where it goes back to accounts for any rewind or fork
                // before it.
                Body::Withdrawn { .. } => {
                    rewound_to = None;
                    forked_at = None;
                    withdrew = Some(ix);
                    quiet_end = true;
                    self.in_turn = false;
                }
                Body::TurnEnded { outcome } => {
                    self.in_turn = false;
                    if !std::mem::take(&mut quiet_end) {
                        self.settled = settled(&outcome);
                    } else if self.settled == Status::New {
                        self.settled = Status::Interrupted;
                    }
                }
                Body::Background { tasks } => self.background = tasks,
                Body::Resume { at } => self.wake = at,
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
        if matches!(self.status, Status::Working | Status::NeedsYou) || handed {
            self.in_turn = true;
        }
        self.owed = handed;
        // A fork the agent hasn't picked up yet: it starts from the copy.
        if let Some(at) = forked_at {
            self.resume_at = Some(at);
        }
        // So does a turn taken back: the next agent starts from before it.
        // (From before the turn it took back: anything logged since, a
        // message the last daemon went before handing over, is no reply.)
        if let Some(w) = withdrew
            && let Some(at) = conversation_at(&events, turn_start(&events, w))
        {
            self.resume_at = Some(at);
        }
        // A rewind the agent hasn't picked up yet still applies.
        if let Some(to) = rewound_to
            && let Some(at) = entered_at(&events, &to)
        {
            self.resume_at =
                Some(conversation_at(&events, at).unwrap_or_else(|| resume_point(&events[..at])));
        }
    }

    async fn reattach(&mut self, row: &Row, held: &[HeldProc]) {
        let Some(proc) = row.proc.clone() else {
            // No agent, so nothing of its runs in the background, and a
            // turn owed (the last daemon went while starting one) can't
            // come: it ends here.
            self.background_gone();
            if self.in_turn {
                self.end_turn(Outcome::Interrupted {
                    reason: Some("dent restarted before the agent started.".into()),
                });
            }
            self.release_held_soon();
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
                ooms: crate::scopes::oom_kills(),
                warned: false,
                high: false,
                over: None,
            });
            let now = Instant::now();
            if !self.in_turn {
                self.idle_at = Some(now + idle());
            } else if self.stop_asked {
                // A stop the last daemon didn't see through: the agent was
                // asked, so its process goes if the turn doesn't end soon.
                self.stop_at = Some(now + STOP_GRACE);
            } else if row.proc_off == 0 || self.owed {
                // It has said nothing yet, or a message was handed to it
                // whose turn never started (the last daemon may have gone
                // before writing it): still starting, or stuck.
                self.start_by = Some(now + start_limit());
            }
        } else {
            let _ = self.d.store().clear_proc(&self.id);
            self.stop_left_behind(&proc);
            self.cut_off("This turn was cut off when dent restarted.");
        }
        self.release_held_soon();
    }

    /// Messages waiting on a turn that ended while dent was away (or that
    /// ended as it came back, its agent gone) go as after a crash; else they
    /// would wait for a turn that never comes. Not at once: a client
    /// reconnecting sends again what it asked meanwhile (a Stop or Edit
    /// that takes them back), and that comes first.
    fn release_held_soon(&mut self) {
        if !self.in_turn && !self.held.is_empty() {
            self.release_at = Some(Instant::now() + RELEASE_GRACE);
        }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        loop {
            let wake = self.wake.map(|at| {
                let now = crate::daemon::now_ms() / 1000;
                Instant::now() + Duration::from_secs(at.saturating_sub(now))
            });
            let deadline = [
                wake,
                self.idle_at,
                self.release_at,
                self.carry_on_at,
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
        // Anything you start yourself takes the place of carrying on.
        if matches!(
            op,
            Op::Send { .. }
                | Op::Rewind { .. }
                | Op::Requeue { .. }
                | Op::Close
                | Op::Move(crate::moving::state::Step::Start { .. })
        ) {
            self.call_off_carry_on();
        }
        // A session on its way to another machine starts nothing here, and
        // one that went is a record (decision 0064).
        if fenced_by_moves(&op)
            && let Some(why) = crate::moving::state::fence(self.moving.as_ref())
        {
            anyhow::bail!(why);
        }
        // A message while the move waits for the turn to end calls it off:
        // you're still working here.
        if matches!(op, Op::Send { .. }) {
            self.move_step(crate::moving::state::Step::Message)?;
        }
        match op {
            Op::Move(step) => self.move_op(step),
            Op::Send {
                id,
                text,
                steer,
                attachments,
            } => self.send(id, text, steer, attachments).await,
            // A new worktree is ready: what waited for it goes.
            Op::SetUp => {
                self.d.set_up_done(&self.id);
                if !self.in_turn && self.asked.is_empty() {
                    self.deliver_held().await;
                }
                Ok(())
            }
            Op::Rewind { to, files } => {
                if !self.agent.caps().rewind {
                    bail!("{} can't go back to an earlier message", self.agent.name());
                }
                self.rewind(to, files).await
            }
            Op::Unqueue { id } => {
                let before = self.held.len();
                self.held.retain(|(h, _, _)| *h != id);
                if self.held.len() == before {
                    bail!("that message has already gone to the agent");
                }
                self.log(Body::Unqueued { id, returned: None });
                Ok(())
            }
            Op::GiveBack => {
                self.d.set_up_done(&self.id);
                for (id, text, attachments) in std::mem::take(&mut self.held) {
                    self.log(Body::Unqueued {
                        id,
                        returned: Some(dent_protocol::Returned { text, attachments }),
                    });
                }
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
            Op::Looked { key, look } => self.looked(&key, &look),
            // A running agent is asked first, and the change kept when it
            // agrees; otherwise the next agent starts with it.
            Op::SetModel(model) => {
                if self.model.as_deref() == Some(model.as_str()) {
                    return Ok(());
                }
                if self.running() && self.agent.caps().set_model {
                    self.control(Ctl::Model(model)).await?;
                } else {
                    self.keep(Ctl::Model(model), None);
                    // One that can't change its model as it runs takes it
                    // in a new process, as a new effort does.
                    if self.running() {
                        self.respawn_soon().await;
                    }
                }
                Ok(())
            }
            Op::SetPermission(p) => {
                if p == self.permission {
                    return Ok(());
                }
                if !self.agent.caps().permissions.contains(&p) {
                    bail!("{} has no {} mode", self.agent.name(), p.label());
                }
                if self.running() {
                    self.control(Ctl::Permission(p)).await?;
                } else {
                    self.keep(Ctl::Permission(p), None);
                }
                Ok(())
            }
            Op::Act { command, act } => {
                if self.agent.request("", &Req::Act(act.clone())).is_none() {
                    bail!("that's for the client to do");
                }
                self.ensure_proc().await?;
                self.control(Ctl::Act { command, act }).await
            }
            Op::SetEffort(effort) => {
                // The same effort again mustn't restart the agent.
                if self.effort.as_deref().unwrap_or("") == effort {
                    return Ok(());
                }
                if !self.agent.caps().effort {
                    bail!("{} has no effort levels", self.agent.name());
                }
                self.effort = (!effort.is_empty()).then(|| effort.clone());
                self.log(Body::Settings {
                    effort: Some(effort),
                    model: None,
                    permission: None,
                    title: None,
                });
                // Effort is fixed when the agent starts.
                self.respawn_soon().await;
                Ok(())
            }
            Op::Restart => {
                if !self.agent.caps().restart {
                    bail!(
                        "{} can't start again on the same conversation",
                        self.agent.name()
                    );
                }
                // As a new effort does: a fresh process resuming the same
                // conversation, which reads skills, plugins and settings anew.
                let later = self.in_turn || !self.background.is_empty();
                if later {
                    self.respawn = true;
                } else {
                    self.retire_proc().await;
                }
                let name = self.agent.name();
                self.log(Body::Notice {
                    text: if later {
                        format!(
                            "{name} starts again once this turn is done, with skills, plugins and settings as they are now."
                        )
                    } else {
                        format!(
                            "{name} starts again with your next message, with skills, plugins and settings as they are now."
                        )
                    },
                });
                Ok(())
            }
            Op::Rename(title) => {
                // Named by you: dent never titles it again on its own.
                let _ = self
                    .d
                    .store()
                    .put(&crate::daemon::titled_key(&self.id), "1");
                self.log(Body::Settings {
                    effort: None,
                    model: None,
                    permission: None,
                    title: Some(title),
                });
                Ok(())
            }
            Op::Resume(at) => {
                if self.wake != at {
                    self.wake = at;
                    self.log(Body::Resume { at });
                }
                Ok(())
            }
            Op::StopTask(task) => {
                if !self.agent.caps().stop_task {
                    bail!(
                        "{} can't stop one piece of background work",
                        self.agent.name()
                    );
                }
                if self.proc.is_none() || !self.background.iter().any(|t| t.id == task) {
                    bail!("that work has already stopped");
                }
                self.control(Ctl::StopTask(task)).await
            }
            Op::Seen(upto) => {
                self.log(Body::Seen { upto });
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
            Op::Memory { proc, bytes } => {
                let limits = crate::memory::limits();
                let name = self.agent.name();
                let Some(p) = self.proc.as_mut().filter(|p| p.id == proc && !p.stopping) else {
                    return Ok(());
                };
                // Twice in a row: one reading can count memory its
                // processes share more than once.
                let again = std::mem::replace(&mut p.high, bytes >= limits.stop);
                if bytes >= limits.stop {
                    if again && p.over.is_none() {
                        p.over = Some(bytes);
                        if let Ok(h) = self.d.holder().await {
                            let _ = h.stop(&proc, Duration::from_secs(2)).await;
                        }
                    }
                } else if bytes >= limits.warn && !p.warned {
                    p.warned = true;
                    self.log(Body::Notice {
                        text: format!(
                            "{name} is using {} of memory. dent stops it at {}, before it slows this machine down.",
                            crate::memory::words(bytes),
                            crate::memory::words(limits.stop)
                        ),
                    });
                }
                Ok(())
            }
            Op::HolderLost => {
                if let Some(p) = self.proc.take() {
                    let _ = self.d.store().clear_proc(&self.id);
                    self.stop_left_behind(&p.id);
                    self.controls_gone();
                    self.cut_off(
                        "This turn was cut off: the part of dent that runs agents stopped.",
                    );
                    drop(p);
                }
                if !self.in_turn && self.asked.is_empty() && !self.held.is_empty() {
                    self.deliver_held().await;
                }
                Ok(())
            }
        }
    }

    /// Start a fresh process that resumes the same conversation: now, or
    /// with the next message once this turn and any background work are
    /// done.
    async fn respawn_soon(&mut self) {
        if self.in_turn || !self.background.is_empty() {
            self.respawn = true;
        } else {
            self.retire_proc().await;
        }
    }

    /// A model or permission mode is now the session's.
    fn keep(&mut self, setting: Ctl, end: Option<u64>) {
        let body = match setting {
            Ctl::Model(m) => {
                self.model = Some(m.clone());
                Body::Settings {
                    effort: None,
                    model: Some(m),
                    permission: None,
                    title: None,
                }
            }
            Ctl::Permission(p) => {
                self.permission = p;
                Body::Settings {
                    effort: None,
                    model: None,
                    permission: Some(p),
                    title: None,
                }
            }
            _ => return,
        };
        match end {
            Some(off) => self.log_from(body, off),
            None => self.log(body),
        }
    }

    /// An agent process that takes messages: there, and not winding down.
    fn running(&self) -> bool {
        self.proc.as_ref().is_some_and(|p| !p.stopping)
    }

    /// The agent process is gone: requests it never answered won't be.
    /// A setting it hadn't taken yet goes to the next one.
    fn controls_gone(&mut self) {
        for (_, c) in std::mem::take(&mut self.controls) {
            self.keep(c, None);
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

    /// A step in its move: persisted, then said.
    fn move_step(&mut self, step: crate::moving::state::Step) -> Result<()> {
        let was = self.moving.clone();
        let now = crate::moving::state::next(was.as_ref(), step).map_err(anyhow::Error::msg)?;
        if now == was {
            return Ok(());
        }
        let key = crate::moving::move_key(&self.id);
        match &now {
            Some(m) => self.d.store().put(&key, &serde_json::to_string(m)?)?,
            None => self.d.store().del(&key)?,
        }
        if let Some(m) = &now {
            self.log(Body::Move {
                id: m.id.clone(),
                to: m.to.clone(),
                to_machine: m.to_machine.clone(),
                state: m.state.clone(),
                to_session: m.to_session.clone(),
            });
        }
        // Over (called off, failed, kept here): its package goes, and its
        // id is spent.
        if let Some(m) = now.as_ref().filter(|m| {
            !matches!(
                m.state,
                dent_protocol::MoveState::Pending
                    | dent_protocol::MoveState::Departing { .. }
                    | dent_protocol::MoveState::Departed
            )
        }) {
            let _ = self.d.store().put(&crate::moving::ended_key(&m.id), "1");
            let _ = std::fs::remove_dir_all(crate::moving::pack::dir(&self.d.paths.state, &m.id));
        }
        // Over (called off, failed, kept here): nothing is kept of it.
        self.moving = now.filter(|m| {
            matches!(
                m.state,
                dent_protocol::MoveState::Pending
                    | dent_protocol::MoveState::Departing { .. }
                    | dent_protocol::MoveState::Departed
            )
        });
        if self.moving.is_none() {
            self.d.store().del(&key)?;
        }
        Ok(())
    }

    fn move_op(&mut self, step: crate::moving::state::Step) -> Result<()> {
        use crate::moving::state::Step;
        let step = match step {
            Step::Start {
                id, to, to_machine, ..
            } => {
                // A move that ended doesn't start again under its id: its
                // package is gone, and the session has moved on since.
                let ended = self
                    .d
                    .store()
                    .get(&crate::moving::ended_key(&id))
                    .ok()
                    .flatten()
                    .is_some();
                if ended && self.moving.as_ref().is_none_or(|m| m.id != id) {
                    anyhow::bail!("that move ended; move it again (a new move, without its id)");
                }
                // Messages waiting would start a turn here once it's fenced.
                if !self.held.is_empty() {
                    anyhow::bail!("send or take back the messages waiting first, then move it");
                }
                // Carrying on when a limit resets would start work after
                // it's fenced, here or on the other machine.
                if self.moving.is_none() && self.wake.take().is_some() {
                    self.log(Body::Resume { at: None });
                }
                // Busy: it waits for the turn, and work left running, to end.
                let busy = self.in_turn || !self.background.is_empty();
                Step::Start {
                    id,
                    to,
                    to_machine,
                    busy,
                }
            }
            other => other,
        };
        self.move_step(step)
    }

    fn update_status(&mut self) {
        // The turn (and work it left) it waited for is over: fenced now.
        if self
            .moving
            .as_ref()
            .is_some_and(|m| m.state == dent_protocol::MoveState::Pending)
            && !self.in_turn
            && self.background.is_empty()
        {
            let _ = self.move_step(crate::moving::state::Step::Quiet);
        }
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
        self.d
            .set_busy(&self.id, matches!(s, Status::Working | Status::Background));
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
        // Anything sent takes the place of carrying on by itself.
        if self.wake.take().is_some() && text != RESUME_TEXT {
            self.log(Body::Resume { at: None });
        }
        // An agent that can't be steered gets it once the turn ends.
        let steer = steer && self.agent.caps().steer;
        let working = self.in_turn && self.proc.is_some();
        // Nothing can be steered into an agent that's being stopped: it
        // waits for the turn to end like any other. While a new worktree
        // is set up, everything waits for that.
        let queued = (working && (!steer || !self.running() || self.stop_asked))
            || self.d.setting_up(&self.id);
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
        // So does going back (a rewind, a message taken back): the running
        // process still holds what came after.
        if (self.respawn || self.resume_at.is_some()) && !steer && self.background.is_empty() {
            self.retire_proc().await;
        }
        let point = self.resume_at.clone();
        if let Err(e) = self.ensure_proc().await {
            self.log(Body::Notice {
                text: format!("Couldn't start {}: {e:#}", self.agent.name()),
            });
            self.end_turn(Outcome::Error {
                message: format!("{e:#}"),
            });
            return Err(e);
        }
        // Moved here: the agent hears first where it is now, or it reaches
        // for the old machine's paths (spike 09).
        let arrived = self
            .d
            .store()
            .get(&crate::moving::arrived_key(&self.id))
            .ok()
            .flatten();
        let text = match arrived.as_deref().and_then(move_notice) {
            Some(notice) => std::borrow::Cow::Owned(format!("{notice}\n\n{text}")),
            None => std::borrow::Cow::Borrowed(text),
        };
        let msg = self.agent.message(&text, steer, attachments);
        let written = self.write(&msg).await;
        if written.is_ok() && arrived.is_some() {
            let _ = self.d.store().del(&crate::moving::arrived_key(&self.id));
        }
        if let Err(e) = written {
            // The agent went between starting and now. A new message gets
            // one more go with a new agent; a steer belonged to the turn.
            let again = if steer {
                Err(e)
            } else {
                eprintln!(
                    "dent-daemon: {}: agent unreachable, starting another: {e:#}",
                    self.id
                );
                self.retire_proc().await;
                // From where the first one started, not the whole conversation.
                self.resume_at = self.resume_at.take().or(point);
                match self.ensure_proc().await {
                    Ok(()) => self.write(&msg).await,
                    Err(e) => Err(e),
                }
            };
            if let Err(e) = again {
                self.log(Body::Notice {
                    text: format!("Couldn't reach {}: {e:#}", self.agent.name()),
                });
                if !steer {
                    self.end_turn(Outcome::Error {
                        message: format!("{e:#}"),
                    });
                }
                return Err(e);
            }
        }
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
        // What was said, from dent's own record, goes in first: the new
        // conversation starts knowing it, as text.
        let events = self.d.store().session_events(&self.id).unwrap_or_default();
        let now = self.last.as_ref().map(|(id, ..)| id.clone());
        let said = recap(&events, RECAP_MAX, now.as_deref());
        self.log(Body::Notice {
            text: if said.is_empty() {
                format!(
                    "{} no longer had this conversation's history, so it started a new one.",
                    self.agent.name()
                )
            } else {
                format!(
                    "{} no longer had this conversation's history, so it started a new one, \
                     given what was said so far as text. Tool calls and their results aren't in it.",
                    self.agent.name()
                )
            },
        });
        match self.last.clone() {
            Some((_, text, attachments)) => {
                let text = if said.is_empty() {
                    text
                } else {
                    format!(
                        "[dent: your earlier history in this conversation was lost. Here is what was \
                         said, as text; tool calls and their results aren't in it.]\n\n{said}\n\n\
                         [dent: the conversation so far ends here. The newest message follows.]\n\n{text}"
                    )
                };
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
        // From where it entered the conversation: a message that waited
        // went in after the reply it waited for.
        let Some(at) = entered_at(&events, &to) else {
            bail!("no such message");
        };
        // Already back there, with nothing said since (a resend after a
        // reconnect or a restart): going again would only put back files
        // changed by hand since.
        let last = events.iter().rev().find_map(|e| match &e.body {
            Body::Rewound { to, files } => Some(Some((to, *files))),
            Body::User { .. } => Some(None),
            _ => None,
        });
        if let Some(Some((was, had))) = last
            && *was == to
            && (had || !files)
        {
            return Ok(());
        }
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
        self.resume_at =
            Some(conversation_at(&events, at).unwrap_or_else(|| resume_point(&events[..at])));
        // The running process holds the whole conversation; the next message
        // starts one that picks up at the rewind.
        self.retire_proc().await;
        self.idle_at = None;
        for (id, _, _) in std::mem::take(&mut self.held) {
            self.log(Body::Unqueued { id, returned: None });
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
            self.controls_gone();
            if let Ok(h) = self.d.holder().await {
                let _ = h.stop(&p.id, Duration::from_secs(1)).await;
            }
        }
    }

    /// The agent process has gone, and the work it ran in the background
    /// with it.
    /// Background work went with the agent: said so, and what it was.
    fn background_gone(&mut self) -> Vec<String> {
        if self.background.is_empty() {
            return Vec::new();
        }
        let what: Vec<String> = self
            .background
            .drain(..)
            .map(|t| t.description)
            .filter(|d| !d.trim().is_empty())
            .collect();
        self.log(Body::Background { tasks: Vec::new() });
        self.log(Body::Notice {
            text: if what.is_empty() {
                "Background work stopped with the agent.".into()
            } else {
                format!(
                    "Background work stopped with the agent: {}.",
                    what.join(", ")
                )
            },
        });
        what
    }

    async fn ensure_proc(&mut self) -> Result<()> {
        if let Some(p) = &self.proc {
            if !p.stopping {
                return Ok(());
            }
            if self.in_turn {
                bail!("the agent is stopping; try again in a moment");
            }
            // Stopping for being idle, its input already closed: let it
            // finish on its own and start another.
            self.retire_proc().await;
        }
        if !std::path::Path::new(&self.cwd).is_dir() {
            // A worktree taken away outside dent comes back on its branch.
            let cwd = std::path::PathBuf::from(&self.cwd);
            let restored = match self.d.workspace_of(&self.id) {
                (Some(branch), Some(root)) => tokio::task::spawn_blocking(move || {
                    crate::workspace::restore(&cwd, std::path::Path::new(&root), &branch)
                })
                .await
                .is_ok_and(|r| r.is_ok()),
                _ => false,
            };
            if !restored {
                bail!("the session's folder {} no longer exists", self.cwd);
            }
            self.log(Body::Notice {
                text: "Its worktree was gone; brought it back on its branch".into(),
            });
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
        // The go-back point is used up only once an agent has started from
        // it: a start that fails leaves it for the next.
        let start = match self.resume_at.clone() {
            Some(Some(at)) => Start::ResumeAt {
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
                Start::Fresh(a)
            }
            None if self.agent_started => Start::Resume(agent_session),
            None => Start::Fresh(agent_session),
        };
        let resumed = !matches!(start, Start::Fresh(_));
        let proc = self.d.store().next_proc(&self.id)?;
        let env = self.d.env();
        // Each new agent also refreshes the PATH for the one after it.
        self.d.refresh_env();
        let asks = self.asks_dir();
        let _ = std::fs::create_dir_all(&asks);
        let argv = self.agent.launch(
            &env,
            &Launch {
                model: self.model.as_deref(),
                permission: self.permission,
                effort: self.effort.as_deref(),
                start: &start,
                asks: &asks,
            },
        );
        let ooms = crate::scopes::oom_kills();
        let argv = crate::scopes::agent(crate::launcher::scrubbed(argv));
        h.spawn(&proc, argv, Some(self.cwd.clone().into()), env.vars.clone())
            .await?;
        self.resume_at = None;
        h.attach(&proc, 0, 0).await?;
        self.proc = Some(Proc {
            id: proc,
            buf: Vec::new(),
            off: 0,
            err: Vec::new(),
            err_off: 0,
            stopping: false,
            resumed,
            ooms,
            warned: false,
            high: false,
            over: None,
        });
        self.start_by = Some(Instant::now() + start_limit());
        self.control(Ctl::Initialize).await?;
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

    /// A control request's id. The daemon's boot is in it, so an answer
    /// to a request an earlier daemon sent never matches a new one.
    fn ctl_id(&mut self) -> String {
        self.next_ctl += 1;
        format!("dent-{}-{}", self.d.boot, self.next_ctl)
    }

    /// A request sent once the current output is handled, for places that
    /// can't wait on the write. One the agent has no words for is dropped.
    fn control_later(&mut self, kind: Ctl) {
        let id = self.ctl_id();
        if let Some(msg) = self.agent.request(&id, &kind.req()) {
            self.controls.insert(id, kind);
            self.outbox.push(msg);
        }
    }

    async fn control(&mut self, kind: Ctl) -> Result<()> {
        let id = self.ctl_id();
        let Some(msg) = self.agent.request(&id, &kind.req()) else {
            bail!("{} can't do that", self.agent.name());
        };
        self.controls.insert(id.clone(), kind);
        let wrote = self.write(&msg).await;
        // Never sent, so never answered: it mustn't apply later either.
        if wrote.is_err() {
            self.controls.remove(&id);
        }
        wrote
    }

    /// Stop always ends the turn: by asking, then by stopping the process,
    /// then by letting go of it.
    async fn interrupt(&mut self) -> Result<()> {
        // Stop means nothing more arrives: held messages come back, those
        // left waiting across a restart (about to go) too, and carrying on
        // by itself.
        self.release_at = None;
        self.call_off_carry_on();
        for (id, _, _) in std::mem::take(&mut self.held) {
            self.log(Body::Unqueued { id, returned: None });
        }
        if !self.in_turn {
            // Stopped before its first turn (its worktree still setting
            // up): it reads stopped, never as a session still starting.
            if self.settled == Status::New {
                self.settled = Status::Interrupted;
            }
            return Ok(());
        }
        self.steered = false;
        if self.proc.is_none() {
            self.stop_asked = true;
            self.end_turn(Outcome::Interrupted { reason: None });
            return Ok(());
        }
        self.stop_asked = true;
        let _ = self.d.store().put(&self.stop_key(), "1");
        // The agent doesn't always end the turn after an interrupt, and one
        // that can't be reached gets its process stopped straight away.
        let asked = self.control(Ctl::Interrupt).await;
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
        if p.tool == crate::plan_tool::SECRET {
            return self.answer_secret(id, answer);
        }
        let msg = self.agent.answer(&id, p, &answer)?;
        self.write(&msg).await?;
        self.asked.remove(&id);
        self.log(Body::Answered { id, answer });
        Ok(())
    }

    /// A desktop's picture of a page: left where dent's tool waits for it
    /// (`look-<key>.json`). The first one counts.
    fn looked(&mut self, key: &str, look: &dent_protocol::Look) -> Result<()> {
        // The key names a file: only what `look_key` makes.
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("not a look");
        }
        let dir = self.asks_dir();
        std::fs::create_dir_all(&dir)?;
        crate::plan_tool::sweep_looks(&dir);
        let file = dir.join(format!("look-{key}.json"));
        if file.exists() {
            return Ok(());
        }
        // Written whole, then moved into place, so the tool never reads half.
        let part = dir.join(format!("look-{key}.part"));
        std::fs::write(&part, serde_json::to_vec(look)?)?;
        std::fs::rename(&part, &file)?;
        Ok(())
    }

    /// Where dent's tools wait for a secret's answer, this session's own.
    fn asks_dir(&self) -> std::path::PathBuf {
        self.d.paths.state.join("asks").join(&self.id)
    }

    /// The person's answer to `ask_secret`: the secret saved where they
    /// chose, and only where it went said back. The value never goes in
    /// the log, nor to the agent.
    fn answer_secret(&mut self, id: String, answer: Answer) -> Result<()> {
        let Some(p) = self.asked.get(&id) else {
            bail!("already answered");
        };
        let name = crate::plan_tool::secret_key(p.input["name"].as_str().unwrap_or_default());
        let (said, logged) = match answer {
            Answer::Secret { value, place } => {
                let file = match place {
                    SecretPlace::Project => std::path::Path::new(&self.cwd).join(".env"),
                    SecretPlace::Session => self
                        .d
                        .paths
                        .state
                        .join("secrets")
                        .join(format!("{}.env", self.id)),
                };
                // A .env git would track is ignored first, so the secret
                // never shows in Changes or goes into a commit.
                let cwd = std::path::Path::new(&self.cwd);
                let ignored_now = place == SecretPlace::Project
                    && crate::secrets::tracked(cwd, &file)
                    && crate::secrets::ignore(cwd, ".env").is_ok();
                crate::secrets::save(&file, &name, &value)?;
                // In a worktree, the main checkout keeps it too, so the
                // next worktree starts with it (they're copied over).
                if place == SecretPlace::Project
                    && let Some(root) = crate::workspace::main_checkout(cwd)
                {
                    let main = root.join(".env");
                    if crate::secrets::tracked(&root, &main) {
                        let _ = crate::secrets::ignore(&root, ".env");
                    }
                    crate::secrets::save(&main, &name, &value)?;
                }
                (
                    serde_json::json!({"saved": file.display().to_string(), "ignored": ignored_now}),
                    Answer::Secret {
                        value: String::new(),
                        place,
                    },
                )
            }
            Answer::Deny { message } => (
                serde_json::json!({"declined": true}),
                Answer::Deny { message },
            ),
            _ => bail!("a secret is given or turned down"),
        };
        let dir = self.asks_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{name}.answer")), said.to_string())?;
        self.asked.remove(&id);
        self.log(Body::Answered { id, answer: logged });
        Ok(())
    }

    async fn timers(&mut self) {
        if self.carry_on_at.is_some_and(|t| t <= Instant::now()) {
            self.carry_on_at = None;
            if let Some((id, text)) = self.carry_on()
                && !self.in_turn
                && self.asked.is_empty()
                && crate::moving::state::fence(self.moving.as_ref()).is_none()
                && self.count_carry_on()
            {
                let _ = self.send(id, text, false, Vec::new()).await;
            }
            let _ = self.d.store().del(&self.carry_on_key());
        }
        let now = Instant::now();
        if self.release_at.is_some_and(|t| t <= now) {
            self.release_at = None;
            if !self.in_turn
                && self.asked.is_empty()
                && !self.held.is_empty()
                && !self.d.setting_up(&self.id)
            {
                self.deliver_held().await;
            }
        }
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
                let mut message = format!("{} didn't start within a minute", self.agent.name());
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
            if self.agent.has_sheet(&command) && self.proc.is_some() {
                let _ = self.control(Ctl::Refresh { command }).await;
            }
        }
        // A used-up limit has reset: carry on where the turn stopped. Not
        // while it's moving or moved: that would run it here too.
        if self
            .wake
            .is_some_and(|at| at <= crate::daemon::now_ms() / 1000)
            && crate::moving::state::fence(self.moving.as_ref()).is_none()
        {
            self.wake = None;
            self.log(Body::Resume { at: None });
            let _ = self
                .send(
                    crate::daemon::new_id(),
                    RESUME_TEXT.into(),
                    false,
                    Vec::new(),
                )
                .await;
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
            match self.agent.parse(&line[..line.len() - 1]) {
                Ok(ev) => self.inbound(ev, end),
                Err(_) if line.iter().all(u8::is_ascii_whitespace) => {}
                Err(e) => eprintln!("dent-daemon: {}: unreadable agent line: {e}", self.id),
            }
        }
        self.update_status();
    }

    /// One line from the agent, which ended at byte `end` of its output.
    fn inbound(&mut self, ev: Event, end: u64) {
        match ev {
            Event::TurnStarted {
                agent_session,
                version,
                permission,
                mcp,
            } => {
                // An MCP server that won't connect, or wants you to sign
                // in, says so once in the chat.
                for (name, status) in mcp {
                    let said = match status.as_str() {
                        "failed" => format!("The {name} MCP server couldn't connect."),
                        "needs-auth" => format!("The {name} MCP server needs you to sign in."),
                        _ => continue,
                    };
                    if self.mcp_told.insert(format!("{name}/{status}")) {
                        self.log_from(Body::Notice { text: said }, end);
                    }
                }
                // A steer has been taken in by the time a turn starts: an
                // abort from here on is a real one.
                self.steered = false;
                if self.agent_session.as_deref() != Some(agent_session.as_str())
                    && !agent_session.is_empty()
                {
                    self.agent_session = Some(agent_session.clone());
                    self.log_from(
                        Body::Agent {
                            agent_session: Some(agent_session),
                            version: Some(version),
                        },
                        end,
                    );
                }
                if let Some(p) = permission
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
            Event::MessageStart { parent: None } => {
                // A reply is coming: any retry worked.
                self.preparing = None;
                self.d.set_activity(&self.id, None);
                self.d.clear_partial(&self.id);
            }
            Event::TextDelta { parent: None, text } => {
                self.thought_ends();
                self.d.delta(&self.id, &text, false)
            }
            // What it's thinking shows on the working line, a finished
            // sentence at a time.
            Event::ThinkingDelta { parent: None, text } => {
                self.thinking.push_str(&text);
                if let Some(latest) = log::latest_sentence(&self.thinking)
                    && self.thought.as_deref() != Some(latest.as_str())
                {
                    self.thought = Some(latest.clone());
                    self.d
                        .set_activity(&self.id, Some(Activity::Thinking { latest }));
                }
            }
            // Writing a long file streams nothing else for minutes: say so.
            Event::ToolStart { parent: None, name } => {
                self.thought_ends();
                self.d.set_activity(
                    &self.id,
                    Some(Activity::Preparing {
                        tool: name.clone(),
                        what: None,
                    }),
                );
                self.preparing = Some((name, Some(String::new())));
            }
            Event::InputDelta { parent: None, json } => {
                if let Some((tool, input)) = &mut self.preparing
                    && let Some(head) = input
                {
                    head.push_str(&json);
                    // The target comes first, if at all.
                    let what = self.agent.target(tool, head);
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
            // The turn's end says the same thing, as an error.
            Event::Assistant {
                synthetic: true, ..
            } => {}
            Event::Assistant {
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
                if parent.is_none() {
                    self.said = true;
                    self.thought_ends();
                    if self.preparing.take().is_some() {
                        self.d.set_activity(&self.id, None);
                    }
                }
                // The agent asking for a secret waits on dent's own card.
                let secrets: Vec<(String, Value)> = blocks
                    .iter()
                    .filter_map(|b| match b {
                        AssistantBlock::ToolUse { id, name, input }
                            if name == crate::plan_tool::SECRET =>
                        {
                            Some((id.clone(), input.clone()))
                        }
                        _ => None,
                    })
                    .collect();
                // A server tool's result comes with its call.
                for b in log::assistant_bodies(parent, uuid, blocks, results, &mut self.reports) {
                    self.log_from(b, end);
                }
                for (id, input) in secrets {
                    self.log_from(
                        Body::Asked {
                            id: id.clone(),
                            tool: crate::plan_tool::SECRET.into(),
                            input: input.to_string(),
                            description: None,
                            keeps: false,
                            suggestions: None,
                        },
                        end,
                    );
                    self.asked.insert(
                        id,
                        Asked {
                            tool: crate::plan_tool::SECRET.into(),
                            input,
                            suggestions: Value::Null,
                            dialog: None,
                        },
                    );
                }
            }
            Event::ToolResults { parent, results } => {
                let media = self.d.paths.state.join("attachments").join(&self.id);
                for b in log::result_bodies(parent, results, &mut self.reports, Some(&media)) {
                    self.log_from(b, end);
                }
            }
            Event::Limits(windows) => self.log_from(Body::Limits { windows }, end),
            Event::TaskStarted { task, call } => {
                if let Some(call) = call {
                    self.task_calls.insert(task, call);
                }
            }
            Event::BackgroundTasks(tasks) => {
                self.background = log::tasks(&tasks, &self.task_calls);
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
            Event::Approval {
                id,
                tool,
                input,
                description,
                keeps,
                suggestions,
            } => {
                self.log_from(
                    Body::Asked {
                        id: id.clone(),
                        tool: tool.clone(),
                        input: input.to_string(),
                        description,
                        keeps,
                        suggestions: (!suggestions.is_null()).then(|| suggestions.to_string()),
                    },
                    end,
                );
                self.asked.insert(
                    id,
                    Asked {
                        tool,
                        input,
                        suggestions,
                        dialog: None,
                    },
                );
            }
            Event::Response { id, ok, body } => {
                let Some(ctl) = self.controls.remove(&id) else {
                    return;
                };
                let models = self.d.models(self.agent.id());
                let reply = self.agent.reply(&ctl.req(), ok, &body, &models);
                self.replied(ctl, reply, end);
            }
            Event::Unanswerable { id, what, server } => {
                if what == "elicitation" {
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
                self.outbox.push(self.agent.refuse(&id, &what));
            }
            Event::Cancelled { id } => {
                if self.asked.remove(&id).is_some() {
                    self.log_from(
                        Body::Answered {
                            id,
                            answer: Answer::Expired,
                        },
                        end,
                    );
                }
            }
            Event::TurnEnded {
                aborted,
                error,
                text,
            } => {
                if error
                    && text.as_deref().is_some_and(|t| self.agent.forgot(t))
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
                } else if error {
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
                self.control_later(Ctl::ContextUsage);
                self.end_turn_at(outcome, Some(end));
            }
            Event::Compacted {
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
            Event::Compacting { on: true, .. } => self.d.set_activity(
                &self.id,
                Some(Activity::Compacting {
                    since: crate::daemon::now_ms(),
                }),
            ),
            Event::Compacting { on: false, failed } => {
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
            Event::Retrying {
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
            Event::CommandsChanged(commands) => {
                self.d.commands_changed(&self.id, commands);
            }
            Event::LocalCommand {
                name,
                args,
                text,
                outcome,
                view,
            } => {
                // A refusal doesn't say which command; it's the one sent.
                let sent = self.last.as_ref().and_then(|(_, t, _)| {
                    let n = dent_protocol::command_name(t)?;
                    Some((n.to_string(), t[1 + n.len()..].trim().to_string()))
                });
                let (name, args) = match (name, sent) {
                    (Some(n), _) => (n, args),
                    (None, Some((n, a))) => (n, a),
                    (None, None) => (String::new(), args),
                };
                self.said = true;
                // A command whose answer is a screen gets its data as a
                // sheet, even where the agent refused to draw it.
                let sheet = self.agent.has_sheet(&name)
                    && args.is_empty()
                    && !matches!(outcome, CommandOutcome::Unknown { .. });
                if sheet {
                    self.control_later(Ctl::Sheet {
                        command: name,
                        args,
                        text,
                        outcome,
                    });
                } else {
                    self.log_from(
                        Body::Command {
                            name,
                            args,
                            text,
                            outcome,
                            view,
                        },
                        end,
                    );
                }
                // It may have changed a setting a command offers.
                self.control_later(Ctl::Settings);
            }
            Event::Notice(text) => self.log_from(Body::Notice { text }, end),
            Event::Question { id, input, options } => {
                self.log_from(
                    Body::Asked {
                        id: id.clone(),
                        tool: "AskUserQuestion".into(),
                        input: input.to_string(),
                        description: None,
                        keeps: false,
                        suggestions: None,
                    },
                    end,
                );
                self.asked.insert(
                    id,
                    Asked {
                        tool: "AskUserQuestion".into(),
                        input,
                        suggestions: Value::Null,
                        dialog: Some(options),
                    },
                );
            }
            Event::MessageStart { .. }
            | Event::TextDelta { .. }
            | Event::ThinkingDelta { .. }
            | Event::ToolStart { .. }
            | Event::InputDelta { .. }
            | Event::Other => {}
        }
    }

    /// What the agent answered to request `ctl`.
    fn replied(&mut self, ctl: Ctl, reply: Reply, end: u64) {
        match (ctl, reply) {
            (Ctl::Initialize, Reply::Refused(why)) => {
                self.start_by = None;
                self.log_from(
                    Body::Notice {
                        text: format!("{} refused: {why}", self.agent.name()),
                    },
                    end,
                );
            }
            (
                Ctl::Initialize,
                Reply::Initialized {
                    models,
                    commands,
                    styles,
                    style,
                },
            ) => {
                self.start_by = None;
                self.d.set_models(self.agent.id(), models);
                self.d.set_commands(&self.id, commands);
                self.d.catalog(&self.id, |c| {
                    c.styles = styles;
                    c.style = style;
                });
                self.control_later(Ctl::Settings);
                // A resumed conversation is as full as it was: say so
                // before the first reply rather than after it.
                if self.agent_started {
                    self.control_later(Ctl::ContextUsage);
                }
            }
            (
                Ctl::ContextUsage,
                Reply::Context {
                    used,
                    max,
                    compacts_at,
                    parts,
                },
            ) => {
                self.context = Some(used);
                self.max_context = max.or(self.max_context);
                self.log_from(
                    Body::Context {
                        used,
                        max: self.max_context,
                        compacts_at,
                        parts,
                    },
                    end,
                );
            }
            (c @ (Ctl::Model(_) | Ctl::Permission(_)), Reply::Done) => {
                self.keep(c, Some(end));
            }
            (Ctl::Settings, Reply::Settings { effective, applied }) => {
                self.d.catalog(&self.id, |c| {
                    c.effective = effective;
                    c.applied = applied;
                });
            }
            (
                Ctl::Sheet {
                    command,
                    args,
                    text,
                    outcome,
                },
                reply,
            ) => {
                let sections = match reply {
                    Reply::Sheet(s) => s,
                    _ => None,
                };
                let body = match sections {
                    Some(sections) => {
                        self.sheets.insert(command.clone(), sections.clone());
                        Body::Command {
                            text: match outcome {
                                // Its own words, when it had some.
                                CommandOutcome::Done if !text.trim().is_empty() => text,
                                _ => log::sheet_text(&sections),
                            },
                            name: command,
                            args,
                            outcome: CommandOutcome::Done,
                            view: Some(View::Sheet { sections }),
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
            (Ctl::Refresh { command }, Reply::Sheet(Some(sections))) => {
                if self.sheets.get(&command) != Some(&sections) {
                    self.sheets.insert(command.clone(), sections.clone());
                    self.d.tell(Out::Sheet {
                        session: self.id.clone(),
                        command,
                        sections,
                    });
                }
            }
            (Ctl::Act { command, act }, reply) => {
                let (url, error) = match reply {
                    Reply::Acted { url } => (url, None),
                    Reply::Refused(why) => (None, Some(why)),
                    _ => (None, None),
                };
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
                    error,
                });
                if self.agent.has_sheet(&command) {
                    self.control_later(Ctl::Refresh { command });
                }
            }
            (
                Ctl::Interrupt
                | Ctl::ContextUsage
                | Ctl::Settings
                | Ctl::Refresh { .. }
                | Ctl::StopTask(_)
                | Ctl::Initialize,
                _,
            ) => {}
            (Ctl::Model(_) | Ctl::Permission(_), reply) => {
                if let Reply::Refused(why) = reply {
                    self.log_from(
                        Body::Notice {
                            text: format!("{} refused: {why}", self.agent.name()),
                        },
                        end,
                    );
                }
            }
        }
    }

    /// Thinking is over (an answer, a call, the whole message): its line
    /// goes from the working line.
    fn thought_ends(&mut self) {
        self.thinking.clear();
        if self.thought.take().is_some() {
            self.d.set_activity(&self.id, None);
        }
    }

    fn end_turn(&mut self, outcome: Outcome) {
        self.end_turn_at(outcome, None);
    }

    fn end_turn_at(&mut self, outcome: Outcome, end: Option<u64>) {
        self.thought_ends();
        if self.preparing.take().is_some() {
            self.d.set_activity(&self.id, None);
        }
        self.stop_at = None;
        self.start_by = None;
        let stopped = std::mem::take(&mut self.stop_asked);
        if stopped {
            let _ = self.d.store().del(&self.stop_key());
        }
        // You stopped it before anything came back: as in Claude Code, the
        // message goes back to you to change and send again, and leaves the
        // conversation, the agent's too (its next process starts from the
        // reply before). Not while background work runs in this process,
        // which a new one would end.
        let withdrawn = (stopped
            && self.in_turn
            && matches!(outcome, Outcome::Interrupted { .. })
            && self.background.is_empty()
            && self.d.partial(&self.id).is_none())
        .then(|| self.d.store().session_events(&self.id).ok())
        .flatten()
        .and_then(|events| unanswered(&events));
        if let Some((ids, at)) = withdrawn {
            self.log(Body::Withdrawn { ids });
            // Nothing left of a first message: it reads stopped, never as
            // a session still starting (clients show New as working).
            if self.settled == Status::New {
                self.settled = Status::Interrupted;
            }
            self.resume_at = Some(at);
            self.last = None;
            self.in_turn = false;
            for id in std::mem::take(&mut self.asked).into_keys() {
                self.log(Body::Answered {
                    id,
                    answer: Answer::Expired,
                });
            }
            self.d.clear_partial(&self.id);
            // The session is as it was before: its last turn's outcome
            // stands, and nothing new is titled or noted.
            let body = Body::TurnEnded { outcome };
            match end {
                Some(off) => self.log_from(body, off),
                None => self.log(body),
            }
            if self.proc.is_some() {
                self.idle_at = Some(Instant::now() + idle());
            }
            self.update_status();
            return;
        }
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
        // After its first turn, a session is titled from what it's about;
        // after any, a branch the agent switched to is noted.
        if was {
            let (d, id, cwd) = (self.d.clone(), self.id.clone(), self.cwd.clone());
            tokio::spawn(async move {
                d.note_workspace(&id, &cwd).await;
                d.note_pr(&id, &cwd).await;
                if let Err(e) = d.title_session(&id, false).await {
                    eprintln!("dent-daemon: {id}: couldn't title it: {e:#}");
                }
            });
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
        self.controls_gone();
        if let Ok(h) = self.d.holder().await {
            let _ = h.release(proc).await;
        }
        self.idle_at = None;
        self.start_by = None;
        if p.stopping && !self.in_turn {
            return;
        }
        let tail = String::from_utf8_lossy(&p.err).trim().to_string();
        if self.in_turn && p.resumed && !p.stopping && self.agent.forgot(&tail) {
            self.start_over().await;
            return;
        }
        // Killed, and the kernel killed something for memory meanwhile.
        let starved = signal == Some(libc::SIGKILL)
            && p.ooms
                .zip(crate::scopes::oom_kills())
                .is_some_and(|(was, now)| now > was);
        let why = match (code, signal) {
            _ if p.over.is_some() => format!(
                "dent stopped it: it used {} of memory, past this machine's limit of {}",
                crate::memory::words(p.over.unwrap_or(0)),
                crate::memory::words(crate::memory::limits().stop)
            ),
            _ if starved => "it ran out of memory".into(),
            (_, Some(s)) => format!("signal {s}"),
            (Some(c), _) => format!("exit code {c}"),
            _ => "no exit status".into(),
        };
        let last = tail.lines().last().unwrap_or_default().to_string();
        if code == Some(127) {
            // `env` couldn't find it: maybe installed since dent started.
            self.d.refresh_env();
        }
        let name = self.agent.name();
        if self.in_turn {
            // Stopped for memory, then by you too: still says why.
            let reason = if p.stopping && p.over.is_none() {
                None
            } else if code == Some(127) {
                Some(format!(
                    "Couldn't find {name} (`{}`). Install it, then send again.",
                    self.agent.command()
                ))
            } else if let Some(used) = p.over {
                Some(format!(
                    "dent stopped {name}: it used {} of memory, past this machine's limit of {}.",
                    crate::memory::words(used),
                    crate::memory::words(crate::memory::limits().stop)
                ))
            } else if last.is_empty() {
                Some(format!("{name} stopped unexpectedly ({why})"))
            } else {
                Some(format!("{name} stopped unexpectedly ({why}): {last}"))
            };
            self.end_turn(Outcome::Interrupted { reason });
        } else if p.over.is_some() {
            self.log(Body::Notice {
                text: format!("{name} exited ({why})"),
            });
        } else if code != Some(0) && !last.is_empty() {
            self.log(Body::Notice {
                text: format!("{name} exited ({why}): {last}"),
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
/// Where message `id` entered the conversation: where it was handed to the
/// agent if it waited first, else where it was sent.
pub(crate) fn entered_at(events: &[dent_protocol::Event], id: &str) -> Option<usize> {
    events
        .iter()
        .position(|e| matches!(&e.body, Body::Delivered { id: d } if d == id))
        .or_else(|| {
            events
                .iter()
                .position(|e| matches!(&e.body, Body::User { id: u, .. } if u == id))
        })
}

/// Where the turn running (or last run) at event `at` began: just after
/// the last turn, rewind, fork or take-back before it.
pub(crate) fn turn_start(events: &[dent_protocol::Event], at: usize) -> usize {
    events[..at.min(events.len())]
        .iter()
        .rposition(|e| {
            matches!(
                e.body,
                Body::TurnEnded { .. }
                    | Body::Rewound { .. }
                    | Body::Forked { .. }
                    | Body::Withdrawn { .. }
            )
        })
        .map_or(0, |b| b + 1)
}

/// Where the agent's conversation stood just before event `at`: the reply
/// it carries on from, `Some(None)` for nothing said yet. `None` when the
/// log can't say exactly: a reply cut short that has no id, or a message
/// that never got one, which going back to the reply before would lose.
pub(crate) fn conversation_at(
    events: &[dent_protocol::Event],
    at: usize,
) -> Option<Option<String>> {
    let mut i = at.min(events.len());
    while i > 0 {
        i -= 1;
        match &events[i].body {
            Body::Assistant {
                parent: None, uuid, ..
            } => return uuid.clone().map(Some),
            Body::User { queued: false, .. } | Body::Delivered { .. } => return None,
            // Gone back to before message `to`: the conversation is as it
            // was then.
            Body::Rewound { to, .. } => i = entered_at(&events[..i], to)?,
            Body::Forked { upto, .. } => return Some(upto.clone()),
            // A turn taken back left nothing in the conversation.
            Body::Withdrawn { .. } => i = turn_start(events, i),
            _ => {}
        }
    }
    Some(None)
}

/// Where each turn taken back lies in `events`: from its start to its end
/// (the [`Body::TurnEnded`] after the [`Body::Withdrawn`]), both included.
pub(crate) fn withdrawn_turns(
    events: &[dent_protocol::Event],
) -> Vec<std::ops::RangeInclusive<usize>> {
    events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.body, Body::Withdrawn { .. }))
        .map(|(w, _)| {
            let end = events[w..]
                .iter()
                .position(|e| matches!(e.body, Body::TurnEnded { .. }))
                .map_or(w, |n| w + n);
            turn_start(events, w)..=end
        })
        .collect()
}

/// Every message taken back ([`Body::Withdrawn`]) in `events`.
pub(crate) fn withdrawn_ids(events: &[dent_protocol::Event]) -> HashSet<String> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            Body::Withdrawn { ids } => Some(ids.iter().cloned()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// What a turn logs that isn't anything coming back: the messages
/// themselves, bookkeeping, and thinking (never shown as a reply).
/// Anything else, a kind added later too, is a reply.
fn nothing_back(body: &Body) -> bool {
    match body {
        Body::Assistant { blocks, .. } => blocks
            .iter()
            .all(|b| matches!(b, dent_protocol::Block::Thinking { .. })),
        Body::Created { .. }
        | Body::User { .. }
        | Body::Delivered { .. }
        | Body::Unqueued { .. }
        | Body::Requeued { .. }
        | Body::Checkpoint { .. }
        | Body::TurnStarted
        // What runs in the background changing (finishing, say); work
        // still running keeps the message (see `unanswered`).
        | Body::Background { .. }
        | Body::Context { .. }
        | Body::Limits { .. }
        | Body::Status { .. }
        | Body::Notice { .. }
        | Body::Agent { .. }
        | Body::Settings { .. }
        | Body::Seen { .. }
        | Body::Resume { .. }
        | Body::PullRequest { .. }
        | Body::Workspace { .. }
        | Body::Closed
        | Body::Reopened
        | Body::Aside { .. }
        | Body::AsideReply { .. } => true,
        _ => false,
    }
}

/// A turn you stopped before anything came back: its messages, in the
/// order sent, and where the agent's conversation stood before them, to go
/// back to. `None` when something came back, a message is a command (it
/// may have done its work already), the place before can't be known, or
/// background work is running in the agent, which going back (a new
/// process) would end.
/// Longest recap handed to a conversation that lost its history, in bytes:
/// the newest of it is kept.
const RECAP_MAX: usize = 24 * 1024;

/// What was said in a session, as text, newest kept within `max` bytes:
/// for an agent that lost the conversation (a transcript gone, one that
/// didn't move). As the agent had it: turns taken back or rewound away
/// aren't in it, a held message is where it was delivered, and what dent
/// sent by itself (carrying on) isn't the person's. `now` (the message
/// being sent with it) isn't in it either.
fn recap(events: &[dent_protocol::Event], max: usize, now: Option<&str>) -> String {
    let gone = withdrawn_ids(events);
    let taken_back = withdrawn_turns(events);
    let by_dent = |t: &str| dent_protocol::by_dent(t).is_some();
    // (message id, for rewinds; what was said)
    let mut turns: Vec<(Option<String>, String)> = Vec::new();
    let mut held: HashMap<String, String> = HashMap::new();
    for (i, e) in events.iter().enumerate() {
        if taken_back.iter().any(|r| r.contains(&i)) {
            continue;
        }
        match &e.body {
            Body::User {
                id, text, queued, ..
            } => {
                if gone.contains(id)
                    || Some(id.as_str()) == now
                    || by_dent(text)
                    || text.trim().is_empty()
                {
                    continue;
                }
                if *queued {
                    held.insert(id.clone(), text.clone());
                } else {
                    turns.push((
                        Some(id.clone()),
                        format!("The person said:\n{}", text.trim()),
                    ));
                }
            }
            Body::Delivered { id } => {
                if let Some(text) = held.remove(id)
                    && Some(id.as_str()) != now
                {
                    turns.push((
                        Some(id.clone()),
                        format!("The person said:\n{}", text.trim()),
                    ));
                }
            }
            Body::Unqueued { id, .. } => {
                held.remove(id);
            }
            // Back to just before message `to`: what came after is gone.
            Body::Rewound { to, .. } => {
                if let Some(at) = turns.iter().position(|(id, _)| id.as_deref() == Some(to)) {
                    turns.truncate(at);
                }
            }
            Body::Assistant {
                parent: None,
                blocks,
                ..
            } => {
                let words: String = blocks
                    .iter()
                    .filter_map(|b| match b {
                        dent_protocol::Block::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !words.trim().is_empty() {
                    turns.push((None, format!("You replied:\n{}", words.trim())));
                }
            }
            _ => {}
        }
    }
    let mut kept: Vec<String> = Vec::new();
    let mut size = 0;
    for (_, t) in turns.iter().rev() {
        if size + t.len() > max {
            // The newest alone is too long: its start, cut where a
            // character ends, rather than nothing at all.
            if kept.is_empty() {
                let mut end = max.saturating_sub(16);
                while !t.is_char_boundary(end) {
                    end -= 1;
                }
                kept.push(format!("{} (cut short)", &t[..end]));
            }
            break;
        }
        size += t.len() + 2;
        kept.push(t.clone());
    }
    let left_out = kept.len() < turns.len();
    kept.reverse();
    let mut out = kept.join("\n\n");
    if left_out && !out.is_empty() {
        out.insert_str(0, "(Earlier parts left out.)\n\n");
    }
    out
}

fn unanswered(events: &[dent_protocol::Event]) -> Option<(Vec<String>, Option<String>)> {
    let working = events.iter().rev().find_map(|e| match &e.body {
        Body::Background { tasks } => Some(!tasks.is_empty()),
        _ => None,
    });
    if working == Some(true) {
        return None;
    }
    let from = turn_start(events, events.len());
    let text_of = |id: &str| {
        events.iter().find_map(|e| match &e.body {
            Body::User { id: i, text, .. } if i == id => Some(text.as_str()),
            _ => None,
        })
    };
    let mut ids: Vec<String> = Vec::new();
    for e in &events[from..] {
        if !nothing_back(&e.body) {
            return None;
        }
        match &e.body {
            Body::User {
                id, queued: false, ..
            }
            | Body::Delivered { id } => {
                if text_of(id).is_none_or(|t| dent_protocol::command_name(t).is_some()) {
                    return None;
                }
                ids.push(id.clone());
            }
            Body::Unqueued { id, .. } => ids.retain(|i| i != id),
            _ => {}
        }
    }
    if ids.is_empty() {
        return None;
    }
    Some((ids, conversation_at(events, from)?))
}

pub(crate) fn resume_point(events: &[dent_protocol::Event]) -> Option<String> {
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

pub use dent_protocol::RESUME_TEXT;

/// What a move fences: anything that starts agent work or changes the
/// session's folder or how its agent runs. Answers to the agent's own
/// questions, stopping, reading and naming still work.
fn fenced_by_moves(op: &Op) -> bool {
    matches!(
        op,
        Op::Send { .. }
            | Op::Requeue { .. }
            | Op::Rewind { .. }
            | Op::SetModel(_)
            | Op::SetPermission(_)
            | Op::SetEffort(_)
            | Op::Act { .. }
            | Op::Restart
            | Op::Resume(_)
            | Op::Reopen
            | Op::SetUp
    )
}
/// What a session that moved here says to its agent before your first
/// message: where it was, where it is, and that the old place isn't here.
fn move_notice(arrived: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arrived).ok()?;
    let (from, old, new) = (
        v["from_machine"].as_str()?,
        v["from_cwd"].as_str()?,
        v["to_cwd"].as_str()?,
    );
    Some(format!(
        "[dent: this session moved to another machine. It worked in {old} on {from}; it works in {new} here, with the same files and changes. {old} isn't on this machine: use paths under {new} from now on.]"
    ))
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

#[cfg(test)]
mod withdraw_tests {
    use super::*;
    use dent_protocol::{Block, Event};

    #[test]
    fn a_recap_is_what_was_said_newest_first_kept_without_the_message_now() {
        let reply = |t: &str| Body::Assistant {
            parent: None,
            blocks: vec![dent_protocol::Block::Text { text: t.into() }],
            uuid: None,
        };
        let events = log(vec![
            user("1", "add a login page"),
            reply("Done: login.tsx"),
            user("2", "now the tests"),
            reply("Added 4 tests."),
            user("3", "what next?"),
        ]);
        let all = recap(&events, 10_000, Some("3"));
        assert!(
            all.starts_with("The person said:\nadd a login page"),
            "{all}"
        );
        assert!(all.ends_with("You replied:\nAdded 4 tests."), "{all}");
        assert!(
            !all.contains("what next?"),
            "the message now goes on its own"
        );
        // Too much: the newest kept, and said that some was left out.
        let some = recap(&events, 60, Some("3"));
        assert!(some.starts_with("(Earlier parts left out.)"), "{some}");
        assert!(
            some.contains("Added 4 tests.") && !some.contains("login page"),
            "{some}"
        );
        assert_eq!(recap(&log(vec![user("1", "hi")]), 100, Some("1")), "");
        // One turn alone too long: its start, not nothing.
        let long = log(vec![user("1", &"é".repeat(100)), user("2", "now")]);
        let cut = recap(&long, 50, Some("2"));
        assert!(cut.ends_with("(cut short)") && cut.contains('é'), "{cut}");
        // Taken back, rewound away, sent by dent, or never delivered: not
        // what the agent had.
        let mut e = vec![
            user("1", "keep this"),
            reply("kept reply"),
            Body::TurnEnded {
                outcome: Outcome::Done,
            },
            user("2", "taken back"),
            Body::Withdrawn {
                ids: vec!["2".into()],
            },
            Body::TurnEnded {
                outcome: Outcome::Interrupted { reason: None },
            },
            user("3", "rewound away"),
            reply("gone reply"),
            Body::Rewound {
                to: "3".into(),
                files: false,
            },
            user("4", RESUME_TEXT),
            reply("carried on"),
        ];
        e.push(Body::User {
            id: "5".into(),
            text: "still waiting".into(),
            queued: true,
            steer: false,
            attachments: Vec::new(),
        });
        e.push(user("6", "now"));
        let r = recap(&log(e), 10_000, Some("6"));
        for missing in [
            "taken back",
            "rewound away",
            "gone reply",
            "usage limit",
            "still waiting",
            "now",
        ] {
            assert!(!r.contains(missing), "{missing} in {r}");
        }
        for kept in ["keep this", "kept reply", "carried on"] {
            assert!(r.contains(kept), "{kept} not in {r}");
        }
    }

    fn log(bodies: Vec<Body>) -> Vec<Event> {
        bodies
            .into_iter()
            .enumerate()
            .map(|(i, body)| Event {
                seq: i as u64 + 1,
                session: "s".into(),
                at: 0,
                body,
            })
            .collect()
    }
    fn user(id: &str, text: &str) -> Body {
        Body::User {
            id: id.into(),
            text: text.into(),
            queued: false,
            steer: false,
            attachments: Vec::new(),
        }
    }
    fn queued(id: &str) -> Body {
        Body::User {
            id: id.into(),
            text: "later".into(),
            queued: true,
            steer: false,
            attachments: Vec::new(),
        }
    }
    fn said(uuid: Option<&str>, block: Block) -> Body {
        Body::Assistant {
            parent: None,
            blocks: vec![block],
            uuid: uuid.map(str::to_string),
        }
    }
    fn text(t: &str) -> Block {
        Block::Text { text: t.into() }
    }
    fn ended() -> Body {
        Body::TurnEnded {
            outcome: Outcome::Done,
        }
    }
    /// A first turn answered with reply `r1`.
    fn answered() -> Vec<Body> {
        vec![
            user("a", "hello"),
            Body::TurnStarted,
            said(Some("r1"), text("hi")),
            ended(),
        ]
    }
    fn with(mut a: Vec<Body>, b: Vec<Body>) -> Vec<Event> {
        a.extend(b);
        log(a)
    }
    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn nothing_back_goes_back_to_the_reply_before() {
        let e = with(answered(), vec![user("b", "go"), Body::TurnStarted]);
        assert_eq!(unanswered(&e), Some((ids(&["b"]), Some("r1".into()))));
        // Only thinking came: still nothing back.
        let e = with(
            answered(),
            vec![
                user("b", "go"),
                Body::TurnStarted,
                said(Some("t"), Block::Thinking { text: "hmm".into() }),
            ],
        );
        assert_eq!(unanswered(&e), Some((ids(&["b"]), Some("r1".into()))));
        // Before it had even started.
        let e = with(answered(), vec![user("b", "go")]);
        assert_eq!(unanswered(&e), Some((ids(&["b"]), Some("r1".into()))));
    }

    #[test]
    fn a_first_message_goes_back_to_nothing_said() {
        let e = log(vec![
            Body::Created {
                agent: "claude".into(),
                cwd: "/w".into(),
                model: None,
                permission: Permission::Full,
                title: String::new(),
            },
            user("a", "go"),
            Body::TurnStarted,
        ]);
        assert_eq!(unanswered(&e), Some((ids(&["a"]), None)));
    }

    #[test]
    fn anything_that_came_back_keeps_the_message() {
        let tool = Block::Tool {
            id: "t".into(),
            name: "Read".into(),
            input: "{}".into(),
        };
        let cases = vec![
            said(Some("r2"), text("Sure")),
            said(Some("r2"), tool),
            Body::ToolResult {
                tool: "t".into(),
                error: false,
                output: String::new(),
                parent: None,
                images: Vec::new(),
            },
        ];
        for c in cases {
            let e = with(
                answered(),
                vec![user("b", "go"), Body::TurnStarted, c.clone()],
            );
            assert_eq!(unanswered(&e), None, "{c:?}");
        }
    }

    #[test]
    fn background_work_running_keeps_the_message() {
        let task = |id: &str| dent_protocol::Task {
            id: id.into(),
            description: "dev server".into(),
            kind: "local_bash".into(),
            call: None,
        };
        let mut a = answered();
        a.insert(
            3,
            Body::Background {
                tasks: vec![task("t")],
            },
        );
        let e = with(a.clone(), vec![user("b", "go"), Body::TurnStarted]);
        assert_eq!(unanswered(&e), None);
        // Once it has finished, the message goes back.
        a.push(Body::Background { tasks: Vec::new() });
        let e = with(a, vec![user("b", "go"), Body::TurnStarted]);
        assert_eq!(unanswered(&e).map(|u| u.0), Some(ids(&["b"])));
    }

    #[test]
    fn a_command_stays_it_may_have_done_its_work() {
        let e = with(answered(), vec![user("b", "/compact"), Body::TurnStarted]);
        assert_eq!(unanswered(&e), None);
        // Words that merely start with a slash, over lines, are a message.
        let e = with(answered(), vec![user("b", "/tmp is full\nwhy?")]);
        assert_eq!(unanswered(&e).map(|u| u.0), Some(ids(&["b"])));
    }

    #[test]
    fn a_waiting_message_handed_over_goes_back_and_one_still_waiting_does_not() {
        // "b" waited through the first turn and went when it ended; "c"
        // was taken out of the queue; "d" still waits.
        let e = log(vec![
            user("a", "hello"),
            queued("b"),
            queued("c"),
            Body::TurnStarted,
            said(Some("r1"), text("hi")),
            ended(),
            Body::Delivered { id: "b".into() },
            queued("d"),
            Body::Unqueued {
                id: "c".into(),
                returned: None,
            },
            Body::TurnStarted,
        ]);
        assert_eq!(unanswered(&e), Some((ids(&["b"]), Some("r1".into()))));
    }

    #[test]
    fn a_steer_with_no_reply_either_goes_back_with_it() {
        let e = with(
            answered(),
            vec![user("b", "go"), Body::TurnStarted, user("c", "and also")],
        );
        assert_eq!(unanswered(&e), Some((ids(&["b", "c"]), Some("r1".into()))));
    }

    #[test]
    fn no_exact_place_to_go_back_to_keeps_the_message() {
        // The reply before was cut short and has no id.
        let e = log(vec![
            user("a", "hello"),
            Body::TurnStarted,
            said(None, text("hi, so")),
            Body::TurnEnded {
                outcome: Outcome::Interrupted { reason: None },
            },
            user("b", "go"),
        ]);
        assert_eq!(unanswered(&e), None);
        // The message before never got a reply at all.
        let e = log(vec![
            user("a", "hello"),
            Body::TurnEnded {
                outcome: Outcome::Error {
                    message: "x".into(),
                },
            },
            user("b", "go"),
        ]);
        assert_eq!(unanswered(&e), None);
    }

    #[test]
    fn going_back_accounts_for_rewinds_forks_and_earlier_take_backs() {
        // Rewound to before "b": the conversation is as it was after r1.
        let mut a = answered();
        a.extend([
            user("b", "two"),
            Body::TurnStarted,
            said(Some("r2"), text("two")),
            ended(),
            Body::Rewound {
                to: "b".into(),
                files: false,
            },
            user("c", "go"),
        ]);
        assert_eq!(
            unanswered(&log(a.clone())),
            Some((ids(&["c"]), Some("r1".into())))
        );
        // A rewind to before anything was said: nothing.
        let e = with(
            answered(),
            vec![
                Body::Rewound {
                    to: "a".into(),
                    files: false,
                },
                user("c", "go"),
            ],
        );
        assert_eq!(unanswered(&e), Some((ids(&["c"]), None)));
        // A fork carries on from the reply it was cut at.
        let e = log(vec![
            Body::Forked {
                from: "p".into(),
                upto: Some("rp".into()),
            },
            user("c", "go"),
        ]);
        assert_eq!(unanswered(&e), Some((ids(&["c"]), Some("rp".into()))));
        // A turn already taken back (thinking and all) left nothing.
        let e = with(
            answered(),
            vec![
                user("b", "go"),
                Body::TurnStarted,
                said(Some("t"), Block::Thinking { text: "hmm".into() }),
                Body::Withdrawn { ids: ids(&["b"]) },
                Body::TurnEnded {
                    outcome: Outcome::Interrupted { reason: None },
                },
                user("c", "go again"),
            ],
        );
        assert_eq!(unanswered(&e), Some((ids(&["c"]), Some("r1".into()))));
        assert_eq!(conversation_at(&e, e.len() - 1), Some(Some("r1".into())));
    }

    #[test]
    fn a_rewind_to_a_message_that_waited_goes_back_to_the_reply_it_waited_for() {
        let e = log(vec![
            user("a", "hello"),
            queued("b"),
            Body::TurnStarted,
            said(Some("r1"), text("hi")),
            ended(),
            Body::Delivered { id: "b".into() },
            Body::TurnStarted,
            said(Some("r2"), text("two")),
            ended(),
        ]);
        assert_eq!(
            conversation_at(&e, entered_at(&e, "b").unwrap()),
            Some(Some("r1".into()))
        );
        let mut b: Vec<Body> = e.into_iter().map(|e| e.body).collect();
        b.extend([
            Body::Rewound {
                to: "b".into(),
                files: false,
            },
            user("c", "go"),
        ]);
        assert_eq!(unanswered(&log(b)), Some((ids(&["c"]), Some("r1".into()))));
    }

    #[test]
    fn a_turn_taken_back_lies_from_its_start_to_its_end() {
        let e = with(
            answered(),
            vec![
                user("b", "go"),
                Body::TurnStarted,
                said(Some("t"), Block::Thinking { text: "hmm".into() }),
                Body::Withdrawn { ids: ids(&["b"]) },
                Body::TurnEnded {
                    outcome: Outcome::Interrupted { reason: None },
                },
                user("c", "next"),
            ],
        );
        assert_eq!(withdrawn_turns(&e), vec![4..=8]);
    }
}
