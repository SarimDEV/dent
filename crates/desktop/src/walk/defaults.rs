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
