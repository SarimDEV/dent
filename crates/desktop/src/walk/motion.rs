use super::*;

/// The chrome's motion, for a recording of the window: a first tooltip,
/// the next one while warm, the palette, a menu, a session's rows arriving,
/// a toast and a fold.
pub(super) fn motion() -> Vec<(f32, Act)> {
    let mut v: Vec<(f32, Act)> = vec![
        (
            0.,
            Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
        ),
        (1.0, Hover("new-session", 0., 0.)),
        (1.5, Hover("new-session", -crate::ds::size::CONTROL, 0.)),
        (1.2, Hover("starter", 0., 0.)),
        (1.0, Key("cmd-k")),
        (1.0, Key("escape")),
        (0.8, Click("chip-model")),
        (1.0, Key("escape")),
    ];
    v.extend(new_session("Fix the footer on small screens"));
    v.extend([
        (
            0.1,
            Until("the agent finishes", |d, _| status_is(d, Status::Done), 30.),
        ),
        (1.0, Key("cmd-w")),
        (1.5, Click("closed-toggle")),
        (1.0, Click("closed-toggle")),
        (1.0, Wait),
    ]);
    v
}
