use super::*;

/// Kill the daemon and the holder with it, as a reboot would: the agent
/// it held is gone when the next daemon starts.
pub(super) fn kill_daemon_and_holder(_: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
    let state = crate::live::default_state();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in state.as_os_str().as_encoded_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let tmp = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let dir = std::path::Path::new(&tmp).join(format!("dent-{h:016x}"));
    // The daemon first and at once, so it can't see the holder go and act.
    let _ = std::process::Command::new("pkill")
        .args([
            "-KILL",
            "-f",
            &format!("dent-daemon --state {}", state.display()),
        ])
        .status();
    let _ = std::process::Command::new("pkill")
        .args([
            "-f",
            &format!("dent-session-holder --dir {}", dir.display()),
        ])
        .status();
}

thread_local! {
    /// What ran off the window or overlapped at the last fit check.
    pub(super) static UNFIT: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Measure the drawn controls against the window: each one inside it, and
/// the header's buttons clear of each other.
pub(super) fn check_fit(d: &mut Dent, w: &mut Window, _: &mut Context<Dent>) {
    let size = w.viewport_size();
    let (vw, vh) = (size.width.as_f32() + 0.5, size.height.as_f32() + 0.5);
    let mut bad = Vec::new();
    let names = [
        "composer",
        "send",
        "attach",
        "transcript",
        "fork",
        "side-chat",
        "terminal",
        "close",
        "context-gauge",
        "deny",
        "allow",
        "allow-session",
    ];
    let shown: Vec<(&str, Bounds<Pixels>)> = names
        .iter()
        .filter_map(|n| drawn(d, n).map(|b| (*n, b)))
        .collect();
    for (n, b) in &shown {
        if b.right().as_f32() > vw || b.bottom().as_f32() > vh || b.left().as_f32() < -0.5 {
            bad.push(format!("{n} runs off the window ({b:?} in {vw}x{vh})"));
        }
    }
    let header = ["fork", "side-chat", "terminal", "close"];
    for (i, (a, ab)) in shown.iter().enumerate() {
        for (b, bb) in &shown[i + 1..] {
            if header.contains(a) && header.contains(b) && ab.intersects(bb) {
                bad.push(format!("{a} overlaps {b}"));
            }
        }
    }
    for b in &bad {
        eprintln!("      fit: {b}");
    }
    UNFIT.with(|u| *u.borrow_mut() = bad);
}

pub(super) fn fits(_: &Dent, _: &App) -> bool {
    UNFIT.with(|u| u.borrow().is_empty())
}

/// Twenty files with long names in odd scripts, attached at once.
pub(super) fn attach_twenty(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>) {
    let paths: Vec<std::path::PathBuf> = (1..=20)
        .map(|n| {
            let p = outside(&format!(
                "notes {n:02} 日本語 📄 with an extended name that keeps going and going.txt"
            ));
            let _ = std::fs::write(&p, format!("notes {n}\n"));
            p
        })
        .collect();
    d.attach_paths(&paths, w, cx);
}

pub(super) fn session_titled<'a>(d: &'a Dent, title: &str) -> Option<&'a crate::data::Session> {
    d.sessions.iter().find(|s| s.title == title).map(|s| &**s)
}

/// How many replies in the session titled so say `needle`.
pub(super) fn replies_saying(d: &Dent, title: &str, needle: &str) -> usize {
    session_titled(d, title).map_or(0, |s| {
        s.items
            .iter()
            .filter(|i| matches!(i, Item::Text(t) if t.contains(needle)))
            .count()
    })
}

pub(super) fn down_a_page(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>) {
    d.transcript.scroll_by(w.viewport_size().height * 0.6);
    cx.notify();
}

pub(super) fn scroll_to_mine(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    let at = current(d).and_then(|s| s.items.iter().rposition(|i| matches!(i, Item::User { .. })));
    if let Some(ix) = at {
        d.transcript.scroll_to(ListOffset {
            item_ix: d.item_row(ix),
            offset_in_item: px(0.),
        });
        cx.notify();
    }
}

