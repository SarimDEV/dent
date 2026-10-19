use super::*;

/// The coloured command's output, as it came back.
pub(super) fn coloured_output(d: &Dent) -> Option<String> {
    current(d)?.items.iter().find_map(|i| match i {
        Item::Tools { lines, .. } if lines.len() == 1 => lines[0]
            .detail
            .as_ref()
            .filter(|dt| dt.input.contains("--color=always"))
            .and_then(|dt| dt.output.as_ref().map(|(_, o)| o.clone())),
        _ => None,
    })
}

/// The reply streaming in, as it stands.
pub(super) fn partial(d: &Dent) -> String {
    match current(d).and_then(|s| s.items.last()) {
        Some(Item::Text(t)) if status_is(d, Status::Working) => t.clone(),
        _ => String::new(),
    }
}

/// A note and a small red picture from outside the project, attached.
pub(super) fn attach_outside(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>) {
    let notes = outside("my notes.txt");
    let _ = std::fs::write(&notes, "notes\n");
    let red = outside("red.png");
    let _ = image::RgbaImage::from_pixel(48, 48, image::Rgba([220, 30, 30, 255])).save(&red);
    d.attach_paths(&[notes, red], w, cx);
}

/// Visual changes, each held still to be looked at: command output in
/// colour, a long paste folded into a chip, files on a waiting message,
/// and half-typed formatting held back as a reply streams.
pub(super) fn visuals() -> Vec<(f32, Act)> {
    let mut v = new_session("show me colour output please");
    v.extend([
        (
            0.1,
            Until(
                "the coloured command finishes",
                |d, _| done_saying(d, "Tests ran") && coloured_output(d).is_some(),
                15.,
            ),
        ),
        (
            0.,
            Call(|d, _, cx| {
                let id = current(d).and_then(|s| {
                    s.items.iter().find_map(|i| match i {
                        Item::Tools { lines, .. }
                            if lines.len() == 1
                                && lines[0]
                                    .detail
                                    .as_ref()
                                    .is_some_and(|dt| dt.input.contains("--color=always")) =>
                        {
                            Some(lines[0].id.clone())
                        }
                        _ => None,
                    })
                });
                if let Some(id) = id {
                    d.open_calls.insert(id);
                }
                cx.notify();
            }),
        ),
        (
            0.3,
            Until(
                "colour codes are drawn as colours, never as escapes",
                |d, _| {
                    coloured_output(d).is_some_and(|o| {
                        let p = crate::ansi::parsed(&o);
                        o.contains('\x1b')
                            && !p.0.contains('\x1b')
                            && !p.0.contains('\r')
                            && p.0.contains("Building [====================] 100%")
                            && !p.0.contains("30%")
                            && p.1.iter().any(|(r, st)| {
                                &p.0[r.clone()] == "✓ passed"
                                    && st.fg == Some(crate::ansi::Colour::Indexed(2))
                            })
                    }) && drawn(d, "call-out-all").is_some()
                },
                3.,
            ),
        ),
        (0.3, Call(to_end)),
        (0.4, Shot("ansi-output")),
        // A long paste folds into a chip among the words.
        (0., Call(save_clipboard)),
        (0., Call(copy_long_log)),
        (0.2, Type(Field::Composer, "Here's the sync log ")),
        (0.1, Key("cmd-v")),
        (0.1, Keys(" why does event 42 apply twice?")),
        (
            0.2,
            Until(
                "a long paste folds into one chip among the words",
                |d, cx| {
                    let c = chips(d, cx);
                    let text = d.composer.read(cx).value().to_string();
                    c.len() == 1
                        && c[0] == "Pasted text · 60 lines"
                        && text.starts_with("Here's the sync log [Pasted text #")
                        && text.ends_with("why does event 42 apply twice?")
                },
                3.,
            ),
        ),
        (0.3, Shot("paste-chip")),
        (
            0.,
            Call(|d, w, cx| {
                let field = d.composer.clone();
                let first = field
                    .read(cx)
                    .tokens()
                    .first()
                    .map(|t| (t.token().id().to_string(), t.range()));
                if let Some((id, at)) = first {
                    d.open_paste(&field, &id, at, w, cx);
                }
            }),
        ),
        (
            0.2,
            Until(
                "opened, the chip becomes the whole paste in place, on lines of its own",
                |d, cx| {
                    let text = d.composer.read(cx).value().to_string();
                    chips(d, cx).is_empty()
                        && text.starts_with("Here's the sync log\n2026-10-16")
                        && text.contains("applied event 60")
                        && text.ends_with("\nwhy does event 42 apply twice?")
                },
                3.,
            ),
        ),
        (0.3, Shot("paste-open")),
        (0., Call(restore_clipboard)),
        (
            0.,
            Call(|d, w, cx| d.composer.update(cx, |c, cx| c.set_value("", w, cx))),
        ),
        // Files on a message waiting for the reply to finish.
        (0.2, Type(Field::Composer, "a long answer, slowly")),
        (0.1, Key("enter")),
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        // From outside the project, so Changes stays shut.
        (0., Call(attach_outside)),
        (0.1, Type(Field::Composer, "and look at these two")),
        (0.1, Key("enter")),
        (
            0.2,
            Until(
                "a waiting message shows the files it carries",
                |d, _| {
                    queued_files(d)
                        == [(
                            "and look at these two".to_string(),
                            vec!["my notes.txt".to_string(), "red.png".to_string()],
                        )]
                },
                5.,
            ),
        ),
        (0.2, Call(to_end)),
        (0.4, Shot("queued-files")),
        (
            0.,
            Call(|d, w, cx| d.attach_paths(&[outside("red.png")], w, cx)),
        ),
        (0.1, Key("enter")),
        (
            0.2,
            Until(
                "a picture sent on its own waits as just its file",
                |d, _| {
                    queued_files(d)
                        .last()
                        .is_some_and(|(t, f)| t.is_empty() && f == &["red.png".to_string()])
                },
                5.,
            ),
        ),
        (0.2, Call(to_end)),
        (0.4, Shot("queued-picture")),
        // A long paste waits as its chip.
        (0., Call(save_clipboard)),
        (0., Call(copy_long_log)),
        (0.1, Key("cmd-v")),
        (0.1, Keys(" what changed here?")),
        (0.1, Key("enter")),
        (
            0.2,
            Until(
                "a waiting message shows its paste as a chip, not 60 lines",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.items.iter().any(|i| {
                            matches!(i, Item::Queued { text, .. } if text.contains("applied event 60"))
                        })
                    })
                },
                5.,
            ),
        ),
        (0., Call(restore_clipboard)),
        (0.2, Call(to_end)),
        (0.4, Shot("queued-paste")),
        (
            0.1,
            Until(
                "both go once the reply finishes",
                |d, _| status_is(d, Status::Done) && queued_files(d).is_empty() && mine(d) >= 4,
                60.,
            ),
        ),
        // Half-typed formatting waits until it's whole.
        (
            0.,
            Call(|d, _, cx| {
                d.panel_open = false;
                cx.notify();
            }),
        ),
        (0.3, Type(Field::Composer, "stream half-typed formatting")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "half-typed bold shows as bold, its stars held back",
                |d, _| {
                    let p = partial(d);
                    p.ends_with("**bo") && crate::mdstream::terminate(&p).ends_with("**bo**")
                },
                15.,
            ),
        ),
        (0.8, Shot("stream-1")),
        (
            0.1,
            Until(
                "a link whose address is half typed is a link already",
                |d, _| {
                    let p = partial(d);
                    p.ends_with("(https://exa")
                        && crate::mdstream::terminate(&p).ends_with("(https://exa)")
                },
                15.,
            ),
        ),
        (0.8, Shot("stream-2")),
        (
            0.1,
            Until(
                "a table waits for its delimiter row",
                |d, _| {
                    let p = partial(d);
                    p.ends_with("| --") && !crate::mdstream::terminate(&p).contains('|')
                },
                15.,
            ),
        ),
        (0.8, Shot("stream-3")),
        (
            0.1,
            Until(
                "a list's next number waits for its dot",
                |d, _| {
                    let p = partial(d);
                    p.ends_with("again\n2")
                        && crate::mdstream::terminate(&p).trim_end().ends_with("again")
                },
                15.,
            ),
        ),
        (0.8, Shot("stream-4")),
        (
            0.1,
            Until(
                "and the reply ends whole",
                |d, _| done_saying(d, "That's the lot."),
                20.,
            ),
        ),
        (0.3, Call(to_end)),
        (0.4, Shot("stream-done")),
    ]);
    v
}
