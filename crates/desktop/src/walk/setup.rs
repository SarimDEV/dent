use super::*;

use crate::settings::Pane;

/// Pages and Terminal runs pressed for so far, as one line to search.
fn opened(d: &Dent) -> String {
    d.opened_outside.join("\n")
}

/// Settings (⌘,), a page of its own: every agent and this Mac say how they
/// stand, and a preference does what its menu item does.
pub(super) fn settings() -> Vec<(f32, Act)> {
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (0.2, Key("cmd-,")),
        (
            0.1,
            Until(
                "⌘, opens Settings",
                |d, _| d.pane == Some(Pane::Settings) && drawn(d, "pane-settings").is_some(),
                3.,
            ),
        ),
        (0.1, Click("settings-tab-agents")),
        (
            0.1,
            Until(
                "the agent says its version and who it's signed in as",
                |d, _| {
                    d.agents_known
                        && d.agents.first().is_some_and(|a| {
                            crate::agents::version_line(a) == "2.1.301"
                                && crate::agents::account_line(a).contains("walk@example.com")
                        })
                        && drawn(d, "row-agent-claude").is_some()
                },
                15.,
            ),
        ),
        // This Mac's own settings sit on its Agents page.
        (
            0.1,
            Until(
                "this Mac says how Git and GitHub stand",
                |d, _| {
                    d.machine_check.as_ref().is_some_and(|m| {
                        m.git.is_some()
                            && m.git_user.as_deref() == Some("walk <walk@example.com>")
                            && matches!(&m.gh, dent_protocol::Gh::SignedIn { user: Some(u) } if u == "walk")
                    }) && drawn(d, "row-gh").is_some()
                        && drawn(d, "row-login-item").is_some()
                },
                15.,
            ),
        ),
        (
            0.1,
            Until(
                "a scratch home never offers to start at login",
                |d, _| {
                    d.machine_check
                        .as_ref()
                        .is_some_and(|m| m.login_item_blocked.is_some())
                        && drawn(d, "login-item-switch").is_none()
                },
                2.,
            ),
        ),
        (0.1, Click("settings-tab-general")),
    ];
    both_themes(&mut v, "settings");
    v.extend([
        (0.1, Click("sound-switch")),
        (
            0.1,
            Until(
                "the sound switch does what the menu does",
                |_, _| !crate::notify::SOUND.load(std::sync::atomic::Ordering::Relaxed),
                2.,
            ),
        ),
        (0.2, Click("sound-switch")),
        (
            0.1,
            Until(
                "and back",
                |_, _| crate::notify::SOUND.load(std::sync::atomic::Ordering::Relaxed),
                2.,
            ),
        ),
        (0.2, Key("escape")),
        (0.1, Until("Esc closes it", |d, _| d.pane.is_none(), 2.)),
    ]);
    v
}

/// The first launch's welcome: what dent needs, then out of the way.
pub(super) fn welcome() -> Vec<(f32, Act)> {
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (
            0.1,
            Call(|d, _, cx| {
                d.onboarded = false;
                d.open_pane(Pane::Welcome, cx);
            }),
        ),
        (
            0.1,
            Until(
                "the welcome says how the agent and this Mac stand",
                |d, _| {
                    drawn(d, "pane-welcome").is_some()
                        && d.agents_known
                        && d.machine_check.is_some()
                        && drawn(d, "welcome-done").is_some()
                },
                15.,
            ),
        ),
    ];
    both_themes(&mut v, "welcome");
    v.extend([
        (0.2, Click("welcome-done")),
        (
            0.1,
            Until(
                "Get started closes it for good",
                |d, _| d.pane.is_none() && d.onboarded,
                2.,
            ),
        ),
    ]);
    v
}

/// Claude Code signed out (`FAKE_CLAUDE_SIGNED_OUT`): the new-session page
/// says so, and Sign in runs its sign-in in Terminal.
pub(super) fn signed_out() -> Vec<(f32, Act)> {
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (
            0.1,
            Until(
                "the new-session page says it's signed out",
                |d, _| drawn(d, "agent-banner").is_some() && drawn(d, "banner-sign-in").is_some(),
                15.,
            ),
        ),
    ];
    both_themes(&mut v, "signed-out");
    v.extend([
        (0.2, Click("banner-sign-in")),
        (
            0.1,
            Until(
                "Sign in runs its sign-in in Terminal",
                |d, _| opened(d).contains("terminal: ") && opened(d).contains("auth login"),
                2.,
            ),
        ),
        (0.2, Key("cmd-,")),
        (
            0.1,
            Until(
                "Settings offers it too",
                |d, _| drawn(d, "sign-in-claude").is_some(),
                10.,
            ),
        ),
        (0.3, Shot("settings-signed-out")),
        (0.2, Key("escape")),
    ]);
    v
}

/// Claude Code not installed (`DENT_CLAUDE` names nothing): the page says
/// so, with where to read how.
pub(super) fn missing() -> Vec<(f32, Act)> {
    vec![
        (0.3, Key("cmd-n")),
        (
            0.1,
            Until(
                "the new-session page says it isn't installed",
                |d, _| drawn(d, "agent-banner").is_some() && drawn(d, "banner-install").is_some(),
                15.,
            ),
        ),
        (0.4, Shot("missing")),
        (0.2, Click("banner-install")),
        (
            0.1,
            Until(
                "How to install opens its setup page",
                |d, _| opened(d).contains("code.claude.com/docs/en/setup"),
                2.,
            ),
        ),
    ]
}