/// The edges: odd and huge content, many files, many sessions, switching
/// mid-stream, and the daemon restarting under a waiting approval, a queued
/// message and a take-back. Run at a narrow size too.
pub(super) fn edges() -> Vec<(f32, Act)> {
    let odd = leak(format!(
        "odd content: https://example.com/{}?q={} and {} 日本語のタイトル 👩‍👩‍👧‍👦 مرحبا بالعالم e\u{301}",
        "deep/path/".repeat(12),
        "x".repeat(60),
        "Pneumonoultramicroscopicsilicovolcanoconiosis".repeat(3)
    ));
    let mut v = new_session(odd);
    v.extend([
        (
            0.1,
            Until(
                "the odd reply finishes",
                |d, _| done_saying(d, "That's the lot."),
                30.,
            ),
        ),
        (0.1, Call(scroll_to_mine)),
        (0.6, Shot("edge-odd-mine")),
        (0.1, Call(check_fit)),
        (
            0.1,
            Until("with odd words nothing runs off the window", fits, 0.1),
        ),
        (0.1, Call(to_reply)),
        (0.6, Shot("edge-odd-1")),
        (0.1, Call(down_a_page)),
        (0.6, Shot("edge-odd-2")),
        (0.1, Call(down_a_page)),
        (0.6, Shot("edge-odd-3")),
        (0.1, Call(to_end)),
        (0.6, Shot("edge-odd-end")),
    ]);
    // Ten thousand lines in one reply: it arrives, and the window still draws.
    v.extend(ask_and_wait("ten thousand lines please"));
    v.extend([
        (
            0.1,
            Until(
                "all ten thousand lines arrive",
                |d, _| says(d, "line 10000:"),
                1.,
            ),
        ),
        (0.1, Call(to_end)),
        (0.6, Shot("edge-huge")),
        (0.1, Call(check_fit)),
        (
            0.1,
            Until("after a huge reply nothing runs off the window", fits, 0.1),
        ),
    ]);
    // Twenty files on one message: in the box, waiting, then sent.
    v.extend([
        (0.2, Type(Field::Composer, "a long answer, slowly")),
        (0.1, Key("enter")),
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.1, Call(attach_twenty)),
        (0.2, Type(Field::Composer, "with twenty files")),
        (
            0.2,
            Until(
                "twenty files sit in the box",
                |d, _| d.attachments.len() == 20,
                3.,
            ),
        ),
        (0.4, Shot("edge-files-box")),
        (0.1, Call(check_fit)),
        (
            0.1,
            Until("with twenty files the box stays in the window", fits, 0.1),
        ),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "the message waits with its twenty files",
                |d, _| queued_files(d).iter().any(|(_, f)| f.len() == 20),
                5.,
            ),
        ),
        (0.1, Call(to_end)),
        (0.6, Shot("edge-files-queued")),
        (
            0.1,
            Until(
                "then it's sent and answered",
                |d, _| queued_files(d).is_empty() && answered(d),
                40.,
            ),
        ),
        (0.1, Call(to_end)),
        (0.6, Shot("edge-files-sent")),
    ]);
    // The daemon restarts under a waiting approval: it still waits, and
    // allowing it still works.
    fn asks(d: &Dent) -> bool {
        has(d, |i| matches!(i, Item::Approval { .. })) && status_is(d, Status::Needs)
    }
    v.extend(new_session("please run it"));
    v.extend([
        (0.1, Until("an approval waits", |d, _| asks(d), 10.)),
        (0.1, Call(|d, _, _| d.drops = 0)),
        (0.1, Call(kill_daemon)),
        (0.1, Until("the daemon goes", |d, _| d.drops > 0, 5.)),
        (
            0.1,
            Until(
                "after a restart the approval still waits",
                |d, _| d.connection.is_ok() && asks(d),
                20.,
            ),
        ),
        (0.3, Click("allow")),
        (
            0.1,
            Until(
                "and allowing it then finishes the turn",
                |d, _| status_is(d, Status::Done),
                15.,
            ),
        ),
    ]);
    // A queued message survives a restart and is sent after.
    v.extend(new_session("a long answer, slowly"));
    v.extend([
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "after the restart")),
        (0.1, Key("enter")),
        (0.1, Until("the message waits", |d, _| queued_now(d), 5.)),
        (0.1, Call(|d, _, _| d.drops = 0)),
        (0.1, Call(kill_daemon_and_holder)),
        (0.1, Until("the daemon goes", |d, _| d.drops > 0, 5.)),
        (
            0.1,
            Until(
                "a message queued before a restart that lost the agent is sent once after it",
                |d, _| {
                    !queued_now(d)
                        && answered(d)
                        && current(d).is_some_and(|s| {
                            s.items
                                .iter()
                                .filter(|i| matches!(i, Item::User { text, .. } if text == "after the restart"))
                                .count()
                                == 1
                        })
                },
                40.,
            ),
        ),
        (0.1, Call(to_end)),
        (0.4, Shot("edge-queued-restart")),
    ]);
    // Stopping while the daemon is away (and the agent with it): the
    // waiting message comes back to the box once, and isn't sent.
    v.extend(new_session("a long answer, slowly"));
    v.extend([
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "take me back")),
        (0.1, Key("enter")),
        (0.1, Until("the message waits", |d, _| queued_now(d), 5.)),
        (0.1, Call(|d, _, _| d.drops = 0)),
        (0.1, Call(kill_daemon_and_holder)),
        (0.1, Until("the daemon goes", |d, _| d.drops > 0, 5.)),
        (0.1, Key("cmd-.")),
        (
            0.1,
            Until(
                "stopped across a restart that lost the agent, the waiting message comes back to the box once and isn't sent",
                |d, cx| {
                    d.connection.is_ok()
                        && !status_is(d, Status::Working)
                        && composer_is(d, cx, "take me back")
                        && !has(d, |i| matches!(i, Item::User { text, .. } if text == "take me back"))
                        && !queued_now(d)
                },
                30.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
    ]);
    // Switching back and forth while two replies stream: each ends whole.
    v.extend(new_session("first stream, a long answer slowly"));
    v.extend(new_session("second stream, a long answer slowly"));
    v.push((0.1, Until("it streams", |d, _| streaming(d), 10.)));
    for _ in 0..10 {
        v.extend([(0.3, Key("ctrl-tab")), (0.05, Call(let_go))]);
    }
    v.extend([(
        0.1,
        Until(
            "switching mid-stream ten times, both replies end whole, once each",
            |d, _| {
                [
                    "first stream, a long answer slowly",
                    "second stream, a long answer slowly",
                ]
                .iter()
                .all(|t| {
                    session_titled(d, t).is_some_and(|s| s.status == Status::Done)
                        && replies_saying(d, t, "That's the plan") == 1
                        && replies_saying(d, t, "Let me walk through") == 1
                })
            },
            40.,
        ),
    )]);
    // Thirty sessions: the list, Ctrl+Tab and search still work.
    for n in 1..=24 {
        v.extend(new_session(leak(format!("hello number {n}"))));
    }
    v.extend([
        (0.1, Until("thirty sessions", |d, _| d.sessions.len() >= 30, 10.)),
        (0.6, Shot("edge-many")),
        (0.1, Call(check_fit)),
        (0.1, Until("with thirty sessions nothing runs off the window", fits, 0.1)),
        (0.2, Key("ctrl-tab")),
        (0.05, Call(let_go)),
        (
            0.1,
            Until(
                "Ctrl+Tab among thirty goes back one",
                |d, _| titled(d, "hello number 23"),
                2.,
            ),
        ),
        (0.2, Key("cmd-k")),
        (0.3, Keys("number 7")),
        (
            0.1,
            Until(
                "⌘K finds one of thirty by its name",
                |d, cx| {
                    d.palette_results(cx)
                        .iter()
                        .any(|id| d.session(id).is_some_and(|s| s.title == "hello number 7"))
                },
                3.,
            ),
        ),
        (0.4, Shot("edge-many-search")),
        (0.1, Key("enter")),
        (
            0.1,
            Until("and Return opens it", |d, _| titled(d, "hello number 7"), 2.),
        ),
        // Every field this walk gave the keyboard (thirty new-session
        // fields, replies, search) rests: none blinks behind the window.
        (0.5, Key("escape")),
        (
            0.5,
            Idle(
                "after thirty sessions' fields had the keyboard, the window rests (no caret blinks behind it)",
                3.,
                // A stray caret draws 2 a second; the sidebar's once-a-minute
                // redraw can land in the window, so 1 a second is the line.
                1.0,
            ),
        ),
    ]);
    v
}
