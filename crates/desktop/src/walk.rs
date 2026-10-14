//! Scripted walkthroughs of the real app against a daemon running the fake
//! agent. Clicks and keys go through the window's normal event dispatch
//! (real hit-testing, real focus), checks read the state, and stills capture
//! only the window.
//!
//! `scripts/walk.sh checks|stills` sets up a scratch home, state and project
//! and runs `dent-desktop --walk NAME`. Each check prints PASS or FAIL.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::{Dent, Tab};
use crate::data::Item;
use crate::theme::Status;

type Pred = fn(&Dent, &App) -> bool;
type Call = fn(&mut Dent, &mut Window, &mut Context<Dent>);

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq)]
pub enum Field {
    Composer,
    Starter,
    Comment,
}

/// The walkthrough's vocabulary; not every scene uses every step.
#[allow(dead_code)]
#[derive(Clone)]
pub enum Act {
    Wait,
    Click(&'static str),
    /// Click it if it's on screen; some controls exist only on some platforms.
    ClickIfShown(&'static str),
    Key(&'static str),
    Type(Field, &'static str),
    /// Type into the focused field key by key, through normal input.
    Keys(&'static str),
    Page(&'static str),
    Call(Call),
    /// Passes when the predicate holds within the timeout (seconds).
    Until(&'static str, Pred, f32),
    /// Sit still for `secs` without asking for frames, then pass if the
    /// window drew at most `max_fps` frames a second meanwhile.
    Idle(&'static str, f32, f32),
    Shot(&'static str),
}

pub struct Walk {
    name: String,
    steps: Vec<(f32, Act)>,
    ix: usize,
    due: Instant,
    waiting_since: Option<Instant>,
    results: Vec<(&'static str, bool)>,
    shots: Option<PathBuf>,
    hold: f32,
    /// While idling: when it ends, and the frame count it started from.
    pub resting: Option<(Instant, u64, f32, &'static str, f32)>,
}

use Act::*;

impl Walk {
    pub fn new(name: &str, shots: Option<PathBuf>) -> Self {
        let steps = match name {
            "stills" => stills(),
            "real" => real(),
            _ => checks(),
        };
        Self {
            name: name.into(),
            steps,
            ix: 0,
            due: Instant::now() + Duration::from_millis(1500),
            waiting_since: None,
            results: Vec::new(),
            shots,
            hold: 0.3,
            resting: None,
        }
    }
}

pub fn tick(me: Entity<Dent>, window: &mut Window, cx: &mut App) {
    // Finish an idle stretch: how many frames did the window draw?
    let rested = me.update(cx, |this, _| {
        let frames = this.frames;
        let d = this.walk.as_mut()?;
        let (until, start, secs, name, max) = d.resting?;
        if Instant::now() < until {
            return Some(None);
        }
        d.resting = None;
        // The frame that ends the rest doesn't count.
        let fps = (frames.saturating_sub(start + 1)) as f32 / secs;
        Some(Some((name, fps, max)))
    });
    match rested {
        Some(None) => return,
        Some(Some((name, fps, max))) => {
            eprintln!("      {name}: {fps:.1} frames a second");
            note(&me, cx, name, fps <= max);
        }
        None => {}
    }
    let step = me.update(cx, |this, _| {
        let d = this.walk.as_mut()?;
        if Instant::now() < d.due {
            return None;
        }
        Some(d.steps.get(d.ix).cloned().map(|(_, a)| a))
    });
    let Some(step) = step else { return };
    let Some(act) = step else {
        // Done: report and quit.
        let (name, results, hold) = me.update(cx, |this, _| {
            let d = this.walk.take().expect("walk");
            (d.name, d.results, d.hold)
        });
        let failed = results.iter().filter(|(_, ok)| !ok).count();
        for (name, ok) in &results {
            println!("{} {name}", if *ok { "PASS" } else { "FAIL" });
        }
        println!("{name}: {} checks, {failed} failed", results.len());
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs_f32(hold))
                .await;
            cx.update(|cx| cx.quit());
        })
        .detach();
        return;
    };
    let mut advance = true;
    match act {
        Wait => {}
        Click(name) | ClickIfShown(name) => {
            let optional = matches!(act, ClickIfShown(_));
            if let Some(at) = center(&me, name, cx) {
                // Arrive first, like a pointer would, so hover follows the click.
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                mouse(window, cx, at, true);
                mouse(window, cx, at, false);
            } else if !optional {
                note(
                    &me,
                    cx,
                    leak(format!("click target “{name}” on screen")),
                    false,
                );
            }
        }
        Key(k) => {
            if let Ok(ks) = Keystroke::parse(k) {
                window.dispatch_keystroke(ks, cx);
            }
        }
        Type(field, text) => {
            me.update(cx, |this, cx| {
                let input = match field {
                    Field::Composer => this.composer.clone(),
                    Field::Starter => this.starter.clone(),
                    Field::Comment => this.comment_input.clone(),
                };
                input.update(cx, |i, cx| {
                    i.set_value(text, window, cx);
                    i.focus(window, cx);
                });
                cx.notify();
            });
        }
        Keys(text) => {
            for ch in text.chars() {
                let key = if ch == ' ' {
                    "space".to_string()
                } else {
                    ch.to_lowercase().to_string()
                };
                let ks = Keystroke {
                    modifiers: Modifiers {
                        shift: ch.is_uppercase(),
                        ..Default::default()
                    },
                    key,
                    key_char: Some(ch.to_string()),
                };
                window.dispatch_keystroke(ks, cx);
            }
        }
        Page(js) => {
            me.update(cx, |this, _| {
                if let Some(p) = &this.preview {
                    p.borrow().script(js);
                }
            });
        }
        Call(f) => me.update(cx, |this, cx| f(this, window, cx)),
        Idle(name, secs, max) => me.update(cx, |this, _| {
            let frames = this.frames;
            if let Some(d) = this.walk.as_mut() {
                d.resting = Some((
                    Instant::now() + Duration::from_secs_f32(secs),
                    frames,
                    secs,
                    name,
                    max,
                ));
            }
        }),
        Until(name, pred, timeout) => {
            let ok = pred(me.read(cx), cx);
            let since = me.update(cx, |this, _| {
                *this
                    .walk
                    .as_mut()
                    .expect("walk")
                    .waiting_since
                    .get_or_insert_with(Instant::now)
            });
            if ok || since.elapsed().as_secs_f32() > timeout {
                note(&me, cx, name, ok);
                me.update(cx, |this, _| {
                    this.walk.as_mut().expect("walk").waiting_since = None
                });
            } else {
                advance = false;
            }
        }
        Shot(name) => {
            let dir = me.read(cx).walk.as_ref().and_then(|d| d.shots.clone());
            if let Some(dir) = dir {
                let _ = std::fs::create_dir_all(&dir);
                still(dir.join(format!("{name}.png")));
            }
        }
    }
    if advance {
        me.update(cx, |this, _| {
            let d = this.walk.as_mut().expect("walk");
            d.ix += 1;
            let wait = d.steps.get(d.ix).map(|(w, _)| *w).unwrap_or(0.);
            d.due = Instant::now() + Duration::from_secs_f32(wait);
        });
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn note(me: &Entity<Dent>, cx: &mut App, name: &'static str, ok: bool) {
    me.update(cx, |this, _| {
        if let Some(d) = this.walk.as_mut() {
            d.results.push((name, ok));
        }
    });
    eprintln!("{} {name}", if ok { "pass" } else { "FAIL" });
}

fn center(me: &Entity<Dent>, name: &str, cx: &App) -> Option<Point<Pixels>> {
    // Only what the latest frame drew.
    let d = me.read(cx);
    let fresh = crate::ui::PROBE_FRAME
        .with(|f| f.borrow().get(name).copied())
        .is_some_and(|at| at + 1 >= d.frames);
    if !fresh {
        return None;
    }
    d.probes.borrow().get(name).map(|b| b.center())
}

fn mouse(window: &mut Window, cx: &mut App, at: Point<Pixels>, down: bool) {
    let ev = if down {
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        })
    } else {
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
        })
    };
    window.dispatch_event(ev, cx);
}

// ---- Helpers for checks ----

fn current(d: &Dent) -> Option<&crate::data::Session> {
    d.current()
}

fn status_is(d: &Dent, s: Status) -> bool {
    current(d).is_some_and(|x| x.status == s)
}

fn has(d: &Dent, f: fn(&Item) -> bool) -> bool {
    current(d).is_some_and(|s| s.items.iter().any(f))
}

fn says(d: &Dent, needle: &str) -> bool {
    current(d).is_some_and(|s| {
        s.items
            .iter()
            .any(|i| matches!(i, Item::Text(t) if t.contains(needle)))
    })
}

fn done_saying(d: &Dent, needle: &str) -> bool {
    status_is(d, Status::Done) && says(d, needle)
}

fn streaming(d: &Dent) -> bool {
    status_is(d, Status::Working)
        && current(d).is_some_and(|s| matches!(s.items.last(), Some(Item::Text(t)) if t.len() > 40))
}

/// The scratch project the walk script made.
fn use_project(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    d.defaults.project = std::env::var("DENT_WALK_PROJECT").ok();
    d.defaults.project_picked = true;
    cx.notify();
}

/// A small red picture in the project, attached as if dropped on the window.
fn attach_red(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>) {
    let dir = std::env::var("DENT_WALK_PROJECT").unwrap_or_else(|_| "/tmp".into());
    let path = std::path::Path::new(&dir).join("red.png");
    let img = image::RgbaImage::from_pixel(48, 48, image::Rgba([220, 30, 30, 255]));
    let _ = img.save(&path);
    d.attach_paths(&[path], w, cx);
}

/// The last message you sent in the open session.
fn last_sent(d: &Dent) -> Option<String> {
    current(d)?.items.iter().rev().find_map(|i| match i {
        Item::User { id, .. } => Some(id.clone()),
        _ => None,
    })
}

fn in_project(name: &str) -> std::path::PathBuf {
    std::path::Path::new(&std::env::var("DENT_WALK_PROJECT").unwrap_or_default()).join(name)
}

/// Kill the daemon, as a crash or an upgrade would.
fn kill_daemon(_: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
    let state = crate::live::default_state();
    let _ = std::process::Command::new("pkill")
        .args(["-f", &format!("dent-daemon --state {}", state.display())])
        .status();
}

fn new_session(text: &'static str) -> Vec<(f32, Act)> {
    vec![
        (0.3, Key("cmd-n")),
        (0.2, Call(use_project)),
        (0.2, Type(Field::Starter, text)),
        (0.2, Click("start")),
        (
            0.1,
            Until(
                "a new session opens",
                |d, _| current(d).is_some() && !d.creating,
                5.,
            ),
        ),
    ]
}

// ---- Scenes ----

/// The edge cases, against the real daemon and the fake agent.
fn checks() -> Vec<(f32, Act)> {
    let mut v: Vec<(f32, Act)> = vec![(
        0.,
        Until(
            "connects to the daemon, starting it",
            |d, _| d.connection.is_ok(),
            15.,
        ),
    )];
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
                |d, cx| d.slash_matches(cx).iter().any(|c| c.name == "compact"),
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
        (
            0.1,
            Call(|d, w, cx| {
                d.renaming = d.selected.clone();
                d.rename_input.update(cx, |i, cx| {
                    i.set_value("Greeting test", w, cx);
                    i.focus(w, cx);
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
                "allowing runs it and the turn finishes",
                |d, _| {
                    done_saying(d, "Ran it")
                        && has(
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
                        && !has(d, |i| matches!(i, Item::Note { icon: "square", .. }))
                },
                20.,
            ),
        ),
        (0.3, Type(Field::Composer, "a long answer please")),
        (0.1, Key("enter")),
        (0.1, Until("streams a third time", |d, _| streaming(d), 10.)),
        (0.1, Key("cmd-.")),
        (
            0.1,
            Until(
                "⌘. stops it within three seconds",
                |d, _| status_is(d, Status::Idle),
                3.,
            ),
        ),
        (0.3, Shot("stopped")),
        (0.2, Type(Field::Composer, "")),
        (0.2, Key("alt-up")),
        (
            0.1,
            Until(
                "Alt+Up jumps back through your own messages",
                |d, _| {
                    let top = d.transcript.logical_scroll_top().item_ix;
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
        (1.0, Shot("viewer-page")),
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
        (0.1, Call(attach_red)),
        (0.3, Shot("attachment")),
        (0.1, Type(Field::Composer, "what is in this picture")),
        (0.1, Key("enter")),
        (0.1, Until("a dropped picture goes with the message", |d, _| has(d, |i| matches!(i, Item::User { text: t, .. } if t.contains("Attached") && t.contains("red.png"))) && status_is(d, Status::Done), 10.)),
    ]);
    v.extend(new_session("make a plan please"));
    v.extend([
        (
            0.1,
            Until(
                "the agent's plan shows above the composer",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.todos.len() == 3 && s.todos.iter().any(|(st, ..)| st == "in_progress")
                    })
                },
                10.,
            ),
        ),
        (
            0.2,
            Call(|d, _, cx| {
                d.plan_open = true;
                cx.notify();
            }),
        ),
        (0.4, Shot("plan")),
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
                10.,
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
            Idle("with nothing moving, the window draws nothing", 3., 0.5),
        ),
    ]);
    v
}

/// A short session against the real agent on its smallest model: tiny
/// prompts, so it costs next to nothing. Lenient about wording, strict
/// about the flow.
fn real() -> Vec<(f32, Act)> {
    fn haiku(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
        d.defaults.model = Some("haiku".into());
        d.defaults.permission = dent_protocol::Permission::Ask;
        cx.notify();
    }
    fn done(d: &Dent) -> bool {
        status_is(d, Status::Done)
    }
    let mut v: Vec<(f32, Act)> = vec![(
        0.,
        Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
    )];
    v.push((0.2, Call(haiku)));
    v.extend(new_session("Reply with just the word ok."));
    v.extend([
        (0.1, Until("Haiku replies and the turn ends", |d, _| done(d) && says(d, "ok"), 60.)),
        (0.5, Shot("real-reply")),
        (0.2, Type(Field::Composer, "Use the Bash tool to run exactly: echo hi > hi.txt  Then reply with just: done")),
        (0.2, Key("enter")),
        (0.1, Until("it asks before running a command", |d, _| has(d, |i| matches!(i, Item::Approval { .. })), 60.)),
        (0.5, Shot("real-approval")),
        (0.2, Click("allow")),
        (0.1, Until("allowed, it runs and finishes", |d, _| done(d) && says(d, "done"), 60.)),
        (0.2, Type(Field::Composer, "Use the AskUserQuestion tool to ask me tea or coffee, with two options. Then reply with just my answer.")),
        (0.2, Key("enter")),
        (0.1, Until("it asks a question with choices", |d, _| has(d, |i| matches!(i, Item::Question { .. })), 60.)),
        (0.5, Shot("real-question")),
        (0.2, Click("opt-0-0")),
        (0.1, Until("the answer reaches it", |d, _| done(d), 60.)),
        (0.2, Type(Field::Composer, "Use the Write tool to create hello.html: a minimal page with an h1 saying Hello from Haiku. Then reply with just: written")),
        (0.2, Key("enter")),
        (0.1, Until("it asks before writing a file", |d, _| has(d, |i| matches!(i, Item::Approval { .. })), 60.)),
        (0.2, Click("allow-session")),
        (0.1, Until("the page opens in the viewer", |d, _| done(d) && has(d, |i| matches!(i, Item::Made { .. })), 60.)),
        (1.2, Shot("real-page")),
        (0.2, Call(attach_red)),
        (0.2, Type(Field::Composer, "What colour is this image? Reply with one word.")),
        (0.2, Key("enter")),
        (0.1, Until("it sees the attached picture", |d, _| done(d) && says(d, "ed"), 60.)),
        (0.5, Shot("real-image")),
        // Back to before the picture: the agent shouldn't know it saw one.
        (0.2, Call(|d, w, cx| {
            if let Some(m) = last_sent(d) {
                d.rewind(&m, false, w, cx);
            }
        })),
        (0.1, Until("the chat goes back", |d, _| !has(d, |i| matches!(i, Item::User { text, .. } if text.contains("red.png"))), 10.)),
        (0.2, Type(Field::Composer, "Did I send you an image in this conversation? Reply yes or no.")),
        (0.2, Key("enter")),
        (0.1, Until("after rewinding, it has no memory of the picture", |d, _| done(d) && current(d).is_some_and(|s| matches!(s.items.iter().rev().find(|i| matches!(i, Item::Text(_))), Some(Item::Text(t)) if t.to_lowercase().contains("no"))), 90.)),
        (0.5, Shot("real-rewound")),
        (0.3, Key("cmd-n")),
        (0.6, Shot("real-new")),
    ]);
    v
}

/// Still frames in the current theme and size, after the checks' sessions.
fn stills() -> Vec<(f32, Act)> {
    let mut v = checks();
    v.retain(|(_, a)| !matches!(a, Idle(..)));
    v.extend([
        (0.3, Key("cmd-n")),
        (0.3, Keys("Fix the footer on small screens")),
        (0.8, Shot("new-session")),
        (0.3, Key("escape")),
        (0.2, Key("cmd-k")),
        (0.3, Keys("long")),
        (0.6, Shot("search")),
        (0.2, Key("escape")),
    ]);
    v
}

/// The window only, never what's around it: by window number when the
/// `WINID` helper (tools/winid.swift) is given, else the window's rectangle.
pub fn still(path: PathBuf) {
    // On Linux: dent's own window, found by process, never the whole screen.
    #[cfg(target_os = "linux")]
    {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            let id = std::process::Command::new("xdotool")
                .args([
                    "search",
                    "--onlyvisible",
                    "--pid",
                    &std::process::id().to_string(),
                ])
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .and_then(|s| s.lines().last().map(str::to_string));
            let Some(id) = id else {
                eprintln!("still: no window to capture for {}", path.display());
                return;
            };
            let _ = std::process::Command::new("import")
                .args(["-window", &id])
                .arg(&path)
                .status();
        });
        return;
    }
    #[allow(unreachable_code)]
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        let id = std::env::var("WINID").ok().and_then(|tool| {
            let out = std::process::Command::new(tool)
                .arg(std::process::id().to_string())
                .output()
                .ok()?;
            Some(String::from_utf8(out.stdout).ok()?.trim().to_string()).filter(|s| !s.is_empty())
        });
        // Only ever the window itself: without its number, no still.
        let Some(id) = id else {
            eprintln!("still: set WINID to capture {}", path.display());
            return;
        };
        let _ = std::process::Command::new("screencapture")
            .args(["-x", "-o", &format!("-l{id}")])
            .arg(&path)
            .status();
    });
}
