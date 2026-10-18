use super::*;

pub(super) fn zoom_is(z: f32) -> bool {
    (crate::ds::zoom() - z).abs() < 0.001
}

/// ⇧Tab's modes, text size, appearance and the shortcuts sheet.
pub(super) fn view_choices() -> Vec<(f32, Act)> {
    use dent_protocol::Permission as P;
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (0.2, Type(Field::Starter, "")),
        (0.1, Key("shift-tab")),
        (
            0.1,
            Until(
                "⇧Tab on a new session steps the mode to Edits",
                |d, _| d.defaults.permission == P::Edits,
                1.,
            ),
        ),
        (0.1, Key("shift-tab")),
        (0.1, Key("shift-tab")),
        (
            0.1,
            Until(
                "on through Plan and round to Ask, never to Full",
                |d, _| d.defaults.permission == P::Ask,
                1.,
            ),
        ),
    ];
    v.extend(new_session("check the parser"));
    v.extend([
        (0.1, Until("it answers", |d, _| answered(d), 20.)),
        (0.2, Type(Field::Composer, "")),
        (0.1, Key("shift-tab")),
        (
            0.1,
            Until(
                "in a session, ⇧Tab changes the session's own mode",
                |d, _| current(d).is_some_and(|s| s.permission == P::Edits),
                3.,
            ),
        ),
        (0.1, Key("shift-tab")),
        (0.1, Key("shift-tab")),
        (
            0.1,
            Until(
                "and round again",
                |d, _| current(d).is_some_and(|s| s.permission == P::Ask),
                3.,
            ),
        ),
        (0.2, Key("cmd-=")),
        (0.1, Key("cmd-=")),
        (
            0.1,
            Until(
                "⌘+ makes the text bigger, a step at a time",
                |_, _| zoom_is(1.2),
                1.,
            ),
        ),
        (0.4, Shot("text-bigger")),
        (0.8, Key("cmd-0")),
        (0.1, Until("⌘0 puts it back", |_, _| zoom_is(1.), 1.)),
        (0.1, Key("cmd--")),
        (0.1, Until("⌘− makes it smaller", |_, _| zoom_is(0.92), 1.)),
        (0.1, Key("cmd-0")),
        // The walk's own theme (`--dark` or not) is kept, to put back after.
        (
            0.1,
            Call(|d, w, cx| {
                WALK_DARK.with(|k| k.set(d.forced_dark));
                d.set_appearance(crate::prefs::Appearance::Dark, w, cx)
            }),
        ),
        (
            0.1,
            Until("Dark Appearance wears Carbon", |d, _| d.t.dark, 1.),
        ),
        (
            0.1,
            Call(|d, w, cx| d.set_appearance(crate::prefs::Appearance::Light, w, cx)),
        ),
        (
            0.1,
            Until("Light Appearance wears Paper", |d, _| !d.t.dark, 1.),
        ),
        // Back to the theme the walk was asked for, so the stills after
        // this are in it too.
        (
            0.,
            Call(|d, w, cx| {
                d.set_appearance(crate::prefs::Appearance::System, w, cx);
                d.forced_dark = WALK_DARK.with(|k| k.get());
                d.apply_theme(w, cx);
            }),
        ),
        (0.1, Key("cmd-/")),
        (
            0.1,
            Until(
                "⌘/ shows every shortcut",
                |d, _| d.keys_open && drawn(d, "keys").is_some(),
                1.,
            ),
        ),
        (0.4, Shot("keys")),
        (0.8, Key("escape")),
        (0.1, Until("Esc puts it away", |d, _| !d.keys_open, 1.)),
        (0.1, Keys("?")),
        (
            0.1,
            Until(
                "? typed in the reply box is just a ?",
                |d, cx| !d.keys_open && composer_is(d, cx, "?"),
                1.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
    ]);
    v
}

thread_local! {
    /// The theme a walk was started in, while a scene changes it.
    pub(super) static WALK_DARK: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}
