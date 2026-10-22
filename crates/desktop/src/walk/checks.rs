use super::*;

thread_local! {
    /// The viewer's width once dragged narrower.
    pub(super) static NARROWED: std::cell::Cell<f32> = const { std::cell::Cell::new(0.) };
}

/// A page in a folder outside the project, with a stylesheet and a picture
/// beside it (scripts/walk.sh puts them there).
pub(super) fn outside_page() -> &'static str {
    let dir = std::env::var("DENT_WALK_OUTSIDE").unwrap_or_else(|_| "/nonexistent".into());
    leak(format!("write {dir}/page.html with assets"))
}

/// The edge cases, against the real daemon and the fake agent.
pub(super) fn checks() -> Vec<(f32, Act)> {
    let mut v: Vec<(f32, Act)> = vec![(
        0.,
        Until(
            "connects to the daemon, starting it",
            |d, _| d.connection.is_ok(),
            15.,
        ),
    )];
    v.extend(typing());
    v.extend(settings());
    v.extend(welcome());
    v.extend([
        (0.3, Key("cmd-n")),
        (
            0.1,
            Until(
                "the model menu names versions before any session starts",
                |d, _| {
                    crate::bridge::model_name(d.new_models(), d.defaults.model.as_deref())
                        == "Default · Sonnet 5.5"
                        && d.new_models().iter().any(|m| m.name == "Fable 5.1")
                },
                25.,
            ),
        ),
        (0.2, Click("chip-model")),
        (
            0.1,
            Until(
                "older versions wait behind a row",
                |d, _| drawn(d, "m-opus").is_some() && drawn(d, "m-claude-opus-4-6").is_none(),
                3.,
            ),
        ),
        (0.3, Shot("model-menu")),
        (0.8, Click("m-older")),
        (
            0.1,
            Until(
                "and open in place",
                |d, _| drawn(d, "m-claude-opus-4-6").is_some(),
                3.,
            ),
        ),
        (0.3, Shot("model-menu-older")),
        (0.8, Click("m-claude-opus-4-6")),
        (0.2, Click("chip-effort")),
        (
            0.1,
            Until(
                "effort offers only what the model takes",
                |d, _| {
                    d.defaults.model.as_deref() == Some("claude-opus-4-6")
                        && drawn(d, "m-effort-max").is_some()
                        && drawn(d, "m-effort-xhigh").is_none()
                },
                3.,
            ),
        ),
        (0.3, Shot("effort-menu")),
        (0.8, Click("m-effort-default")),
        (0.2, Click("chip-model")),
        (0.2, Click("m-default")),
        (
            0.1,
            Until(
                "sessions from the terminal are offered",
                |d, _| !d.recent.is_empty(),
                5.,
            ),
        ),
        (
            0.1,
            Until(
                "one still open in a terminal is listed but can't be picked",
                |d, _| d.recent.first().is_some_and(|r| r.open) && d.recent.iter().any(|r| !r.open),
                5.,
            ),
        ),
        (0.2, Click("chip-pickup")),
        (0.3, Shot("terminal-sessions")),
        (0.1, Click("adopt-0")),
        (
            0.3,
            Until(
                "clicking it does nothing",
                |d, _| current(d).is_none() && !d.recent.is_empty(),
                1.,
            ),
        ),
        (0.1, Click("adopt-1")),
        (
            0.1,
            Until(
                "picking one up brings its history",
                |d, _| {
                    says(d, "tidied the README")
                        && status_is(d, Status::Done)
                        && has(
                            d,
                            |i| matches!(i, Item::Note { text, .. } if text.contains("terminal")),
                        )
                },
                5.,
            ),
        ),
    ]);
    v.extend(new_session("hello there"));
    v.extend([
        (
            0.1,
            Until(
                "a reply streams in and finishes",
                |d, _| done_saying(d, "hello there"),
                10.,
            ),
        ),
        (
            0.2,
            Until(
                "the inbox lists it under Done",
                |d, _| {
                    d.done()
                        .iter()
                        .any(|s| Some(s.id.as_str()) == d.selected.as_deref())
                },
                2.,
            ),
        ),
        (0.2, Type(Field::Composer, "/com")),
        (
            0.1,
            Until(
                "typing / offers the agent's commands",
                |d, cx| {
                    d.completion(cx)
                        .is_some_and(|c| c.items.iter().any(|c| c.name == "compact"))
                },
                5.,
            ),
        ),
        (0.3, Shot("slash")),
        (0.1, Key("tab")),
        (
            0.1,
            Until(
                "Tab completes the command",
                |d, cx| d.composer.read(cx).value() == "/compact ",
                3.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
        (0.2, Type(Field::Composer, "look at @read")),
        (
            0.1,
            Until(
                "typing @ offers the project's files",
                |d, cx| {
                    d.completion(cx)
                        .is_some_and(|c| c.items.iter().any(|c| c.name == "README.md"))
                },
                5.,
            ),
        ),
        (0.3, Shot("mention")),
        (0.1, Key("tab")),
        (
            0.1,
            Until(
                "Tab puts the file in the message",
                |d, cx| d.composer.read(cx).value() == "look at @README.md ",
                3.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
        (
            0.1,
            Call(|d, w, cx| {
                d.renaming = d.selected.clone();
                d.rename_input.update(cx, |i, cx| {
                    i.set_value("Greeting test", w, cx);
                    i.focus_field(w, cx);
                });
                cx.notify();
            }),
        ),
        (0.2, Key("enter")),
        (
            0.1,
            Until(
                "renaming a session sticks",
                |d, _| {
                    current(d).is_some_and(|s| s.title == "Greeting test") && d.renaming.is_none()
                },
                5.,
            ),
        ),
        (0.2, Click("fork")),
        (
            0.1,
            Until(
                "Fork opens a new session carrying the conversation",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.title.starts_with("Fork of")
                            && s.items.iter().any(|i| matches!(i, Item::User { .. }))
                    })
                },
                5.,
            ),
        ),
        (0.3, Shot("fork")),
    ]);
    v.extend(new_session("please run it"));
    v.extend([
        (
            0.1,
            Until(
                "an approval appears and the session needs you",
                |d, _| {
                    has(d, |i| matches!(i, Item::Approval { .. })) && status_is(d, Status::Needs)
                },
                10.,
            ),
        ),
        (0.3, Shot("approval")),
        (0.2, Click("allow")),
        (
            0.1,
            Until(
                "allowing runs it and the turn finishes, leaving no note of its own",
                |d, _| {
                    done_saying(d, "Ran it")
                        && !has(
                            d,
                            |i| matches!(i, Item::Note { text, .. } if text.starts_with("Allowed")),
                        )
                },
                10.,
            ),
        ),
        (0.3, Type(Field::Composer, "run it again")),
        (0.2, Key("enter")),
        (
            0.1,
            Until(
                "a second approval",
                |d, _| has(d, |i| matches!(i, Item::Approval { .. })),
                10.,
            ),
        ),
        (0.2, Type(Field::Composer, "not on main")),
        (0.2, Click("deny")),
        (
            0.1,
            Until(
                "deny sends the composer's text as the reason",
                |d, _| done_saying(d, "not on main"),
                10.,
            ),
        ),
    ]);
    v.extend(new_session("ask me which cache"));
    v.extend([
        (
            0.1,
            Until(
                "a question with choices",
                |d, _| has(d, |i| matches!(i, Item::Question { .. })),
                10.,
            ),
        ),
        (0.3, Shot("question")),
        (0.2, Click("opt-0-1")),
        (
            0.1,
            Until("one click answers it", |d, _| done_saying(d, "Redis"), 10.),
        ),
    ]);
    v.extend(new_session("a long answer please"));
    v.extend([
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "and then this")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "a message sent while it works waits below the reply",
                |d, _| has(d, |i| matches!(i, Item::Queued { .. })),
                5.,
            ),
        ),
        (0.2, Shot("queued")),
        (
            0.1,
            Until(
                "the waiting message goes after the reply",
                |d, _| {
                    done_saying(d, "and then this") && !has(d, |i| matches!(i, Item::Queued { .. }))
                },
                20.,
            ),
        ),
        (0.3, Type(Field::Composer, "another long one")),
        (0.1, Key("enter")),
        (0.1, Until("streams again", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "the steer thing")),
        (0.1, Key("cmd-enter")),
        (
            0.1,
            Until(
                "⌘↩ steers into the running turn",
                |d, _| {
                    done_saying(d, "steer thing")
                        && !has(d, |i| matches!(i, Item::Note { kind: crate::data::NoteKind::Stopped, .. }))
                },
                20.,
            ),
        ),
        (0.3, Type(Field::Composer, "a long answer please")),
        (0.1, Key("enter")),
        (0.1, Until("streams a third time", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "save this for later")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "it waits",
                |d, _| has(d, |i| matches!(i, Item::Queued { .. })),
                5.,
            ),
        ),
        (0.1, Key("cmd-.")),
        (
            0.1,
            Until(
                "⌘. stops it within three seconds",
                |d, _| status_is(d, Status::Idle),
                3.,
            ),
        ),
        (
            0.1,
            Until(
                "a message still waiting comes back to the composer",
                |d, cx| {
                    d.composer.read(cx).value() == "save this for later"
                        && !has(d, |i| matches!(i, Item::Queued { .. }))
                },
                3.,
            ),
        ),
        (
            0.,
            Until(
                "and can be sent straight away",
                |d, _| d.just_sent.is_none(),
                0.2,
            ),
        ),
        (0.3, Shot("stopped")),
        (0.2, Type(Field::Composer, "a long answer please")),
        (0.1, Key("enter")),
        (0.1, Until("streams again", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "keep this draft")),
        (0.1, Key("escape")),
        (
            0.1,
            Until(
                "Esc in the reply box stops it too, right after the last stop, and the draft stays",
                |d, cx| {
                    status_is(d, Status::Idle) && d.composer.read(cx).value() == "keep this draft"
                },
                3.,
            ),
        ),
        (0.2, Type(Field::Composer, "")),
        (0.2, Wheel("reply", 2400.)),
        (
            0.3,
            Until(
                "reading back offers the way to the latest, and the arrows know where your messages are",
                |d, _| drawn(d, "to-latest").is_some() && drawn(d, "mine-next").is_some(),
                2.,
            ),
        ),
        (0.6, Shot("reading-back")),
        (0.1, Click("mine-next")),
        (
            0.3,
            Until(
                "the down arrow takes you to your next message",
                |d, _| {
                    let top = d.top_item();
                    top > 0 && current(d).is_some_and(|s| matches!(s.items.get(top), Some(Item::User { .. })))
                },
                2.,
            ),
        ),
        (0.2, Click("to-latest")),
        (
            0.3,
            Until(
                "and the pill back to the latest",
                |d, _| d.transcript.is_following_tail() && drawn(d, "to-latest").is_none(),
                2.,
            ),
        ),
        (0.3, Key("alt-up")),
        (
            0.1,
            Until(
                "Alt+Up jumps back through your own messages",
                |d, _| {
                    let top = d.top_item();
                    top > 0
                        && current(d)
                            .is_some_and(|s| matches!(s.items.get(top), Some(Item::User { .. })))
                },
                3.,
            ),
        ),
    ]);
    v.extend(new_session("write site/page.html please"));
    v.extend([
        (
            0.1,
            Until(
                "writing a file asks first",
                |d, _| has(d, |i| matches!(i, Item::Approval { .. })),
                10.,
            ),
        ),
        (0.2, Click("allow-session")),
        (
            0.1,
            Until(
                "the page it wrote gets a card and opens in the viewer",
                |d, _| {
                    has(d, |i| matches!(i, Item::Made { .. }))
                        && current(d).is_some_and(|s| matches!(d.tab(s), Tab::Art(_)))
                },
                10.,
            ),
        ),
        (0.1, Until("the page's card offers Review, with no separate line", |d, _| has(d, |i| matches!(i, Item::Made { review: Some(p), .. } if p == "site/page.html")) && !has(d, |i| matches!(i, Item::Changed { paths } if paths.iter().any(|p| p == "site/page.html"))), 5.)),
        (1.0, Shot("viewer-page")),
        (0.1, Call(|d, _, _| d.panel_width = 600.)),
        (0.3, Press("resize", 0.5, 0.5)),
        (0.05, MoveBy(150., 0.)),
        (
            0.,
            Until(
                "dragging the panel's edge across a live page narrows it, the page standing aside meanwhile",
                |d, _| d.panel_width < 460. && d.dragging && !d.preview_visible(),
                1.,
            ),
        ),
        (0., Call(|d, _, _| NARROWED.with(|n| n.set(d.panel_width)))),
        (0.05, MoveBy(-250., 0.)),
        (0.05, Release),
        (
            0.1,
            Until(
                "and back out wider, the page returning",
                // As wide as the window lets it be: a narrow window
                // keeps the chat its minimum.
                |d, _| {
                    d.panel_width > NARROWED.with(|n| n.get()) + 5.
                        && !d.dragging
                        && d.preview_visible()
                },
                1.,
            ),
        ),
        (0.2, Type(Field::Composer, outside_page())),
        (0.1, Key("enter")),
        (0.1, Until("it asks to write there", |d, _| has(d, |i| matches!(i, Item::Approval { .. })) && status_is(d, Status::Needs), 10.)),
        (0.2, Click("allow")),
        (
            0.1,
            Until(
                "a page written outside the project opens",
                |d, _| {
                    status_is(d, Status::Done)
                        && current(d).is_some_and(|s| matches!(d.tab(s), Tab::Art(a) if a.ends_with("page.html") && a.contains("outside")))
                },
                10.,
            ),
        ),
        (1.0, Page("window.ipc.postMessage(JSON.stringify({t:'probe', msg: getComputedStyle(document.querySelector('h1')).color + '|' + document.querySelector('img').naturalWidth}))")),
        (
            0.1,
            Until(
                "and finds its own styles and pictures beside it",
                |d, _| d.page_probe.as_deref() == Some("rgb(200, 30, 30)|1"),
                3.,
            ),
        ),
        (0.2, Shot("outside-page")),
        (0.1, Call(|d, _, _| {
            if let Some(p) = &d.preview {
                p.borrow().take_focus();
            }
        })),
        (
            0.1,
            Until(
                "a click into the page gives it the keyboard",
                |d, _| !cfg!(target_os = "macos") || d.preview.as_ref().is_some_and(|p| p.borrow().has_focus()),
                2.,
            ),
        ),
        (0.1, Press("composer", 0.5, 0.5)),
        (0.05, Release),
        (
            0.1,
            Until(
                "and a click back in the chat takes it back",
                |d, _| d.preview.as_ref().is_some_and(|p| !p.borrow().has_focus()),
                2.,
            ),
        ),
        (
            0.2,
            Until(
                "its changes arrive",
                |d, _| current(d).is_some_and(|s| !s.diff.is_empty()),
                5.,
            ),
        ),
        (0.2, Click("tab-changes")),
        (0.6, Shot("changes")),
        (0.2, Key("cmd-w")),
        (
            0.1,
            Until(
                "⌘W closes a finished session",
                |d, _| d.sessions.iter().any(|s| s.closed),
                3.,
            ),
        ),
        (
            0.2,
            Call(|d, _, cx| {
                eprintln!(
                    "      before ⌘Z: toast={:?} composer={:?} starter={:?}",
                    d.toast.as_ref().map(|t| t.action.clone()),
                    d.composer.read(cx).value().to_string(),
                    d.starter.read(cx).value().to_string()
                )
            }),
        ),
        (0.2, Key("cmd-z")),
        (
            0.5,
            Call(|d, w, cx| {
                eprintln!(
                    "      after ⌘Z: toast={:?} closed={:?} inbox_focused={} composer_focused={}",
                    d.toast.as_ref().map(|t| t.action.clone()),
                    d.sessions
                        .iter()
                        .filter(|s| s.closed)
                        .map(|s| s.title.clone())
                        .collect::<Vec<_>>(),
                    d.inbox_focus.is_focused(w),
                    d.composer.read(cx).focus_handle(cx).is_focused(w),
                )
            }),
        ),
        (
            0.1,
            Until(
                "⌘Z brings it back",
                |d, _| d.sessions.iter().all(|s| !s.closed),
                3.,
            ),
        ),
    ]);
    v.extend(new_session("hello again"));
    v.extend([
        (0.1, Until("ready for the next message", |d, _| status_is(d, Status::Done), 10.)),
        (0.1, Call(save_clipboard)),
        (0.2, Press("reply", 0.01, 0.2)),
        (0.05, MoveBy(400., 0.)),
        (0.05, MoveBy(2000., 40.)),
        (0.4, Shot("selecting")),
        (0.05, Release),
        (0.3, Shot("selected")),
        (0.1, Key("cmd-c")),
        (
            0.2,
            Until(
                "a reply's text can be selected and copied",
                |d, cx| {
                    let got = copied(cx);
                    let reply = current(d).and_then(|s| {
                        s.items.iter().rev().find_map(|i| match i {
                            Item::Text(t) => Some(t.clone()),
                            _ => None,
                        })
                    });
                    got.trim().len() > 3
                        && reply.is_some_and(|r| r.contains(got.split_whitespace().next().unwrap_or("\0")))
                },
                2.,
            ),
        ),
        (0.1, Call(restore_clipboard)),
        (0.1, Key("escape")),
        (0.2, Key("cmd-3")),
        (
            0.2,
            Until(
                "⌘ 3 opens your own shell, focused",
                |d, _| current(d).is_some_and(|s| d.shells.contains_key(&s.id)) && drawn(d, "shell").is_some(),
                5.,
            ),
        ),
        (1.0, Keys("echo dent-$((6*7))")),
        (0.1, Key("enter")),
        (
            0.2,
            Until(
                "what you type runs in it",
                |d, _| current(d).and_then(|s| d.shells.get(&s.id)).is_some_and(|sh| sh.text().contains("dent-42")),
                5.,
            ),
        ),
        (0.3, Shot("terminal")),
        (0.1, Key("cmd-a")),
        (
            0.1,
            Until(
                "⌘A in your terminal selects all of it, not the chat",
                |d, _| {
                    current(d).and_then(|s| d.shells.get(&s.id)).and_then(|sh| sh.copy()).is_some_and(|t| t.contains("dent-42"))
                        && !crate::select::has_selection()
                },
                1.,
            ),
        ),
        (0.1, Key("ctrl-c")),
        (0.1, Key("cmd-2")),
        (0.1, Call(attach_red)),
        (0.3, Shot("attachment")),
        (0.1, Type(Field::Composer, "what is in this picture")),
        (0.1, Key("enter")),
        (0.1, Until("a dropped picture goes with the message", |d, _| has(d, |i| matches!(i, Item::User { files, .. } if files.iter().any(|f| f.ends_with("red.png")))) && status_is(d, Status::Done), 10.)),
    ]);
    v.extend(new_session("this will fail"));
    v.extend([
        (
            0.1,
            Until(
                "a failed turn says what went wrong",
                |d, _| {
                    status_is(d, Status::Needs)
                        && has(d, |i| {
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
        (0.2, Click("retry-2")),
        (
            0.1,
            Until(
                "Try again sends the message again",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.items
                            .iter()
                            .filter(|i| matches!(i, Item::User { .. }))
                            .count()
                            == 2
                    })
                },
                10.,
            ),
        ),
    ]);
    v.extend(new_session("start the dev server please"));
    v.extend([
        (0.1, Until("a dev server it starts gets a live page", |d, _| {
            status_is(d, Status::Done)
                && current(d).is_some_and(|s| !s.servers.is_empty() && matches!(d.tab(s), Tab::Art(a) if d.art(a).is_some_and(|a| a.kind == crate::data::Kind::Server)))
        }, 10.)),
        (1.5, Shot("dev-server")),
        (0.1, Page("window.ipc.postMessage(JSON.stringify({t:'probe', msg: document.title}))")),
        (0.1, Until("the live page talks back", |d, _| d.page_probe.as_deref() == Some("Dev"), 3.)),
    ]);
    v.extend(new_session("use a subagent please"));
    v.extend([(
        0.1,
        Until("a subagent shows how far it got", |d, _| {
            status_is(d, Status::Done)
                && has(d, |i| matches!(i, Item::Tools { summary, lines, .. } if summary.contains("Subagent:") && summary.contains("Survey the tests · 3 steps") && !lines.iter().any(|l| l.text.contains("Survey the tests"))))
        }, 10.),
    ), (
        0.1,
        Until("a subagent's own subagent sits a level deeper, with its report", |d, _| {
            has(d, |i| matches!(i, Item::Tools { lines, .. }
                if lines.iter().any(|l| l.depth == 0 && l.text.contains("Time the db tests"))
                && lines.iter().any(|l| l.depth == 1 && l.text.contains("db tests with timings"))
                && lines.iter().any(|l| l.depth == 0 && l.report.as_deref().is_some_and(|r| r.contains("Slow tests")))))
        }, 3.),
    ), (
        0.1,
        Call(|d, _, cx| {
            let Some(s) = d.current_rc() else { return };
            if let Some(Item::Tools { lines, .. }) = s.items.iter().find(|i| matches!(i, Item::Tools { lines, .. } if lines.iter().any(|l| l.report.is_some()))) {
                d.open_tools.insert(lines[0].id.clone());
                cx.notify();
            }
        }),
    ), (0.8, Shot("subagent"))]);
    // Side chat: a question with the session's context, kept out of it.
    v.extend([
        (0.2, Key("cmd-;")),
        (
            0.1,
            Until(
                "⌘; opens the side chat",
                |d, _| current(d).is_some_and(|s| d.tab(s) == Tab::Aside),
                3.,
            ),
        ),
        (0.1, Type(Field::Aside, "which tests are slow?")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "a side question gets an answer, and the conversation doesn't change",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.asides.iter().any(|a| {
                            a.question == "which tests are slow?"
                                && matches!(&a.answer, Some((t, false, Some(_))) if t.contains("which tests are slow?"))
                        }) && !s.items.iter().any(|i| matches!(i, Item::User { text, .. } if text.contains("which tests")))
                    })
                },
                10.,
            ),
        ),
        (0.6, Shot("side-chat")),
        (1.2, Click("promote-0")),
        (
            0.1,
            Until(
                "Make it a fork opens a session that carries on from the answer",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.title.starts_with("Fork of")
                            && s.items.iter().any(|i| matches!(i, Item::User { text, .. } if text == "which tests are slow?"))
                    })
                },
                10.,
            ),
        ),
    ]);
    // Slowly, so the plan is still up for its stills.
    v.extend(new_session("make a plan slowly please"));
    v.extend([
        (
            0.1,
            Until(
                "the agent's plan shows above the composer",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.todos.len() == 3 && s.todos.iter().any(|(st, ..)| st == "in_progress")
                    }) && plan_above_composer(d)
                },
                10.,
            ),
        ),
        (0.4, Shot("plan")),
        (
            0.6,
            Call(|d, _, cx| {
                d.plan_open = false;
                cx.notify();
            }),
        ),
        (
            0.,
            Until(
                "folded, the plan is one line while the turn runs",
                |d, _| status_is(d, Status::Working) && !d.plan_open,
                1.,
            ),
        ),
        (0.3, Shot("plan-folded")),
        (
            0.1,
            Call(|d, _, cx| {
                d.plan_open = true;
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "the plan moves along and the turn ends",
                |d, _| {
                    status_is(d, Status::Done)
                        && current(d).is_some_and(|s| {
                            s.todos.iter().filter(|(st, ..)| st == "completed").count() == 1
                        })
                },
                // Two steps held 12 s each: the turn runs about 24 s.
                30.,
            ),
        ),
        (
            0.1,
            Call(|d, _, cx| {
                d.plan_open = false;
                cx.notify();
            }),
        ),
    ]);
    v.extend(new_session("write first.md please"));
    v.extend([
        (0.1, Until("asks to write", |d, _| has(d, |i| matches!(i, Item::Approval { .. })), 10.)),
        (0.2, Click("allow-session")),
        (0.1, Until("the first file is written", |d, _| status_is(d, Status::Done) && in_project("first.md").exists(), 10.)),
        (0.2, Type(Field::Composer, "write second.md please")),
        (0.1, Key("enter")),
        // The fake agent asks again; the real one remembers the session rule.
        (0.1, Until("asks again or writes", |d, _| has(d, |i| matches!(i, Item::Approval { .. })) || in_project("second.md").exists(), 10.)),
        (0.2, ClickIfShown("allow")),
        (0.1, Until("the second file is written", |d, _| status_is(d, Status::Done) && in_project("second.md").exists(), 10.)),
        (0.2, Call(|d, _, cx| {
            if let (Some(s), Some(m)) = (d.selected.clone(), last_sent(d)) {
                d.rewinding = Some((s, m));
            }
            cx.notify();
        })),
        (0.4, Shot("rewind-confirm")),
        (0.8, Wait),
        (0.1, Call(|d, w, cx| {
            if let Some(m) = last_sent(d) {
                d.rewind(&m, true, w, cx);
            }
        })),
        (0.1, Until("Edit from here takes the chat and the files back, and the message returns to the box", |d, cx| {
            !in_project("second.md").exists()
                && in_project("first.md").exists()
                && d.composer.read(cx).value().contains("second.md")
                && !has(d, |i| matches!(i, Item::User { text, .. } if text.contains("second.md")))
        }, 10.)),
        (0.2, Type(Field::Composer, "")),
    ]);
    v.extend(rich_reply());
    v.extend(new_session("a long answer please"));
    v.extend([
        (
            0.1,
            Until("streams before the crash", |d, _| streaming(d), 10.),
        ),
        (0.1, Call(|d, _, _| d.drops = 0)),
        (0.1, Call(kill_daemon)),
        (
            0.1,
            Until("the app notices the daemon went", |d, _| d.drops > 0, 5.),
        ),
        (
            0.1,
            Until(
                "it starts a new one and the reply still finishes",
                |d, _| d.connection.is_ok() && done_saying(d, "That's the plan"),
                20.,
            ),
        ),
        // The inbox has focus, so no caret blinks.
        (0.5, Key("escape")),
        (
            0.5,
            Idle(
                "idle, the window draws no more than a caret blink (2.5 a second)",
                3.,
                2.5,
            ),
        ),
    ]);
    // Daily use, after the scenes that lean on files written at the start
    // still being fresh.
    v.extend(breath());
    v.extend(file_links());
    v.extend(recall());
    v.extend(find());
    v.extend(find_viewer());
    v.extend(answer_keys());
    v.extend(reply_actions());
    v.extend(switching());
    v.extend(view_choices());
    v.extend(pins());
    v.extend(failures());
    v.extend(queue_order());
    v.extend(narration());
    v.extend(attaching());
    v.extend(turn_changes());
    v.extend(visuals());
    v.extend(fold());
    v.extend(composing());
    v.extend(edges());
    v
}
