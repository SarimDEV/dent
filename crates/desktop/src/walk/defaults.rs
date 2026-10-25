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

/// Settings › Agents with twenty machines (some asleep, long names, three
/// set otherwise), to see it stays clean: tabs that fit and "N more",
/// Mixed grouped by value, a machine's tab, the value menu.
pub(super) fn settings_many() -> Vec<(f32, Act)> {
    vec![
        (
            0.2,
            Call(|d, _, cx| {
                let names = [
                    "dent-linux",
                    "gpu-box",
                    "mac-mini",
                    "gpu-2",
                    "build-box",
                    "studio",
                    "sarims-work-laptop-2024",
                    "ci-runner-east-1",
                    "ci-runner-east-2",
                    "ci-runner-west-1",
                    "homelab-nas",
                    "raspberrypi",
                    "office-imac",
                    "staging-api",
                    "staging-worker",
                    "prod-shadow",
                    "old-thinkpad",
                    "designer-mac",
                    "research-gpu-cluster-node-07",
                ];
                let asleep = ["mac-mini", "old-thinkpad", "designer-mac"];
                for (i, n) in names.iter().enumerate() {
                    let id = format!("m{i:02}");
                    let mut v = crate::machines::MachineView::saved(dent_remote::store::Saved {
                        id: id.clone(),
                        name: (*n).into(),
                        ..Default::default()
                    });
                    v.state = if asleep.contains(n) {
                        dent_client_core::machine::State::Away(dent_link::Goodbye::Sleep)
                    } else {
                        dent_client_core::machine::State::Online
                    };
                    v.rtt = (!asleep.contains(n)).then_some(4 + i as u64);
                    v.agents = d.agents.clone();
                    v.agents_known = true;
                    d.machines.push(v);
                    let ask = ["gpu-box", "gpu-2", "build-box"].contains(n);
                    let worktree =
                        n.starts_with("ci-") || n.starts_with("staging") || *n == "gpu-box";
                    d.setups.insert(
                        id,
                        dent_protocol::Machine {
                            git_user: Some(if n.starts_with("ci-") {
                                "ci-bot <ci@example.com>".into()
                            } else {
                                "Sarim Malik <sarim@example.com>".into()
                            }),
                            login_item: true,
                            defaults: Some(dent_protocol::Defaults {
                                permission: Some(if ask {
                                    dent_protocol::Permission::Ask
                                } else {
                                    dent_protocol::Permission::Full
                                }),
                                worktree: Some(worktree),
                                ..Default::default()
                            }),
                            defaults_set: true,
                            ..Default::default()
                        },
                    );
                }
                if let Some(s) = d.setups.get_mut("") {
                    s.defaults = Some(dent_protocol::Defaults {
                        permission: Some(dent_protocol::Permission::Full),
                        worktree: Some(false),
                        ..Default::default()
                    });
                }
                cx.notify();
            }),
        ),
        (0.3, Key("cmd-,")),
        (0.2, Click("settings-tab-agents")),
        (
            0.1,
            Until(
                "All machines lists the twenty, the rest behind more",
                |d, _| {
                    drawn(d, "agents-tab-all").is_some()
                        && drawn(d, "chip-settingsmachines").is_some()
                },
                5.,
            ),
        ),
        (0.6, Shot("settings-all")),
        (0.2, Click("chip-settingsmachines")),
        (
            0.1,
            Until(
                "the others are listed",
                |d, _| d.menu == Some(crate::app::Menu::SettingsMachines),
                3.,
            ),
        ),
        (0.5, Shot("settings-more")),
        (
            0.1,
            Call(|d, w, cx| {
                if let Some(e) = d
                    .menu_entries(crate::app::Menu::SettingsMachines, cx)
                    .into_iter()
                    .find(|e| e.key == "agents-more-m04")
                {
                    d.choose_entry(&e, w, cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "chosen from more, it takes a tab, and says what differs",
                |d, _| {
                    d.agents_tab.as_deref() == Some("m04") && drawn(d, "agents-tab-m04").is_some()
                },
                3.,
            ),
        ),
        (0.6, Shot("settings-machine")),
        (0.2, Click("agents-tab-all")),
        (0.2, Click("chip-setdefault(permission)")),
        (
            0.1,
            Until("its values, set on all", |d, _| d.menu.is_some(), 3.),
        ),
        (0.5, Shot("settings-set-all")),
        (0.1, Key("escape")),
        // Searched, the machine lit is one of those found, and Return opens it.
        (0.2, Click("chip-settingsmachines")),
        (
            0.1,
            Until(
                "the list opens",
                |d, _| d.menu == Some(crate::app::Menu::SettingsMachines),
                3.,
            ),
        ),
        (0.4, Keys("gpu-b")),
        (
            0.1,
            Until(
                "typed, a machine found is lit",
                |d, _| d.menu == Some(crate::app::Menu::SettingsMachines) && d.menu_ix == Some(0),
                3.,
            ),
        ),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "and Return opens it",
                |d, _| d.agents_tab.as_deref() == Some("m01"),
                3.,
            ),
        ),
        // One set like most others says nothing differs.
        (0.2, Click("agents-tab-m00")),
        (
            0.1,
            Until(
                "a machine like the others is marked nowhere",
                |d, _| {
                    drawn(d, "row-tab-permission").is_some() && drawn(d, "use-permission").is_none()
                },
                3.,
            ),
        ),
        // Make all: every machine the same, those away taking it when back.
        (0.2, Click("agents-tab-all")),
        (0.2, Click("make-all-permission")),
        (
            0.1,
            Until(
                "Make all makes them one",
                |d, _| {
                    drawn(d, "make-all-permission").is_none()
                        && drawn(d, "row-all-permission").is_some()
                },
                3.,
            ),
        ),
        (0.5, Shot("settings-made-all")),
        // Twenty machines on their page: every shape used, solid then
        // outlined, before any comes again.
        (0.2, Click("settings-tab-machines")),
        (
            0.1,
            Until(
                "all twelve shapes are used before any repeats",
                |d, _| {
                    let shapes = d.shapes();
                    let used: std::collections::HashSet<_> = shapes.values().collect();
                    shapes.len() >= 20 && used.len() == 12
                },
                3.,
            ),
        ),
        (0.6, Shot("settings-machines-shapes")),
    ]
}

/// Settings › Agents with only this Mac: no tabs, its page as it is.
pub(super) fn settings_one() -> Vec<(f32, Act)> {
    vec![
        (0.3, Key("cmd-,")),
        (0.2, Click("settings-tab-agents")),
        (
            0.1,
            Until(
                "one machine: its own settings, no tabs",
                |d, _| {
                    drawn(d, "agents-tab-all").is_none() && drawn(d, "row-tab-permission").is_some()
                },
                5.,
            ),
        ),
        (0.5, Shot("settings-one")),
        (0.2, Click("settings-tab-general")),
        (
            0.1,
            Until(
                "General holds this app's own",
                |d, _| drawn(d, "row-notifications").is_some() && drawn(d, "row-git").is_none(),
                3.,
            ),
        ),
        (0.4, Shot("settings-general")),
    ]
}

/// Settings › Agents against a machine really saved (`DENT_CLIENT_STATE`):
/// it says how new sessions start there; one set on its tab is kept on
/// it, and set back. Not in `checks`: it needs another machine.
pub(super) fn settings_live() -> Vec<(f32, Act)> {
    vec![
        (
            0.1,
            Until(
                "a saved machine comes online and says its defaults",
                |d, _| {
                    d.machines.iter().any(|m| {
                        m.rtt.is_some() && d.setups.get(&m.id).is_some_and(|s| s.defaults.is_some())
                    })
                },
                30.,
            ),
        ),
        (0.3, Key("cmd-,")),
        (0.2, Click("settings-tab-agents")),
        (0.6, Shot("live-all")),
        (
            0.2,
            Call(|d, _, cx| {
                let Some(m) = d
                    .machines
                    .iter()
                    .find(|m| m.rtt.is_some())
                    .map(|m| m.id.clone())
                else {
                    return;
                };
                LIVE_WAS
                    .with(|w| *w.borrow_mut() = d.setups.get(&m).and_then(|s| s.defaults.clone()));
                d.agents_tab = Some(m.clone());
                let value = dent_protocol::Defaults {
                    effort: Some("high".into()),
                    ..Default::default()
                };
                d.set_field(crate::defaults::Field::Effort, &value, Some(&m), cx);
            }),
        ),
        (
            0.1,
            Until(
                "set on its tab, the machine itself keeps it",
                |d, _| {
                    d.pending_defaults.is_empty()
                        && d.machines.iter().filter(|m| m.rtt.is_some()).any(|m| {
                            d.setups
                                .get(&m.id)
                                .and_then(|s| s.defaults.as_ref())
                                .is_some_and(|x| x.effort.as_deref() == Some("high"))
                        })
                },
                15.,
            ),
        ),
        (0.6, Shot("live-machine")),
        // Put back as it was.
        (
            0.2,
            Call(|d, _, cx| {
                let Some(m) = d.agents_tab.clone() else {
                    return;
                };
                let was = LIVE_WAS.with(|w| w.borrow().clone()).unwrap_or_default();
                d.put_defaults(&m, None, was, cx);
            }),
        ),
        (
            0.1,
            Until(
                "and back as it was",
                |d, _| d.pending_defaults.is_empty(),
                15.,
            ),
        ),
    ]
}

thread_local! {
    /// The machine's defaults before [`settings_live`] changed them.
    static LIVE_WAS: std::cell::RefCell<Option<dent_protocol::Defaults>> = const { std::cell::RefCell::new(None) };
}

/// The walk's project, as the daemon names it.
fn walk_proj() -> String {
    let p = std::env::var("DENT_WALK_PROJECT").unwrap_or_default();
    std::fs::canonicalize(&p)
        .map(|p| p.display().to_string())
        .unwrap_or(p)
}

/// A project given its own defaults in Settings › Agents: found in the
/// machine's projects, a choice set and kept there, the machine's shown
/// for the rest, then taken off again.
pub(super) fn settings_projects() -> Vec<(f32, Act)> {
    vec![
        (
            0.1,
            Call(|d, _, _| {
                d.live.send(dent_protocol::Request::AddProject {
                    path: walk_proj(),
                    create: false,
                });
            }),
        ),
        (0.3, Key("cmd-,")),
        (0.2, Click("settings-tab-agents")),
        (
            0.1,
            Until(
                "a machine keeping defaults offers to give a project its own",
                |d, _| drawn(d, "row-proj-add").is_some(),
                5.,
            ),
        ),
        (0.2, Click("chip-keepproject")),
        (
            0.1,
            Until(
                "its projects are listed",
                |d, cx| {
                    d.menu == Some(crate::app::Menu::KeepProject)
                        && d.keep_project_entries(cx)
                            .iter()
                            .any(|e| e.key == format!("keep-{}", walk_proj()))
                },
                5.,
            ),
        ),
        (0.4, Shot("projects-choose")),
        (0.3, Keys("zzz-none")),
        (
            0.1,
            Until(
                "searched for nothing, it says so",
                |d, cx| d.keep_project_entries(cx).iter().all(|e| !e.choosable()),
                3.,
            ),
        ),
        (
            0.1,
            Call(|d, w, cx| {
                d.menu_query
                    .update(cx, |f, cx| crate::app::put(f, "", w, cx));
                if let Some(e) = d
                    .keep_project_entries(cx)
                    .into_iter()
                    .find(|e| e.key == format!("keep-{}", walk_proj()))
                {
                    d.choose_entry(&e, w, cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "chosen, it opens with each choice as the machine's",
                |d, _| {
                    drawn(d, "row-proj-permission").is_some()
                        && drawn(d, "chip-setprojectdefault(permission)").is_some()
                },
                3.,
            ),
        ),
        (0.4, Shot("projects-open")),
        (0.2, Click("chip-setprojectdefault(permission)")),
        (
            0.1,
            Until(
                "its menu offers the machine's or one of its own",
                |d, _| {
                    d.menu
                        == Some(crate::app::Menu::SetProjectDefault(
                            crate::defaults::Field::Permission,
                        ))
                },
                3.,
            ),
        ),
        (
            0.1,
            Call(|d, w, cx| {
                let f = crate::defaults::Field::Permission;
                if let Some(e) = d
                    .menu_entries(crate::app::Menu::SetProjectDefault(f), cx)
                    .into_iter()
                    .find(|e| e.choosable() && !e.on && !e.key.ends_with("-machine"))
                {
                    d.choose_entry(&e, w, cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "set, the machine keeps it for that project alone",
                |d, _| {
                    let s = d.setups.get("");
                    s.and_then(|s| s.project_defaults.get(&walk_proj()))
                        .is_some_and(|p| p.permission.is_some() && p.worktree.is_none())
                        && d.pending_defaults.is_empty()
                },
                5.,
            ),
        ),
        (0.4, Shot("projects-set")),
        (0.2, Click("proj-done")),
        (
            0.1,
            Until(
                "closed, it says what it sets, to change or remove",
                |d, _| {
                    drawn(d, &format!("proj-remove-{}", walk_proj())).is_some()
                        && drawn(d, "row-proj-permission").is_none()
                },
                3.,
            ),
        ),
        (0.4, Shot("projects-closed")),
        (
            0.1,
            Call(|d, _, cx| {
                d.put_defaults("", Some(walk_proj()), Default::default(), cx);
            }),
        ),
        (
            0.1,
            Until(
                "removed, it's gone from the machine and the page",
                |d, _| {
                    d.setups
                        .get("")
                        .is_some_and(|s| !s.project_defaults.contains_key(&walk_proj()))
                        && drawn(d, &format!("proj-remove-{}", walk_proj())).is_none()
                },
                5.,
            ),
        ),
    ]
}

/// Who commits are by, changed on this Mac's row: something that isn't a
/// name and address is refused there, then a real one is saved. Walked
/// with a Git settings file of its own (see walk.sh).
pub(super) fn git_user() -> Vec<(f32, Act)> {
    // It saves who commits are by for real: only with walk.sh's own Git
    // settings file, never yours.
    let own = std::env::var("GIT_CONFIG_GLOBAL").is_ok_and(|p| p.contains("dent-walk."));
    if !own {
        return vec![(
            0.,
            Until(
                "walked by walk.sh, with a Git settings file of its own",
                |_, _| false,
                0.1,
            ),
        )];
    }
    vec![
        (0.3, Key("cmd-,")),
        (0.2, Click("settings-tab-agents")),
        (
            0.1,
            Until(
                "Commits by can be changed",
                |d, _| drawn(d, "git-user-change").is_some(),
                5.,
            ),
        ),
        (0.2, Click("git-user-change")),
        (0.3, Keys("just a name")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "not a name and address: said on the row, the field kept",
                |d, _| {
                    d.git_user_edit
                        .as_ref()
                        .is_some_and(|e| e.error.is_some() && e.sent.is_none())
                },
                3.,
            ),
        ),
        (0.4, Shot("git-user-bad")),
        (0.1, Key("cmd-a")),
        (0.3, Keys("Ada Lovelace <ada@example.com>")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "saved there, the field closes and the row says who",
                |d, _| {
                    d.git_user_edit.is_none()
                        && d.setups.get("").and_then(|s| s.git_user.as_deref())
                            == Some("Ada Lovelace <ada@example.com>")
                },
                8.,
            ),
        ),
        (0.4, Shot("git-user-saved")),
        (0.2, Click("git-user-change")),
        (0.1, Key("escape")),
        (
            0.1,
            Until(
                "Esc closes it unsaved",
                |d, _| d.git_user_edit.is_none(),
                3.,
            ),
        ),
    ]
}

/// Settings at its limits: a machine with a name too long for its tab,
/// sixty projects with their own (two of one name, one with a path past
/// any width), five hundred projects to find one in, someone committing
/// under a 200-character name, and machines split evenly. Walked at any
/// size, light or dark, for the stills.
pub(super) fn settings_limits() -> Vec<(f32, Act)> {
    vec![
        (
            0.2,
            Call(|d, _, cx| {
                use dent_protocol::{Defaults, Permission};
                let long = "research-gpu-cluster-node-07-in-the-basement-rack-b-left";
                for (i, n) in [long, "gpu-box", "mac-mini", "ci-runner", "ci-runner-2"]
                    .iter()
                    .enumerate()
                {
                    let id = format!("l{i}");
                    let mut v = crate::machines::MachineView::saved(dent_remote::store::Saved {
                        id: id.clone(),
                        name: (*n).into(),
                        ..Default::default()
                    });
                    v.state = dent_client_core::machine::State::Online;
                    v.rtt = Some(3 + i as u64);
                    v.agents = d.agents.clone();
                    v.agents_known = true;
                    d.machines.push(v);
                    let mut projects = std::collections::BTreeMap::new();
                    if i == 0 {
                        for k in 0..60 {
                            let path = match k {
                                0 => "/Users/ada/work/app".to_string(),
                                1 => "/Users/ada/personal/app".to_string(),
                                2 => format!(
                                    "/Users/ada/{}/deep",
                                    "very-long-folder-name/".repeat(12)
                                ),
                                k => {
                                    format!("/Users/ada/src/project-{k:02}-{}", "x".repeat(k % 30))
                                }
                            };
                            projects.insert(
                                path,
                                Defaults {
                                    permission: Some(if k % 2 == 0 {
                                        Permission::Ask
                                    } else {
                                        Permission::Full
                                    }),
                                    worktree: (k % 3 == 0).then_some(true),
                                    effort: (k % 5 == 0).then(|| "high".to_string()),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                    d.setups.insert(
                        id.clone(),
                        dent_protocol::Machine {
                            git: Some("git version 2.50.1".into()),
                            git_user: Some(format!(
                                "{} <{}@example.com>",
                                "Bartholomew-Maximilian ".repeat(9).trim(),
                                "b".repeat(40)
                            )),
                            git_user_settable: true,
                            login_item: true,
                            // With this Mac's Supervised, three and three.
                            defaults: Some(Defaults {
                                permission: Some(if i == 0 || i == 4 {
                                    Permission::Ask
                                } else {
                                    Permission::Full
                                }),
                                ..Default::default()
                            }),
                            defaults_set: true,
                            project_defaults: projects,
                            ..Default::default()
                        },
                    );
                    if i == 0 {
                        let all: Vec<dent_protocol::Project> = (0..500)
                            .map(|k| dent_protocol::Project {
                                path: format!(
                                    "/Users/ada/src/{}/repo-{k:03}",
                                    "nested/".repeat(k % 7)
                                ),
                                name: format!("repo-{k:03}"),
                                last_used: 0,
                            })
                            .collect();
                        d.tab_projects.insert(id, Ok(all));
                    }
                }
                cx.notify();
            }),
        ),
        (0.3, Key("cmd-,")),
        (0.2, Click("settings-tab-agents")),
        (
            0.1,
            Until(
                "split evenly, none is called the odd one out",
                |d, _| {
                    drawn(d, "row-all-permission").is_some()
                        && drawn(d, "make-all-permission").is_none()
                },
                5.,
            ),
        ),
        (0.5, Shot("limits-all")),
        (
            0.1,
            Call(|d, _, cx| {
                d.agents_tab = Some("l0".into());
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "sixty projects with their own, two of one name told apart",
                |d, _| {
                    drawn(d, "row-proj-/Users/ada/work/app").is_some()
                        && drawn(d, "row-proj-add").is_some()
                },
                5.,
            ),
        ),
        (0.5, Shot("limits-machine")),
        (0.2, Click("chip-keepproject")),
        (
            0.1,
            Until(
                "five hundred to choose from",
                |d, cx| d.keep_project_entries(cx).len() >= 400,
                3.,
            ),
        ),
        (0.4, Shot("limits-choose")),
        (0.3, Keys("repo-49")),
        (
            0.1,
            Until(
                "found by name among them",
                |d, cx| {
                    let e = d.keep_project_entries(cx);
                    !e.is_empty()
                        && e.len() < 20
                        && e.iter()
                            .all(|e| !e.choosable() || e.key.contains("repo-49"))
                },
                3.,
            ),
        ),
        (0.4, Shot("limits-found")),
        (0.1, Key("escape")),
        (
            0.1,
            Call(|d, w, cx| {
                d.edit_git_user("l0", Some("x".repeat(400)), w, cx);
            }),
        ),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "a 400-character entry is refused on the row",
                |d, _| d.git_user_edit.as_ref().is_some_and(|e| e.error.is_some()),
                3.,
            ),
        ),
        (0.4, Shot("limits-git-user")),
        (0.1, Key("escape")),
        (
            0.2,
            Call(|d, _, cx| {
                d.projects_all_shown = true;
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "Show all shows every one",
                |d, _| drawn(d, "row-proj-more").is_none() && d.projects_all_shown,
                3.,
            ),
        ),
        (0.2, Wheel("pane-settings", -6000.)),
        (0.5, Shot("limits-machine-end")),
    ]
}
