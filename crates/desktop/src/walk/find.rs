use super::*;

pub(super) fn find_text(d: &Dent, cx: &App) -> String {
    d.find_input.read(cx).value().to_string()
}

pub(super) fn find_at(d: &Dent) -> Option<(usize, usize)> {
    d.find.as_ref().map(|f| (f.at, f.hits.len()))
}

/// ⌘F finds in the session: counts, steps round, says when nothing matches.
pub(super) fn find() -> Vec<(f32, Act)> {
    // The project menu's search is a text field like any other: what you
    // type there is typed, not taken as a shortcut.
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (0.2, Click("chip-project")),
        (0.2, Keys("2?")),
        (0.1, Key("cmd-a")),
        (0.1, Keys("x?")),
        (
            0.1,
            Until(
                "keys typed in the project search stay there",
                |d, cx| d.menu_query.read(cx).value() == "x?" && !d.keys_open,
                1.,
            ),
        ),
        (0.1, Key("escape")),
    ];
    v.extend(new_session("check the parser"));
    v.push((
        0.1,
        Until(
            "the first turn ends",
            |d, _| status_is(d, Status::Done),
            20.,
        ),
    ));
    v.extend(ask_and_wait("fix the footer"));
    v.extend(ask_and_wait("tidy the docs"));
    v.extend(ask_and_wait("the parser again"));
    v.extend([
        (0.3, Key("cmd-f")),
        (
            0.1,
            Until(
                "⌘F opens the bar with the keyboard in it",
                |d, _| d.find.is_some() && drawn(d, "find-bar").is_some(),
                1.,
            ),
        ),
        (0.1, Keys("parser")),
        (
            0.1,
            Until(
                "it counts every match, in your messages and the replies",
                |d, _| find_at(d).is_some_and(|(_, n)| n == 4),
                1.,
            ),
        ),
        (0.4, Shot("find")),
        // A still takes a moment; nothing changes until it's taken.
        (
            0.8,
            Call(|d, _, _| MARK.with(|m| m.set(find_at(d).map_or(0, |f| f.0)))),
        ),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "Return goes to the next match, round the end",
                |d, _| find_at(d).is_some_and(|(at, _)| at == (mark() + 1) % 4),
                1.,
            ),
        ),
        (0.1, Key("shift-enter")),
        (
            0.1,
            Until(
                "⇧Return goes back",
                |d, _| find_at(d).is_some_and(|(at, _)| at == mark()),
                1.,
            ),
        ),
        (0.1, Key("cmd-g")),
        (0.1, Key("cmd-g")),
        (
            0.1,
            Until(
                "⌘G steps on too",
                |d, _| find_at(d).is_some_and(|(at, _)| at == (mark() + 2) % 4),
                1.,
            ),
        ),
        // A click in the chat, then one back in the field: the editing
        // keys are the field's again.
        (0.1, Call(save_clipboard)),
        (0.2, Press("reply", 0.01, 0.2)),
        (0.05, Release),
        (0.2, Press("find-field", 0.5, 0.5)),
        (0.05, Release),
        (0.1, Key("cmd-a")),
        (0.1, Keys("zzqx")),
        (
            0.1,
            Until(
                "after a click in the chat, ⌘A in the field takes what's typed there",
                |d, cx| find_text(d, cx) == "zzqx" && !crate::select::has_selection(),
                1.,
            ),
        ),
        (
            0.1,
            Until(
                "something not there finds nothing",
                |d, _| find_at(d).is_some_and(|(_, n)| n == 0),
                1.,
            ),
        ),
        (0.3, Shot("find-none")),
        (
            0.8,
            Call(|_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string("kept".into()))),
        ),
        (0.1, Key("cmd-c")),
        (
            0.1,
            Until(
                "⌘C with nothing selected leaves the clipboard be",
                |_, cx| clipboard_text(cx) == "kept",
                1.,
            ),
        ),
        (0.1, Key("cmd-a")),
        (0.1, Key("cmd-x")),
        (
            0.1,
            Until(
                "⌘X cuts",
                |d, cx| find_text(d, cx).is_empty() && clipboard_text(cx) == "zzqx",
                1.,
            ),
        ),
        (0.1, Key("cmd-v")),
        (
            0.1,
            Until("⌘V pastes", |d, cx| find_text(d, cx) == "zzqx", 1.),
        ),
        (0.1, Key("cmd-z")),
        (
            0.1,
            Until("⌘Z undoes", |d, cx| find_text(d, cx).is_empty(), 1.),
        ),
        (0.1, Keys("abc")),
        (0.1, Key("ctrl-b")),
        (0.1, Key("ctrl-d")),
        (
            0.1,
            Until(
                "⌃B steps back and ⌃D deletes forward",
                |d, cx| find_text(d, cx) == "ab",
                1.,
            ),
        ),
        (0.1, Key("ctrl-a")),
        (0.1, Key("ctrl-k")),
        (
            0.1,
            Until(
                "⌃A goes to the start and ⌃K deletes the rest",
                |d, cx| find_text(d, cx).is_empty(),
                1.,
            ),
        ),
        // Text selected in the chat after: ⌘C copies that.
        (0.1, Keys("parse")),
        (0.2, Press("reply", 0.01, 0.2)),
        (0.05, MoveBy(400., 0.)),
        (0.05, Release),
        (0.2, Key("cmd-c")),
        (
            0.1,
            Until(
                "text selected in the chat since is what ⌘C copies",
                |d, cx| {
                    let got = clipboard_text(cx);
                    got.trim().len() > 3 && got != "kept" && find_text(d, cx) == "parse"
                },
                1.,
            ),
        ),
        (0.1, Call(restore_clipboard)),
        (0.8, Key("escape")),
        (
            0.1,
            Until(
                "Esc puts the bar away and the keyboard goes back to the reply box",
                |d, _| d.find.is_none() && drawn(d, "find-bar").is_none(),
                1.,
            ),
        ),
        // The reply box keeps the keyboard while you select in the chat;
        // typing in it hands ⌘A back to it.
        (0.2, Press("reply", 0.01, 0.2)),
        (0.05, MoveBy(400., 0.)),
        (0.05, Release),
        (0.1, Keys("hi")),
        (0.1, Key("cmd-a")),
        (0.1, Keys("yo")),
        (
            0.1,
            Until(
                "typing after selecting in the chat gives ⌘A back to the field",
                |d, cx| d.composer.read(cx).value() == "yo" && crate::select::has_selection(),
                1.,
            ),
        ),
        (0.1, Key("cmd-a")),
        (0.1, Key("backspace")),
    ]);
    v
}

