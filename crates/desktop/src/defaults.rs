//! How new sessions start, kept on each machine ([decision 0076]): the
//! machine's defaults, a project's own over them, and what's picked in the
//! composer over both, for that session only. This device asks each
//! machine for them as it connects, and keeps a change for a machine away
//! until it's back.
//!
//! [decision 0076]: ../../../docs/decisions/0076-defaults-on-machines.md

use dent_protocol::{Defaults, Permission, Request};
use gpui_kit::Context;

use crate::app::{Dent, Waiting};

/// A change to a machine's defaults (or, with `project`, a project's
/// there) not yet heard to have taken: sent again as it comes back.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct Pending {
    /// Which change this is: its answer clears it, not another's.
    #[serde(default)]
    pub id: u64,
    pub machine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub defaults: Defaults,
    /// Made before the machine said what it has: what's set here goes
    /// over what it has, rather than replacing it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub merge: bool,
}

/// Numbers changes, from the clock, so ones kept across a restart stay
/// apart.
fn next_id() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let base = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64);
    base + N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// One of a new session's choices.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Agent,
    Model,
    Permission,
    Effort,
    Worktree,
}

impl Field {
    /// `to` with this choice as `from` has it.
    fn take(self, to: &mut Defaults, from: &Defaults) {
        match self {
            Field::Agent => to.agent = from.agent.clone(),
            Field::Model => to.model = from.model.clone(),
            Field::Permission => to.permission = from.permission,
            Field::Effort => to.effort = from.effort.clone(),
            Field::Worktree => to.worktree = from.worktree,
        }
    }

    /// `d` leaves this choice to what's under it.
    pub fn is_unset(self, d: &Defaults) -> bool {
        self.of(d).is_empty()
    }

    /// Only this choice of `d`.
    fn of(self, d: &Defaults) -> Defaults {
        let mut v = Defaults::default();
        self.take(&mut v, d);
        v
    }

    /// Only this choice of `d`, as it reads with what's unset left to
    /// what's under it.
    pub fn effective(self, d: &Defaults) -> Defaults {
        let mut e = self.of(d);
        if self == Field::Permission && e.permission.is_none() {
            e.permission = Some(Permission::Full);
        }
        if self == Field::Worktree && e.worktree.is_none() {
            e.worktree = Some(false);
        }
        e
    }

    /// The choice in words, for "Make … the default".
    fn word(self) -> &'static str {
        match self {
            Field::Agent => "this agent",
            Field::Model => "this model",
            Field::Effort => "this effort",
            Field::Permission | Field::Worktree => "this",
        }
    }
}

/// What a row does, given the app.
type Go = Box<dyn Fn(&mut Dent, &mut Context<Dent>)>;

impl Dent {
    /// What a new session on `machine` in `project` starts with: its
    /// defaults, the project's over them, changes on their way counted.
    /// From a machine that doesn't keep them (older, or not heard from),
    /// what this device used last.
    pub fn defaults_for(&self, machine: &str, project: Option<&str>) -> Defaults {
        if !self
            .setups
            .get(machine)
            .is_some_and(|s| s.defaults.is_some())
        {
            return self.last_used.clone();
        }
        let base = self.machine_defaults(machine);
        let over = project
            .map(|p| self.project_defaults(machine, p))
            .unwrap_or_default();
        base.under(&over)
    }

    /// What `machine` keeps (for `project`, or itself) with the changes
    /// on their way there counted already.
    fn kept_with_pending(&self, machine: &str, project: Option<&str>, kept: Defaults) -> Defaults {
        self.pending_defaults
            .iter()
            .filter(|p| p.machine == machine && p.project.as_deref() == project)
            .fold(kept, |d, p| {
                if p.merge {
                    d.under(&p.defaults)
                } else {
                    p.defaults.clone()
                }
            })
    }

    /// A machine's own defaults (not a project's).
    fn machine_defaults(&self, machine: &str) -> Defaults {
        let kept = self
            .setups
            .get(machine)
            .and_then(|s| s.defaults.clone())
            .unwrap_or_default();
        self.kept_with_pending(machine, None, kept)
    }

    /// A project's own there.
    fn project_defaults(&self, machine: &str, project: &str) -> Defaults {
        let kept = self
            .setups
            .get(machine)
            .and_then(|s| s.project_defaults.get(project).cloned())
            .unwrap_or_default();
        self.kept_with_pending(machine, Some(project), kept)
    }

