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

use dent_client_core::carry::{Carrier, CloneThere, Ended, Phase, Side, Step};
use dent_client_core::fleet;
use dent_client_core::machine::State;
use dent_protocol::{Bring, GitRemote, Pack, Reply, Request};
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
    /// Which run this is, so a timer or answer meant for an earlier run of
    /// the same session's move never reaches this one.
    pub run: u64,
    /// The project it clones there first, kept for Retry.
    pub clone: Option<CloneThere>,
}

/// Cloning a session's project to a machine without it, being chosen in
/// the machine menu: what it needs, once its machine said, and the
/// ignored files ticked to go along (environment files at first).
pub struct CloneDraft {
    pub session: String,
    pub to: String,
    /// The checkout it's cloned from, on the session's machine.
    pub dir: String,
    pub pack: Option<Result<Pack, String>>,
    pub files: crate::bring::FileChoice,
    /// The request its list went out as: a timer for an earlier one never
    /// reaches this one.
    pub asked: u64,
}

/// Counts runs.
static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

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
    /// What couldn't be found out: the remotes ("") or a machine's
    /// answer. Such a machine is offered as online, and the move asks
    /// again.
    pub unsure: std::collections::HashSet<String>,
}

/// How long an answer may take before the move counts as stopped: packing
/// a big repository, or making the session there, takes a while; a chunk
/// or a question doesn't.
/// How long cloning's list of files may take to come.
const PACK_PATIENCE: Duration = Duration::from_secs(15);

/// How long the machine menu waits to hear what each machine has.
const PROBE_PATIENCE: Duration = Duration::from_secs(6);

fn patience(req: &Request) -> Duration {
    Duration::from_secs(match req {
        Request::MovePack { have: Some(_), .. } | Request::MoveAccept { .. } => 600,
        Request::MoveRead { .. } | Request::MoveWrite { .. } | Request::ReadFiles { .. } => 120,
        _ => 60,
    })
}

impl Dent {
    /// Move session `id` (as this device keeps it) to `to` ("" is this
    /// one). A move the session says is under way goes on with the same
    /// id, to the machine it was going to; anything else is a new move.
    pub fn start_move(&mut self, id: &str, to: &str, cx: &mut Context<Self>) {
        // Retried: a clone it was to make there is made still.
        let clone = self
            .moves
            .get(id)
            .filter(|r| r.to == to)
            .and_then(|r| r.clone.clone());
        self.start_move_with(id, to, clone, cx);
    }