pub(super) fn find_scope(d: &Dent) -> Option<crate::find::Scope> {
    d.find.as_ref().map(|f| f.scope)
}

/// Look whether the current match is on screen, for the next step.
pub(super) fn look_shown(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>) {
    SHOWN.with(|v| v.set(match_shown(d, w, cx)));
}

/// The current match is on screen, as last drawn.
pub(super) fn match_shown(d: &Dent, w: &Window, cx: &App) -> bool {
    let Some(f) = d.find.as_ref() else {
        return false;
    };
    let (Some(s), Some(&(ix, k))) = (current(d), f.hits.get(f.at)) else {
        return false;
    };
    let needle = d.find_input.read(cx).value().to_string();
    crate::select::in_view(d.scope_area(s, f.scope), ix as u32, k, &needle, w)
}

pub(super) fn doc_top(d: &Dent) -> f32 {
    current(d)
        .and_then(|s| match d.tab(s) {
            Tab::Art(id) => d.doc_scrolls.get(id),
            _ => None,
        })
        .map_or(0., |h| -h.offset().y.as_f32())
}

/// A long document, with a word three times far down it.
pub(super) fn write_long_doc(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    let mut md = String::from("# A long read\n\n");
    for i in 1..=160 {
        let word = if matches!(i, 25 | 90 | 150) {
            " The zebra crossed here."
        } else {
            ""
        };
        md.push_str(&format!(
            "Paragraph {i} of the long read, with enough words to wrap.{word}\n\n"
        ));
    }
    let path = in_project("long.md");
    let _ = std::fs::write(&path, md);
    d.open_file(&path.to_string_lossy(), cx);
}

