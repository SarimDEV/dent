use super::*;

fn box_says(d: &Dent, cx: &App, text: &str) -> bool {
    d.composer.read(cx).value() == text
}

fn sent(d: &Dent, text: &str) -> bool {
    sent_order(d).iter().any(|t| t == text)
}

/// Stopped before any reply, a message comes back to the box to change and
/// send again, as in Claude Code; one with a reply under way stays sent.
pub(super) fn take_back() -> Vec<(f32, Act)> {
    let mut v = new_session("hi");
    v.extend([
        (
            0.1,
            Until("it answers", |d, _| status_is(d, Status::Done), 10.),
        ),
        (0.3, Type(Field::Composer, "work quietly on the parser")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "it works on it",
                |d, _| status_is(d, Status::Working) && sent(d, "work quietly on the parser"),
                5.,
            ),
        ),
        (0.4, Shot("takeback-working")),
        (0.1, Key("escape")),
        (
            0.1,
            Until(
                "Esc before any reply brings the message back to the box, and it leaves the chat",
                |d, cx| {
                    box_says(d, cx, "work quietly on the parser")
                        && !sent(d, "work quietly on the parser")
                        && !current(d).is_some_and(|s| s.running())
                },
                4.,
            ),
        ),
        (
            0.,
            Until(
                "with no Stopped note under the last reply",
                |d, _| {
                    !has(d, |i| {
                        matches!(
                            i,
                            Item::Note {
                                kind: crate::data::NoteKind::Stopped,
                                ..
                            }
                        )
                    })
                },
                0.2,
            ),
        ),
        (0.3, Shot("takeback-back")),
        // Carried on and sent again; then what's typed while it stops goes
        // after the message coming back.
        (
            0.2,
            Type(Field::Composer, "work quietly on the parser, and the lexer"),
        ),
        (0.1, Key("enter")),
        (
            0.1,
            Until("it works again", |d, _| status_is(d, Status::Working), 5.),
        ),
        (0.2, Type(Field::Composer, "and the docs")),
        (0.1, Key("escape")),
        (
            0.1,
            Until(
                "the message comes back ahead of what was typed",
                |d, cx| {
                    box_says(
                        d,
                        cx,
                        "work quietly on the parser, and the lexer\n\nand the docs",
                    ) && sent_order(d) == ["hi"]
                },
                4.,
            ),
        ),
        (0.3, Shot("takeback-typed")),
        // A waiting message comes back after it.
        (0.2, Type(Field::Composer, "work quietly again")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "it works a third time",
                |d, _| status_is(d, Status::Working),
                5.,
            ),
        ),
        (0.2, Type(Field::Composer, "then this")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "that waits",
                |d, _| has(d, |i| matches!(i, Item::Queued { .. })),
                5.,
            ),
        ),
        (0.1, Key("escape")),
        (
            0.1,
            Until(
                "both come back, in the order sent",
                |d, cx| {
                    box_says(d, cx, "work quietly again\n\nthen this")
                        && !has(d, |i| matches!(i, Item::Queued { .. }))
                        && sent_order(d) == ["hi"]
                },
                4.,
            ),
        ),
        // A reply under way: the message stays, and the stop shows.
        (0.2, Type(Field::Composer, "a long answer please")),
        (0.1, Key("enter")),
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.2, Key("escape")),
        (
            0.1,
            Until(
                "stopped mid-reply, the message stays sent and the box empty",
                |d, cx| {
                    status_is(d, Status::Idle)
                        && box_says(d, cx, "")
                        && sent(d, "a long answer please")
                        && has(d, |i| {
                            matches!(
                                i,
                                Item::Note {
                                    kind: crate::data::NoteKind::Stopped,
                                    ..
                                }
                            )
                        })
                },
                4.,
            ),
        ),
        (0.3, Shot("takeback-kept")),
        // The agent carries on as if the taken-back messages were never sent.
        (0.2, Type(Field::Composer, "short one")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "and the next message goes as usual",
                |d, _| status_is(d, Status::Done) && sent(d, "short one"),
                10.,
            ),
        ),
    ]);
    // A new session's first message: back in its box, and the session
    // reads stopped, not still starting.
    v.extend(new_session("work quietly from the start"));
    v.extend([
        (
            0.1,
            Until("it works on it", |d, _| status_is(d, Status::Working), 5.),
        ),
        (0.3, Key("escape")),
        (
            0.1,
            Until(
                "a first message stopped before any reply comes back, and the session isn't left working",
                |d, cx| {
                    box_says(d, cx, "work quietly from the start")
                        && sent_order(d).is_empty()
                        && !status_is(d, Status::Working)
                        && !current(d).is_some_and(|s| s.running())
                        && drawn(d, "resume").is_none()
                },
                5.,
            ),
        ),
        (0.3, Shot("takeback-first")),
    ]);
    v
}