    /// The new session's choices from the defaults where it goes: each
    /// one still as it was last put there takes the new default; one
    /// picked since stays (`all`: every one, as after a session starts).
    pub fn apply_defaults(&mut self, all: bool) {
        let to = self.defaults_for(
            &self.defaults.machine.clone(),
            self.defaults.project.as_deref(),
        );
        let was = std::mem::replace(&mut self.applied, to.clone());
        let d = &mut self.defaults;
        if all || d.agent == was.agent {
            d.agent = to.agent.clone();
        }
        if all || d.model == was.model {
            d.model = to.model.clone();
        }
        if all || Some(d.permission) == was.permission.or(Some(Permission::Full)) {
            d.permission = to.permission.unwrap_or(Permission::Full);
        }
        if all || d.effort == was.effort {
            d.effort = to.effort.clone();
        }
        if all || Some(d.worktree) == was.worktree.or(Some(false)) {
            d.worktree = to.worktree.unwrap_or(false);
        }
    }

    /// Set how new sessions start on `machine` (or `project` there),
    /// replaced whole: at once here, and there as soon as it's reachable.
    pub fn put_defaults(
        &mut self,
        machine: &str,
        project: Option<String>,
        defaults: Defaults,
        cx: &mut Context<Self>,
    ) {
        self.put_change(machine, project, defaults, false, cx);
    }

    /// [`Dent::put_defaults`], or with `merge`, what's set going over what
    /// the machine has (for one not heard from yet).
    fn put_change(
        &mut self,
        machine: &str,
        project: Option<String>,
        defaults: Defaults,
        merge: bool,
        cx: &mut Context<Self>,
    ) {
        // A whole set replaces what was waiting; a merge goes after it.
        if !merge {
            self.pending_defaults
                .retain(|p| !(p.machine == machine && p.project == project));
        }
        self.pending_defaults.push(Pending {
            id: next_id(),
            machine: machine.to_string(),
            project,
            defaults,
            merge,
        });
        self.send_pending(machine);
        self.apply_defaults(false);
        self.prefs_changed();
        cx.notify();
    }

    /// Send `machine` what's waiting for it, if it's reachable now and has
    /// said it keeps defaults.
    pub fn send_pending(&mut self, machine: &str) {
        let Some(setup) = self.setups.get(machine).cloned() else {
            return;
        };
        let Some(kept) = setup.defaults.clone() else {
            return;
        };
        let reachable = if machine.is_empty() {
            self.connection.is_ok()
        } else {
            self.machines.iter().any(|m| {
                m.id == machine
                    && matches!(
                        m.state,
                        dent_client_core::machine::State::Online
                            | dent_client_core::machine::State::Slow
                    )
            })
        };
        if !reachable {
            return;
        }
        let out: Vec<Pending> = self
            .pending_defaults
            .iter()
            .filter(|p| p.machine == machine)
            .cloned()
            .collect();
        for p in out {
            // A merge goes as a whole set: what it has, with this over it.
            let whole = if p.merge {
                let has = match &p.project {
                    None => kept.clone(),
                    Some(pr) => setup.project_defaults.get(pr).cloned().unwrap_or_default(),
                };
                has.under(&p.defaults)
            } else {
                p.defaults.clone()
            };
            let n = self.live.send_to(
                machine,
                Request::SetDefaults {
                    project: p.project.clone(),
                    defaults: whole,
                },
            );
            self.waiting_insert(n, Waiting::SetDefaults(p));
        }
    }

    /// Ask `machine` how it's set up, its defaults among it.
    pub fn ask_setup(&mut self, machine: &str) {
        let n = self.live.send_to(machine, Request::Machine);
        self.waiting_insert(n, Waiting::MachineSetup(machine.to_string()));
    }

    /// How `machine` is set up came: its defaults count from now, and
    /// what was kept for it goes. One whose defaults were never set takes
    /// what this device used, so nothing changes under you the first time.
    pub fn setup_came(
        &mut self,
        machine: String,
        setup: dent_protocol::Machine,
        cx: &mut Context<Self>,
    ) {
        let keeps = setup.defaults.is_some();
        let fresh = keeps
            && !setup.defaults_set
            && !self.pending_defaults.iter().any(|p| p.machine == machine);
        self.setups.insert(machine.clone(), setup);
        if !keeps {
            // Too old to keep defaults: what was kept for it can't go.
            if self.pending_defaults.iter().any(|p| p.machine == machine) {
                self.pending_defaults.retain(|p| p.machine != machine);
                let name = self.machine_name(&machine);
                self.toast(
                    crate::app::ToastAction::Info,
                    format!(
                        "{name} runs an older dent, which can't keep defaults: update dent there"
                    ),
                );
                self.prefs_changed();
            }
        } else if fresh && !self.last_used.is_empty() {
            let d = self.last_used.clone();
            self.put_defaults(&machine, None, d, cx);
        } else {
            self.send_pending(&machine);
        }
        if machine == self.defaults.machine {
            self.apply_defaults(false);
        }
        cx.notify();
    }

