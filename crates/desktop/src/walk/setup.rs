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
        (0.1, Click("settings-tab-general")),
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

fn made_up_machines() -> Vec<crate::machines::MachineView> {
    use dent_client_core::machine::State;
    use dent_link::{Access, Goodbye, Trust};
    let m = |id: &str, name: &str, state: State, rtt: Option<u64>, via: &str| {
        let mut v = crate::machines::MachineView::saved(dent_remote::Saved {
            id: id.into(),
            name: name.into(),
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
    ]
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

/// A picture in a session on a machine really saved: the session that
/// `dent ask NAME "look" --file pic.png` started there shows the picture,
/// copied to this Mac. Not in `checks`: it needs another machine.
pub(super) fn pictures_live() -> Vec<(f32, Act)> {
    fn copied(d: &Dent) -> Option<String> {
        d.sessions.iter().find_map(|s| {
            s.items.iter().find_map(|i| match i {
                Item::User { files, .. }
                    if files
                        .iter()
                        .any(|f| f.contains("Library/Caches/dent/machines")) =>
                {
                    Some(s.id.clone())
                }
                _ => None,
            })
        })
    }
    vec![
        (0.3, Key("cmd-n")),
        (
            0.1,
            Until(
                "a saved machine comes online",
                |d, _| d.machines.iter().any(|m| m.rtt.is_some()),
                30.,
            ),
        ),
        (
            0.1,
            Until(
                "its session's picture is copied here",
                |d, _| copied(d).is_some(),
                30.,
            ),
        ),
        (
            0.1,
            Call(|d, w, cx| {
                if let Some(id) = copied(d) {
                    d.select(Some(id), w, cx);
                }
            }),
        ),
        (1.0, Shot("pictures-live-light")),
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
