use super::*;

pub(super) fn composer_text(d: &Dent, cx: &App) -> String {
    d.composer.read(cx).value().to_string()
}

/// The reply box and what it does with your words: what comes back to it,
/// what Return does while a card waits, the queue's own keys, and drafts
/// put aside.
pub(super) fn composing() -> Vec<(f32, Act)> {
    let mut v = new_session("a long answer, slowly");
    v.extend([
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "queued one")),
        (0.1, Key("enter")),
        (0.1, Until("the message waits", |d, _| queued_now(d), 5.)),
        // A draft holding a folded paste, then the queue comes back to it.
        (0., Call(save_clipboard)),
        (0., Call(copy_long_log)),
        (0.1, Type(Field::Composer, "draft with ")),
        (0.1, Key("cmd-v")),
        (
            0.1,
            Until(
                "the draft holds a folded paste",
                |d, cx| chips(d, cx).len() == 1,
                2.,
            ),
        ),
        (0.1, Key("cmd-.")),
        (
            0.2,
            Until(
                "a take-back after a draft leaves the draft's paste a chip, so it's sent whole",
                |d, cx| {
                    let text = composer_text(d, cx);
                    chips(d, cx).len() == 1
                        && text.starts_with("draft with [Pasted text #")
                        && text.ends_with("\n\nqueued one")
                },
                5.,
            ),
        ),
        (0., Call(restore_clipboard)),
        (0.1, Type(Field::Composer, "")),
        (
            0.1,
            Until("it has stopped", |d, _| !status_is(d, Status::Working), 10.),
        ),
        // ⇧Tab: the chip follows the key at once, whatever the daemon says when.
        (0.1, Key("shift-tab")),
        (
            0.,
            Until(
                "⇧Tab shows the next mode on the chip at once",
                |d, _| {
                    current(d)
                        .is_some_and(|s| d.permission_shown(s) == dent_protocol::Permission::Edits)
                },
                0.2,
            ),
        ),
        (0.1, Key("shift-tab")),
        (0.1, Key("shift-tab")),
        (
            0.1,
            Until(
                "and round again to Ask",
                |d, _| current(d).is_some_and(|s| s.permission == dent_protocol::Permission::Ask),
                5.,
            ),
        ),
        // The side chat's box gives the keyboard back when the viewer goes.
        (0.1, Key("cmd-;")),
        (
            0.1,
            Until(
                "⌘; puts the keyboard in the side chat's box",
                |d, _| d.panel_open && !d.composer_keyed,
                2.,
            ),
        ),
        (0.1, Key("cmd-\\")),
        (
            0.1,
            Until(
                "hiding the viewer gives the keyboard back to the reply box",
                |d, _| !d.panel_open && d.composer_keyed,
                2.,
            ),
        ),
    ]);
    // While an approval waits, Return sends what's typed as the reason for
    // denying (main's rule), and Esc with words typed keeps the keyboard.
    fn asks(d: &Dent) -> bool {
        has(d, |i| matches!(i, Item::Approval { .. })) && status_is(d, Status::Needs)
    }
    v.extend(new_session("please run it"));
    v.extend([
        (0.1, Until("an approval waits", |d, _| asks(d), 10.)),
        (0.4, Type(Field::Composer, "")),
        (0.1, Key("escape")),
        (0.3, Held("enter")),
        (
            0.2,
            Until(
                "a held Return on the card answers nothing",
                |d, _| d.ask_keyed && asks(d),
                1.,
            ),
        ),
        (0.1, Type(Field::Composer, "not that one")),
        (0.1, Key("escape")),
        (
            0.1,
            Until(
                "Esc with words typed keeps the keyboard in the box",
                |d, cx| !d.ask_keyed && d.composer_keyed && composer_is(d, cx, "not that one"),
                1.,
            ),
        ),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "Return denies with what's typed as the reason",
                |d, _| {
                    status_is(d, Status::Done)
                        && current(d).is_some_and(|s| {
                            s.items.iter().any(|i| {
                                matches!(i, Item::Note { text, .. }
                                    if text.starts_with("Denied") && text.ends_with(": not that one"))
                            })
                        })
                },
                10.,
            ),
        ),
    ]);
    // The queue: Remove, ⌥↑, and an edited message keeping its place.
    fn waiting_texts(d: &Dent) -> Vec<String> {
        queued_ids(d).into_iter().map(|(_, _, t)| t).collect()
    }
    v.extend(new_session("a long answer, slowly"));
    v.extend([
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "one")),
        (0.1, Key("enter")),
        (0.2, Type(Field::Composer, "two")),
        (0.1, Key("enter")),
        (0.2, Type(Field::Composer, "three")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "three wait, each with Remove",
                |d, _| {
                    let q = queued_ids(d);
                    q.len() == 3
                        && q.iter()
                            .all(|(ix, ..)| drawn(d, &format!("remove-{ix}")).is_some())
                },
                5.,
            ),
        ),
        (0.2, Call(to_end)),
        (0.3, Shot("queued-remove")),
        (
            0.,
            Call(|d, _, cx| {
                let sid = d.selected.clone().unwrap_or_default();
                if let Some((_, msg, _)) = queued_ids(d).get(1).cloned() {
                    d.remove_queued(&sid, &msg, cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "Remove takes it out of the queue, with Undo",
                |d, _| {
                    waiting_texts(d) == ["one", "three"]
                        && d.toast.as_ref().is_some_and(|t| {
                            matches!(t.action, crate::app::ToastAction::Unremove { .. })
                        })
                },
                3.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
        (0.1, Key("cmd-z")),
        (
            0.3,
            Until(
                "⌘Z in the empty box leaves a removed message removed",
                |d, cx| composer_is(d, cx, "") && waiting_texts(d) == ["one", "three"],
                1.,
            ),
        ),
        (0.1, Click("toast-action")),
        (
            0.1,
            Until(
                "Undo puts it back in the queue where it waited, not in the box",
                |d, cx| composer_is(d, cx, "") && waiting_texts(d) == ["one", "two", "three"],
                3.,
            ),
        ),
        (
            0.,
            Call(|d, _, cx| {
                let sid = d.selected.clone().unwrap_or_default();
                if let Some((_, msg, _)) = queued_ids(d).last().cloned() {
                    d.remove_queued(&sid, &msg, cx);
                }
                d.toast = None;
            }),
        ),
        (
            0.1,
            Until(
                "and Remove again",
                |d, _| waiting_texts(d) == ["one", "two"],
                3.,
            ),
        ),
        (
            0.,
            Call(|d, _, cx| {
                let sid = d.selected.clone().unwrap_or_default();
                if let Some((_, msg, _)) = queued_ids(d).first().cloned() {
                    d.unqueue(&sid, &msg, cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "Edit brings the first back to the box",
                |d, cx| composer_is(d, cx, "one") && waiting_texts(d) == ["two"],
                3.,
            ),
        ),
        (0.1, Keys(" edited")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "sent again, it goes back to its place, ahead of the one after it",
                |d, _| waiting_texts(d) == ["one edited", "two"],
                3.,
            ),
        ),
        (0.2, Key("alt-up")),
        (
            0.1,
            Until(
                "⌥↑ with messages waiting still moves through yours, the queue untouched",
                |d, cx| composer_is(d, cx, "") && waiting_texts(d) == ["one edited", "two"],
                2.,
            ),
        ),
        (0.2, Type(Field::Composer, "")),
        (0.1, Key("up")),
        (
            0.1,
            Until(
                "↑ in the empty box takes the newest waiting message back to it",
                |d, cx| composer_is(d, cx, "two") && waiting_texts(d) == ["one edited"],
                3.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
        (0.1, Key("cmd-.")),
        (
            0.1,
            Until(
                "stopped, the last waiting one comes back",
                |d, cx| !status_is(d, Status::Working) && composer_is(d, cx, "one edited"),
                10.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
    ]);
    // An iPhone photo (HEIC) attached goes as a JPEG the agent can see.
    v.extend([
        (
            0.,
            Call(|d, w, cx| {
                let png = outside("photo.png");
                let heic = outside("photo.heic");
                let _ =
                    image::RgbImage::from_pixel(2600, 1200, image::Rgb([30, 120, 200])).save(&png);
                let _ = std::process::Command::new("sips")
                    .args(["-s", "format", "heic"])
                    .arg(&png)
                    .arg("--out")
                    .arg(&heic)
                    .output();
                d.attach_paths(&[heic], w, cx);
            }),
        ),
        (
            0.1,
            Until(
                "a HEIC photo attaches as a JPEG copy, no wider than 2000 px",
                |d, _| {
                    d.attachments.iter().any(|a| {
                        matches!(a, crate::app::Attachment::File { path }
                            if path.ends_with("/photo.jpg")
                                && image::image_dimensions(path).is_ok_and(|(w, _)| w == 2000))
                    })
                },
                5.,
            ),
        ),
        (0.3, Shot("heic-attached")),
        (
            0.,
            Call(|d, _, cx| {
                d.attachments.clear();
                cx.notify();
            }),
        ),
    ]);
    // ⇧⌘V pastes a long text as it is, to edit in place.
    v.extend([
        (0., Call(save_clipboard)),
        (0., Call(copy_long_log)),
        (0.1, Type(Field::Composer, "")),
        (0.1, Key("cmd-shift-v")),
        (
            0.2,
            Until(
                "⇧⌘V pastes a long text whole, not folded",
                |d, cx| {
                    let text = composer_text(d, cx);
                    chips(d, cx).is_empty()
                        && text.starts_with("2026-10-16")
                        && text.contains("applied event 60")
                },
                2.,
            ),
        ),
        (0., Call(restore_clipboard)),
        (0.1, Type(Field::Composer, "")),
    ]);
    // ↑ through a long message that wraps walks its lines as they show.
    const WRAPS: &str = "This one line is long enough to wrap over several lines of the reply box, so the arrows have lines to walk before they step back through what was sent, the way a message of several lines already did; it goes on a little more to be sure of it.";
    v.extend([
        (0.1, Type(Field::Composer, WRAPS)),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "a long line is answered",
                |d, _| {
                    status_is(d, Status::Done) && sent_order(d).last().is_some_and(|t| t == WRAPS)
                },
                20.,
            ),
        ),
        (0.2, Key("up")),
        (
            0.1,
            Until(
                "↑ brings it back, the caret at its end",
                |d, cx| composer_is(d, cx, WRAPS) && d.composer.read(cx).cursor() == WRAPS.len(),
                1.,
            ),
        ),
        (0.2, Key("up")),
        (
            0.1,
            Until(
                "↑ on a wrapped line moves the caret up a line, not back a message",
                |d, cx| composer_is(d, cx, WRAPS) && d.composer.read(cx).cursor() < WRAPS.len(),
                1.,
            ),
        ),
        (
            0.,
            Call(|d, w, cx| {
                d.composer.update(cx, |c, cx| {
                    let end = c.value().len();
                    c.set_selected_range(end..end, cx);
                    let _ = w;
                });
            }),
        ),
        (0.2, Key("down")),
        (
            0.1,
            Until(
                "↓ on its last line steps forward: the box empties",
                |d, cx| composer_is(d, cx, ""),
                1.,
            ),
        ),
    ]);
    // ⌘S puts a draft aside and brings it back.
    v.extend([
        (0., Call(|d, _, _| d.stashes.clear())),
        (0., Call(save_clipboard)),
        (0., Call(copy_long_log)),
        (0.1, Type(Field::Composer, "first thought ")),
        (0.1, Key("cmd-v")),
        (
            0.,
            Call(|d, w, cx| {
                let red = outside("red.png");
                let _ = image::RgbaImage::from_pixel(48, 48, image::Rgba([220, 30, 30, 255]))
                    .save(&red);
                d.attach_paths(&[red], w, cx);
            }),
        ),
        (0.2, Key("cmd-s")),
        (
            0.1,
            Until(
                "⌘S puts the draft aside, paste and file too, and empties the box",
                |d, cx| {
                    composer_is(d, cx, "")
                        && d.attachments.is_empty()
                        && d.stashes.len() == 1
                        && d.stashes[0].files().len() == 1
                        && crate::prefs::load().stashes.len() == 1
                },
                2.,
            ),
        ),
        (0.1, Type(Field::Composer, "second thought")),
        (0.1, Key("cmd-s")),
        (0.3, Key("cmd-s")),
        (
            0.1,
            Until(
                "⌘S in the empty box with two put aside lists them",
                |d, _| d.menu == Some(crate::app::Menu::Stash) && d.stashes.len() == 2,
                2.,
            ),
        ),
        (0.3, Shot("stash-menu")),
        (0.1, Key("down")),
        (0.1, Key("enter")),
        (
            0.2,
            Until(
                "picking the older one brings it back whole: words, paste chip and file",
                |d, cx| {
                    composer_text(d, cx).starts_with("first thought [Pasted text #")
                        && chips(d, cx).len() == 1
                        && d.attachments.len() == 1
                        && d.stashes.len() == 1
                        && d.menu.is_none()
                },
                2.,
            ),
        ),
        (0., Call(restore_clipboard)),
        (
            0.,
            Call(|d, w, cx| {
                d.attachments.clear();
                d.composer.update(cx, |c, cx| crate::app::put(c, "", w, cx));
            }),
        ),
        (0.1, Key("cmd-s")),
        (
            0.1,
            Until(
                "with one left, ⌘S brings it straight back",
                |d, cx| composer_is(d, cx, "second thought") && d.stashes.is_empty(),
                2.,
            ),
        ),
        // Picking a completion changes only its word: a folded paste
        // beside it stays a chip and goes in full.
        (0., Call(copy_long_log)),
        (0.1, Type(Field::Composer, "")),
        (0.1, Key("cmd-v")),
        (0.1, Keys(" see @READ")),
        (
            0.1,
            Until(
                "the @ menu offers the README",
                |d, cx| {
                    d.completion(cx)
                        .is_some_and(|c| c.items.iter().any(|i| i.name == "README.md"))
                },
                3.,
            ),
        ),
        (0.1, Key("tab")),
        (
            0.1,
            Until(
                "picking a file keeps a folded paste a chip, sent in full",
                |d, cx| {
                    let f = d.composer.read(cx);
                    f.tokens().len() == 1
                        && f.value().contains("@README.md")
                        && d.box_draft(cx)
                            .words_out(&d.pastes)
                            .contains("applied event 60")
                },
                3.,
            ),
        ),
        (0.1, Key("cmd-z")),
        (
            0.1,
            Until(
                "⌘Z takes the pick back, the chip kept",
                |d, cx| {
                    let f = d.composer.read(cx);
                    f.tokens().len() == 1 && !f.value().contains("@README.md")
                },
                2.,
            ),
        ),
        (0., Call(restore_clipboard)),
        (0.1, Type(Field::Composer, "")),
        (0., Call(save_now)),
    ]);
    v
}
