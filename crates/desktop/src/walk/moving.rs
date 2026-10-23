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
