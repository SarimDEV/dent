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
    pub machine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub defaults: Defaults,
}

impl Dent {
    /// What a new session on `machine` in `project` starts with: its
    /// defaults, the project's over them. From a machine that doesn't keep
    /// them (older, or not heard from), what this device used last.
    pub fn defaults_for(&self, machine: &str, project: Option<&str>) -> Defaults {
        let Some(setup) = self.setups.get(machine) else {
            return self.last_used.clone();
        };
        let Some(kept) = &setup.defaults else {
            return self.last_used.clone();
        };
        // A change on its way there counts already.
        let on_way = |project: Option<&str>| {
            self.pending_defaults
                .iter()
                .rev()
                .find(|p| p.machine == machine && p.project.as_deref() == project)
                .map(|p| p.defaults.clone())
        };
        let base = on_way(None).unwrap_or_else(|| kept.clone());
        let over = project
            .and_then(|p| on_way(Some(p)).or_else(|| setup.project_defaults.get(p).cloned()))
            .unwrap_or_default();
        base.under(&over)
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
        self.pending_defaults
            .retain(|p| !(p.machine == machine && p.project == project));
        self.pending_defaults.push(Pending {
            machine: machine.to_string(),
            project,
            defaults,
        });
        self.send_pending(machine);
        self.apply_defaults(false);
        self.prefs_changed();
        cx.notify();
    }

    /// Send `machine` what's waiting for it, if it's reachable now.
    pub fn send_pending(&mut self, machine: &str) {
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
            let n = self.live.send_to(
                machine,
                Request::SetDefaults {
                    project: p.project.clone(),
                    defaults: p.defaults.clone(),
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

    /// How `machine` is set up came: its defaults count from now. One
    /// that has none yet takes what this device used, so nothing changes
    /// under you the first time.
    pub fn setup_came(
        &mut self,
        machine: String,
        setup: dent_protocol::Machine,
        cx: &mut Context<Self>,
    ) {
        let fresh = setup.defaults.as_ref().is_some_and(Defaults::is_empty)
            && setup.project_defaults.is_empty()
            && !self.pending_defaults.iter().any(|p| p.machine == machine);
        self.setups.insert(machine.clone(), setup);
        if fresh && !self.last_used.is_empty() {
            let d = self.last_used.clone();
            self.put_defaults(&machine, None, d, cx);
        }
        if machine == self.defaults.machine {
            self.apply_defaults(false);
        }
        cx.notify();
    }

    /// A change took there (its answer says how it's set up now), or it
    /// can't keep defaults at all (older): either way it waits no more.
    pub fn set_defaults_came(
        &mut self,
        sent: Pending,
        answer: Result<dent_protocol::Machine, String>,
        cx: &mut Context<Self>,
    ) {
        match answer {
            Ok(setup) => {
                self.pending_defaults.retain(|p| *p != sent);
                self.setups.insert(sent.machine.clone(), setup);
            }
            Err(e) if e.contains("unknown variant") => {
                self.pending_defaults.retain(|p| *p != sent);
                let name = self.machine_name(&sent.machine);
                self.toast(
                    crate::app::ToastAction::Info,
                    format!(
                        "{name} runs an older dent, which can't keep defaults: update dent there"
                    ),
                );
            }
            // Gone again meanwhile: it goes when it's back.
            Err(_) => {}
        }
        self.prefs_changed();
        cx.notify();
    }
}

/// What a row does, given the app.
type Go = Box<dyn Fn(&mut Dent, &mut Context<Dent>)>;

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

    fn of(self, d: &Defaults) -> Defaults {
        let mut v = Defaults::default();
        self.take(&mut v, d);
        v
    }

    /// The choice in words, for "Make … the default".
    fn word(self) -> &'static str {
        match self {
            Field::Agent => "this agent",
            Field::Model => "this model",
            Field::Permission => "this",
            Field::Effort => "this effort",
            Field::Worktree => "this",
        }
    }
}

impl Dent {
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

    /// A machine's own defaults (not a project's), as this device knows
    /// them, a change on its way counted.
    fn machine_defaults(&self, machine: &str) -> Defaults {
        self.pending_defaults
            .iter()
            .rev()
            .find(|p| p.machine == machine && p.project.is_none())
            .map(|p| p.defaults.clone())
            .or_else(|| self.setups.get(machine).and_then(|s| s.defaults.clone()))
            .unwrap_or_default()
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
        let mut d = match &project {
            None => self.machine_defaults(machine),
            Some(p) => self
                .pending_defaults
                .iter()
                .rev()
                .find(|x| x.machine == machine && x.project.as_deref() == Some(p.as_str()))
                .map(|x| x.defaults.clone())
                .or_else(|| {
                    self.setups
                        .get(machine)
                        .and_then(|s| s.project_defaults.get(p).cloned())
                })
                .unwrap_or_default(),
        };
        f.take(&mut d, &now);
        self.put_defaults(machine, project, d, cx);
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
        let is_default = f.of(&self.applied) == now
            || (f == Field::Permission
                && now.permission == Some(Permission::Full)
                && self.applied.permission.is_none())
            || (f == Field::Worktree
                && now.worktree == Some(false)
                && self.applied.worktree.is_none());
        let mut v = vec![Entry::separator()];
        if is_default {
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
        if let Some(p) = self.defaults.project.clone() {
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
