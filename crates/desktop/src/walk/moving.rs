use super::*;

/// Moving a session to a machine really saved (`DENT_CLIENT_STATE`): the
/// machine chip under the reply box opens the list of machines, choosing
/// one moves the session there, the window follows it, and the session it
/// left reads as moved. Not in `checks`: it needs another machine. The
/// session works in this machine's scratch folder, which moves anywhere.
pub(super) fn move_live() -> Vec<(f32, Act)> {
    vec![
        (
            0.1,
            Until(
                "a saved machine comes online",
                |d, _| d.machines.iter().any(|m| m.rtt.is_some()),
                30.,
            ),
        ),
        (0.3, Key("cmd-n")),
        (0.2, Type(Field::Starter, "say: ready to move")),
        (0.2, Click("start")),
        (0.1, Until("it answers here", |d, _| answered(d), 20.)),
        (0.2, Click("chip-sessionmachine")),
        (
            0.1,
            Until(
                "the machines are listed, each saying whether it can take it",
                |d, _| drawn(d, "menu").is_some() || d.menu.is_some(),
                3.,
            ),
        ),
        (1.0, Shot("move-menu")),
        (0.1, Key("down")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "while it goes, the banner says so",
                |d, _| drawn(d, "move-banner").is_some() || !d.moves.is_empty(),
                5.,
            ),
        ),
        (0.2, Shot("move-going")),
        (
            0.1,
            Until(
                "it arrives there and the window follows it",
                |d, _| {
                    current(d)
                        .is_some_and(|s| s.id.contains('/') && s.title.contains("ready to move"))
                },
                90.,
            ),
        ),
        (0.8, Shot("move-arrived")),
        (
            0.1,
            Call(|d, w, cx| {
                let left = d
                    .sessions
                    .iter()
                    .find(|s| !s.id.contains('/') && s.title.contains("ready to move"))
                    .map(|s| s.id.clone());
                d.select(left, w, cx);
            }),
        ),
        (
            0.1,
            Until(
                "the session it left says where it carries on",
                |d, _| drawn(d, "moved-card").is_some(),
                10.,
            ),
        ),
        (0.5, Shot("move-left")),
        // And back again: from the other machine to this one.
        (
            0.1,
            Call(|d, w, cx| {
                let there = d
                    .sessions
                    .iter()
                    .find(|s| {
                        s.id.contains('/')
                            && s.title.contains("ready to move")
                            && s.moving.is_none()
                    })
                    .map(|s| s.id.clone());
                d.select(there, w, cx);
            }),
        ),
        (0.3, Click("chip-sessionmachine")),
        (
            0.1,
            Until(
                "from there, this Mac is offered",
                |d, _| {
                    d.move_entries()
                        .iter()
                        .any(|e| e.key == "move-this" && e.choosable())
                },
                10.,
            ),
        ),
        (0.6, Shot("move-menu-back")),
        (0.1, Click("move-this")),
        (
            0.1,
            Until(
                "it comes back here, and the window follows",
                |d, _| {
                    current(d).is_some_and(|s| {
                        !s.id.contains('/')
                            && s.title.contains("ready to move")
                            && s.moving.is_none()
                    }) && d
                        .sessions
                        .iter()
                        .filter(|s| s.title.contains("ready to move"))
                        .count()
                        >= 3
                },
                90.,
            ),
        ),
        (0.8, Shot("move-back")),
    ]
}