/// An agent that can do less (`DENT_CAPS_OFF`): what it can't do isn't
/// offered, and asking says why instead of failing.
pub(super) fn limited() -> Vec<(f32, Act)> {
    // A message sent mid-turn waits rather than steering.
    let mut v = new_session("a long answer please");
    v.extend([
        (
            0.1,
            Until(
                "the session knows what its agent can't do",
                |d, _| {
                    d.current()
                        .is_some_and(|s| !s.caps.steer && !s.caps.stop_task)
                },
                10.,
            ),
        ),
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "steer this in")),
        (0.1, Key("cmd-enter")),
        (
            0.1,
            Until(
                "⌘↩ says it can't steer, and the message waits",
                |d, _| {
                    d.toast
                        .as_ref()
                        .is_some_and(|t| t.text.contains("can't take a message mid-turn"))
                        && queued_now(d)
                },
                5.,
            ),
        ),
        (0.3, Shot("limited-steer")),
        (
            0.1,
            Until(
                "the waiting message has no Send now",
                |d, _| {
                    let q = queued_ids(d);
                    !q.is_empty()
                        && drawn(d, &format!("steer-{}", q[0].0)).is_none()
                        && drawn(d, &format!("unqueue-{}", q[0].0)).is_some()
                },
                3.,
            ),
        ),
        (
            0.1,
            Until(
                "both turns finish",
                |d, _| status_is(d, Status::Done) && !queued_now(d),
                40.,
            ),
        ),
        (
            0.1,
            Until(
                "no Try again or Fork where it can't go back",
                |d, _| {
                    drawn(d, "close").is_some()
                        && drawn(d, "try-again").is_none()
                        && drawn(d, "fork").is_none()
                },
                3.,
            ),
        ),
    ]);
    // Background work it can't stop piece by piece has no Stop.
    v.extend(new_session("keep watching the app and the tests"));
    v.extend([
        (
            0.1,
            Until(
                "background work shows with no Stop",
                |d, _| {
                    drawn(d, "background-strip").is_some()
                        && d.current().is_some_and(|s| {
                            !s.background.is_empty()
                                && s.background
                                    .iter()
                                    .all(|t| drawn(d, &format!("bg-stop-{}", t.id)).is_none())
                        })
                },
                20.,
            ),
        ),
        (0.3, Shot("limited-background")),
        (
            0.1,
            Call(|d, _, cx| {
                // From Ask, ⇧Tab steps to Edits and back, past the Plan
                // mode it doesn't take.
                d.set_permission(dent_protocol::Permission::Ask, cx);
            }),
        ),
        (
            0.1,
            Until(
                "⇧Tab steps only through modes it takes",
                |d, _| {
                    d.current().is_some_and(|s| {
                        crate::app::next_permission(&s.caps, dent_protocol::Permission::Edits)
                            == dent_protocol::Permission::Ask
                    })
                },
                2.,
            ),
        ),
        (0.2, Click("chip-sessionpermission")),
        (
            0.1,
            Until(
                "and its menu leaves Plan out",
                |d, _| drawn(d, "m-edits").is_some() && drawn(d, "m-plan").is_none(),
                3.,
            ),
        ),
        (0.3, Shot("limited-modes")),
        (0.2, Key("escape")),
    ]);
    v
}

