//! Moving a session to another machine from the desktop ([decision 0064]):
//! the machine chip under the reply box says where the session runs, and
//! choosing another machine moves it. This device carries the move with
//! client-core's [`Carrier`], one request at a time through the app's
//! table of requests waiting on a reply, so a move to or from any machine
//! goes the way `dent move` does.
//!
//! [decision 0064]: ../../../docs/decisions/0064-moving-sessions.md

use std::collections::HashMap;
use std::time::Duration;

use dent_client_core::carry::{Carrier, Ended, Phase, Side, Step};
use dent_client_core::fleet;
use dent_client_core::machine::State;
use dent_protocol::{GitRemote, Reply, Request};
use gpui_kit::Context;

use crate::app::{Dent, Waiting};

/// A move this device is carrying, by the session's id here.
pub struct Run {
    pub carrier: Carrier,
    /// The machines it leaves and goes to ("" is this one), and the name
    /// the one it goes to has here.
    pub from: String,
    pub to: String,
    pub to_name: String,
    /// The request out now, given up on if it isn't answered in time.
    pub out: Option<u64>,
    /// How it stopped, while it waits for Retry or Continue here.
    pub stopped: Option<Ended>,
}

impl Run {
    pub fn phase(&self) -> Phase {
        self.carrier.phase()
    }
}

/// What the other machines have of a session's project, asked as its
/// machine menu opens: its Git remotes, then, by machine, whether each has
/// a checkout of the same repository.
#[derive(Default)]
pub struct Probe {
    pub session: String,
    pub remotes: Option<Vec<GitRemote>>,
    pub has: HashMap<String, bool>,
}

/// How long an answer may take before the move counts as stopped: packing
/// a big repository, or making the session there, takes a while; a chunk
/// or a question doesn't.
fn patience(req: &Request) -> Duration {
    Duration::from_secs(match req {
        Request::MovePack { have: Some(_), .. } | Request::MoveAccept { .. } => 600,
        Request::MoveRead { .. } | Request::MoveWrite { .. } => 120,
        _ => 60,
    })
}

impl Dent {
    /// Move session `id` (as this device keeps it) to `to` ("" is this
    /// one). The same session moved again after it stopped goes on with
    /// the same move.
    pub fn start_move(&mut self, id: &str, to: &str, cx: &mut Context<Self>) {
        if self.moves.get(id).is_some_and(|r| r.stopped.is_none()) {
            return;
        }
        let (from, plain) = fleet::split(id);
        let (from, plain) = (from.to_string(), plain.to_string());
        let to_name = self.machine_name(to);
        // Picking it up again keeps the move's id: the machines pick up
        // where it stood. So does one the session says is under way (this
        // app restarted, or another device began it).
        let live = self
            .session(id)
            .and_then(|s| match (&s.moving, &s.move_id) {
                (
                    Some((
                        _,
                        _,
                        dent_protocol::MoveState::Pending
                        | dent_protocol::MoveState::Departing { .. },
                    )),
                    Some(m),
                ) => Some(m.clone()),
                _ => None,
            });
        let move_id = match (self.moves.get(id), live) {
            (Some(r), _) if r.to == to => r.carrier.id().to_string(),
            (_, Some(m)) => m,
            _ => dent_client_core::carry::new_id(crate::cards::now_ms(), rand_u64()),
        };
        let mut carrier = Carrier::new(&plain, &move_id, &to_name);
        let step = carrier.start();
        self.moves.insert(
            id.to_string(),
            Run {
                carrier,
                from,
                to: to.to_string(),
                to_name,
                out: None,
                stopped: None,
            },
        );
        self.move_step(id, step, cx);
        cx.notify();
    }

