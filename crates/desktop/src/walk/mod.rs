//! Scripted walkthroughs of the real app against a daemon running the fake
//! agent. Clicks and keys go through the window's normal event dispatch
//! (real hit-testing, real focus), checks read the state, and stills capture
//! only the window.
//!
//! `scripts/walk.sh NAME` sets up a scratch home, state and project and runs
//! `dent-desktop --walk NAME`. Each check prints PASS or FAIL. This file is
//! the runner and the scene table ([`SCENES`]); each scene has its own file,
//! and helpers more than one scene uses live in `common.rs`.

use crate::ui::FocusField;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::{Dent, Tab};
use crate::data::Item;
use crate::theme::Status;

mod ambient;
mod attaching;
mod bench;
mod changes;
mod checks;
mod common;
mod composing;
mod demo;
mod edges;
mod failures;
mod find;
mod finish;
mod keys;
mod look;
mod markdown;
mod motion;
mod narration;
mod parity;
mod pickup;
mod pins;
mod polish;
mod prs;
mod queue;
mod real;
mod recall;
mod reply;
mod setup;
mod stills;
mod sweep;
mod switching;
mod takeback;
mod typing;
mod view;
mod visuals;
mod worktrees;
mod zoom;
use {
    ambient::*, attaching::*, bench::*, changes::*, checks::*, common::*, composing::*, demo::*,
    edges::*, failures::*, find::*, finish::*, keys::*, look::*, markdown::*, motion::*,
    narration::*, parity::*, pickup::*, pins::*, polish::*, prs::*, queue::*, real::*, recall::*,
    reply::*, setup::*, stills::*, sweep::*, switching::*, takeback::*, typing::*, view::*,
    visuals::*, worktrees::*, zoom::*,
};

type Pred = fn(&Dent, &App) -> bool;

type Call = fn(&mut Dent, &mut Window, &mut Context<Dent>);

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq)]
pub enum Field {
    Composer,
    Starter,
    Comment,
    Aside,
    /// The base branch menu's search.
    Branches,
}