    /// A change took there (its answer says how it's set up now). Not
    /// taken, it's tried again when the machine is back.
    pub fn set_defaults_came(
        &mut self,
        sent: Pending,
        answer: Result<dent_protocol::Machine, String>,
        cx: &mut Context<Self>,
    ) {
        if let Ok(setup) = answer {
            self.pending_defaults.retain(|p| p.id != sent.id);
            self.setups.insert(sent.machine.clone(), setup);
            if sent.machine == self.defaults.machine {
                self.apply_defaults(false);
            }
            self.prefs_changed();
        }
        cx.notify();
    }

    /// Set choice `f` on `machine` to what `value` has: over what it has,
    /// or, not heard from yet, merged over it when it says.
    pub fn set_field_on(
        &mut self,
        f: Field,
        value: &Defaults,
        machine: &str,
        cx: &mut Context<Self>,
    ) {
        match self.setups.get(machine) {
            None => self.put_change(machine, None, f.of(value), true, cx),
            Some(s) if s.defaults.is_none() => {}
            Some(_) => {
                let mut d = self.machine_defaults(machine);
                f.take(&mut d, value);
                self.put_defaults(machine, None, d, cx);
            }
        }
    }

    /// The new session's choices as defaults (all of them set).
    fn chosen(&self) -> Defaults {
        let d = &self.defaults;
        Defaults {
            agent: d.agent.clone(),
            model: d.model.clone(),
            permission: Some(d.permission),
            effort: d.effort.clone(),
            worktree: Some(d.worktree),
        }
    }

    /// Make choice `f` as it is now the default on `machine` (or for
    /// `project` there).
    fn make_default(
        &mut self,
        f: Field,
        machine: &str,
        project: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let now = self.chosen();
        match self.setups.get(machine) {
            // Not heard from yet: only this goes, over whatever it has.
            None => self.put_change(machine, project, f.of(&now), true, cx),
            // Can't keep defaults at all.
            Some(s) if s.defaults.is_none() => {}
            Some(_) => {
                let mut d = match &project {
                    None => self.machine_defaults(machine),
                    Some(p) => self.project_defaults(machine, p),
                };
                f.take(&mut d, &now);
                self.put_defaults(machine, project, d, cx);
            }
        }
    }

    /// The rows under a new session's choice: make it the default on its
    /// machine, for its project, or on every machine; or, when it is, say
    /// so. Nothing from a machine that can't keep defaults.
    pub fn default_entries(&self, f: Field) -> Vec<crate::chat::Entry> {
        use crate::chat::Entry;
        let machine = self.defaults.machine.clone();
        if !self
            .setups
            .get(&machine)
            .is_some_and(|s| s.defaults.is_some())
        {
            return Vec::new();
        }
        let name = crate::data::machine_short(&self.machine_name(&machine));
        let now = f.of(&self.chosen());
        let project = self.defaults.project.clone();
        let theirs = project
            .as_deref()
            .map(|p| f.of(&self.project_defaults(&machine, p)))
            .unwrap_or_default();
        let mut v = vec![Entry::separator()];
        // The project's own, then the machine's, each said as such.
        if let Some(p) = &project
            && !theirs.is_empty()
            && theirs == now
        {
            v.push(Entry::note(&format!(
                "The default for {} on {name}.",
                Dent::project_name(p)
            )));
            return v;
        }
        if theirs.is_empty() && f.effective(&self.machine_defaults(&machine)) == f.effective(&now) {
            v.push(Entry::note(&format!(
                "The default on {name}. Change it in Settings › Agents."
            )));
            return v;
        }
        let act = |go: Go| -> Option<crate::chat::Act> {
            Some(std::rc::Rc::new(
                move |this: &mut Dent, _: &mut gpui_kit::Window, cx: &mut Context<Dent>| {
                    go(this, cx)
                },
            ))
        };
        let m = machine.clone();
        v.push(Entry::row(
            "make-default-machine".into(),
            &format!("Make {} the default on {name}", f.word()),
            Some("For new sessions there, from any device".into()),
            false,
            act(Box::new(move |this, cx| this.make_default(f, &m, None, cx))),
        ));
        if let Some(p) = project {
            let m = machine.clone();
            let pname = Dent::project_name(&p);
            v.push(Entry::row(
                "make-default-project".into(),
                &format!("Only for {pname}"),
                Some(format!("Its new sessions on {name}")),
                false,
                act(Box::new(move |this, cx| {
                    this.make_default(f, &m, Some(p.clone()), cx)
                })),
            ));
        }
        if self.has_machines() {
            v.push(Entry::row(
                "make-default-all".into(),
                "On all machines",
                Some("Each takes it as soon as it's reachable".into()),
                false,
                act(Box::new(move |this, cx| {
                    let mut all: Vec<String> =
                        this.added_machines().map(|m| m.id.clone()).collect();
                    all.insert(0, String::new());
                    for m in all {
                        this.make_default(f, &m, None, cx);
                    }
                })),
            ));
        }
        v
    }
}