/// How a move that stopped partway looks: why, with Retry and Continue
/// here. Drawn from a stopped move put in place, so it needs no second
/// machine.
pub(super) fn move_stopped() -> Vec<(f32, Act)> {
    let mut v = new_session("hello there");
    v.extend([
        (0.1, Until("it answers", |d, _| answered(d), 20.)),
        (
            0.1,
            Call(|d, _, cx| {
                let Some(id) = current(d).map(|s| s.id.clone()) else {
                    return;
                };
                d.moves.insert(
                    id.clone(),
                    crate::moves::Run {
                        carrier: dent_client_core::carry::Carrier::new(&id, "m1", "dent-linux"),
                        from: String::new(),
                        to: "ab12".into(),
                        to_name: "dent-linux".into(),
                        out: None,
                        run: 0,
                        stopped: Some(dent_client_core::carry::Ended::Stopped {
                            side: dent_client_core::carry::Side::To,
                            why: "dent-linux didn't answer".into(),
                        }),
                    },
                );
                // Partway: it may be there already, so it's held here.
                if let Some(v) = d.core.list.get_mut(&id) {
                    v.moving = Some((
                        "dent-linux".into(),
                        None,
                        dent_protocol::MoveState::Departing { sent: true },
                    ));
                }
                d.rebuild(&id);
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "stopped partway, it says why, with Retry and Continue here",
                |d, _| drawn(d, "move-banner").is_some() && drawn(d, "move-keep").is_some(),
                3.,
            ),
        ),
        (0.4, Shot("move-stopped")),
    ]);
    v
}

/// The machine menu, banner and moved card at their limits: a long
/// machine name, every way a machine can stand, a move partway through
/// sending, and the record a long-named machine left. Drawn from state put
/// in place, so it needs no second machine.
pub(super) fn move_limits() -> Vec<(f32, Act)> {
    let mut v = new_session("hello there");
    v.extend([
        (0.1, Until("it answers", |d, _| answered(d), 20.)),
        (
            0.1,
            Call(|d, _, cx| {
                use dent_client_core::machine::State;
                let mut add = |id: &str, name: &str, state: State, rtt: Option<u64>| {
                    let mut m = crate::machines::MachineView::saved(dent_remote::store::Saved {
                        id: id.into(),
                        name: name.into(),
                        ..Default::default()
                    });
                    m.state = state;
                    m.rtt = rtt;
                    d.machines.push(m);
                };
                add("aa01", "dent-linux", State::Online, Some(18));
                add(
                    "aa02",
                    "Sarim's MacBook Pro (Work, 2024 M4 Max)",
                    State::Online,
                    Some(240),
                );
                add("aa03", "studio", State::Away(dent_link::Goodbye::Sleep), None);
                add(
                    "aa04",
                    "build-box",
                    State::Unreachable { why: "timed out".into() },
                    None,
                );
                add("aa05", "pi", State::NeedsCode { pairing_open: false }, None);
                add("aa06", "new-one", State::Connecting, None);
                cx.notify();
            }),
        ),
        (0.3, Click("chip-sessionmachine")),
        (
            0.1,
            Until("the menu opens", |d, _| d.menu.is_some(), 3.),
        ),
        (
            0.1,
            Call(|d, _, cx| {
                // As the probe would answer: one has it, one doesn't.
                if let Some(p) = d.move_probe.as_mut() {
                    p.remotes = Some(vec![dent_protocol::GitRemote {
                        name: "origin".into(),
                        url: "git@example.com:a/proj.git".into(),
                    }]);
                    p.has.insert("aa01".into(), true);
                    p.has.insert("aa02".into(), false);
                }
                cx.notify();
            }),
        ),
        (0.6, Shot("limits-menu")),
        (0.1, Key("escape")),
        (
            0.1,
            Call(|d, _, cx| {
                let Some(id) = current(d).map(|s| s.id.clone()) else {
                    return;
                };
                use dent_client_core::carry::{Carrier, Side};
                use dent_protocol::Reply;
                let mut c = Carrier::new(&id, "m2", "Sarim's MacBook Pro (Work, 2024 M4 Max)");
                c.start();
                c.answer(Side::From, Ok(Reply::MoveRemotes { remotes: vec![] }));
                c.answer(Side::To, Ok(Reply::MoveHave { found: true, tips: vec![] }));
                c.answer(
                    Side::From,
                    Ok(Reply::MovePacked {
                        manifest: Box::new(dent_protocol::MoveManifest {
                            id: "m2".into(),
                            from_machine: String::new(),
                            from_name: String::new(),
                            from_session: id.clone(),
                            agent: "claude".into(),
                            agent_session: String::new(),
                            cwd: String::new(),
                            title: String::new(),
                            model: None,
                            permission: Default::default(),
                            effort: None,
                            repo: None,
                            files_from: String::new(),
                            parts: vec![dent_protocol::MovePart {
                                name: "conversation".into(),
                                bytes: 3_400_000,
                                sha256: String::new(),
                            }],
                        }),
                    }),
                );
                c.answer(
                    Side::To,
                    Ok(Reply::MoveParts { have: vec![("conversation".into(), 1_200_000)] }),
                );
                d.moves.insert(
                    id.clone(),
                    crate::moves::Run {
                        carrier: c,
                        from: String::new(),
                        to: "aa02".into(),
                        to_name: "Sarim's MacBook Pro (Work, 2024 M4 Max)".into(),
                        out: None,
                        run: 0,
                        stopped: None,
                    },
                );
                d.rebuild(&id);
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "partway through sending, the banner counts it",
                |d, _| drawn(d, "move-banner").is_some(),
                3.,
            ),
        ),
        (0.4, Shot("limits-sending")),
        (
            0.1,
            Until(
                "while it goes, the menu offers nowhere else, and not there again",
                |d, _| {
                    let e = d.move_entries();
                    let row = |k: &str| e.iter().find(|e| e.key == k);
                    row("move-aa01").is_some_and(|e| {
                        !e.choosable()
                            && e.note.as_deref().is_some_and(|n| n.starts_with("It's moving to"))
                    }) && row("move-aa02").is_some_and(|e| !e.choosable())
                },
                3.,
            ),
        ),
        (
            0.1,
            Call(|d, _, cx| {
                let Some(id) = current(d).map(|s| s.id.clone()) else {
                    return;
                };
                if let Some(r) = d.moves.get_mut(&id) {
                    r.stopped = Some(dent_client_core::carry::Ended::Stopped {
                        side: dent_client_core::carry::Side::To,
                        why: "stopped taking conversation at 1200000 bytes: no space left on device (os error 28)".into(),
                    });
                }
                if let Some(v) = d.core.list.get_mut(&id) {
                    v.moving = Some((
                        "Sarim's MacBook Pro (Work, 2024 M4 Max)".into(),
                        None,
                        dent_protocol::MoveState::Departing { sent: true },
                    ));
                }
                d.rebuild(&id);
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "stopped for a long reason, the banner says it whole",
                |d, _| drawn(d, "move-retry").is_some(),
                3.,
            ),
        ),
        (0.4, Shot("limits-stopped")),
        (
            0.1,
            Call(|d, _, cx| {
                let Some(id) = current(d).map(|s| s.id.clone()) else {
                    return;
                };
                // Under way as the session says (this app restarted): the
                // same move goes on, and only to where it was going.
                d.moves.remove(&id);
                if let Some(v) = d.core.list.get_mut(&id) {
                    v.moving = Some(("dent-linux".into(), None, dent_protocol::MoveState::Pending));
                    v.move_id = Some("m3".into());
                    v.move_to = Some("aa01".into());
                }
                d.rebuild(&id);
                d.start_move(&id, "aa02", cx);
                let refused = !d.moves.contains_key(&id);
                d.start_move(&id, "aa01", cx);
                let same = d.moves.get(&id).is_some_and(|r| r.carrier.id() == "m3");
                // Ended there (its id spent): picked again, it's a new move.
                d.moves.remove(&id);
                if let Some(v) = d.core.list.get_mut(&id) {
                    v.moving = None;
                }
                d.rebuild(&id);
                d.start_move(&id, "aa01", cx);
                let fresh = d.moves.get(&id).is_some_and(|r| r.carrier.id() != "m3");
                d.moves.remove(&id);
                // The refusal's note isn't part of the stills after.
                d.toast = None;
                IDS.store(refused && same && fresh, std::sync::atomic::Ordering::Relaxed);
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "a move under way goes on under its id, only to where it was going; one that ended starts anew",
                |_, _| IDS.load(std::sync::atomic::Ordering::Relaxed),
                1.,
            ),
        ),
        (
            0.1,
            Call(|d, _, cx| {
                let Some(id) = current(d).map(|s| s.id.clone()) else {
                    return;
                };
                // Stopped here, then ended there (a message sent, or
                // Continue here on another device): it holds nothing.
                d.moves.insert(
                    id.clone(),
                    crate::moves::Run {
                        carrier: dent_client_core::carry::Carrier::new(&id, "m4", "studio"),
                        from: String::new(),
                        to: "aa03".into(),
                        to_name: "studio".into(),
                        out: None,
                        run: 0,
                        stopped: Some(dent_client_core::carry::Ended::Stopped {
                            side: dent_client_core::carry::Side::To,
                            why: "studio didn't answer".into(),
                        }),
                    },
                );
                if let Some(v) = d.core.list.get_mut(&id) {
                    v.moving = None;
                }
                d.rebuild(&id);
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "a stopped move that ended there since blocks no other machine",
                |d, _| {
                    d.move_entries()
                        .iter()
                        .any(|e| e.key == "move-aa01" && e.choosable())
                },
                2.,
            ),
        ),
        (0.1, Call(|d, _, _| {
            if let Some(id) = current(d).map(|s| s.id.clone()) {
                d.moves.remove(&id);
            }
        })),
        (
            0.1,
            Call(|d, _, cx| {
                let Some(id) = current(d).map(|s| s.id.clone()) else {
                    return;
                };
                d.moves.remove(&id);
                if let Some(v) = d.core.list.get_mut(&id) {
                    v.moving = Some((
                        "Sarim's MacBook Pro (Work, 2024 M4 Max)".into(),
                        Some("s-there".into()),
                        dent_protocol::MoveState::Departed,
                    ));
                    v.move_to = Some("aa02".into());
                }
                d.rebuild(&id);
                cx.notify();
            }),
        ),
        (
            0.1,
            Until("the record says where it went", |d, _| drawn(d, "moved-card").is_some(), 3.),
        ),
        (0.4, Shot("limits-left")),
    ]);
    v
}

/// What [`move_limits`] found of the ids a move goes on with.
static IDS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