/// The walkthrough's vocabulary; not every scene uses every step.
#[allow(dead_code)]
#[derive(Clone)]
pub enum Act {
    Wait,
    /// A key as the Mac sends it while held down: a repeat.
    Held(&'static str),
    Click(&'static str),
    /// Right-click it: a press and release of the other button.
    RightClick(&'static str),
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
    /// A trackpad swipe over a probe: one movement of `dy` points (negative
    /// moves on through the content), then the gesture's zero-movement end.
    Wheel(&'static str, f32),
    /// A trackpad swipe over a probe that isn't straight: `dx`, `dy` points
    /// spread over ten movements, then the gesture's zero-movement end.
    Swipe(&'static str, f32, f32),
    /// Open every session in turn, timing each, and report.
    Sweep,
    /// Press the mouse at a point in a probe, given as fractions of its size.
    Press(&'static str, f32, f32),
    /// Move the pressed mouse by this much, in a few steps.
    MoveBy(f32, f32),
    /// Let go of the mouse where it is.
    Release,
    /// Move the pointer over a probe, offset by this much, and rest there.
    Hover(&'static str, f32, f32),
    /// Rest the pointer on the first link drawn to an address starting so.
    HoverLink(&'static str),
    /// Double-click a probe at a point given as fractions of its size.
    DoubleClick(&'static str, f32, f32),
    /// Pinch over a probe's middle until it's scaled by this much, in a
    /// few movements, then lift the fingers.
    Pinch(&'static str, f32),
    /// ⌘-scroll a trackpad over a probe's middle by this many points, in
    /// a few movements, then lift the fingers.
    ZoomScroll(&'static str, f32),
    /// Files from Finder dragged in and held over a probe, at fractions of
    /// its size: the project's red.png and "my notes.txt".
    DragFiles(&'static str, f32, f32),
    /// Let go of the dragged files where they are. (Without the platform's
    /// drop event, which would bring the window to the front.)
    DropFiles,
}

thread_local! {
    static POINTER: std::cell::Cell<Point<Pixels>> = const { std::cell::Cell::new(point(px(0.), px(0.))) };
    static CLIPBOARD: std::cell::RefCell<Option<ClipboardItem>> = const { std::cell::RefCell::new(None) };
}

/// The walk copies text; your clipboard goes back the way it was after.
pub fn save_clipboard(_: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    CLIPBOARD.with(|c| *c.borrow_mut() = cx.read_from_clipboard());
}

pub fn restore_clipboard(_: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    if let Some(item) = CLIPBOARD.with(|c| c.borrow_mut().take()) {
        cx.write_to_clipboard(item);
    }
}

fn copied(cx: &App) -> String {
    cx.read_from_clipboard()
        .and_then(|c| c.text())
        .unwrap_or_default()
}

/// Progress through [`Act::Sweep`]: a queue of small steps, one a frame,
/// so each runs after the last one has been drawn.
struct Sweep {
    todo: std::collections::VecDeque<SweepStep>,
    /// The session opened last, and when.
    opened: Option<(String, Instant)>,
    /// (session, ms from selecting to drawn, items).
    times: Vec<(String, f64, usize)>,
}

enum SweepStep {
    Open(String),
    /// Note how long the last session took to draw.
    Timed,
    /// Scroll the conversation to its first item that matches.
    ScrollTo(fn(&Item) -> bool),
    Still(String),
    /// Open every group that holds a subagent.
    OpenAgents,
    Frame,
    /// Stills are taken off the main thread: give one time to finish.
    Until(Instant),
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
    sweep: Option<Sweep>,
}

use Act::*;

type Scene = fn() -> Vec<(f32, Act)>;

/// Every scene `scripts/walk.sh NAME` runs, after waiting for the daemon.
/// Join names with `+` to run several in one window (`find+keys`), so a
/// change walks only the scenes it touched. `checks` is the full suite.
const SCENES: &[(&str, &[Scene])] = &[
    ("look", &[look]),
    ("typing", &[typing]),
    ("finish", &[finish]),
    ("pictures", &[pictures]),
    ("polish", &[polish]),
    ("looks", &[answer_keys, reply_actions, composing, breath]),
    ("breath", &[breath]),
    ("ambient", &[ambient]),
    ("find", &[find]),
    ("findview", &[find_viewer]),
    ("links", &[file_links]),
    ("pickup", &[pickup]),
    ("attach", &[attaching]),
    ("attaching", &[attaching]),
    ("narrate", &[narration]),
    ("keys", &[answer_keys]),
    ("reply", &[reply_actions]),
    ("switch", &[switching]),
    ("view", &[view_choices]),
    ("pins", &[pins]),
    ("fail", &[failures]),
    ("queue", &[queue_order]),
    ("takeback", &[take_back]),
    ("changes", &[turn_changes, diff_swipe]),
    ("recall", &[recall]),
    ("visuals", &[visuals]),
    ("bench", &[bench]),
    ("rows", &[rows]),
    ("crash-mid-reply", &[crash_mid_reply]),
    ("fold", &[fold]),
    ("pages", &[pages]),
    ("look-hidden", &[look_hidden]),
    ("pages-fallback", &[pages_fallback]),
    ("composing", &[composing]),
    ("edges", &[edges]),
    ("parity", &[parity]),
    ("zoom-links", &[zoom_links]),
    ("worktrees", &[worktrees]),
    ("worktree-edges", &[worktree_edges]),
    ("prs", &[prs]),
    ("look-pr", &[look_pr]),
    ("project-defaults", &[project_defaults]),
    ("scripts", &[scripts]),
    ("settings", &[settings]),
    ("machines", &[machines]),
    ("machines-live", &[machines_live]),
    ("bring-live", &[bring_live]),
    ("kept-live", &[kept_live]),
    ("files-live", &[files_live]),
    ("break-live", &[break_live]),
    ("remote-happy-live", &[remote_happy_live]),
    ("resync-live", &[resync_live]),
    ("many-live", &[many_live]),
    ("sleep-live", &[sleep_live]),
    ("stress-live", &[stress_live]),
    ("away", &[away]),
    ("welcome", &[welcome]),
    // These three need the agent set up so (walk.sh does it by name).
    ("signed-out", &[signed_out]),
    ("missing", &[missing]),
    ("limited", &[limited]),
];

/// Scenes that start on their own: no daemon, the real agent, or a
/// connect step of their own.
const OWN_START: &[(&str, Scene)] = &[
    ("checks", checks),
    ("stills", stills),
    ("real", real),
    ("real-pages", real_pages),
    ("compare", compare),
    ("sweep", sweep),
    ("markdown", markdown),
    ("motion", motion),
];

fn scene(name: &str) -> Vec<(f32, Act)> {
    if let Some((_, f)) = OWN_START.iter().find(|(n, _)| *n == name) {
        return f();
    }
    let mut v = vec![(
        0.,
        Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
    )];
    if let Some(id) = name.strip_prefix("demo-") {
        let id = leak(id.to_string());
        *DEMO.lock().unwrap_or_else(|e| e.into_inner()) = id;
        v.extend(demo(id));
        return v;
    }
    for part in name.split('+') {
        match SCENES.iter().find(|(n, _)| *n == part) {
            Some((_, fs)) => fs.iter().for_each(|f| v.extend(f())),
            // A typo used to run the whole suite; now it says so.
            None => {
                let known: Vec<_> = OWN_START
                    .iter()
                    .map(|(n, _)| *n)
                    .chain(SCENES.iter().map(|(n, _)| *n))
                    .collect();
                eprintln!(
                    "walk: no scene called {part:?}; known: {}, demo-<id>",
                    known.join(", ")
                );
                return vec![(0., Until("the scene exists", |_, _| false, 0.))];
            }
        }
    }
    v
}

impl Walk {
    pub fn new(name: &str, shots: Option<PathBuf>) -> Self {
        let steps = scene(name);
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
            sweep: None,
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
        RightClick(name) => match center(&me, name, cx) {
            Some(at) => {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                for down in [true, false] {
                    let ev = if down {
                        PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Right,
                            position: at,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                            first_mouse: false,
                        })
                    } else {
                        PlatformInput::MouseUp(MouseUpEvent {
                            button: MouseButton::Right,
                            position: at,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        })
                    };
                    window.dispatch_event(ev, cx);
                }
            }
            None => note(
                &me,
                cx,
                leak(format!("right-click target “{name}” on screen")),
                false,
            ),
        },
        Wheel(name, dy) => match center(&me, name, cx) {
            Some(at) => {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                for (d, phase) in [(dy, TouchPhase::Started), (0., TouchPhase::Ended)] {
                    window.dispatch_event(
                        PlatformInput::ScrollWheel(ScrollWheelEvent {
                            position: at,
                            delta: ScrollDelta::Pixels(point(px(0.), px(d))),
                            modifiers: Modifiers::default(),
                            touch_phase: phase,
                        }),
                        cx,
                    );
                }
            }
            None => note(
                &me,
                cx,
                leak(format!("wheel target “{name}” on screen")),
                false,
            ),
        },
        Swipe(name, dx, dy) => match center(&me, name, cx) {
            Some(at) => {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                let moves = std::iter::once((dx / 10., dy / 10., TouchPhase::Started))
                    .chain(std::iter::repeat_n(
                        (dx / 10., dy / 10., TouchPhase::Moved),
                        9,
                    ))
                    .chain([(0., 0., TouchPhase::Ended)]);
                for (x, y, phase) in moves {
                    window.dispatch_event(
                        PlatformInput::ScrollWheel(ScrollWheelEvent {
                            position: at,
                            delta: ScrollDelta::Pixels(point(px(x), px(y))),
                            modifiers: Modifiers::default(),
                            touch_phase: phase,
                        }),
                        cx,
                    );
                }
            }
            None => note(
                &me,
                cx,
                leak(format!("swipe target “{name}” on screen")),
                false,
            ),
        },
        Key(k) => {
            if let Ok(ks) = Keystroke::parse(k) {
                window.dispatch_keystroke(ks, cx);
            }
        }
        Held(k) => {
            if let Ok(keystroke) = Keystroke::parse(k) {
                window.dispatch_event(
                    PlatformInput::KeyDown(KeyDownEvent {
                        keystroke,
                        is_held: true,
                        prefer_character_input: false,
                    }),
                    cx,
                );
            }
        }
        Type(field, text) => {
            me.update(cx, |this, cx| {
                let input = match field {
                    Field::Composer => this.composer.clone(),
                    Field::Starter => this.starter.clone(),
                    Field::Comment => this.comment_input.clone(),
                    Field::Aside => this.aside_input.clone(),
                    Field::Branches => this.branch_query.clone(),
                };
                input.update(cx, |i, cx| {
                    // As typed: the caret ends up after the text.
                    crate::app::put(i, text, window, cx);
                    i.focus_field(window, cx);
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
        Sweep => {
            advance = sweep_step(&me, window, cx);
        }
        DragFiles(name, fx, fy) => {
            let at = me.read(cx).probes.borrow().get(name).copied().map(|b| {
                point(
                    b.origin.x + b.size.width * fx,
                    b.origin.y + b.size.height * fy,
                )
            });
            match at {
                Some(at) => {
                    POINTER.with(|p| p.set(at));
                    let dir = std::env::var("DENT_WALK_PROJECT").unwrap_or_else(|_| "/tmp".into());
                    let dir = std::path::Path::new(&dir);
                    let _ = std::fs::write(dir.join("my notes.txt"), "notes\n");
                    let paths = ExternalPaths(
                        [dir.join("red.png"), dir.join("my notes.txt")]
                            .into_iter()
                            .collect(),
                    );
                    window.dispatch_event(
                        PlatformInput::FileDrop(FileDropEvent::Entered {
                            position: at,
                            paths,
                        }),
                        cx,
                    );
                    window.dispatch_event(
                        PlatformInput::FileDrop(FileDropEvent::Pending { position: at }),
                        cx,
                    );
                }
                None => note(
                    &me,
                    cx,
                    leak(format!("drag target “{name}” on screen")),
                    false,
                ),
            }
        }
        DropFiles => {
            let at = POINTER.with(|p| p.get());
            mouse(window, cx, at, false);
            window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Exited), cx);
        }
        Press(name, fx, fy) => {
            let at = me.read(cx).probes.borrow().get(name).copied().map(|b| {
                point(
                    b.origin.x + b.size.width * fx,
                    b.origin.y + b.size.height * fy,
                )
            });
            match at {
                Some(at) => {
                    POINTER.with(|p| p.set(at));
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position: at,
                            pressed_button: None,
                            modifiers: Modifiers::default(),
                        }),
                        cx,
                    );
                    mouse(window, cx, at, true);
                }
                None => note(
                    &me,
                    cx,
                    leak(format!("press target “{name}” on screen")),
                    false,
                ),
            }
        }
        MoveBy(dx, dy) => {
            let from = POINTER.with(|p| p.get());
            for k in 1..=6 {
                let at = point(
                    from.x + px(dx * k as f32 / 6.),
                    from.y + px(dy * k as f32 / 6.),
                );
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
            }
            POINTER.with(|p| p.set(point(from.x + px(dx), from.y + px(dy))));
        }
        Release => {
            let at = POINTER.with(|p| p.get());
            mouse(window, cx, at, false);
        }
        HoverLink(prefix) => match crate::select::link_bounds(prefix) {
            Some(b) => {
                POINTER.with(|p| p.set(b.center()));
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: b.center(),
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
            }
            None => note(
                &me,
                cx,
                leak(format!("a link to {prefix} on screen")),
                false,
            ),
        },
        DoubleClick(name, fx, fy) => {
            let at = me.read(cx).probes.borrow().get(name).copied().map(|b| {
                point(
                    b.origin.x + b.size.width * fx,
                    b.origin.y + b.size.height * fy,
                )
            });
            match at {
                Some(at) => {
                    POINTER.with(|p| p.set(at));
                    for n in 1..=2 {
                        window.dispatch_event(
                            PlatformInput::MouseDown(MouseDownEvent {
                                button: MouseButton::Left,
                                position: at,
                                modifiers: Modifiers::default(),
                                click_count: n,
                                first_mouse: false,
                            }),
                            cx,
                        );
                        window.dispatch_event(
                            PlatformInput::MouseUp(MouseUpEvent {
                                button: MouseButton::Left,
                                position: at,
                                modifiers: Modifiers::default(),
                                click_count: n,
                            }),
                            cx,
                        );
                    }
                }
                None => note(
                    &me,
                    cx,
                    leak(format!("double-click target “{name}” on screen")),
                    false,
                ),
            }
        }
        Pinch(name, by) => match center(&me, name, cx) {
            Some(at) => {
                // Eight equal movements of the fingers, as ratios.
                let each = by.powf(1. / 8.) - 1.;
                let moves = std::iter::once((0., TouchPhase::Started))
                    .chain(std::iter::repeat_n((each, TouchPhase::Moved), 8))
                    .chain([(0., TouchPhase::Ended)]);
                for (delta, phase) in moves {
                    window.dispatch_event(
                        PlatformInput::Pinch(PinchEvent {
                            position: at,
                            delta,
                            modifiers: Modifiers::default(),
                            phase,
                        }),
                        cx,
                    );
                }
            }
            None => note(
                &me,
                cx,
                leak(format!("pinch target “{name}” on screen")),
                false,
            ),
        },
        ZoomScroll(name, dy) => match center(&me, name, cx) {
            Some(at) => {
                let cmd = Modifiers {
                    platform: true,
                    ..Modifiers::default()
                };
                let moves = std::iter::once((0., TouchPhase::Started))
                    .chain(std::iter::repeat_n((dy / 8., TouchPhase::Moved), 8))
                    .chain([(0., TouchPhase::Ended)]);
                for (d, phase) in moves {
                    window.dispatch_event(
                        PlatformInput::ScrollWheel(ScrollWheelEvent {
                            position: at,
                            delta: ScrollDelta::Pixels(point(px(0.), px(d))),
                            modifiers: cmd,
                            touch_phase: phase,
                        }),
                        cx,
                    );
                }
            }
            None => note(
                &me,
                cx,
                leak(format!("scroll target “{name}” on screen")),
                false,
            ),
        },
        Hover(name, dx, dy) => match center(&me, name, cx) {
            Some(at) => {
                let at = point(at.x + px(dx), at.y + px(dy));
                POINTER.with(|p| p.set(at));
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
            }
            None => note(
                &me,
                cx,
                leak(format!("hover target “{name}” on screen")),
                false,
            ),
        },
        // The scene waits for the picture, so it shows this moment and not a
        // later one, and a still that couldn't be taken fails the walk.
        Shot(name) => {
            use std::sync::atomic::Ordering::SeqCst;
            let dir = me.read(cx).walk.as_ref().and_then(|d| d.shots.clone());
            if let Some(dir) = dir {
                let path = dir.join(format!("{name}.png"));
                let asked = SHOT_ASKED.with(|a| a.get());
                let stuck = SHOT_SINCE
                    .with(|t| t.get())
                    .is_some_and(|t| t.elapsed() > Duration::from_secs(15));
                match SHOT.load(SeqCst) {
                    // A capture that never comes back fails, not the walk.
                    TAKING if asked && stuck => {
                        SHOT_ASKED.with(|a| a.set(false));
                        note(&me, cx, "the still was saved", false);
                    }
                    TAKING => advance = false,
                    TAKEN if asked => {
                        SHOT_ASKED.with(|a| a.set(false));
                        if std::fs::metadata(&path).map_or(true, |m| m.len() == 0) {
                            note(&me, cx, "the still was saved", false);
                        }
                    }
                    _ => {
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::fs::remove_file(&path);
                        SHOT_ASKED.with(|a| a.set(true));
                        SHOT_SINCE.with(|t| t.set(Some(Instant::now())));
                        still(path);
                        advance = false;
                    }
                }
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

/// One step of a sweep. True when every session has been opened.
fn sweep_step(me: &Entity<Dent>, window: &mut Window, cx: &mut App) -> bool {
    me.update(cx, |this, _| {
        let all: Vec<String> = this.sessions.iter().map(|s| s.id.clone()).collect();
        let d = this.walk.as_mut().expect("walk");
        if d.sweep.is_some() {
            return;
        }
        // DENT_WALK_SESSIONS picks the sessions, and each is photographed
        // at its end, its first compaction and its first slash command.
        let picked = std::env::var("DENT_WALK_SESSIONS").ok();
        let ids: Vec<String> = match &picked {
            Some(list) => list.split(',').map(str::to_owned).collect(),
            None => all,
        };
        let mut todo = std::collections::VecDeque::new();
        for (n, id) in ids.into_iter().enumerate() {
            todo.push_back(SweepStep::Open(id));
            todo.push_back(SweepStep::Timed);
            if picked.is_some() && d.shots.is_some() {
                let n = n + 1;
                todo.extend([
                    SweepStep::Frame,
                    SweepStep::Still(format!("session-{n}-end")),
                    SweepStep::ScrollTo(|i| matches!(i, Item::Note { text, .. } if text == "Conversation compacted")),
                    SweepStep::Frame,
                    SweepStep::Still(format!("session-{n}-compacted")),
                    SweepStep::ScrollTo(|i| matches!(i, Item::User { text, files, .. } if crate::draft::sent_command(text, files).is_some())),
                    SweepStep::Frame,
                    SweepStep::Still(format!("session-{n}-slash")),
                    SweepStep::ScrollTo(|i| matches!(i, Item::User { files, .. } if !files.is_empty())),
                    SweepStep::Frame,
                    SweepStep::Still(format!("session-{n}-picture")),
                    SweepStep::OpenAgents,
                    SweepStep::ScrollTo(has_agent),
                    SweepStep::Frame,
                    SweepStep::Still(format!("session-{n}-agents")),
                ]);
            }
        }
        d.sweep = Some(Sweep {
            todo,
            opened: None,
            times: Vec::new(),
        });
    });
    let step = me.update(cx, |this, _| {
        this.walk.as_mut()?.sweep.as_mut()?.todo.pop_front()
    });
    match step {
        Some(SweepStep::Open(id)) => {
            me.update(cx, |this, cx| {
                this.select(Some(id.clone()), window, cx);
                if let Some(sw) = this.walk.as_mut().and_then(|d| d.sweep.as_mut()) {
                    sw.opened = Some((id, Instant::now()));
                }
            });
            false
        }
        Some(SweepStep::Timed) => {
            me.update(cx, |this, _| {
                let items = this.current().map(|s| s.items.len()).unwrap_or(0);
                if let Some(sw) = this.walk.as_mut().and_then(|d| d.sweep.as_mut())
                    && let Some((id, at)) = sw.opened.take()
                {
                    sw.times
                        .push((id, at.elapsed().as_secs_f64() * 1000.0, items));
                }
            });
            false
        }
        Some(SweepStep::ScrollTo(f)) => {
            me.update(cx, |this, cx| {
                let at = this.current().and_then(|s| s.items.iter().position(f));
                if let Some(ix) = at {
                    this.transcript.scroll_to(ListOffset {
                        item_ix: this.item_row(ix),
                        offset_in_item: px(0.),
                    });
                    cx.notify();
                }
            });
            false
        }
        Some(SweepStep::Still(name)) => {
            if let Some(dir) = me.read(cx).walk.as_ref().and_then(|d| d.shots.clone()) {
                let _ = std::fs::create_dir_all(&dir);
                still(dir.join(format!("{name}.png")));
            }
            let until = Instant::now() + Duration::from_millis(1200);
            me.update(cx, |this, _| {
                if let Some(sw) = this.walk.as_mut().and_then(|d| d.sweep.as_mut()) {
                    sw.todo.push_front(SweepStep::Until(until));
                }
            });
            false
        }
        Some(SweepStep::Until(at)) => {
            me.update(cx, |this, cx| {
                if Instant::now() < at
                    && let Some(sw) = this.walk.as_mut().and_then(|d| d.sweep.as_mut())
                {
                    sw.todo.push_front(SweepStep::Until(at));
                }
                cx.notify();
            });
            false
        }
        Some(SweepStep::OpenAgents) => {
            me.update(cx, |this, cx| {
                let keys: Vec<String> = this
                    .current()
                    .map(|s| {
                        s.items
                            .iter()
                            .filter(|i| has_agent(i))
                            .filter_map(|i| match i {
                                Item::Tools { lines, .. } => lines.first().map(|l| l.id.clone()),
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                this.open_tools.extend(keys);
                cx.notify();
            });
            false
        }
        Some(SweepStep::Frame) => {
            me.update(cx, |_, cx| cx.notify());
            false
        }
        None => {
            let sw = me
                .update(cx, |this, _| {
                    this.walk.as_mut().and_then(|d| d.sweep.take())
                })
                .expect("sweep");
            let mut times = sw.times;
            let empty = times.iter().filter(|t| t.2 == 0).count();
            times.sort_by(|a, b| b.1.total_cmp(&a.1));
            let total: f64 = times.iter().map(|t| t.1).sum();
            eprintln!(
                "      opened {} sessions, {:.0} ms on average",
                times.len(),
                total / times.len().max(1) as f64
            );
            for (id, ms, items) in times.iter().take(8) {
                eprintln!("      {ms:>8.1} ms  {items:>6} items  {id}");
            }
            note(
                me,
                cx,
                "every session opens and shows its conversation",
                empty == 0 && !times.is_empty(),
            );
            note(
                me,
                cx,
                "each opens within half a second",
                times.first().is_none_or(|t| t.1 < 500.0),
            );
            true
        }
    }
}

fn has_agent(i: &Item) -> bool {
    matches!(i, Item::Tools { summary, lines, .. } if summary.starts_with("Subagent")
        || lines.iter().any(|l| l.text.starts_with("Subagent")))
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

fn current(d: &Dent) -> Option<&crate::data::Session> {
    d.current()
}

/// Where the latest frame drew a probe.
fn drawn(d: &Dent, name: &str) -> Option<Bounds<Pixels>> {
    let fresh = crate::ui::PROBE_FRAME
        .with(|f| f.borrow().get(name).copied())
        .is_some_and(|at| at + 2 >= d.frames);
    fresh
        .then(|| d.probes.borrow().get(name).copied())
        .flatten()
}

fn my_ids(d: &Dent) -> Vec<String> {
    current(d).map_or(Vec::new(), |s| {
        s.items
            .iter()
            .filter_map(|i| match i {
                Item::User { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect()
    })
}

fn review(d: &mut Dent, turn: usize, path: &str, cx: &mut Context<Dent>) {
    let sid = d.selected.clone().unwrap_or_default();
    let t = my_ids(d).get(turn).cloned();
    d.review_turn(&sid, t, Some(path.to_string()), cx);
}

/// Where the latest still is: not asked for, being taken, or done (saved or not).
static SHOT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(IDLE);

const IDLE: u8 = 0;

const TAKING: u8 = 1;

const TAKEN: u8 = 2;

thread_local! {
    /// A `Shot` step asked for the still in flight (a sweep's own don't count).
    static SHOT_ASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// When it asked.
    static SHOT_SINCE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// The window only, never what's around it: by window number when the
/// `WINID` helper (tools/winid.swift) is given, else the window's rectangle.
/// `SHOT` says when it's done.
pub fn still(path: PathBuf) {
    use std::sync::atomic::Ordering::SeqCst;
    SHOT.store(TAKING, SeqCst);

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
                SHOT.store(TAKEN, SeqCst);
                return;
            };
            let _ = std::process::Command::new("import")
                .args(["-window", &id])
                .arg(&path)
                .status();
            SHOT.store(TAKEN, SeqCst);
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
            SHOT.store(TAKEN, SeqCst);
            return;
        };
        let _ = std::process::Command::new("screencapture")
            .args(["-x", "-o", &format!("-l{id}")])
            .arg(&path)
            .status();
        SHOT.store(TAKEN, SeqCst);
    });
}
