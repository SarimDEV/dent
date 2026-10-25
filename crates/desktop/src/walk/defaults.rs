use super::*;

/// How new sessions start, kept on the machine (decision 0076): a machine
/// with none takes what this device used; a choice in the composer is for
/// that session only; Make default keeps it there, and the next session
/// starts with it.
pub(super) fn defaults() -> Vec<(f32, Act)> {
    vec![
        (0.3, Key("cmd-n")),
        (
            0.1,
            Until(
                "this machine says how it's set up, defaults among it",
                |d, _| d.setups.get("").is_some_and(|s| s.defaults.is_some()),
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "having none, it takes what this device used",
                |d, _| {
                    d.setups
                        .get("")
                        .and_then(|s| s.defaults.as_ref())
                        .is_some_and(|x| x.permission.is_some())
                        && d.pending_defaults.is_empty()
                },
                10.,
            ),
        ),
        (
            0.1,
            Call(|d, _, _| {
                START_PERM.store(d.defaults.permission as u8 as usize, Relaxed);
            }),
        ),
        // Picked for this session only.
        (0.2, Click("chip-permission")),
        (0.1, Until("the menu opens", |d, _| d.menu.is_some(), 3.)),
        (0.5, Shot("defaults-menu")),
        (0.1, Click(other_permission_key())),
        (0.2, Type(Field::Starter, "say: one off")),
        (0.2, Click("start")),
        (0.1, Until("it answers", |d, _| answered(d), 20.)),
        (0.3, Key("cmd-n")),
        (
            0.1,
            Until(
                "the next one starts from the default again",
                |d, _| d.defaults.permission as u8 as usize == START_PERM.load(Relaxed),
                3.,
            ),
        ),
        // Made the default: kept on the machine, and the next one has it.
        (0.2, Click("chip-permission")),
        (0.1, Click(other_permission_key())),
        (0.2, Click("chip-permission")),
        (
            0.1,
            Until(
                "picked, the menu offers to make it the default",
                |d, _| drawn(d, "make-default-machine").is_some(),
                3.,
            ),
        ),
        (0.5, Shot("defaults-make")),
        (0.1, Click("make-default-machine")),
        (
            0.1,
            Until(
                "the machine keeps it",
                |d, _| {
                    d.pending_defaults.is_empty()
                        && d.setups
                            .get("")
                            .and_then(|s| s.defaults.as_ref())
                            .and_then(|x| x.permission)
                            .is_some_and(|p| p as u8 as usize != START_PERM.load(Relaxed))
                },
                10.,
            ),
        ),
        (0.2, Type(Field::Starter, "say: with the new default")),
        (0.2, Click("start")),
        (0.1, Until("it answers", |d, _| answered(d), 20.)),
        (0.3, Key("cmd-n")),
        (
            0.1,
            Until(
                "the next one starts with the new default",
                |d, _| d.defaults.permission as u8 as usize != START_PERM.load(Relaxed),
                3.,
            ),
        ),
        (0.2, Click("chip-permission")),
        (
            0.1,
            Until(
                "and its menu says it's the default",
                |d, _| d.menu.is_some() && drawn(d, "make-default-machine").is_none(),
                3.,
            ),
        ),
        (0.5, Shot("defaults-is")),
        (0.1, Key("escape")),
        // Set from another device meanwhile: what arrives takes the place
        // of what the composer has, where nothing's been picked.
        (
            0.1,
            Call(|d, _, cx| {
                let mut m = d.setups.get("").cloned().unwrap_or_default();
                m.defaults = Some(dent_protocol::Defaults {
                    permission: Some(dent_protocol::Permission::Plan),
                    ..Default::default()
                });
                m.defaults_set = true;
                d.setup_came(String::new(), m, cx);
            }),
        ),
        (
            0.1,
            Until(
                "defaults set elsewhere reach the composer",
                |d, _| d.defaults.permission == dent_protocol::Permission::Plan,
                2.,
            ),
        ),
        // Made the default everywhere while a machine was never heard
        // from: only that goes there, over what it has.
        (
            0.1,
            Call(|d, w, cx| {
                d.defaults.effort = Some("high".into());
                let mut v = crate::machines::MachineView::saved(dent_remote::store::Saved {
                    id: "zz01".into(),
                    name: "far-away".into(),
                    ..Default::default()
                });
                v.state = dent_client_core::machine::State::Unreachable { why: "off".into() };
                d.machines.push(v);
                if let Some(e) = d
                    .menu_entries(crate::app::Menu::Effort, cx)
                    .into_iter()
                    .find(|e| e.key == "make-default-all")
                {
                    d.choose_entry(&e, w, cx);
                }
                let m = dent_protocol::Machine {
                    defaults: Some(dent_protocol::Defaults {
                        model: Some("opus".into()),
                        permission: Some(dent_protocol::Permission::Ask),
                        ..Default::default()
                    }),
                    defaults_set: true,
                    ..Default::default()
                };
                d.setup_came("zz01".into(), m, cx);
            }),
        ),
        (
            0.1,
            Until(
                "a machine not heard from keeps what it had, with only that over it",
                |d, _| {
                    let x = d.defaults_for("zz01", None);
                    x.model.as_deref() == Some("opus")
                        && x.permission == Some(dent_protocol::Permission::Ask)
                        && x.effort.as_deref() == Some("high")
                },
                2.,
            ),
        ),
        // Cleared on purpose: not taken for a machine never set up.
        (
            0.1,
            Call(|d, _, cx| {
                let m = dent_protocol::Machine {
                    defaults: Some(Default::default()),
                    defaults_set: true,
                    ..Default::default()
                };
                d.pending_defaults.retain(|p| p.machine != "zz02");
                d.setup_came("zz02".into(), m, cx);
            }),
        ),
        (
            0.1,
            Until(
                "defaults cleared on purpose aren't filled in from this device",
                |d, _| !d.pending_defaults.iter().any(|p| p.machine == "zz02"),
                1.,
            ),
        ),
    ]
}

use std::sync::atomic::Ordering::Relaxed;

/// The permission new sessions started with, before.
static START_PERM: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The permission menu's row for one other than what new sessions start
/// with now (Full, else Ask).
fn other_permission_key() -> &'static str {
    if START_PERM.load(Relaxed) == dent_protocol::Permission::Full as u8 as usize {
        "m-ask"
    } else {
        "m-full"
    }
}
