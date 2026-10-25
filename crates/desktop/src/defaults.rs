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
    /// For a merge: the choices it sets back to the default, which
    /// `defaults` can't say (there, nothing means "leave it").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unset: Vec<String>,
}

impl Pending {
    /// Whether it changes choice `f`: a merge only what it names; a whole
    /// set, what it's said to replace.
    pub fn touches(&self, f: Field) -> bool {
        !self.merge || !f.is_unset(&self.defaults) || self.unset.iter().any(|k| k == f.name())
    }

    /// What `base` becomes with this over it.
    fn over(&self, base: &Defaults) -> Defaults {
        if !self.merge {
            return self.defaults.clone();
        }
        let mut d = base.under(&self.defaults);
        for f in self.unset.iter().filter_map(|k| Field::named(k)) {
            f.take(&mut d, &Defaults::default());
        }
        d
    }
}

/// A change made while `had` still waits for the same machine and project,
/// folded into one: a whole set takes the change into it; two merges make
/// one, the later choice winning.
fn fold(had: Option<Pending>, new: Pending) -> Pending {
    match had {
        Some(old) if new.merge => {
            let sets: Vec<&str> = Field::ALL
                .into_iter()
                .filter(|f| !f.is_unset(&new.defaults))
                .map(Field::name)
                .collect();
            let mut unset: Vec<String> = old
                .unset
                .iter()
                .filter(|k| !sets.contains(&k.as_str()))
                .cloned()
                .collect();
            for k in &new.unset {
                if !unset.contains(k) {
                    unset.push(k.clone());
                }
            }
            Pending {
                // Over a whole set, still a whole set.
                defaults: if old.merge {
                    let mut d = old.defaults.under(&new.defaults);
                    for f in new.unset.iter().filter_map(|k| Field::named(k)) {
                        f.take(&mut d, &Defaults::default());
                    }
                    d
                } else {
                    new.over(&old.defaults)
                },
                merge: old.merge,
                unset: if old.merge { unset } else { Vec::new() },
                ..new
            }
        }
        _ => new,
    }
}

/// Another agent made the default brings its own model and effort, as
/// `from` has them: another agent's never go with it.
fn agent_alone(f: Field, d: &mut Defaults, from: &Defaults) {
    if f == Field::Agent {
        Field::Model.take(d, from);
        Field::Effort.take(d, from);
    }
}

/// Whether a machine said no to a change for what it is, rather than
/// failing to keep it just then.
fn refused(why: &str) -> bool {
    [
        "isn't a project's folder",
        "isn't a",
        "newer dent",
        "bad request",
    ]
    .iter()
    .any(|w| why.contains(w))
}

