use super::*;

/// The rest of the chat backlog, for stills: the latest thought while it
/// thinks, background work, a used-up limit, a crash, MCP servers in
/// trouble, a nearly full context, emoji, versions of a message, your
/// messages, `@` a session, a long table. Old builds walk it too (what they
/// lack fails its check), for before-and-after stills.
pub(super) fn finish() -> Vec<(f32, Act)> {
    let mut v = new_session("first notes");
    v.push((
        0.1,
        Until("the first session answers", |d, _| answered(d), 20.),
    ));
    v.extend(new_session("take a screenshot of the page please"));
    v.extend([
        (2.0, Shot("thinking")),
        (
            0.1,
            Until("the screenshot turn ends", |d, _| answered(d), 30.),
        ),
    ]);
    v.extend(new_session("keep watching the app and the tests"));
    v.extend([
        (
            0.1,
            Until(
                "it starts work in the background",
                |d, _| says(d, "running in the background"),
                20.,
            ),
        ),
        (0.8, Shot("background")),
    ]);
    v.extend(new_session("hit the limit now"));
    v.extend([
        (
            0.1,
            Until(
                "a used-up limit ends the turn",
                |d, _| {
                    has(d, |i| {
                        matches!(
                            i,
                            Item::Note {
                                kind: crate::data::NoteKind::Error,
                                ..
                            }
                        )
                    })
                },
                10.,
            ),
        ),
        (0.6, Shot("limit")),
    ]);
    v.extend(new_session("crash halfway please"));
    v.extend([
        (
            0.1,
            Until(
                "a crash ends the turn",
                |d, _| {
                    has(d, |i| matches!(i, Item::Note { text, .. } if text.contains("stopped unexpectedly")))
                },
                15.,
            ),
        ),
        (0.6, Shot("crash")),
    ]);
    v.extend(new_session("mcp trouble, please check"));
    v.extend([
        (
            0.1,
            Until(
                "it answers with MCP servers listed",
                |d, _| answered(d),
                20.,
            ),
        ),
        (0.6, Shot("mcp")),
    ]);
    v.extend(new_session("the context is nearly full now"));
    v.extend([
        (0.1, Until("it answers", |d, _| answered(d), 20.)),
        (0.3, Type(Field::Composer, "🎉🚀")),
        (0.3, Key("enter")),
        (
            0.1,
            Until(
                "the emoji are answered",
                |d, _| {
                    answered(d)
                        && current(d).is_some_and(|s| {
                            s.items
                                .iter()
                                .filter(|i| matches!(i, Item::User { .. }))
                                .count()
                                == 2
                        })
                },
                20.,
            ),
        ),
        (0.6, Shot("emoji")),
        // Edit from here: back to before the emoji, and a new message.
        (
            0.2,
            Call(|d, _, cx| {
                if let (Some(s), Some(to)) = (current(d).map(|s| s.id.clone()), last_sent(d)) {
                    d.live.send(dent_protocol::Request::Rewind {
                        session: s,
                        to,
                        files: false,
                    });
                }
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "it goes back to before the emoji",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.items
                            .iter()
                            .filter(|i| matches!(i, Item::User { .. }))
                            .count()
                            == 1
                    })
                },
                10.,
            ),
        ),
        (0.3, Type(Field::Composer, "Ship it 🚀")),
        (0.3, Key("enter")),
        (
            0.1,
            Until(
                "the new version is answered",
                |d, _| {
                    answered(d)
                        && current(d).is_some_and(|s| {
                            s.items
                                .iter()
                                .filter(|i| matches!(i, Item::User { .. }))
                                .count()
                                == 2
                        })
                },
                20.,
            ),
        ),
        (0.6, Shot("versions")),
        (0.2, Click("version-older")),
        (0.8, Shot("old-version")),
        (0.2, Click("latest-version")),
        (0.4, Key("shift-cmd-k")),
        (0.8, Shot("messages")),
        (0.2, Key("escape")),
        (0.3, Keys("compare it with @first")),
        (0.8, Shot("at-session")),
        (0.2, Key("escape")),
        (0.2, Type(Field::Composer, "")),
    ]);
    v.extend(new_session("a long table of the routes, please"));
    v.extend([
        (0.1, Until("the table arrives", |d, _| answered(d), 20.)),
        (0.3, Call(to_reply)),
        (0.8, Shot("table")),
        (0.2, Key("cmd-f")),
        (0.3, Keys("needle")),
        (1.0, Shot("find-row")),
        (0.2, Key("escape")),
        (0.3, Call(to_reply)),
        (0.4, Click("md-open-table")),
        (1.2, Shot("table-open")),
    ]);
    v
}