/// Settings › Machines: the strip places connected machines by round trip,
/// rows open to their details, and Add a machine opens and closes. The
/// machines are made up here: a walk has no other machine to reach.
pub(super) fn machines() -> Vec<(f32, Act)> {
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (0.2, Key("cmd-,")),
        (
            0.1,
            Until(
                "Settings is open",
                |d, _| drawn(d, "pane-settings").is_some(),
                3.,
            ),
        ),
        (0.1, Click("settings-tab-machines")),
        (
            0.1,
            Call(|d, _, cx| {
                d.machines.clear();
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "with only this Mac, the strip says so",
                |d, _| d.machines.is_empty() && drawn(d, "add-machine-empty").is_some(),
                3.,
            ),
        ),
        (0.2, Shot("machines-first-light")),
        (
            0.1,
            Call(|d, _, cx| {
                d.machines = made_up_machines();
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "every machine has its row",
                |d, _| {
                    drawn(d, "machines-strip").is_some()
                        && d.machines
                            .iter()
                            .all(|m| drawn(d, &format!("machine-row-{}", m.id)).is_some())
                },
                3.,
            ),
        ),
        (0.1, Click("machine-row-c27b19e4a0d2")),
        (
            0.1,
            Until(
                "a row opens to its details",
                |d, _| d.machine_open.as_deref() == Some("c27b19e4a0d2"),
                2.,
            ),
        ),
    ];
    both_themes(&mut v, "machines");
    v.extend([
        // A pairing code showing, then one that's run out.
        (
            0.1,
            Call(|d, _, cx| {
                d.pairing = Some(("482913".into(), crate::cards::now_ms() + 4 * 60_000 + 5_000));
                cx.notify();
            }),
        ),
        (0.4, Shot("machines-code-light")),
        (
            0.1,
            Call(|d, _, cx| {
                d.pairing = Some(("482913".into(), crate::cards::now_ms()));
                cx.notify();
            }),
        ),
        (0.4, Shot("machines-code-gone-light")),
        (
            0.1,
            Call(|d, _, cx| {
                d.pairing = None;
                cx.notify();
            }),
        ),
    ]);
    v.extend([
        (0.1, Click("add-machine")),
        (
            0.1,
            Until(
                "Add a machine opens",
                |d, _| drawn(d, "adding").is_some(),
                2.,
            ),
        ),
        (0.4, Shot("machines-adding-light")),
        // Something that can't be an address: said under the field, at once.
        (
            0.1,
            Call(|d, _, cx| d.start_adding("not a host".into(), cx)),
        ),
        (
            0.1,
            Until(
                "an address that can't be one is refused at once, with words",
                |d, _| d.adding.as_ref().is_some_and(|a| a.bad.is_some()),
                2.,
            ),
        ),
        (0.3, Shot("machines-adding-bad-light")),
        (0.2, Key("escape")),
        (
            0.1,
            Until(
                "Esc closes it, and Settings stays",
                |d, _| d.adding.is_none() && d.pane == Some(Pane::Settings),
                2.,
            ),
        ),
        (0.2, Key("escape")),
        (
            0.1,
            Until("Esc again closes Settings", |d, _| d.pane.is_none(), 2.),
        ),
        (0.1, Call(leave_made_up_machines)),
    ]);
    v
}

/// Back to this machine alone, so a scene joined after one with made-up
/// machines starts sessions here: their sessions, waiting messages and
/// the chip's choice go.
fn leave_made_up_machines(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    for m in std::mem::take(&mut d.machines) {
        for id in d.core.forget(&m.id) {
            d.rebuild(&id);
        }
    }
    d.outbox.clear();
    d.set_machine(String::new(), cx);
    cx.notify();
}

/// Updates (decision 0077): machines and this app with a newer build ready,
/// Update all in the header and Update in the status bar, then a press under
/// way. The app here isn't a dent.app, so it never quits.
pub(super) fn updates() -> Vec<(f32, Act)> {
    fn ready(d: &mut Dent) {
        let mut ms = made_up_machines();
        for m in ms.iter_mut().filter(|m| m.rtt.is_some()) {
            m.main_build = 412;
        }
        if let Some(m) = ms.iter_mut().find(|m| m.id == "8e2e34085ddf") {
            m.available = Some(dent_protocol::Available {
                main_build: 415,
                version: "0.1.0-b415-3f2a9c1e".into(),
            });
        }
        d.machines = ms;
        crate::updates::pretend_ready(Some(415));
    }
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (
            0.1,
            Call(|d, _, cx| {
                ready(d);
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "the status bar says there's an update",
                |d, _| drawn(d, "update-ready").is_some(),
                3.,
            ),
        ),
        (0.3, Shot("updates-status-light")),
        (0.2, Key("cmd-,")),
        (
            0.1,
            Until(
                "Settings is open",
                |d, _| drawn(d, "pane-settings").is_some(),
                3.,
            ),
        ),
        (0.1, Click("settings-tab-machines")),
        (
            0.1,
            Until(
                "Update all counts this Mac and the machine with a build ready",
                |d, _| {
                    d.updates_ready() == 2
                        && drawn(d, "update-all").is_some()
                        && drawn(d, "update-this").is_some()
                        && drawn(d, "update-8e2e34085ddf").is_some()
                },
                3.,
            ),
        ),
    ];
    both_themes(&mut v, "updates-ready");
    v.extend([
        (0.1, Click("update-all")),
        (
            0.1,
            Until(
                "a press is under way, and says so",
                |d, _| d.updating.is_some() && drawn(d, "update-all").is_some(),
                2.,
            ),
        ),
        (0.3, Shot("updates-running-light")),
        (
            0.1,
            Call(|d, _, cx| {
                d.updating = None;
                crate::updates::pretend_ready(None);
                for m in &mut d.machines {
                    m.available = None;
                }
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "with nothing ready, nothing offers an update",
                |d, _| {
                    d.updates_ready() == 0
                        && drawn(d, "update-all").is_none()
                        && drawn(d, "update-8e2e34085ddf").is_none()
                },
                2.,
            ),
        ),
        (0.3, Shot("updates-none-light")),
        (0.1, Key("escape")),
    ]);
    // Just updated: the session it was in opens again, once it's there.
    static WAS: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    v.extend(new_session("Tidy the README"));
    v.extend([
        (
            0.1,
            Call(|d, w, cx| {
                let id = d.selected.clone();
                *WAS.lock().unwrap() = id.clone();
                d.select(None, w, cx);
                d.resuming = Some((
                    crate::updates::Resume {
                        session: id,
                        window: None,
                        scroll: Some((0, 0.)),
                    },
                    std::time::Instant::now(),
                ));
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "after an update, the session it was in opens again",
                |d, _| {
                    d.resuming.is_none()
                        && d.selected.is_some()
                        && d.selected == *WAS.lock().unwrap()
                },
                5.,
            ),
        ),
    ]);
    v
}

fn made_up_machines() -> Vec<crate::machines::MachineView> {
    use dent_client_core::machine::State;
    use dent_link::{Access, Goodbye, Trust};
    // DENT_WALK_LONG: every name long, to see what gives way and how.
    let long = std::env::var_os("DENT_WALK_LONG").is_some();
    let m = |id: &str, name: &str, state: State, rtt: Option<u64>, via: &str| {
        let mut v = crate::machines::MachineView::saved(dent_remote::Saved {
            id: id.into(),
            name: if long {
                format!("{name}-in-the-basement-closet-with-a-long-name")
            } else {
                name.into()
            },
            routes: vec![via.into()],
            ..Default::default()
        });
        v.state = state;
        v.rtt = rtt;
        if rtt.is_some() {
            v.via = Some((
                via.into(),
                Trust::Tailscale {
                    access: Access::Full,
                },
            ));
            v.version = "0.1.0".into();
        }
        v
    };
    vec![
        m(
            "8e2e34085ddf",
            "dent-linux",
            State::Online,
            Some(3),
            "100.70.46.115:7480",
        ),
        m(
            "7b20f1c8e94a",
            "gpu-box",
            State::Online,
            Some(24),
            "gpu-box.example.ts.net:7480",
        ),
        m(
            "c27b19e4a0d2",
            "mac-mini",
            State::Away(Goodbye::Sleep),
            None,
            "192.168.0.40:7480",
        ),
        m(
            "5e0d44b91a7c",
            "build-box",
            State::Unreachable {
                why: "timed out".into(),
            },
            None,
            "10.0.0.9:7480",
        ),
        m(
            "9d4f02aa61b3",
            "studio",
            State::Away(Goodbye::Update {
                to: "0.4.3".into(),
                expected_down_ms: 4000,
            }),
            None,
            "studio.example.ts.net:7480",
        ),
    ]
    .into_iter()
    .map(|mut v| {
        // What each runs, as its last welcome said: one went back from an
        // update, one asleep on an older one.
        match v.id.as_str() {
            "8e2e34085ddf" => v.release = "0.4.3".into(),
            "7b20f1c8e94a" => {
                v.release = "0.4.2".into();
                v.update = Some(dent_protocol::UpdateOutcome {
                    id: "u7".into(),
                    from: "0.4.2".into(),
                    to: "0.4.3".into(),
                    rolled_back: true,
                });
            }
            "c27b19e4a0d2" => v.version = "0.4.1".into(),
            _ => {}
        }
        v
    })
    .collect()
}

/// Settings › Machines against the machines really saved
/// (`DENT_CLIENT_STATE`): waits for one to come online with a round trip.
/// Not in `checks`: it needs another machine.
pub(super) fn machines_live() -> Vec<(f32, Act)> {
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (0.2, Key("cmd-,")),
        (0.1, Click("settings-tab-machines")),
        (
            0.1,
            Until(
                "a saved machine comes online with a round trip",
                |d, _| {
                    d.machines
                        .iter()
                        .any(|m| m.rtt.is_some() && !m.saved.name.is_empty())
                },
                30.,
            ),
        ),
    ];
    both_themes(&mut v, "machines-live");
    v.extend([(0.2, Key("escape")), (0.5, Shot("sessions-live"))]);
    v
}

/// Bringing a project from a machine really saved (`DENT_CLIENT_STATE`)
/// to this one: the picker offers the other machine's `hello-world`, lists
/// the files Git ignores there with environment files ticked, and the
/// session starts here once its clone is made. Not in `checks`: it needs
/// another machine with that project.
pub(super) fn bring_live() -> Vec<(f32, Act)> {
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (
            0.1,
            Until(
                "a saved machine comes online",
                |d, _| d.machines.iter().any(|m| m.rtt.is_some()),
                30.,
            ),
        ),
        (0.2, Click("chip-project")),
        (0.2, Keys("hello")),
        (
            0.1,
            Until(
                "the other machine's project is offered",
                |d, cx| {
                    d.project_entries(cx)
                        .iter()
                        .any(|e| e.key.starts_with("b-"))
                },
                10.,
            ),
        ),
        (0.3, Shot("bring-offered-light")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "what comes with it is listed",
                |d, _| d.bring_draft.is_some(),
                10.,
            ),
        ),
    ];
    both_themes(&mut v, "bring-files");
    v.extend([
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "Bring chooses it, with its files read",
                |d, _| d.bringing.is_some() && d.menu.is_none(),
                10.,
            ),
        ),
        (0.3, Shot("bring-chosen-light")),
        (0.1, Type(Field::Starter, "say hi")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "the session works in the clone",
                |d, _| {
                    d.sessions.iter().any(|s| {
                        d.selected.as_deref() == Some(s.id.as_str())
                            && s.items.iter().any(|i| {
                                matches!(i, Item::Note { text, .. } if text.starts_with("Cloned"))
                            })
                    })
                },
                60.,
            ),
        ),
        (0.5, Shot("bring-cloned-light")),
    ]);
    v
}

/// A saved machine's sessions come from the log kept on this Mac, before
/// any link and while the machine is away: run after a scene that saw it
/// (`machines-live`), with its daemon stopped. Not in `checks`.
pub(super) fn kept_live() -> Vec<(f32, Act)> {
    vec![
        (
            0.0,
            Until(
                "another machine's sessions are there from what's kept",
                |d, _| {
                    d.sessions
                        .iter()
                        .any(|s| !dent_client_core::fleet::split(&s.id).0.is_empty())
                },
                1.,
            ),
        ),
        (
            0.1,
            Until(
                "and it isn't online",
                |d, _| {
                    use dent_client_core::machine::State;
                    d.machines
                        .iter()
                        .all(|m| !matches!(m.state, State::Online | State::Slow))
                },
                1.,
            ),
        ),
        (2.0, Shot("kept-live-light")),
    ]
}