/// The changes waiting, one for each machine and project: what an older
/// build kept as several, folded in order.
pub fn folded(all: Vec<Pending>) -> Vec<Pending> {
    let mut out: Vec<Pending> = Vec::new();
    for p in all {
        let had = out
            .iter()
            .position(|o| o.machine == p.machine && o.project == p.project)
            .map(|i| out.remove(i));
        out.push(match had {
            Some(old) if p.merge => fold(Some(old), p),
            _ => p,
        });
    }
    out
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
    const ALL: [Field; 5] = [
        Field::Agent,
        Field::Model,
        Field::Permission,
        Field::Effort,
        Field::Worktree,
    ];

    /// Its name, as a waiting change keeps it.
    fn name(self) -> &'static str {
        match self {
            Field::Agent => "agent",
            Field::Model => "model",
            Field::Permission => "permission",
            Field::Effort => "effort",
            Field::Worktree => "worktree",
        }
    }

    fn named(name: &str) -> Option<Field> {
        Field::ALL.into_iter().find(|f| f.name() == name)
    }

    /// `to` with this choice as `from` has it. The model picked as
    /// "Default" is no model set, so the two never read as different.
    pub(crate) fn take(self, to: &mut Defaults, from: &Defaults) {
        match self {
            Field::Agent => to.agent = from.agent.clone(),
            Field::Model => to.model = from.model.clone().filter(|m| m != "default"),
            Field::Permission => to.permission = from.permission,
            Field::Effort => to.effort = from.effort.clone(),
            Field::Worktree => to.worktree = from.worktree,
        }
    }

    /// As a merge must say it: `d` sets this choice back to the default.
    fn unsets(self, d: &Defaults) -> Vec<String> {
        if self.is_unset(d) {
            vec![self.name().to_string()]
        } else {
            Vec::new()
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
            .fold(kept, |d, p| p.over(&d))
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
    pub(crate) fn project_defaults(&self, machine: &str, project: &str) -> Defaults {
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
        self.put_change(machine, project, defaults, false, Vec::new(), cx);
    }

    /// [`Dent::put_defaults`], or with `merge`, what's set going over what
    /// the machine has (for one not heard from yet).
    fn put_change(
        &mut self,
        machine: &str,
        project: Option<String>,
        defaults: Defaults,
        merge: bool,
        unset: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        // One change waits for each machine and project, what came since
        // folded into it: sent whole, none undoes another.
        let had = self
            .pending_defaults
            .iter()
            .position(|p| p.machine == machine && p.project == project)
            .map(|i| self.pending_defaults.remove(i));
        let new = Pending {
            id: next_id(),
            machine: machine.to_string(),
            project,
            defaults,
            merge,
            unset,
        };
        let new = fold(had, new);
        self.pending_defaults.push(new);
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
            let has = match &p.project {
                None => kept.clone(),
                Some(pr) => setup.project_defaults.get(pr).cloned().unwrap_or_default(),
            };
            let whole = p.over(&has);
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
        // Only a whole set of its own made meanwhile counts: a merge goes
        // over what this device used, and a project's is the project's.
        let fresh = keeps
            && !setup.defaults_set
            && !self
                .pending_defaults
                .iter()
                .any(|p| p.machine == machine && p.project.is_none() && !p.merge);
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
            let mut d = self.last_used.clone();
            for p in self
                .pending_defaults
                .iter()
                .filter(|p| p.machine == machine && p.merge && p.project.is_none())
            {
                d = p.over(&d);
            }
            self.put_defaults(&machine, None, d, cx);
        } else {
            self.send_pending(&machine);
        }
        if machine == self.defaults.machine {
            self.apply_defaults(false);
        }
        cx.notify();
    }

    /// How new sessions start on `machine` changed, from whichever device:
    /// taken as it is there now.
    pub fn defaults_changed_there(
        &mut self,
        machine: &str,
        defaults: &Defaults,
        set: bool,
        projects: &std::collections::BTreeMap<String, Defaults>,
        cx: &mut Context<Self>,
    ) {
        // Not asked yet how it's set up: what it says then has all of it.
        let Some(s) = self.setups.get_mut(machine) else {
            return;
        };
        s.defaults = Some(defaults.clone());
        s.defaults_set = set;
        s.project_defaults = projects.clone();
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
        match answer {
            Ok(setup) => {
                self.pending_defaults.retain(|p| p.id != sent.id);
                self.setups.insert(sent.machine.clone(), setup);
                if sent.machine == self.defaults.machine {
                    self.apply_defaults(false);
                }
                self.prefs_changed();
            }
            // Refused there: sent again it's refused again, so it goes,
            // and you hear why. Anything else (its database busy, say)
            // waits to be sent again.
            Err(why) => {
                if refused(&why) && self.pending_defaults.iter().any(|p| p.id == sent.id) {
                    self.pending_defaults.retain(|p| p.id != sent.id);
                    let name = self.machine_name(&sent.machine);
                    self.toast(
                        crate::app::ToastAction::Info,
                        format!("{name} didn't take that change: {why}"),
                    );
                    if sent.machine == self.defaults.machine {
                        self.apply_defaults(false);
                    }
                    self.prefs_changed();
                }
            }
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
            None => self.put_change(machine, None, f.of(value), true, f.unsets(value), cx),
            Some(s) if s.defaults.is_none() => {}
            Some(_) => {
                let mut d = self.machine_defaults(machine);
                f.take(&mut d, value);
                agent_alone(f, &mut d, value);
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
            None => self.put_change(machine, project, f.of(&now), true, f.unsets(&now), cx),
            // Can't keep defaults at all.
            Some(s) if s.defaults.is_none() => {}
            Some(_) => {
                let mut d = match &project {
                    None => self.machine_defaults(machine),
                    Some(p) => self.project_defaults(machine, p),
                };
                f.take(&mut d, &now);
                agent_alone(f, &mut d, &now);
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

#[cfg(test)]
mod tests {
    use super::{Pending, fold};
    use dent_protocol::{Defaults, Permission};

    fn merge(d: Defaults, unset: &[&str]) -> Pending {
        Pending {
            id: 0,
            machine: "m".into(),
            project: None,
            defaults: d,
            merge: true,
            unset: unset.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn changes_made_while_a_machine_is_away_all_take() {
        let has = Defaults {
            model: Some("opus".into()),
            effort: Some("high".into()),
            permission: Some(Permission::Ask),
            ..Default::default()
        };
        // Model, then effort, then permission back to the default.
        let a = merge(
            Defaults {
                model: Some("sonnet".into()),
                ..Default::default()
            },
            &[],
        );
        let b = merge(
            Defaults {
                effort: Some("low".into()),
                ..Default::default()
            },
            &[],
        );
        let c = merge(Defaults::default(), &["permission"]);
        let one = fold(Some(fold(Some(a), b)), c);
        let now = one.over(&has);
        assert_eq!(now.model.as_deref(), Some("sonnet"));
        assert_eq!(now.effort.as_deref(), Some("low"));
        assert_eq!(now.permission, None);
        // Set back to the default, then set again: the later wins.
        let d = merge(
            Defaults {
                permission: Some(Permission::Full),
                ..Default::default()
            },
            &[],
        );
        assert_eq!(
            fold(Some(one), d).over(&has).permission,
            Some(Permission::Full)
        );
    }

    #[test]
    fn changes_an_older_build_kept_apart_are_folded_into_one() {
        let model = merge(
            Defaults {
                model: Some("sonnet".into()),
                ..Default::default()
            },
            &[],
        );
        let effort = merge(
            Defaults {
                effort: Some("low".into()),
                ..Default::default()
            },
            &[],
        );
        let other = Pending {
            machine: "n".into(),
            ..model.clone()
        };
        let one = super::folded(vec![model, other, effort]);
        assert_eq!(one.len(), 2);
        let m = one.iter().find(|p| p.machine == "m").unwrap();
        let now = m.over(&Defaults::default());
        assert_eq!(now.model.as_deref(), Some("sonnet"));
        assert_eq!(now.effort.as_deref(), Some("low"));
    }

    #[test]
    fn a_change_over_a_whole_set_waiting_stays_whole() {
        let whole = Pending {
            merge: false,
            ..merge(
                Defaults {
                    model: Some("opus".into()),
                    worktree: Some(true),
                    ..Default::default()
                },
                &[],
            )
        };
        let m = merge(Defaults::default(), &["model"]);
        let one = fold(Some(whole), m);
        assert!(!one.merge);
        assert_eq!(
            one.defaults,
            Defaults {
                worktree: Some(true),
                ..Default::default()
            }
        );
        // Whatever the machine has, a whole set is what it gets.
        let has = Defaults {
            effort: Some("high".into()),
            ..Default::default()
        };
        assert_eq!(one.over(&has), one.defaults);
    }
}