    fn move_step(&mut self, id: &str, step: Step, cx: &mut Context<Self>) {
        let Some(run) = self.moves.get_mut(id) else {
            return;
        };
        match step {
            Step::Ask(side, req) => self.move_ask(id, side, req, cx),
            Step::Wait(ms, side, req) => {
                run.out = None;
                let id = id.to_string();
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(ms))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.moves.get(&id).is_some_and(|r| r.stopped.is_none()) {
                            this.move_ask(&id, side, req, cx);
                        }
                    });
                })
                .detach();
            }
            Step::Done(Ok(there)) => {
                let to = run.to.clone();
                self.moves.remove(id);
                // The window follows it to the machine it went to.
                let key = fleet::qualify(&to, &there);
                self.pending_select = Some(key);
                cx.notify();
            }
            // Nothing was packed: it works here as it was.
            Step::Done(Err(Ended::NoProject)) => {
                let name = run.to_name.clone();
                self.moves.remove(id);
                self.toast(
                    crate::app::ToastAction::Info,
                    format!("{name} doesn't have this project, so it stays here"),
                );
                cx.notify();
            }
            // Looked at there and refused: it works here again.
            Step::Done(Err(Ended::NotAccepted(why))) => {
                let name = run.to_name.clone();
                self.moves.remove(id);
                self.toast(
                    crate::app::ToastAction::Info,
                    format!("{name} didn't take it ({why}), so it carries on here"),
                );
                cx.notify();
            }
            Step::Done(Err(e)) => {
                run.out = None;
                run.stopped = Some(e);
                cx.notify();
            }
        }
    }

    fn move_ask(&mut self, id: &str, side: Side, req: Request, cx: &mut Context<Self>) {
        let Some(run) = self.moves.get_mut(id) else {
            return;
        };
        let machine = match side {
            Side::From => run.from.clone(),
            Side::To => run.to.clone(),
        };
        let wait = patience(&req);
        let n = self.live.send_to(&machine, req);
        run.out = Some(n);
        self.waiting_insert(n, Waiting::Move(id.to_string(), side));
        // Unanswered in time (the machine away, the link gone): stopped,
        // to pick up with Retry.
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                if this.moves.get(&id).is_some_and(|r| r.out == Some(n)) {
                    this.waiting_remove(n);
                    let why = format!("{} didn't answer", this.machine_name(&machine));
                    this.move_answer(&id, side, Err(why), cx);
                }
            });
        })
        .detach();
    }

    /// A machine answered the move's last request.
    pub fn move_answer(
        &mut self,
        id: &str,
        side: Side,
        answer: Result<Reply, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = self.moves.get_mut(id) else {
            return;
        };
        run.out = None;
        let step = run.carrier.answer(side, answer);
        self.move_step(id, step, cx);
        cx.notify();
    }

    /// Call the move off (only before anything was sent), or keep the
    /// session here whatever was sent (`continue_here`).
    pub fn stop_move(&mut self, id: &str, continue_here: bool, cx: &mut Context<Self>) {
        let Some(run) = self.moves.remove(id) else {
            return;
        };
        let (_, plain) = fleet::split(id);
        let n = self.live.send_to(
            &run.from,
            Request::MoveStop {
                session: plain.to_string(),
                id: run.carrier.id().to_string(),
                continue_here,
            },
        );
        self.waiting_insert(n, Waiting::MoveStopped);
        cx.notify();
    }

    /// As the machine menu opens: ask what the other machines have of
    /// this session's project.
    pub fn probe_machines(&mut self, id: &str) {
        let (from, plain) = fleet::split(id);
        self.move_probe = Some(Probe {
            session: id.to_string(),
            ..Probe::default()
        });
        let n = self.live.send_to(
            from,
            Request::MovePack {
                session: plain.to_string(),
                id: "probe".into(),
                to: String::new(),
                have: None,
            },
        );
        self.waiting_insert(n, Waiting::MoveRemotes(id.to_string()));
    }

    /// The session's remotes came: each other online machine is asked
    /// whether it has the repository.
    pub fn probe_remotes(&mut self, id: &str, remotes: Vec<GitRemote>) {
        let Some(p) = self.move_probe.as_mut().filter(|p| p.session == id) else {
            return;
        };
        p.remotes = Some(remotes.clone());
        // Outside a repository (dent's scratch folder) it goes anywhere.
        if remotes.is_empty() {
            return;
        }
        let (from, _) = fleet::split(id);
        let from = from.to_string();
        for m in self.online_machines() {
            if m == from {
                continue;
            }
            let n = self.live.send_to(
                &m,
                Request::MoveHave {
                    remotes: remotes.clone(),
                },
            );
            self.waiting_insert(n, Waiting::MoveHas(id.to_string(), m));
        }
    }

    pub fn probe_has(&mut self, id: &str, machine: String, found: bool) {
        if let Some(p) = self.move_probe.as_mut().filter(|p| p.session == id) {
            p.has.insert(machine, found);
        }
    }

    /// The machine this device calls `name` ("" for this one).
    pub fn machine_by_name(&self, name: &str) -> Option<String> {
        if name == self.machine_name("") {
            return Some(String::new());
        }
        self.machines
            .iter()
            .find(|m| m.saved.name == name || m.name() == name)
            .map(|m| m.id.clone())
    }

    /// Every machine that answers now, this one ("") included.
    pub fn online_machines(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .added_machines()
            .filter(|m| matches!(m.state, State::Online | State::Slow))
            .map(|m| m.id.clone())
            .collect();
        if self.connection.is_ok() {
            v.insert(0, String::new());
        }
        v
    }
}