/// Another machine's files, against sessions made there beforehand by
/// `scripts/files-fixtures.sh`: pictures every way a reply names them, the
/// same path holding a different file on this Mac, a missing one, a note,
/// a page with its own styles, a video, a tool's screenshot and a sent
/// picture. Not in `checks`: it needs the VM.
pub(super) fn files_live() -> Vec<(f32, Act)> {
    use crate::files::{Ask, Look, Why};
    fn session(d: &Dent, says: &str) -> Option<(String, String)> {
        d.sessions.iter().find_map(|s| {
            let far = !dent_client_core::fleet::split(&s.id).0.is_empty();
            let has = s.items.iter().any(|i| match i {
                Item::Text(t) => t.contains(says),
                Item::User { text, .. } => text.contains(says),
                _ => false,
            });
            (far && has).then(|| (s.id.clone(), s.cwd.clone()))
        })
    }
    fn pic(d: &Dent, says: &str, path: &str) -> Option<Look<std::path::PathBuf>> {
        let (id, cwd) = session(d, says)?;
        Some(crate::files::picture(&Ask::new(&id, path, &cwd)))
    }
    fn here(d: &Dent, says: &str, path: &str) -> bool {
        matches!(pic(d, says, path), Some(Look::Here(_)))
    }
    fn select(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>, says: &str) {
        if let Some((id, _)) = session(d, says) {
            d.select(Some(id), w, cx);
        }
    }
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (
            0.1,
            Until(
                "the VM is online",
                |d, _| d.machines.iter().any(|m| m.rtt.is_some()),
                30.,
            ),
        ),
        (
            0.1,
            Until(
                "its files session is here",
                |d, _| session(d, "Files test").is_some(),
                10.,
            ),
        ),
        (0.1, Call(|d, w, cx| select(d, w, cx, "Files test"))),
        (
            0.1,
            Until(
                "a picture named from the session's folder shows",
                |d, _| here(d, "Files test", "pics/a.png"),
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "so does one with a space and a # in its name",
                |d, _| here(d, "Files test", "pics/big shot#1.png"),
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "~ is the VM's home, not this Mac's",
                |d, _| match pic(d, "Files test", "~/Pictures/h.png") {
                    Some(Look::Here(p)) => {
                        std::fs::read(p).ok().as_deref()
                            == Some(
                                &include_bytes!("../../../fake-claude/assets/page-tablet.png")[..],
                            )
                    }
                    _ => false,
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "a path this Mac also has shows the VM's file, never this Mac's",
                |d, _| match pic(d, "Files test", "/tmp/dent-same.png") {
                    Some(Look::Here(p)) => {
                        std::fs::read(p).ok().as_deref()
                            == Some(&include_bytes!("../../../fake-claude/assets/page.png")[..])
                    }
                    _ => false,
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "a missing one says it's not on the VM, and where it looked",
                |d, _| {
                    matches!(
                        pic(d, "Files test", "nope.png"),
                        Some(Look::Gone(g)) if g.why == Why::Missing && g.checked.ends_with("/fx/nope.png")
                    )
                },
                10.,
            ),
        ),
    ];
    both_themes(&mut v, "files-live");
    v.extend([
        (
            0.1,
            Call(|d, _, cx| {
                if let Some((_, cwd)) = session(d, "Files test") {
                    d.open_file(&format!("{cwd}/doc.md"), cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "its note opens in the viewer",
                |d, _| d.panel_open && d.arts.iter().any(|a| a.title == "doc.md"),
                10.,
            ),
        ),
        (1.0, Shot("files-live-doc-light")),
        (
            0.1,
            Call(|d, _, cx| {
                d.far_served = 0;
                d.far_missed = 0;
                if let Some((_, cwd)) = session(d, "Files test") {
                    d.open_file(&format!("{cwd}/page.html"), cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "its page opens, its styles and both pictures from the VM",
                |d, _| {
                    d.arts.iter().any(|a| a.title == "page.html")
                        && d.far_served >= 3
                        && d.far_missed == 0
                        && d.far_waiting.is_empty()
                },
                10.,
            ),
        ),
        (2.0, Shot("files-live-page-light")),
        (
            0.1,
            Call(|d, _, cx| {
                d.far_served = 0;
                d.open_file("~/Movies/take.mp4", cx)
            }),
        ),
        (
            0.1,
            Until(
                "its video plays from the VM, a block at a time",
                |_, _| crate::stream::fetched_of("take.mp4") > 0,
                15.,
            ),
        ),
        (2.0, Shot("files-live-video-light")),
        (0.1, Call(|d, w, cx| select(d, w, cx, "screenshot"))),
        (
            0.1,
            Until(
                "a tool's screenshot shows",
                |d, _| {
                    let Some((id, cwd)) = session(d, "screenshot") else {
                        return false;
                    };
                    d.sessions.iter().filter(|s| s.id == id).any(|s| {
                        s.items.iter().any(|i| match i {
                            Item::Tools { pictures, .. } => pictures.iter().any(|p| {
                                matches!(
                                    crate::files::picture(&Ask::new(&id, p, &cwd)),
                                    Look::Here(_)
                                )
                            }),
                            _ => false,
                        })
                    })
                },
                10.,
            ),
        ),
        (0.5, Shot("files-live-screenshot-light")),
        (
            0.1,
            Call(|d, w, cx| select(d, w, cx, "look at this picture")),
        ),
        (
            0.1,
            Until(
                "a picture sent with a message shows",
                |d, _| {
                    let Some((id, cwd)) = session(d, "look at this picture") else {
                        return false;
                    };
                    d.sessions.iter().filter(|s| s.id == id).any(|s| {
                        s.items.iter().any(|i| match i {
                            Item::User { files, .. } => files.iter().any(|p| {
                                matches!(
                                    crate::files::picture(&Ask::new(&id, p, &cwd)),
                                    Look::Here(_)
                                )
                            }),
                            _ => false,
                        })
                    })
                },
                10.,
            ),
        ),
        (0.5, Shot("files-live-sent-light")),
        (0.1, Call(|d, w, cx| select(d, w, cx, "make a video"))),
        (1.5, Shot("files-live-made-video-light")),
    ]);
    v
}

/// Trying to break another machine's files, against the VM (after
/// `scripts/files-fixtures.sh` and the edge sessions in
/// `scripts/break-fixtures.sh`): files that can't be read, a folder named
/// like a picture, an empty one, a link loop, a file too big to show, a
/// 3 GB video, sixty pictures at once, a note rewritten while it's open,
/// and the VM's daemon going away and coming back. Not in `checks`.
pub(super) fn break_live() -> Vec<(f32, Act)> {
    use crate::files::{Ask, Look, Why};
    fn session(d: &Dent, says: &str) -> Option<(String, String)> {
        d.sessions.iter().find_map(|s| {
            let far = !dent_client_core::fleet::split(&s.id).0.is_empty();
            let has = s
                .items
                .iter()
                .any(|i| matches!(i, Item::Text(t) if t.contains(says)));
            (far && has).then(|| (s.id.clone(), s.cwd.clone()))
        })
    }
    fn pic(d: &Dent, says: &str, path: &str) -> Option<Look<std::path::PathBuf>> {
        let (id, cwd) = session(d, says)?;
        Some(crate::files::picture(&Ask::new(&id, path, &cwd)))
    }
    fn gone(d: &Dent, says: &str, path: &str, why: fn(&Why) -> bool) -> bool {
        matches!(pic(d, says, path), Some(Look::Gone(g)) if why(&g.why))
    }
    fn select(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>, says: &str) {
        if let Some((id, _)) = session(d, says) {
            d.select(Some(id), w, cx);
        }
    }
    fn vm(cmd: &str) {
        // The walkthrough runs with a home of its own; the VM is the real
        // home's.
        let user = std::env::var("USER").unwrap_or_default();
        let ok = std::process::Command::new("/opt/homebrew/bin/limactl")
            .args(["shell", "dent-linux", "--", "bash", "-lc", cmd])
            .env("HOME", format!("/Users/{user}"))
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "couldn't run on the VM: {cmd}");
    }
    fn online(d: &Dent) -> bool {
        use dent_client_core::machine::State;
        d.machines
            .iter()
            .any(|m| matches!(m.state, State::Online | State::Slow) && m.proto.is_some())
    }
    fn toast_has(d: &Dent, words: &str) -> bool {
        d.toast.as_ref().is_some_and(|t| t.text.contains(words))
    }
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (0.1, Until("the VM is online", |d, _| online(d), 30.)),
        (
            0.1,
            Until(
                "its edge session is here",
                |d, _| session(d, "Edge files").is_some(),
                10.,
            ),
        ),
        (0.1, Call(|d, w, cx| select(d, w, cx, "Edge files"))),
        (
            0.1,
            Until(
                "a pipe named as a picture is unreadable, and nothing hangs",
                |d, _| {
                    gone(d, "Edge files", "pipe.png", |w| {
                        matches!(w, Why::Unreadable(_))
                    })
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "a file it may not read says so",
                |d, _| {
                    gone(d, "Edge files", "locked.png", |w| {
                        matches!(w, Why::Unreadable(_))
                    })
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "a folder named like a picture is a folder",
                |d, _| gone(d, "Edge files", "dir.png", |w| *w == Why::Folder),
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "a link loop is unreadable",
                |d, _| {
                    gone(d, "Edge files", "loop1", |w| {
                        matches!(w, Why::Unreadable(_))
                    })
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "a name in another script shows",
                |d, _| {
                    matches!(
                        pic(d, "Edge files", "ünïcødé 名前.png"),
                        Some(Look::Here(_))
                    )
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "an empty file is here, empty",
                |d, _| match pic(d, "Edge files", "empty.png") {
                    Some(Look::Here(p)) => std::fs::metadata(p).is_ok_and(|m| m.len() == 0),
                    _ => false,
                },
                10.,
            ),
        ),
        (0.5, Shot("break-edges-light")),
        (
            0.1,
            Call(|d, _, cx| {
                if let Some((_, cwd)) = session(d, "Edge files") {
                    d.open_file(&format!("{cwd}/big.bin"), cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "a file too big to show says so, how big and where",
                |d, _| {
                    toast_has(d, "Too big to show here (47.7 MB)") && toast_has(d, "/fx/big.bin")
                },
                15.,
            ),
        ),
        (
            0.1,
            Call(|d, _, cx| {
                // A 3 GB file of its own each run, so nothing of it is open
                // or kept already.
                let name = format!("sparse-{}.mov", std::process::id());
                vm(&format!("truncate -s 3G ~/fx/{name}"));
                d.far_served = 0;
                if let Some((_, cwd)) = session(d, "Edge files") {
                    d.open_file(&format!("{cwd}/{name}"), cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "a 3 GB video starts playing from there, a block at a time",
                |_, _| crate::stream::fetched_of(&format!("sparse-{}.mov", std::process::id())) > 0,
                15.,
            ),
        ),
        (
            0.1,
            Until(
                "and isn't copied whole",
                |_, _| {
                    let mut total = 0u64;
                    let mut stack = vec![crate::files::cache_root()];
                    while let Some(dir) = stack.pop() {
                        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                            match e.metadata() {
                                Ok(m) if m.is_dir() => stack.push(e.path()),
                                Ok(m) => total += m.len(),
                                _ => {}
                            }
                        }
                    }
                    total < 100 << 20
                },
                1.,
            ),
        ),
        (0.1, Call(|d, w, cx| select(d, w, cx, "Many pictures"))),
        (
            0.1,
            Until(
                "sixty pictures in a reply all come",
                |d, _| {
                    (0..60).all(|i| {
                        matches!(
                            pic(d, "Many pictures", &format!("many/p{i:02}.png")),
                            Some(Look::Here(_))
                        )
                    })
                },
                30.,
            ),
        ),
        (0.5, Shot("break-many-light")),
        (0.1, Call(|d, w, cx| select(d, w, cx, "Files test"))),
        (
            0.1,
            Call(|d, _, cx| {
                if let Some((_, cwd)) = session(d, "Files test") {
                    d.open_file(&format!("{cwd}/doc.md"), cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "its note opens",
                |d, _| d.panel_open && d.arts.iter().any(|a| a.title == "doc.md"),
                10.,
            ),
        ),
        (
            0.1,
            Call(|_, _, _| vm("printf '# Notes\\n\\nRewritten on the VM.\\n' > ~/fx/doc.md")),
        ),
        (
            0.1,
            Until(
                "rewritten there while it's open, the new version shows",
                |d, _| {
                    d.arts
                        .iter()
                        .find(|a| a.title == "doc.md")
                        .and_then(|a| a.versions.last())
                        .is_some_and(|v| String::from_utf8_lossy(v).contains("Rewritten"))
                },
                15.,
            ),
        ),
        (0.5, Shot("break-rewritten-light")),
        (
            0.1,
            Call(|_, _, _| {
                vm(
                    "printf '# Notes\\n\\nA picture beside me:\\n\\n![a](pics/a.png)\\n' > ~/fx/doc.md",
                )
            }),
        ),
        (0.1, Call(|_, _, _| vm("kill $(pgrep -x dent-daemon)"))),
        (
            0.1,
            Until("the VM's daemon goes away", |d, _| !online(d), 20.),
        ),
        (
            0.1,
            Until(
                "a picture already here shows, marked as kept",
                |d, _| {
                    matches!(
                        pic(d, "Many pictures", "many/p00.png"),
                        Some(Look::Stale(_))
                    )
                },
                5.,
            ),
        ),
        (
            0.1,
            Until(
                "one never fetched says the VM is away",
                |d, _| {
                    gone(d, "Many pictures", "many/never-asked.png", |w| {
                        *w == Why::Away
                    })
                },
                5.,
            ),
        ),
        (0.5, Shot("break-away-light")),
        (
            0.1,
            Call(|_, _, _| {
                vm(
                    "cd ~/dent && (DENT_REMOTE_LAN=1 DENT_CLAUDE=$HOME/dent/target/release/dent-fake-claude setsid nohup target/release/dent-daemon --state $HOME/rt/dd2 >> ~/rt/dd2.log 2>&1 < /dev/null &)",
                )
            }),
        ),
        (0.1, Until("it comes back", |d, _| online(d), 60.)),
        (
            0.1,
            Until(
                "and its pictures are current again",
                |d, _| matches!(pic(d, "Many pictures", "many/p00.png"), Some(Look::Here(_))),
                15.,
            ),
        ),
    ];
    both_themes(&mut v, "break-back");
    v
}

/// What people do most, on the VM rather than this Mac: start a session
/// there from the machine chip, approve a command, follow up, send while it
/// works, stop it, answer a question, and start several in a row. Not in
/// `checks`: it needs the VM.
pub(super) fn remote_happy_live() -> Vec<(f32, Act)> {
    fn far(d: &Dent) -> bool {
        current(d).is_some_and(|s| !dent_client_core::fleet::split(&s.id).0.is_empty())
    }
    fn to_vm(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
        if let Some(id) = d
            .machines
            .iter()
            .find(|m| m.rtt.is_some())
            .map(|m| m.id.clone())
        {
            d.set_machine(id, cx);
        }
    }
    fn start(text: &'static str) -> Vec<(f32, Act)> {
        vec![
            (0.3, Key("cmd-n")),
            (0.1, Call(to_vm)),
            (0.2, Type(Field::Starter, text)),
            (0.2, Click("start")),
            (
                0.1,
                Until(
                    leak(format!("“{text}” starts a session on the VM")),
                    |d, _| far(d) && !d.creating,
                    15.,
                ),
            ),
        ]
    }
    let mut v = vec![(
        0.1,
        Until(
            "the VM is online",
            |d, _| d.machines.iter().any(|m| m.rtt.is_some()),
            30.,
        ),
    )];
    v.extend(start("please run it"));
    v.extend([
        (
            0.1,
            Until(
                "an approval from the VM appears and the session needs you",
                |d, _| {
                    has(d, |i| matches!(i, Item::Approval { .. })) && status_is(d, Status::Needs)
                },
                15.,
            ),
        ),
        (0.3, Shot("remote-approval-light")),
        (0.2, Click("allow")),
        (
            0.1,
            Until(
                "allowing runs it there",
                |d, _| done_saying(d, "Ran it"),
                15.,
            ),
        ),
        (0.2, Type(Field::Composer, "say: a follow up")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "a follow up is answered",
                |d, _| done_saying(d, "a follow up"),
                15.,
            ),
        ),
        (0.2, Type(Field::Composer, "a long answer please")),
        (0.1, Key("enter")),
        (
            0.1,
            Until("it streams from the VM", |d, _| streaming(d), 15.),
        ),
        (0.1, Type(Field::Composer, "and then this")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "sent while it works, it waits below the reply",
                |d, _| has(d, |i| matches!(i, Item::Queued { .. })),
                10.,
            ),
        ),
        (0.2, Shot("remote-queued-light")),
        (
            0.1,
            Until(
                "and goes after it",
                |d, _| {
                    done_saying(d, "and then this") && !has(d, |i| matches!(i, Item::Queued { .. }))
                },
                30.,
            ),
        ),
        (0.2, Type(Field::Composer, "another long one")),
        (0.1, Key("enter")),
        (0.1, Until("it streams again", |d, _| streaming(d), 15.)),
        (0.3, Key("cmd-.")),
        (
            0.1,
            Until(
                "⌘. stops it there",
                |d, _| current(d).is_some_and(|s| s.status != Status::Working),
                10.,
            ),
        ),
    ]);
    v.extend(start("ask me which cache"));
    v.extend([
        (
            0.1,
            Until(
                "a question from the VM",
                |d, _| has(d, |i| matches!(i, Item::Question { .. })),
                15.,
            ),
        ),
        (0.2, Click("opt-0-1")),
        (
            0.1,
            Until("one click answers it", |d, _| done_saying(d, "Redis"), 15.),
        ),
    ]);
    for text in ["say: quick one", "say: quick two", "say: quick three"] {
        v.extend(start(text));
        v.push((
            0.1,
            Until(
                leak(format!("“{text}” is answered there")),
                |d, _| answered(d),
                15.,
            ),
        ));
    }
    v.push((0.3, Shot("remote-happy-end-light")));
    v
}

/// The VM's log starts again (its database wiped, as a reinstall would):
/// what this Mac kept of the old one goes once the link says so (on a fast
/// link that's before the first look), and the new log's sessions come. Run after `machines-live` kept
/// history, with the VM's database moved aside. Not in `checks`.
pub(super) fn resync_live() -> Vec<(f32, Act)> {
    fn far_sessions(d: &Dent) -> usize {
        d.sessions
            .iter()
            .filter(|s| !dent_client_core::fleet::split(&s.id).0.is_empty())
            .count()
    }
    vec![
        (
            0.1,
            Until(
                "the VM is online, on its new log",
                |d, _| d.machines.iter().any(|m| m.rtt.is_some()),
                30.,
            ),
        ),
        (
            0.1,
            Until(
                "and the old log's sessions are gone",
                |d, _| far_sessions(d) == 0,
                10.,
            ),
        ),
        (0.5, Shot("resync-light")),
    ]
}

/// Many sessions on the VM (`dent ask` in a loop beforehand): they're all
/// there, and opening each one in turn is timed. Not in `checks`.
pub(super) fn many_live() -> Vec<(f32, Act)> {
    vec![
        (
            0.1,
            Until(
                "the VM is online with its many sessions",
                |d, _| {
                    d.machines.iter().any(|m| m.rtt.is_some())
                        && d.sessions
                            .iter()
                            .filter(|s| !dent_client_core::fleet::split(&s.id).0.is_empty())
                            .count()
                            >= 80
                },
                30.,
            ),
        ),
        (0.5, Shot("many-light")),
        (0.5, Sweep),
    ]
}

thread_local! {
    /// When `sleep-live` found the VM silent.
    static SILENT: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// The VM sleeping for real (logind tells its daemon, which tells this
/// Mac) and waking, then going silent with no goodbye (the whole VM frozen
/// from this Mac) and coming back: each reads as what it is, a picture kept
/// from before shows marked while the VM's away, and it comes back by
/// itself, current again. Run after `files-live`'s fixtures. Not in
/// `checks`.
pub(super) fn sleep_live() -> Vec<(f32, Act)> {
    use crate::files::{Ask, Look};
    use dent_client_core::machine::State;
    fn ask(d: &Dent) -> Option<Ask> {
        d.sessions.iter().find_map(|s| {
            let far = !dent_client_core::fleet::split(&s.id).0.is_empty();
            let has = s
                .items
                .iter()
                .any(|i| matches!(i, Item::Text(t) if t.contains("Files test")));
            (far && has).then(|| Ask::new(&s.id, "pics/a.png", &s.cwd))
        })
    }
    fn state(d: &Dent) -> Option<State> {
        d.machines.first().map(|m| m.state.clone())
    }
    fn online(d: &Dent) -> bool {
        matches!(state(d), Some(State::Online | State::Slow)) && d.machines[0].proto.is_some()
    }
    fn here(d: &Dent) -> bool {
        ask(d).is_some_and(|a| matches!(crate::files::picture(&a), Look::Here(_)))
    }
    fn kept(d: &Dent) -> bool {
        ask(d).is_some_and(|a| crate::files::kept(&a).is_some())
    }
    /// The VM's whole machine, stopped or let go on: from outside, as if
    /// its lid shut with no warning.
    fn freeze(stop: bool) {
        let ok = std::process::Command::new("pkill")
            .args([
                if stop { "-STOP" } else { "-CONT" },
                "-f",
                "com.apple.Virtualization.VirtualMachine",
            ])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "couldn't reach the VM's process");
    }
    vec![
        (0.1, Until("the VM is online", |d, _| online(d), 30.)),
        (
            0.1,
            Until("its files session is here", |d, _| ask(d).is_some(), 10.),
        ),
        (
            0.1,
            Call(|d, w, cx| {
                if let Some(a) = ask(d) {
                    d.select(Some(a.session.clone()), w, cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "its picture is here, current",
                |d, _| here(d) && !kept(d),
                10.,
            ),
        ),
        (0.3, Shot("sleep-awake")),
        (
            0.1,
            Call(|_, _, _| {
                // Asked to sleep: logind tells the daemon first. (The VM's
                // kernel can't really suspend, so it wakes at once.)
                let user = std::env::var("USER").unwrap_or_default();
                std::thread::spawn(move || {
                    let _ = std::process::Command::new("/opt/homebrew/bin/limactl")
                        .args(["shell", "dent-linux", "--", "sudo", "systemctl", "suspend"])
                        .env("HOME", format!("/Users/{user}"))
                        .status();
                });
            }),
        ),
        (
            0.0,
            Until(
                "the VM says it's going to sleep",
                |d, _| matches!(state(d), Some(State::Away(dent_link::Goodbye::Sleep))),
                15.,
            ),
        ),
        (0.0, Shot("sleep-asleep")),
        (
            0.1,
            Until("awake, it's back by itself", |d, _| online(d), 20.),
        ),
        (
            0.1,
            Until(
                "and its picture is current",
                |d, _| here(d) && !kept(d),
                10.,
            ),
        ),
        // A document with a picture on a line of its own, open beside.
        (0.1, Call(|d, _, cx| d.open_file_at("doc.md", None, cx))),
        (2.0, Shot("sleep-doc")),
        (0.1, Call(|_, _, _| freeze(true))),
        (
            0.1,
            Until(
                "silent, the VM reads as unreachable once it's tried",
                |d, _| {
                    matches!(
                        crate::machines_page::Standing::of(&d.machines[0]),
                        crate::machines_page::Standing::Unreachable
                    )
                },
                30.,
            ),
        ),
        (
            0.1,
            Until(
                "its picture shows, marked as kept from before",
                |d, _| kept(d),
                5.,
            ),
        ),
        // Tried again and again meanwhile: it reads as away throughout,
        // not back and forth with each try.
        (
            0.1,
            Call(|_, _, _| SILENT.set(Some(std::time::Instant::now()))),
        ),
        (
            0.1,
            Until(
                "and keeps reading as away between tries",
                |d, _| {
                    let away = matches!(
                        crate::machines_page::Standing::of(&d.machines[0]),
                        crate::machines_page::Standing::Unreachable
                    );
                    assert!(away, "it read as {:?}", d.machines[0].state);
                    SILENT.get().is_some_and(|t| t.elapsed().as_secs() >= 12)
                },
                15.,
            ),
        ),
        (0.5, Shot("sleep-silent")),
        (1.0, Shot("sleep-silent-picture")),
        (0.1, Call(|_, _, _| freeze(false))),
        (
            0.1,
            Until("let go on, it's back by itself", |d, _| online(d), 40.),
        ),
        (
            0.1,
            Until(
                "and its picture is current again",
                |d, _| here(d) && !kept(d),
                10.,
            ),
        ),
        (0.3, Shot("sleep-back")),
        // Its row in Settings › Machines, as it's named.
        (
            0.1,
            Call(|d, _, cx| {
                d.settings_tab = crate::settings::Tab::Machines;
                d.open_pane(Pane::Settings, cx);
            }),
        ),
        (1.0, Shot("sleep-machines")),
    ]
}

thread_local! {
    /// The stamp `stress-live` saw before the picture was rewritten.
    static BEFORE: std::cell::RefCell<Option<dent_protocol::Stamp>> = const { std::cell::RefCell::new(None) };
}

/// Trying to break what's shown of another machine: a dozen tabs of one
/// name, its daemon gone while a picture there is rewritten, then back,
/// then a message sent and answered as usual. Needs `files-live`'s
/// fixtures and `~/fx/dup/d0..d11/index.html` on the VM. Not in `checks`.
pub(super) fn stress_live() -> Vec<(f32, Act)> {
    use crate::files::{Ask, Look};
    use crate::machines_page::Standing;
    use dent_protocol::FileStat;
    fn sid(d: &Dent) -> Option<(String, String)> {
        d.sessions.iter().find_map(|s| {
            let far = !dent_client_core::fleet::split(&s.id).0.is_empty();
            let has = s
                .items
                .iter()
                .any(|i| matches!(i, Item::Text(t) if t.contains("Files test")));
            (far && has).then(|| (s.id.clone(), s.cwd.clone()))
        })
    }
    fn ask(d: &Dent) -> Option<Ask> {
        sid(d).map(|(id, cwd)| Ask::new(&id, "pics/a.png", &cwd))
    }
    fn stamp(d: &Dent) -> Option<dent_protocol::Stamp> {
        match crate::files::stat(&ask(d)?) {
            Look::Here(FileStat::File { stamp, .. }) => Some(stamp),
            _ => None,
        }
    }
    fn current(d: &Dent) -> bool {
        ask(d).is_some_and(|a| {
            matches!(crate::files::picture(&a), Look::Here(_)) && crate::files::kept(&a).is_none()
        })
    }
    fn standing(d: &Dent) -> Option<Standing> {
        d.machines.first().map(Standing::of)
    }
    fn vm(cmd: &'static str) {
        let user = std::env::var("USER").unwrap_or_default();
        std::thread::spawn(move || {
            let _ = std::process::Command::new("/opt/homebrew/bin/limactl")
                .args(["shell", "dent-linux", "--", "bash", "-lc", cmd])
                .env("HOME", format!("/Users/{user}"))
                .status();
        });
    }
    vec![
        (
            0.1,
            Until(
                "the VM is online",
                |d, _| standing(d) == Some(Standing::Online),
                30.,
            ),
        ),
        (
            0.1,
            Until("its files session is here", |d, _| sid(d).is_some(), 10.),
        ),
        (
            0.1,
            Call(|d, w, cx| {
                if let Some((id, _)) = sid(d) {
                    d.select(Some(id), w, cx);
                }
            }),
        ),
        // A dozen files of one name, each opened.
        (
            0.3,
            Call(|d, _, cx| {
                for i in 0..12 {
                    d.open_file_at(
                        crate::data::intern(&format!("dup/d{i}/index.html")),
                        None,
                        cx,
                    );
                }
            }),
        ),
        (
            0.1,
            Until(
                "all twelve open, each told apart",
                |d, _| {
                    let Some(s) = d.current() else { return false };
                    let pages = d
                        .opened
                        .get(&s.id)
                        .into_iter()
                        .flatten()
                        .filter_map(|a| d.art(a))
                        .filter(|a| a.path.ends_with("index.html"))
                        .count();
                    pages == 12
                },
                20.,
            ),
        ),
        (0.5, Shot("stress-tabs")),
        (0.1, Call(|d, _, cx| d.open_file_at("doc.md", None, cx))),
        (
            0.1,
            Until("its picture is here, current", |d, _| current(d), 15.),
        ),
        (0.1, Call(|d, _, _| BEFORE.set(stamp(d)))),
        // Its daemon gone, and the picture rewritten meanwhile.
        (
            0.1,
            Call(|_, _, _| {
                vm(
                    "pkill -f '^target/release/dent-daemon'; sleep 1; cp ~/Pictures/h.png ~/fx/pics/a.png",
                )
            }),
        ),
        (
            0.1,
            Until(
                "gone, the VM reads as unreachable",
                |d, _| standing(d) == Some(Standing::Unreachable),
                40.,
            ),
        ),
        (
            0.1,
            Until(
                "and its picture shows, marked as kept",
                |d, _| ask(d).is_some_and(|a| crate::files::kept(&a).is_some()),
                5.,
            ),
        ),
        (0.5, Shot("stress-gone")),
        (
            0.1,
            Call(|_, _, _| {
                vm(
                    "cd ~/dent && (DENT_REMOTE_LAN=1 DENT_CLAUDE=$HOME/dent/target/release/dent-fake-claude setsid nohup target/release/dent-daemon --state $HOME/rt/dd2 >> $HOME/rt/dd2.log 2>&1 < /dev/null &)",
                )
            }),
        ),
        (
            0.1,
            Until(
                "back, it's online by itself",
                |d, _| standing(d) == Some(Standing::Online),
                40.,
            ),
        ),
        (
            0.1,
            Until(
                "and the picture is the new one, unmarked",
                |d, _| current(d) && stamp(d).is_some() && stamp(d) != BEFORE.with(|b| *b.borrow()),
                15.,
            ),
        ),
        (0.5, Shot("stress-back")),
        // As usual afterwards: a message there, answered.
        (0.2, Type(Field::Composer, "say: still here after all that")),
        (0.2, Key("enter")),
        (
            0.1,
            Until(
                "a message afterwards is answered as usual",
                |d, _| {
                    d.current().is_some_and(|s| {
                        s.items
                            .iter()
                            .any(|i| matches!(i, Item::Text(t) if t.contains("still here after all that")))
                    })
                },
                30.,
            ),
        ),
        (0.5, Shot("stress-after")),
    ]
}

/// Another machine asleep: its session says why under the header, a
/// message written meanwhile waits on this Mac with Edit and Remove, and a
/// new session can be started there from the machine chip. The machine
/// and its session are made up here.
pub(super) fn away() -> Vec<(f32, Act)> {
    vec![
        (0.3, Key("cmd-n")),
        (
            0.1,
            Call(|d, w, cx| {
                use dent_client_core::machine::State;
                let mut m = made_up_machines();
                m.retain(|m| m.id == "c27b19e4a0d2");
                m[0].state = State::Away(dent_link::Goodbye::Sleep);
                d.machines = m;
                let mut out = dent_protocol::Out::Events {
                    events: vec![
                        dent_protocol::Event {
                            seq: 1,
                            session: "s1".into(),
                            at: crate::cards::now_ms(),
                            body: dent_protocol::Body::Created {
                                agent: "claude".into(),
                                cwd: "/Users/walk/ios".into(),
                                model: None,
                                permission: dent_protocol::Permission::default(),
                                title: "bump the iOS deps".into(),
                            },
                        },
                        dent_protocol::Event {
                            seq: 2,
                            session: "s1".into(),
                            at: crate::cards::now_ms(),
                            body: dent_protocol::Body::User {
                                id: "u1".into(),
                                text: "update Alamofire and the Firebase pods".into(),
                                queued: false,
                                steer: false,
                                attachments: Vec::new(),
                            },
                        },
                    ],
                };
                dent_client_core::fleet::qualify_out("c27b19e4a0d2", &mut out);
                for id in d.core.apply_from("c27b19e4a0d2", &out) {
                    d.rebuild(&id);
                }
                d.select(Some("c27b19e4a0d2/s1".into()), w, cx);
            }),
        ),
        (
            0.2,
            Until(
                "the asleep machine's session says so",
                |d, _| drawn(d, "away-banner").is_some(),
                3.,
            ),
        ),
        (
            0.,
            Until(
                "it waits in the sidebar's Paused, not Working",
                |d, _| {
                    let [_, _, working, paused] = d.shown_sections();
                    paused.iter().any(|s| s.id == "c27b19e4a0d2/s1")
                        && !working.iter().any(|s| s.id == "c27b19e4a0d2/s1")
                },
                1.,
            ),
        ),
        (
            0.1,
            Call(|d, _, cx| {
                d.send_draft(
                    "c27b19e4a0d2/s1",
                    &crate::app::Draft::words("fix the fixtures, then run the full suite"),
                    false,
                );
                cx.notify();
            }),
        ),
        (
            0.2,
            Until(
                "a message for it waits here",
                |d, _| {
                    d.outbox.len() == 1
                        && d.session("c27b19e4a0d2/s1").is_some_and(|s| {
                            s.items
                                .iter()
                                .any(|i| matches!(i, crate::data::Item::Queued { .. }))
                        })
                },
                2.,
            ),
        ),
        (0.4, Shot("away-light")),
        (0.1, Call(|d, w, cx| set_dark(d, true, w, cx))),
        (0.4, Shot("away-dark")),
        (0.1, Call(|d, w, cx| set_dark(d, false, w, cx))),
        (
            0.1,
            Call(|d, _, cx| {
                let msg = d.outbox[0].id.clone();
                d.remove_queued("c27b19e4a0d2/s1", &msg, cx);
            }),
        ),
        (
            0.1,
            Until(
                "Remove takes it back here, and nothing is sent",
                |d, _| d.outbox.is_empty(),
                2.,
            ),
        ),
        (0.2, Key("cmd-n")),
        (0.2, Click("chip-machine")),
        (
            0.1,
            Until(
                "the machine chip opens its menu",
                |d, _| d.menu == Some(crate::app::Menu::Machine),
                2.,
            ),
        ),
        (0.4, Shot("away-chip-light")),
        (0.1, Key("escape")),
        (0.1, Call(leave_made_up_machines)),
    ]
}