/// ⌘F in the viewer: a document, the changes and side chat, each where
/// you last clicked; the chat again after a click there.
pub(super) fn find_viewer() -> Vec<(f32, Act)> {
    let mut v = new_session("write notes.md please");
    v.extend([
        (
            0.1,
            Until(
                "asks to write",
                |d, _| has(d, |i| matches!(i, Item::Approval { .. })),
                10.,
            ),
        ),
        (0.2, Click("allow")),
        (
            0.1,
            Until(
                "it writes",
                |d, _| status_is(d, Status::Done) && in_project("notes.md").exists(),
                10.,
            ),
        ),
        (0.2, Call(write_long_doc)),
        (
            0.1,
            Until(
                "the long document opens in the viewer",
                |d, _| {
                    current(d)
                        .is_some_and(|s| matches!(d.tab(s), Tab::Art(a) if a.ends_with("long.md")))
                        && drawn(d, "doc").is_some()
                },
                3.,
            ),
        ),
        (0.3, Press("doc", 0.5, 0.2)),
        (0.05, Release),
        (0.1, Key("cmd-f")),
        (
            0.1,
            Until(
                "⌘F after a click in the document searches the document",
                |d, _| {
                    matches!(find_scope(d), Some(crate::find::Scope::Doc(_)))
                        && drawn(d, "find-bar").is_some()
                },
                1.,
            ),
        ),
        (0.1, Keys("zebra")),
        (
            0.1,
            Until(
                "it counts the matches in the document",
                |d, _| find_at(d) == Some((0, 3)),
                2.,
            ),
        ),
        (0.5, Call(look_shown)),
        (0.1, Until("and scrolls the first into view", shown, 0.5)),
        (0.4, Shot("find-doc")),
        (
            0.8,
            Call(|d, _, _| MARK.with(|m| m.set(doc_top(d) as usize))),
        ),
        (0.1, Key("cmd-g")),
        (
            0.1,
            Until(
                "⌘G goes on down the document",
                |d, _| find_at(d) == Some((1, 3)) && doc_top(d) as usize > mark() + 50,
                2.,
            ),
        ),
        (0.5, Call(look_shown)),
        (0.1, Until("to the next one, in view", shown, 0.5)),
        (0.1, Key("escape")),
        (0.1, Until("Esc puts it away", |d, _| d.find.is_none(), 1.)),
        // The changes: the file it wrote.
        (0.2, Key("cmd-2")),
        (
            0.1,
            Until(
                "Changes shows what it wrote",
                |d, _| {
                    current(d).is_some_and(|s| d.tab(s) == Tab::Changes)
                        && drawn(d, "diff").is_some()
                },
                3.,
            ),
        ),
        (0.2, Press("diff", 0.6, 0.1)),
        (0.05, Release),
        (0.1, Key("cmd-f")),
        (0.1, Keys("one")),
        (
            0.1,
            Until(
                "⌘F after a click in the changes searches the diff",
                |d, _| {
                    find_scope(d) == Some(crate::find::Scope::Changes)
                        && find_at(d).is_some_and(|(_, n)| n == 1)
                        && drawn(d, "find-bar").is_some()
                },
                2.,
            ),
        ),
        (0.4, Shot("find-changes")),
        (0.8, Key("escape")),
        // Side chat.
        (0.2, Key("cmd-;")),
        (
            0.1,
            Until(
                "side chat opens",
                |d, _| current(d).is_some_and(|s| d.tab(s) == Tab::Aside),
                3.,
            ),
        ),
        (0.1, Type(Field::Aside, "where is the walrus?")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "it's answered",
                |d, _| current(d).is_some_and(|s| s.asides.iter().any(|a| a.answer.is_some())),
                10.,
            ),
        ),
        (0.3, Press("asides", 0.5, 0.05)),
        (0.05, Release),
        (0.1, Key("cmd-f")),
        (0.1, Keys("walrus")),
        (
            0.1,
            Until(
                "⌘F after a click in side chat searches side chat",
                |d, _| {
                    find_scope(d) == Some(crate::find::Scope::Aside)
                        && find_at(d).is_some_and(|(_, n)| n >= 1)
                },
                2.,
            ),
        ),
        (0.5, Call(look_shown)),
        (0.1, Until("with the match in view", shown, 0.5)),
        (0.8, Key("escape")),
        // Your terminal, while it has the keyboard.
        (0.2, Key("cmd-3")),
        (
            0.1,
            Until(
                "the terminal opens",
                |d, _| {
                    current(d).is_some_and(|s| d.shells.contains_key(&s.id))
                        && drawn(d, "shell").is_some()
                },
                5.,
            ),
        ),
        (1.0, Keys("echo walrus-$((6*7)) here")),
        (0.1, Key("enter")),
        (
            0.2,
            Until(
                "it prints",
                |d, _| {
                    current(d)
                        .and_then(|s| d.shells.get(&s.id))
                        .is_some_and(|sh| sh.text().contains("walrus-42 here"))
                },
                5.,
            ),
        ),
        (0.1, Key("cmd-f")),
        (0.1, Keys("walrus-42")),
        (
            0.1,
            Until(
                "⌘F in the terminal searches its scrollback",
                |d, _| {
                    find_scope(d) == Some(crate::find::Scope::Terminal)
                        && find_at(d) == Some((0, 1))
                },
                2.,
            ),
        ),
        (0.4, Shot("find-terminal")),
        (0.8, Key("escape")),
        (
            0.1,
            Until(
                "Esc gives the keyboard back to the terminal",
                |d, _| d.find.is_none(),
                1.,
            ),
        ),
        (
            0.1,
            Call(|d, w, _| SHOWN.with(|v| v.set(d.term_focus.is_focused(w)))),
        ),
        (
            0.1,
            Until("the terminal has the keyboard again", shown, 0.5),
        ),
        // A page, while it has the keyboard.
        (0.2, Type(Field::Composer, "write page.html please")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "asks to write the page",
                |d, _| has(d, |i| matches!(i, Item::Approval { .. })),
                10.,
            ),
        ),
        (0.2, Click("allow")),
        (
            0.1,
            Until(
                "it writes the page",
                |d, _| status_is(d, Status::Done) && in_project("page.html").exists(),
                10.,
            ),
        ),
        (
            0.2,
            Call(|d, _, cx| d.open_file(&in_project("page.html").to_string_lossy(), cx)),
        ),
        (
            0.1,
            Until(
                "the page is showing",
                |d, _| {
                    status_is(d, Status::Done)
                        && current(d).is_some_and(
                            |s| matches!(d.tab(s), Tab::Art(a) if a.ends_with("page.html")),
                        )
                },
                10.,
            ),
        ),
        (
            1.5,
            Call(|d, _, _| {
                if let Some(p) = &d.preview {
                    p.borrow().take_focus();
                }
            }),
        ),
        (0.3, Key("cmd-f")),
        (0.1, Keys("hello")),
        (
            0.1,
            Until(
                "⌘F in a page searches the page, which counts what it finds",
                |d, _| {
                    matches!(find_scope(d), Some(crate::find::Scope::Page(_)))
                        && d.find.as_ref().is_some_and(|f| f.page_count >= 1)
                        && find_at(d).is_some_and(|(_, n)| n >= 1)
                },
                3.,
            ),
        ),
        (0.4, Shot("find-page")),
        (0.8, Key("escape")),
        // Back in the chat: it searches the chat.
        (0.2, Press("reply", 0.01, 0.2)),
        (0.05, Release),
        (0.1, Key("cmd-f")),
        (
            0.1,
            Until(
                "after a click in the chat, ⌘F searches the chat",
                |d, _| find_scope(d) == Some(crate::find::Scope::Chat),
                1.,
            ),
        ),
        (0.1, Key("escape")),
    ]);
    v
}
