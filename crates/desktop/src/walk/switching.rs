use super::*;

/// Ctrl+Tab: a tap goes back to the last session, held it steps further.
pub(super) fn switching() -> Vec<(f32, Act)> {
    let mut v = Vec::new();
    for t in ["alpha notes", "beta notes", "gamma notes"] {
        v.extend(new_session(t));
        v.push((
            0.1,
            Until(leak(format!("“{t}” answers")), |d, _| answered(d), 20.),
        ));
    }
    v.extend([
        (0.2, Key("ctrl-tab")),
        (0.05, Call(let_go)),
        (
            0.1,
            Until(
                "a tap of Ctrl+Tab goes back to the session before",
                |d, _| titled(d, "beta notes") && d.switcher.is_none(),
                1.,
            ),
        ),
        (0.2, Key("ctrl-tab")),
        (0.05, Call(let_go)),
        (
            0.1,
            Until(
                "another tap comes back again",
                |d, _| titled(d, "gamma notes"),
                1.,
            ),
        ),
        (
            0.05,
            Until(
                "a quick tap never shows the list",
                |d, _| drawn(d, "switch-0").is_none(),
                0.2,
            ),
        ),
        (0.2, Key("ctrl-tab")),
        (0.1, Key("ctrl-tab")),
        (
            0.4,
            Until(
                "held, the list shows, most recent first, the one two back lit",
                |d, _| {
                    drawn(d, "switch-2").is_some() && d.switcher.as_ref().is_some_and(|s| s.at == 2)
                },
                1.,
            ),
        ),
        (0.2, Shot("switcher")),
        (0.8, Call(let_go)),
        (
            0.1,
            Until(
                "letting go opens it",
                |d, _| titled(d, "alpha notes") && drawn(d, "switch-0").is_none(),
                1.,
            ),
        ),
        (0.2, Key("ctrl-tab")),
        (0.3, Key("escape")),
        (
            0.1,
            Until(
                "Esc stays where you are",
                |d, _| titled(d, "alpha notes") && d.switcher.is_none(),
                1.,
            ),
        ),
        (0.2, Key("ctrl-shift-tab")),
        (
            0.05,
            Until(
                "Ctrl+Shift+Tab goes the other way, to the last in the list",
                |d, _| {
                    d.switcher
                        .as_ref()
                        .is_some_and(|s| s.at + 1 == s.order.len() && s.order.len() > 2)
                },
                1.,
            ),
        ),
        (0.05, Call(let_go)),
        (
            0.1,
            Until(
                "and letting go opens it",
                |d, _| d.switcher.is_none() && !titled(d, "alpha notes"),
                1.,
            ),
        ),
    ]);
    v
}
