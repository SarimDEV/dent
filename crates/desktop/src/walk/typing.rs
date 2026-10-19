use super::*;

/// The text box's own keys, quick to run alone.
pub(super) fn typing() -> Vec<(f32, Act)> {
    vec![
        (
            0.2,
            Call(|d, w, cx| {
                d.starter.update(cx, |s, cx| {
                    s.set_value("", w, cx);
                    s.focus_field(w, cx);
                })
            }),
        ),
        (
            0.1,
            Until(
                "the new-session box has the keyboard",
                |d, cx| d.starter.read(cx).value().is_empty(),
                1.,
            ),
        ),
        (0.1, Keys("a")),
        (0.1, Key("alt-enter")),
        (0.1, Keys("b")),
        (0.1, Key("shift-enter")),
        (0.1, Keys("c")),
        (
            0.1,
            Until(
                "⌥ Return and Shift-Return start a new line, not a session",
                |d, cx| d.starter.read(cx).value() == "a\nb\nc",
                1.,
            ),
        ),
        (
            0.1,
            Call(|d, w, cx| d.starter.update(cx, |s, cx| s.set_value("", w, cx))),
        ),
    ]
}
