use super::*;

/// The art of the session's first card for a file of `kind`.
fn card_art(d: &Dent, kind: crate::data::Kind) -> Option<&'static str> {
    current(d)?.items.iter().find_map(|i| match i {
        Item::Made { art, .. } if d.art(art).is_some_and(|a| a.kind == kind) => Some(*art),
        _ => None,
    })
}

fn open_card(d: &mut Dent, kind: crate::data::Kind, cx: &mut Context<Dent>) {
    if let Some(a) = card_art(d, kind) {
        d.open_art(a, None, cx);
    }
}

fn picks(d: &Dent) -> usize {
    let Some(Item::Question { id, .. }) = current(d).and_then(|s| {
        s.items
            .iter()
            .rev()
            .find(|i| matches!(i, Item::Question { .. }))
    }) else {
        return 0;
    };
    d.picks.get(id).map_or(0, |p| p.iter().flatten().count())
}

const TEST_KEY: &str = "sk_test_51Hparity";

/// The October parity batch: media, visuals, secrets, question cards,
/// Tab for effort, restarting the agent.
pub(super) fn parity() -> Vec<(f32, Act)> {
    let mut v = new_session("make a video of the onboarding");
    v.extend([
        (
            0.1,
            Until(
                "a video the reply names gets a card",
                |d, _| card_art(d, crate::data::Kind::Media).is_some(),
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "the card gets a still of the video",
                |d, _| d.posters.values().any(Option::is_some),
                10.,
            ),
        ),
        (0.3, Call(to_end)),
    ]);
    both_themes(&mut v, "video-card");
    v.extend([
        (0.2, Call(|d, _, cx| open_card(d, crate::data::Kind::Media, cx))),
        (
            0.2,
            Until(
                "the video opens in the viewer",
                |d, _| {
                    d.panel_open
                        && current(d).is_some_and(|s| {
                            matches!(d.tab(s), Tab::Art(a) if d.art(a).is_some_and(|a| a.kind == crate::data::Kind::Media))
                        })
                },
                5.,
            ),
        ),
        (1.5, Shot("video-viewer")),
    ]);
    v.extend(new_session("show a visual of sign-ups"));
    v.extend([
        (
            0.1,
            Until(
                "the visual is a page it made, named by its title",
                |d, _| {
                    card_art(d, crate::data::Kind::Html)
                        .and_then(|a| d.art(a))
                        .is_some_and(|a| a.title == "Sign-ups by week")
                },
                10.,
            ),
        ),
        (0.3, Call(to_end)),
        (
            0.1,
            Until(
                "it shows in the chat, as tall as the page says",
                |d, _| {
                    let said = d.inline.borrow().heights.values().next().copied();
                    said.is_some_and(|h| {
                        (h - crate::inline::GUESS_H).abs() > 1.
                            && drawn(d, "inline-page")
                                .is_some_and(|b| (b.size.height.as_f32() - h).abs() < 1.5)
                    })
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "it's live there, with a still kept for when it can't be",
                |d, _| d.inline.borrow().live() == 1 && d.inline.borrow().stills() == 1,
                10.,
            ),
        ),
    ]);
    both_themes(&mut v, "visual-inline");
    v.extend([
        (0.2, Click("inline-open")),
        (
            0.1,
            Until(
                "Open beside shows it in the viewer too",
                |d, _| {
                    d.panel_open
                        && current(d).is_some_and(|s| {
                            matches!(d.tab(s), Tab::Art(a) if d.art(a).is_some_and(|a| a.kind == crate::data::Kind::Html))
                        })
                },
                5.,
            ),
        ),
        (0.8, Shot("visual-beside")),
    ]);
    // The agent looks at its page before showing it: a window draws it out
    // of sight and the picture goes back to the agent.
    v.extend(new_session("look before you show the sign-ups"));
    v.extend([
        (
            0.1,
            Until(
                "the agent sees its page, drawn by this window, before showing it",
                |d, _| {
                    says(d, "Checked it first")
                        && current(d).is_some_and(|s| {
                            s.items.iter().any(|i| matches!(i, Item::Tools { lines, .. } if lines.iter().any(|l| l.text.starts_with("Looked at the page"))))
                        })
                },
                45.,
            ),
        ),
        (
            0.1,
            Until(
                "and then it shows in the chat",
                |d, _| drawn(d, "inline-page").is_some(),
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "its picture stays the agent's: none in the chat",
                |d, _| tool_picture_count(d) == 0,
                2.,
            ),
        ),
        (0.3, Call(to_end)),
        (0.6, Shot("look-then-show")),
    ]);
    v.extend(new_session("we need a key for the payment tests"));
    v.extend([
        (
            0.1,
            Until(
                "the secret's card waits on you",
                |d, _| has(d, |i| matches!(i, Item::Secret { .. })) && status_is(d, Status::Needs),
                10.,
            ),
        ),
        (0.3, Call(to_end)),
    ]);
    both_themes(&mut v, "secret-card");
    v.extend([
        (
            0.1,
            Call(|d, w, cx| {
                d.secret_input
                    .update(cx, |i, cx| i.set_value(TEST_KEY, w, cx));
            }),
        ),
        (0.3, Shot("secret-typed")),
        (0.1, Click("secret-project")),
        (
            0.1,
            Until(
                "saved in the project's .env, and the agent carries on",
                |d, _| {
                    done_saying(d, "load it from there")
                        && current(d).is_some_and(|s| {
                            std::fs::read_to_string(std::path::Path::new(&s.cwd).join(".env"))
                                .is_ok_and(|e| e.contains(&format!("STRIPE_SECRET_KEY={TEST_KEY}")))
                        })
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "the value is nowhere in the log, and the field is empty",
                |d, cx| {
                    let db = std::env::var_os("DENT_STATE")
                        .map(|s| std::path::Path::new(&s).join("dent.db"));
                    let logged = db.and_then(|p| std::fs::read(p).ok()).is_some_and(|b| {
                        b.windows(TEST_KEY.len()).any(|w| w == TEST_KEY.as_bytes())
                    });
                    !logged && d.secret_input.read(cx).value().is_empty()
                },
                3.,
            ),
        ),
        (
            0.1,
            Until(
                "the .env is ignored, so Changes never shows the secret",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.diff.iter().any(|f| f.path == ".gitignore")
                            && s.diff.iter().all(|f| f.path != ".env")
                    })
                },
                5.,
            ),
        ),
        (0.3, Shot("secret-saved")),
    ]);
    v.extend(new_session("ask two things about the cache"));
    v.extend([
        (
            0.1,
            Until(
                "a card of two questions",
                |d, _| {
                    has(
                        d,
                        |i| matches!(i, Item::Question { questions, .. } if questions.len() == 2),
                    )
                },
                10.,
            ),
        ),
        (0.2, Key("escape")),
        (0.2, Key("1")),
        (0.4, Until("1 answers the first", |d, _| picks(d) == 1, 2.)),
        (0.2, Shot("question-first-answered")),
        (0.2, Key("backspace")),
        (
            0.1,
            Until("⌫ takes the first answer back", |d, _| picks(d) == 0, 2.),
        ),
        (0.2, Click("skip-question")),
        (
            0.1,
            Until(
                "Skip: the agent carries on, said as skipped",
                |d, _| noted(d, "Skipped the question") && done_saying(d, "I'll assume"),
                10.,
            ),
        ),
        (0.3, Call(to_end)),
        (0.2, Shot("question-skipped")),
        // Tab in the reply box steps the effort; the viewer goes, so the
        // chip has room for its word.
        (0.2, Key("cmd-\\")),
        (
            0.2,
            Call(|d, w, cx| d.composer.update(cx, |c, cx| c.focus_field(w, cx))),
        ),
        (0.1, Key("tab")),
        (
            0.1,
            Until(
                "Tab steps the effort from Default to the next level",
                |d, _| current(d).is_some_and(|s| d.effort_shown(s).is_some()),
                3.,
            ),
        ),
        (
            0.1,
            Until(
                "the daemon takes it: the session's own effort changed",
                |d, _| current(d).is_some_and(|s| s.effort.is_some()),
                5.,
            ),
        ),
        (0.3, Shot("effort-tab")),
        (0.1, Type(Field::Composer, "/restart")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "/restart says the agent starts again",
                |d, _| noted(d, "Claude Code starts again"),
                5.,
            ),
        ),
        (0.3, Shot("restart-note")),
    ]);
    v
}