impl Dent {
    /// The machine menu of the open session: every other machine, each
    /// saying whether it can take it, and why not when it can't.
    pub fn move_entries(&self) -> Vec<crate::chat::Entry> {
        use crate::chat::Entry;
        use crate::machines_page::Standing;
        let Some(s) = self.current() else {
            return Vec::new();
        };
        let id = s.id.clone();
        let (here, _) = fleet::split(&id);
        let project = s.project.clone();
        let probe = self.move_probe.as_ref().filter(|p| p.session == id);
        // Outside a repository (dent's scratch folder), anywhere will do.
        let anywhere = probe
            .and_then(|p| p.remotes.as_ref())
            .is_some_and(|r| r.is_empty());
        let has = |m: &str| probe.and_then(|p| p.has.get(m).copied());
        let go = |to: String| -> Option<crate::chat::Act> {
            let id = id.clone();
            Some(std::rc::Rc::new(
                move |this: &mut Dent, _: &mut gpui_kit::Window, cx: &mut Context<Dent>| {
                    this.menu = None;
                    this.start_move(&id, &to, cx);
                },
            ))
        };
        let mut v = Vec::new();
        // This Mac, when the session runs elsewhere.
        if !here.is_empty() {
            let mut e = Entry::row(
                "move-this".into(),
                &self.machine_name(""),
                Some(match has("") {
                    Some(false) if !anywhere => format!("Doesn't have {project}"),
                    _ => "This Mac".into(),
                }),
                false,
                go(String::new()),
            );
            if has("") == Some(false) && !anywhere {
                e.off = Some("It doesn't have this project; bring it there first");
                e.act = None;
            }
            v.push(e);
        }
        for m in self.added_machines() {
            if m.id == here {
                continue;
            }
            let ms = m.rtt.map(|ms| format!(" · {ms} ms")).unwrap_or_default();
            let (note, off): (String, Option<&'static str>) = match Standing::of(m) {
                Standing::Online => match has(&m.id) {
                    Some(false) if !anywhere => (
                        format!("Doesn't have {project}"),
                        Some("It doesn't have this project; bring it there first"),
                    ),
                    Some(true) => (format!("Has {project}{ms}"), None),
                    _ if anywhere => (format!("Online{ms}"), None),
                    _ => (format!("Checking for {project}…{ms}"), None),
                },
                Standing::Connecting => ("Connecting…".into(), Some("It isn't connected yet")),
                Standing::Asleep => ("Asleep · wake it first".into(), Some("It's asleep")),
                Standing::Unreachable => ("Can't reach it".into(), Some("It can't be reached")),
                Standing::NeedsCode => (
                    "Needs a code".into(),
                    Some("Pair with it in Settings › Machines"),
                ),
                Standing::Blocked => ("Can't be used".into(), Some("See Settings › Machines")),
            };
            let mut e = Entry::row(
                format!("move-{}", m.id),
                &crate::data::machine_short(m.name()),
                Some(note),
                false,
                go(m.id.clone()),
            );
            if off.is_some() {
                e.off = off;
                e.act = None;
            }
            v.push(e);
        }
        v.push(Entry::note(
            "It goes once this turn ends, with its uncommitted work and its whole conversation, and carries on there.",
        ));
        v
    }
}

fn rand_u64() -> u64 {
    let mut b = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut b);
    }
    u64::from_le_bytes(b)
}
