use super::*;

thread_local! {
    /// Whether the sidebar showed before a scene toggled it.
    pub(super) static SIDEBAR_WAS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the open right-click menu has a row saying `label`.
pub(super) fn menu_has(d: &Dent, label: &str) -> bool {
    d.context
        .as_ref()
        .is_some_and(|m| m.items.iter().any(|(l, _)| l.as_ref() == label))
}

thread_local! {
    pub(super) static MARKED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn files_attached(d: &Dent) -> Vec<String> {
    d.attachments
        .iter()
        .filter_map(|a| match a {
            crate::app::Attachment::File { path } => {
                Some(path.rsplit('/').next().unwrap_or(path).to_string())
            }
            _ => None,
        })
        .collect()
}

/// Attaching: a session keeps what you attached while you look elsewhere,
/// the paperclip is there, a drag says where it lands, a drop on the
/// terminal types the paths, and clipped tool output opens in full.
/// A file and line named in a reply opens in the viewer at that line; one
/// that isn't there says so.
pub(super) fn file_links() -> Vec<(f32, Act)> {
    let mut v = new_session("Narrate as you go from src/lib.rs:60 (see https://example.com/notes)");
    v.extend([
        (
            0.1,
            Until("it finishes", |d, _| done_saying(d, "All done"), 20.),
        ),
        (
            0.,
            Until(
                "the file and address in your own message are links",
                |_, _| {
                    let links = crate::select::drawn_links();
                    links.iter().any(|l| l.ends_with("/src/lib.rs:60"))
                        && links.iter().any(|l| l == "https://example.com/notes")
                },
                3.,
            ),
        ),
        (0.2, Shot("own-links")),
        (
            0.,
            Until(
                "the session knows the agent's own id, to resume in a terminal",
                |d, _| {
                    current(d)
                        .and_then(|s| s.resume_command())
                        .is_some_and(|c| c.contains("claude --resume "))
                },
                3.,
            ),
        ),
        (
            0.,
            Call(|_, _, _| {
                // The file the tool row read, so its menu is whole.
                let f = in_project("src/handler.rs");
                let _ = std::fs::create_dir_all(f.parent().unwrap_or(&f));
                let _ = std::fs::write(f, "fn handle() {}\n");
            }),
        ),
        (0.2, RightClick("tool-row")),
        (
            0.1,
            Until(
                "right-clicking a file's tool row opens it, shows it in Finder or copies its path",
                |d, _| {
                    menu_has(d, "Open") && menu_has(d, "Show in Finder") && menu_has(d, "Copy Path")
                },
                3.,
            ),
        ),
        (0.3, Shot("menu-tool")),
        (0.1, Key("escape")),
        (
            0.1,
            Call(|d, w, _| {
                if let Some(s) = current(d) {
                    let area = crate::select::area(&format!("chat/{}", s.id));
                    crate::select::select_all(area, w);
                }
            }),
        ),
        (0.2, RightClick("transcript")),
        (
            0.1,
            Until(
                "with words selected, the chat's menu copies them",
                |d, _| menu_has(d, "Copy"),
                3.,
            ),
        ),
        (0.3, Shot("menu-chat")),
        (
            0.1,
            Call(|d, w, cx| {
                let quote = d.context.as_ref().and_then(|m| {
                    m.items
                        .iter()
                        .find(|(l, _)| l.as_ref() == "Quote in Reply")
                        .map(|(_, what)| what.clone())
                });
                if let Some(what) = quote {
                    d.run_context(what, w, cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "Quote in Reply puts the words in the reply box as a quote",
                |d, cx| {
                    let v = d.composer.read(cx).value().to_string();
                    v.starts_with("> Narrate as you go") && v.ends_with("\n\n")
                },
                3.,
            ),
        ),
        (0.3, Shot("quoted")),
        (
            0.,
            Call(|d, w, cx| d.composer.update(cx, |c, cx| c.set_value("", w, cx))),
        ),
        (0.2, Key("cmd-k")),
        (0.3, Keys("requests arrive")),
        (
            0.1,
            Until(
                "search finds words said inside the session",
                |d, cx| {
                    let here = d.selected.clone();
                    d.palette_results(cx).first() == here.as_ref()
                },
                3.,
            ),
        ),
        (0.2, Key("enter")),
        (
            0.1,
            Until(
                "opening it goes to where they were said, with ⌘F holding them",
                |d, cx| {
                    d.find.as_ref().is_some_and(|f| !f.hits.is_empty())
                        && d.find_input.read(cx).value() == "requests arrive"
                },
                3.,
            ),
        ),
        (0.3, Shot("search-match")),
        (0.1, Key("escape")),
        (0.2, Key("escape")),
        (0.2, Key("cmd-k")),
        (0.3, Keys("readme")),
        (
            0.1,
            Until(
                "⌘K lists the project's files that match",
                |d, cx| d.palette_files(cx) == ["README.md"],
                5.,
            ),
        ),
        (
            0.,
            Call(|d, cx_w, cx| {
                let _ = cx_w;
                d.palette = Some(d.palette_results(cx).len());
            }),
        ),
        (0.3, Shot("quick-open")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "and ↩ opens the file in the viewer",
                |d, _| {
                    current(d).is_some_and(|s| {
                        matches!(d.tab(s), crate::app::Tab::Art(a)
                            if d.art(a).is_some_and(|x| x.path.ends_with("/README.md")))
                    })
                },
                3.,
            ),
        ),
        (
            0.1,
            Call(|d, w, cx| {
                crate::select::clear(w);
                d.run_context(
                    crate::context::Do::RunInTerminal("echo from-dent".into()),
                    w,
                    cx,
                )
            }),
        ),
        (
            0.1,
            Until(
                "Run in My Terminal types the command there, not run",
                |d, _| {
                    current(d)
                        .and_then(|s| d.shells.get(&s.id))
                        .is_some_and(|sh| sh.text().contains("echo from-dent"))
                },
                5.,
            ),
        ),
        (
            0.,
            Call(|_, w, cx| {
                let src = in_project("src/lib.rs");
                let _ = std::fs::create_dir_all(src.parent().unwrap_or(&src));
                // Line n reads "// line n", so the still checks itself.
                let body: String = (2..=81).map(|n| format!("// line {n}\n")).collect();
                let _ = std::fs::write(&src, format!("fn handler() {{}}\n{body}"));
                crate::select::open_link(&format!("{}:60", src.display()), w, cx);
            }),
        ),
        (
            0.1,
            Until(
                "src/lib.rs:60 opens the file in the viewer",
                |d, _| drawn(d, "text-view").is_some(),
                3.,
            ),
        ),
        (
            0.,
            Until(
                "at line 60, marked",
                |d, _| {
                    d.marked_line.values().any(|&l| l == 60)
                        && d.text_list.logical_scroll_top().item_ix > 40
                },
                3.,
            ),
        ),
        (0.3, Shot("file-at-line")),
        (
            0.,
            Call(|_, w, cx| {
                let gone = in_project("src/gone.rs");
                crate::select::open_link(&format!("{}:9", gone.display()), w, cx);
            }),
        ),
        (
            0.1,
            Until(
                "a file that isn't there says so",
                |d, _| {
                    d.toast
                        .as_ref()
                        .is_some_and(|t| t.text.starts_with("Can't find"))
                },
                3.,
            ),
        ),
        (0.3, Shot("file-missing")),
        // Leave nothing over the next scene.
        (
            0.,
            Call(|d, _, cx| {
                d.toast = None;
                cx.notify();
            }),
        ),
    ]);
    v
}

pub(super) fn attaching() -> Vec<(f32, Act)> {
    let mut v = new_session("Narrate as you go");
    v.extend([
        (
            0.1,
            Until("it finishes", |d, _| done_saying(d, "All done"), 20.),
        ),
        (
            0.,
            Until(
                "the paperclip is in the reply box",
                |d, _| drawn(d, "attach").is_some(),
                3.,
            ),
        ),
        (0.1, Call(attach_red)),
        (
            0.,
            Call(|d, _, _| MARKED.with(|m| *m.borrow_mut() = d.selected.clone())),
        ),
        (0.3, Key("cmd-n")),
        (
            0.1,
            Until(
                "a new session starts with nothing attached",
                |d, _| d.selected.is_none() && d.attachments.is_empty(),
                3.,
            ),
        ),
        (
            0.,
            Until(
                "and has its own paperclip",
                |d, _| drawn(d, "attach").is_some(),
                3.,
            ),
        ),
        (
            0.2,
            Call(|d, w, cx| {
                let id = MARKED.with(|m| m.borrow().clone());
                d.select(id, w, cx);
            }),
        ),
        (
            0.1,
            Until(
                "back in the session, its attachment is still there",
                |d, _| files_attached(d) == ["red.png"],
                3.,
            ),
        ),
        (0.2, DragFiles("composer", 0.5, 0.5)),
        (
            0.1,
            Until(
                "dragging files over the window says where they'd go",
                |d, cx| {
                    cx.has_active_drag()
                        && drawn(d, "drop-message").is_some()
                        && drawn(d, "drop-shell").is_none()
                },
                3.,
            ),
        ),
        (0.2, Shot("drop-hint")),
        (0.1, DropFiles),
        (
            0.1,
            Until(
                "dropped, they join the message once each",
                |d, cx| !cx.has_active_drag() && files_attached(d) == ["red.png", "my notes.txt"],
                3.,
            ),
        ),
        (0.3, Click("terminal")),
        (
            0.1,
            Until(
                "the terminal opens",
                |d, _| {
                    current(d).is_some_and(|s| d.shells.contains_key(&s.id))
                        && drawn(d, "shell").is_some()
                },
                10.,
            ),
        ),
        (1.0, DragFiles("shell", 0.5, 0.5)),
        (
            0.1,
            Until(
                "over the terminal, only the terminal says so",
                |d, _| drawn(d, "drop-shell").is_some() && drawn(d, "drop-message").is_none(),
                3.,
            ),
        ),
        (0.3, Shot("drop-terminal")),
        (0.2, DropFiles),
        (
            0.1,
            Until(
                "dropped on the terminal, the paths are typed there, quoted",
                |d, _| {
                    current(d)
                        .and_then(|s| d.shells.get(&s.id))
                        .is_some_and(|sh| sh.text().replace('\n', "").contains("my notes.txt' "))
                        && files_attached(d).len() == 2
                },
                5.,
            ),
        ),
        // The terminal has the keyboard after the drop: ⌘K clears it there.
        (0.3, Key("cmd-k")),
        (
            0.1,
            Until(
                "⌘K in your terminal clears what came before, and search stays shut",
                // What's typed and not yet run stays, as in Terminal.
                |d, _| {
                    d.palette.is_none()
                        && current(d)
                            .and_then(|s| d.shells.get(&s.id))
                            .is_some_and(|sh| !sh.text().contains("Last login"))
                },
                5.,
            ),
        ),
        (
            0.,
            Call(|d, _, cx| {
                // Open the Read call, to reach its clipped output.
                let id = current(d).and_then(|s| {
                    s.items.iter().find_map(|i| match i {
                        Item::Tools { lines, .. }
                            if lines.len() == 1 && lines[0].text.contains("handler.rs") =>
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
                "clipped output says how much more",
                |d, _| drawn(d, "call-out-all").is_some(),
                3.,
            ),
        ),
        (0.2, Shot("output-folded")),
        (0.1, Click("call-out-all")),
        (0.5, Shot("output-all")),
        (
            0.2,
            Call(|d, w, cx| d.composer.update(cx, |c, cx| c.focus_field(w, cx))),
        ),
        (0.2, Key("home")),
        (
            0.1,
            Until(
                "Home goes to the start of the chat",
                |d, _| {
                    let top = d.transcript.logical_scroll_top();
                    top.item_ix == 0 && top.offset_in_item == px(0.)
                },
                3.,
            ),
        ),
        (0.2, Key("pagedown")),
        (
            0.1,
            Until(
                "Page Down moves down a page",
                |d, _| {
                    let top = d.transcript.logical_scroll_top();
                    top.item_ix > 0 || top.offset_in_item > px(200.)
                },
                3.,
            ),
        ),
        (0.2, Key("end")),
        (
            0.1,
            Until(
                "End goes back to the newest",
                |d, _| {
                    let n = current(d).map_or(0, |s| s.items.len());
                    n > 0 && d.transcript.bounds_for_item(n - 1).is_some()
                },
                3.,
            ),
        ),
        (
            0.2,
            Call(|d, _, _| SIDEBAR_WAS.with(|w| w.set(d.sidebar_hidden))),
        ),
        (0.2, Key("ctrl-cmd-s")),
        (
            0.1,
            Until(
                "⌃⌘S shows or hides the sidebar",
                |d, _| d.sidebar_hidden != SIDEBAR_WAS.with(|w| w.get()),
                3.,
            ),
        ),
        (0.3, Shot("sidebar-toggled")),
        (0.2, Key("ctrl-cmd-s")),
        (
            0.1,
            Until(
                "and again puts it back",
                |d, _| d.sidebar_hidden == SIDEBAR_WAS.with(|w| w.get()),
                3.,
            ),
        ),
        (0.2, Click("pic-chip")),
        (
            0.1,
            Until(
                "a picture opened large offers Copy and Open in Preview",
                |d, _| d.lightbox.is_some() && drawn(d, "lightbox-copy").is_some(),
                3.,
            ),
        ),
        (0.4, Shot("picture-large")),
        (0.1, Click("lightbox-copy")),
        (
            0.1,
            Until(
                "Copy puts the picture on the clipboard",
                |_, cx| {
                    cx.read_from_clipboard().is_some_and(|c| {
                        c.entries()
                            .iter()
                            .any(|e| matches!(e, ClipboardEntry::Image(_)))
                    })
                },
                3.,
            ),
        ),
        (0.2, Key("escape")),
        (
            0.2,
            Until(
                "and a press shows all of it",
                |d, _| d.expanded.iter().any(|k| k.starts_with("call-out-all-")),
                3.,
            ),
        ),
    ]);
    v
}
