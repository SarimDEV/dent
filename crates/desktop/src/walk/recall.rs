use super::*;

/// ↑ brings back what you sent, a waiting message first; drafts are kept.
pub(super) fn recall() -> Vec<(f32, Act)> {
    let mut v = new_session("first prompt");
    v.extend([
        (
            0.1,
            Until(
                "the first turn ends",
                |d, _| status_is(d, Status::Done),
                20.,
            ),
        ),
        (0.2, Type(Field::Composer, "second prompt")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "the second turn ends",
                |d, _| status_is(d, Status::Done) && mine(d) == 2,
                20.,
            ),
        ),
        (0.2, Key("up")),
        (
            0.1,
            Until(
                "↑ in an empty composer brings back the last message",
                |d, cx| composer_is(d, cx, "second prompt"),
                1.,
            ),
        ),
        (0.1, Key("up")),
        (
            0.1,
            Until(
                "↑ again goes further back",
                |d, cx| composer_is(d, cx, "first prompt"),
                1.,
            ),
        ),
        (0.1, Key("up")),
        (
            0.1,
            Until(
                "↑ at the first message stays on it",
                |d, cx| composer_is(d, cx, "first prompt"),
                1.,
            ),
        ),
        (0.1, Key("down")),
        (
            0.1,
            Until(
                "↓ comes forward",
                |d, cx| composer_is(d, cx, "second prompt"),
                1.,
            ),
        ),
        (0.1, Key("down")),
        (
            0.1,
            Until(
                "↓ past the newest empties the box",
                |d, cx| composer_is(d, cx, ""),
                1.,
            ),
        ),
        (0.1, Key("up")),
        (0.1, Keys(" edited")),
        (0.1, Key("up")),
        (
            0.1,
            Until(
                "once edited, ↑ moves the caret and leaves the text",
                |d, cx| composer_is(d, cx, "second prompt edited"),
                1.,
            ),
        ),
        (0.1, Type(Field::Composer, "line one\nline two")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "a message of two lines is answered",
                |d, _| status_is(d, Status::Done) && mine(d) == 3,
                20.,
            ),
        ),
        (0.2, Key("up")),
        (0.1, Key("up")),
        (
            0.1,
            Until(
                "↑ through a message of several lines walks its lines first",
                |d, cx| {
                    composer_is(d, cx, "line one\nline two") && d.composer.read(cx).cursor() < 9
                },
                1.,
            ),
        ),
        (0.1, Key("up")),
        (
            0.1,
            Until(
                "then goes on to the message before",
                |d, cx| composer_is(d, cx, "second prompt"),
                1.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
        (0.2, Type(Field::Composer, "a long answer please")),
        (0.1, Key("enter")),
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "change me")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "the message waits",
                |d, _| has(d, |i| matches!(i, Item::Queued { .. })),
                5.,
            ),
        ),
        (0.2, Key("up")),
        (
            0.1,
            Until(
                "↑ takes a waiting message back out of the queue to edit",
                |d, cx| {
                    composer_is(d, cx, "change me") && !has(d, |i| matches!(i, Item::Queued { .. }))
                },
                3.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
        (
            0.1,
            Until(
                "taken back and cleared, it's never sent",
                |d, _| status_is(d, Status::Done) && mine(d) == 4,
                20.,
            ),
        ),
        (0.5, Wait),
        (0.1, Type(Field::Composer, "half a thought")),
        (0.1, Call(save_now)),
        (
            0.1,
            Until(
                "an unsent draft is saved for the next launch",
                |d, _| {
                    let id = d.selected.clone().unwrap_or_default();
                    mine(d) == 4
                        && crate::prefs::load()
                            .drafts
                            .get(&id)
                            .map(|d| d.text.as_str())
                            == Some("half a thought")
                },
                2.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
    ]);
    v
}