    /// [`Dent::start_move`], cloning the project there first if it has
    /// none.
    pub fn start_move_with(
        &mut self,
        id: &str,
        to: &str,
        clone: Option<CloneThere>,
        cx: &mut Context<Self>,
    ) {
        if self.moves.get(id).is_some_and(|r| r.stopped.is_none()) {
            return;
        }
        let (from, plain) = fleet::split(id);
        let (from, plain) = (from.to_string(), plain.to_string());
        let to_name = self.machine_name(to);
        // Under way (stopped partway, this app restarted, or another
        // device began it): the machines pick up where it stood. A move
        // that ended keeps no claim: its id is spent there.
        let live = self.live_move(id);
        let move_id = match live {
            Some((going, m, target)) => {
                let same = match (self.moves.get(id), target) {
                    (Some(r), _) => r.to == to,
                    (None, Some(target)) => target == to,
                    (None, None) => going == to_name,
                };
                if !same {
                    self.toast(
                        crate::app::ToastAction::Info,
                        format!("It's moving to {going}: call that off or continue here first"),
                    );
                    return;
                }
                m
            }
            None => dent_client_core::carry::new_id(crate::cards::now_ms(), rand_u64()),
        };
        let mut carrier =
            Carrier::new(&plain, &move_id, &to_name).to_machine(&self.machine_key(to));
        if let Some(c) = clone.clone() {
            carrier = carrier.clone_there(c);
        }
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
                run: RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                clone,
            },
        );
        self.move_step(id, step, cx);
        cx.notify();
    }

    /// The move session `id` says is under way: where to by name, its id,
    /// and the machine it goes to, when that's known here.
    pub fn live_move(&self, id: &str) -> Option<(String, String, Option<String>)> {
        let s = self.session(id)?;
        match (&s.moving, &s.move_id) {
            (
                Some((
                    to,
                    _,
                    dent_protocol::MoveState::Pending | dent_protocol::MoveState::Departing { .. },
                )),
                Some(m),
            ) => Some((to.clone(), m.clone(), self.move_target(s))),
            _ => None,
        }
    }

    /// Machine `m` ("" is this one) by the id every device knows it by.
    pub fn machine_key(&self, m: &str) -> String {
        if m.is_empty() {
            self.machine_id.clone()
        } else {
            m.to_string()
        }
    }

    /// The machine session `s` is moving or moved to, as this device
    /// keeps it: by its id when its daemon said, else by name when the
    /// name is one machine's.
    pub fn move_target(&self, s: &crate::data::Session) -> Option<String> {
        match s.move_to.as_deref() {
            Some(key) if key == self.machine_id => Some(String::new()),
            Some(key) => self
                .machines
                .iter()
                .any(|m| m.id == key)
                .then(|| key.to_string()),
            None => {
                let (to, ..) = s.moving.as_ref()?;
                self.machine_by_name(to)
            }
        }
    }

    fn move_step(&mut self, id: &str, step: Step, cx: &mut Context<Self>) {
        let Some(run) = self.moves.get_mut(id) else {
            return;
        };
        match step {
            Step::Ask(side, req) => self.move_ask(id, side, req, cx),
            Step::Wait(ms, side, req) => {
                run.out = None;
                let (id, this_run) = (id.to_string(), run.run);
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(ms))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        // Still this run, waiting: not one called off and
                        // begun again meanwhile.
                        if this.moves.get(&id).is_some_and(|r| {
                            r.run == this_run && r.stopped.is_none() && r.out.is_none()
                        }) {
                            this.move_ask(&id, side, req, cx);
                        }
                    });
                })
                .detach();
            }
            Step::Done(Ok(there)) => {
                let to = run.to.clone();
                self.moves.remove(id);
                // The window follows it to the machine it went to, when
                // it's still the one open: a move finishing elsewhere
                // doesn't take the window away from what you're doing.
                if self.current().is_some_and(|s| s.id == id) {
                    self.pending_select = Some(fleet::qualify(&to, &there));
                }
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
                // Stopped before the machine it leaves took it on: nothing
                // is held, so there's nothing to retry or keep. It says
                // why and works here as it was.
                let name = run.to_name.clone();
                if self.live_move(id).is_none() {
                    self.moves.remove(id);
                    let why = match e {
                        Ended::Stopped { why, .. } => why,
                        _ => "it stopped".into(),
                    };
                    // Ended there (called off, or a message sent while it
                    // waited): the chat says so already.
                    if why.contains("move ended") {
                        cx.notify();
                        return;
                    }
                    self.toast(
                        crate::app::ToastAction::Info,
                        format!("Couldn't move it to {name}: {why}"),
                    );
                    cx.notify();
                    return;
                }
                let Some(run) = self.moves.get_mut(id) else {
                    return;
                };
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
                    this.move_answer(&id, side, n, Err(why), cx);
                }
            });
        })
        .detach();
    }

    /// A machine answered request `n` of the move. Only the one out now
    /// counts: an answer to an earlier run's request is dropped.
    pub fn move_answer(
        &mut self,
        id: &str,
        side: Side,
        n: u64,
        answer: Result<Reply, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = self.moves.get_mut(id).filter(|r| r.out == Some(n)) else {
            return;
        };
        run.out = None;
        let step = run.carrier.answer(side, answer);
        self.move_step(id, step, cx);
        cx.notify();
    }

    /// Call the move off (only before anything was sent), or keep the
    /// session here whatever was sent (`continue_here`).
    /// One this device isn't carrying (begun before this app restarted,
    /// or by another device) is stopped by the id the session gives it.
    pub fn stop_move(&mut self, id: &str, continue_here: bool, cx: &mut Context<Self>) {
        let (from, plain) = fleet::split(id);
        let (from, move_id) = match self.moves.remove(id) {
            Some(run) => {
                // Its step out now is forgotten: the answer has nowhere to go.
                if let Some(n) = run.out {
                    self.waiting_remove(n);
                }
                (run.from, run.carrier.id().to_string())
            }
            None => match self.live_move(id) {
                Some((_, m, _)) => (from.to_string(), m),
                None => return,
            },
        };
        let n = self.live.send_to(
            &from,
            Request::MoveStop {
                session: plain.to_string(),
                id: move_id,
                continue_here,
            },
        );
        self.waiting_insert(n, Waiting::MoveStopped);
        cx.notify();
    }

    /// As the machine menu opens: ask what the other machines have of
    /// this session's project. Whatever hasn't answered in a few seconds
    /// (a machine online but hung) is offered as online, unsure.
    pub fn probe_machines_then(&mut self, id: &str, cx: &mut Context<Self>) {
        self.probe_machines(id);
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PROBE_PATIENCE).await;
            let _ = this.update(cx, |this, cx| {
                let machines: Vec<String> = this.online_machines();
                let Some(p) = this.move_probe.as_mut().filter(|p| p.session == id) else {
                    return;
                };
                if p.remotes.is_none() {
                    p.unsure.insert(String::new());
                }
                for m in machines {
                    if !p.has.contains_key(&m) {
                        p.unsure.insert(m);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

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
                to_machine: String::new(),
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

    /// The remotes, or machine `machine`'s answer, couldn't be had.
    pub fn probe_unsure(&mut self, id: &str, machine: String) {
        if let Some(p) = self.move_probe.as_mut().filter(|p| p.session == id) {
            p.unsure.insert(machine);
        }
    }

    pub fn probe_has(&mut self, id: &str, machine: String, found: bool) {
        if let Some(p) = self.move_probe.as_mut().filter(|p| p.session == id) {
            p.has.insert(machine, found);
        }
    }

    /// Every machine this device calls `name` ("" for this one): two
    /// machines can share one.
    pub fn machines_named(&self, name: &str) -> Vec<String> {
        let mut v = Vec::new();
        if name == self.machine_name("") {
            v.push(String::new());
        }
        v.extend(
            self.machines
                .iter()
                .filter(|m| m.saved.name == name || m.name() == name)
                .map(|m| m.id.clone()),
        );
        v
    }

    /// The machine called `name`, only when that name is one machine's.
    pub fn machine_by_name(&self, name: &str) -> Option<String> {
        match &self.machines_named(name)[..] {
            [one] => Some(one.clone()),
            _ => None,
        }
    }

    /// Where session `s`, moved to `name` as `there`, is: the machine it
    /// went to, or, from an older daemon that said only its name, the
    /// machine of that name that has it.
    pub fn moved_to(&self, s: &crate::data::Session, name: &str, there: &str) -> Option<String> {
        if s.move_to.is_some() {
            return self.move_target(s).map(|m| fleet::qualify(&m, there));
        }
        let all = self.machines_named(name);
        all.iter()
            .find(|m| self.session(&fleet::qualify(m, there)).is_some())
            .or(all.first())
            .map(|m| fleet::qualify(m, there))
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
        if let Some(c) = self.move_clone.as_ref().filter(|c| c.session == id) {
            return self.clone_entries(c);
        }
        let (here, _) = fleet::split(&id);
        let project = s.project.clone();
        let probe = self.move_probe.as_ref().filter(|p| p.session == id);
        // Outside a repository (dent's scratch folder), anywhere will do.
        let anywhere = probe
            .and_then(|p| p.remotes.as_ref())
            .is_some_and(|r| r.is_empty());
        let has = |m: &str| probe.and_then(|p| p.has.get(m).copied());
        let unsure = |m: &str| probe.is_some_and(|p| p.unsure.contains("") || p.unsure.contains(m));
        // Under way already: only where it's going, and that's not chosen
        // again (its banner carries it). A stopped run counts only while
        // the session says the move is under way: one that ended there
        // since holds nothing.
        let live = self.live_move(&id).is_some();
        let going = self
            .moves
            .get(&id)
            .filter(|r| r.stopped.is_none() || live)
            .map(|r| (r.to_name.clone(), Some(r.to.clone()), r.stopped.is_none()))
            .or_else(|| {
                self.live_move(&id)
                    .map(|(to, _, target)| (to, target, false))
            });
        let go = |to: String| -> Option<crate::chat::Act> {
            let id = id.clone();
            Some(std::rc::Rc::new(
                move |this: &mut Dent, _: &mut gpui_kit::Window, cx: &mut Context<Dent>| {
                    this.menu = None;
                    this.start_move(&id, &to, cx);
                },
            ))
        };
        // Without the project: chosen, it opens what cloning it there
        // takes, to look over before anything starts.
        let clone = |to: String| -> Option<crate::chat::Act> {
            let id = id.clone();
            Some(std::rc::Rc::new(
                move |this: &mut Dent, _: &mut gpui_kit::Window, cx: &mut Context<Dent>| {
                    this.look_to_clone(&id, &to, cx);
                },
            ))
        };
        let hold = |e: &mut Entry, name: &str, machine: &str| {
            let Some((to, target, carrying)) = &going else {
                return;
            };
            let there = target.as_deref().map_or(to == name, |t| t == machine);
            if !there {
                e.note = Some(format!("It's moving to {}", crate::data::machine_short(to)));
                e.off = Some("Call that move off, or continue here, first");
                e.act = None;
            } else if *carrying {
                e.note = Some("On its way there".into());
                e.off = Some("It's on its way there");
                e.act = None;
            }
        };
        let mut v = Vec::new();
        // This Mac, when the session runs elsewhere.
        if !here.is_empty() {
            let lacks = has("") == Some(false) && !anywhere;
            let mut e = Entry::row(
                "move-this".into(),
                &self.machine_name(""),
                Some(if lacks {
                    format!("Doesn't have {project} · Clone it here")
                } else {
                    "This Mac".into()
                }),
                false,
                if lacks {
                    clone(String::new())
                } else {
                    go(String::new())
                },
            );
            if lacks {
                e = e.keeps_open();
            }
            hold(&mut e, &self.machine_name(""), "");
            v.push(e);
        }
        for m in self.added_machines() {
            if m.id == here {
                continue;
            }
            let ms = m.rtt.map(|ms| format!(" · {ms} ms")).unwrap_or_default();
            let lacks = matches!(Standing::of(m), Standing::Online)
                && has(&m.id) == Some(false)
                && !anywhere;
            let (note, off): (String, Option<&'static str>) = match Standing::of(m) {
                Standing::Online => match has(&m.id) {
                    _ if lacks => (format!("Doesn't have {project} · Clone it there"), None),
                    Some(true) => (format!("Has {project}{ms}"), None),
                    _ if anywhere || unsure(&m.id) => (format!("Online{ms}"), None),
                    _ => (format!("Checking for {project}…{ms}"), None),
                },
                Standing::Connecting => ("Connecting…".into(), Some("It isn't connected yet")),
                Standing::Updating => (
                    "Updating · back in a moment".into(),
                    Some("It's updating; its sessions keep running"),
                ),
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
                if lacks {
                    clone(m.id.clone())
                } else {
                    go(m.id.clone())
                },
            );
            if lacks {
                e = e.keeps_open();
            }
            if off.is_some() {
                e.off = off;
                e.act = None;
            }
            hold(&mut e, &self.machine_name(&m.id), &m.id);
            v.push(e);
        }
        v.push(Entry::note(
            "It goes when this turn ends, with its uncommitted work and its whole conversation.",
        ));
        v
    }
}

impl Dent {
    /// Ask the session's machine what cloning its project to `to` takes:
    /// its remotes and the files Git ignores.
    pub fn look_to_clone(&mut self, id: &str, to: &str, cx: &mut Context<Self>) {
        let Some(s) = self.session(id) else {
            return;
        };
        let dir = s.root.clone().unwrap_or_else(|| s.cwd.clone());
        let (from, _) = fleet::split(id);
        let n = self.live.send_to(from, Request::Pack { dir: dir.clone() });
        self.waiting_insert(n, Waiting::MoveClonePack(id.to_string(), to.to_string()));
        // Its machine hung: said, rather than "Looking…" for good.
        let (sid, tid, from) = (id.to_string(), to.to_string(), from.to_string());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PACK_PATIENCE).await;
            let _ = this.update(cx, |this, cx| {
                if this.move_clone.as_ref().is_some_and(|c| c.asked == n) {
                    let why = format!("{} didn't answer", this.machine_name(&from));
                    this.clone_pack_came(&sid, &tid, Err(why), cx);
                }
            });
        })
        .detach();
        self.move_clone = Some(CloneDraft {
            session: id.to_string(),
            to: to.to_string(),
            dir,
            pack: None,
            files: Default::default(),
            asked: n,
        });
        self.menu_ix = None;
        cx.notify();
    }

    /// What cloning it takes came, or why it can't be cloned.
    pub fn clone_pack_came(
        &mut self,
        id: &str,
        to: &str,
        pack: Result<Option<Pack>, String>,
        cx: &mut Context<Self>,
    ) {
        let there = self.machine_name(to);
        // The machine menu closed meanwhile: old news, which mustn't light
        // a row in whatever menu is open now.
        if self.menu != Some(crate::app::Menu::SessionMachine) {
            self.move_clone = None;
            return;
        }
        let Some(c) = self
            .move_clone
            .as_mut()
            .filter(|c| c.session == id && c.to == to && c.pack.is_none())
        else {
            return;
        };
        let name = Dent::project_name(&c.dir);
        c.pack = Some(match pack {
            Err(e) => Err(e),
            Ok(None) => Err(format!(
                "{name} isn't a Git repository, so it can't be cloned"
            )),
            Ok(Some(p)) if p.remotes.is_empty() => Err(format!(
                "{name} has no remote {there} can clone from: push it somewhere first"
            )),
            Ok(Some(p)) => {
                c.files = crate::bring::FileChoice::of(&p);
                Ok(p)
            }
        });
        // Clone and move is lit: Return goes.
        let at = self
            .move_entries()
            .iter()
            .position(|e| e.key == "move-clone-go");
        self.menu_ix = at;
        cx.notify();
    }

    /// The machine menu while a clone is looked over: Clone and move, then
    /// the files Git ignores, each ticked to come along or not.
    fn clone_entries(&self, c: &CloneDraft) -> Vec<crate::chat::Entry> {
        use crate::chat::Entry;
        let there = self.machine_name(&c.to);
        let back = {
            Some(std::rc::Rc::new(
                |this: &mut Dent, _: &mut gpui_kit::Window, cx: &mut Context<Dent>| {
                    this.move_clone = None;
                    this.menu_ix = None;
                    cx.notify();
                },
            ) as crate::chat::Act)
        };
        let mut v = vec![
            Entry::row(
                "move-clone-back".into(),
                "‹ Other machines",
                None,
                false,
                back,
            )
            .keeps_open(),
        ];
        let p = match &c.pack {
            None => {
                v.push(Entry::note(&format!(
                    "Looking at {} here…",
                    Dent::project_name(&c.dir)
                )));
                return v;
            }
            Some(Err(e)) => {
                v.push(Entry::note(e));
                return v;
            }
            Some(Ok(p)) => p,
        };
        let files = c.files.and_files();
        let bytes = c.files.bytes(p);
        let too_much = c.files.too_much(p);
        let (id, to) = (c.session.clone(), c.to.clone());
        let clone = CloneThere {
            dir: c.dir.clone(),
            paths: c.files.ticked.iter().cloned().collect(),
            // The session's branch: the clone there lets go of it if it
            // checks it out, so the session lands on it.
            bring: Bring {
                name: p.name.clone(),
                remotes: p.remotes.clone(),
                branch: self.session(&c.session).and_then(|s| s.branch.clone()),
                files: Vec::new(),
            },
        };
        let mut go = Entry::row(
            "move-clone-go".into(),
            &format!(
                "Clone {} to {} and move",
                p.name,
                crate::data::machine_short(&there)
            ),
            Some(format!(
                "Clones {}{files}, then moves it there",
                crate::bring::short(&p.remotes[0].url)
            )),
            false,
            Some(std::rc::Rc::new(
                move |this: &mut Dent, _: &mut gpui_kit::Window, cx: &mut Context<Dent>| {
                    this.menu = None;
                    this.move_clone = None;
                    this.start_move_with(&id, &to, Some(clone.clone()), cx);
                },
            )),
        );
        if too_much {
            go.note = Some(format!(
                "The ticked files come to {}; at most {} go along",
                crate::bring::size(bytes),
                crate::bring::size(dent_protocol::CARRIED_MAX as u64)
            ));
            go.off = Some("Untick some files first");
            go.act = None;
        }
        v.push(go);
        crate::bring::file_entries(
            "move",
            p,
            &c.files,
            |this| this.move_clone.as_mut().map(|c| &mut c.files),
            &mut v,
        );
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
