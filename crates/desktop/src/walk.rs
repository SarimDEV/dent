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
    Aside,
}

/// The walkthrough's vocabulary; not every scene uses every step.
#[allow(dead_code)]
#[derive(Clone)]
pub enum Act {
    Wait,
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

impl Walk {
    pub fn new(name: &str, shots: Option<PathBuf>) -> Self {
        let steps = match name {
            "stills" => stills(),
            "real" => real(),
            "sweep" => sweep(),
            "markdown" => markdown(),
            "motion" => motion(),
            "find" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(find());
                v
            }
            "links" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(file_links());
                v
            }
            "attach" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(attaching());
                v
            }
            "narrate" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(narration());
                v
            }
            "findview" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(find_viewer());
                v
            }
            "keys" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(answer_keys());
                v
            }
            "reply" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(reply_actions());
                v
            }
            "switch" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(switching());
                v
            }
            "view" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(view_choices());
                v
            }
            "pins" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(pins());
                v
            }
            "fail" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(failures());
                v
            }
            "queue" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(queue_order());
                v
            }
            "changes" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(turn_changes());
                v
            }
            "recall" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(recall());
                v
            }
            "typing" => {
                let mut v = vec![(
                    0.,
                    Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
                )];
                v.extend(typing());
                v
            }
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
                    Field::Aside => this.aside_input.clone(),
                };
                input.update(cx, |i, cx| {
                    // As typed: the caret ends up after the text.
                    crate::app::put(i, text, window, cx);
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
                    SweepStep::ScrollTo(|i| matches!(i, Item::User { text, .. } if text.starts_with('/'))),
                    SweepStep::Frame,
                    SweepStep::Still(format!("session-{n}-slash")),
                    SweepStep::ScrollTo(|i| matches!(i, Item::User { text, .. } if text.contains("Attached"))),
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
                        item_ix: ix,
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
    matches!(i, Item::Tools { summary, lines } if summary.starts_with("Subagent")
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

/// Every session the daemon has, opened one by one: for checking the app
/// against real transcripts loaded with `dent-daemon check-transcripts --into`.
fn sweep() -> Vec<(f32, Act)> {
    vec![
        (
            0.5,
            Until(
                "every session's history arrives",
                |d, _| d.core.live && d.sessions.len() > 1,
                120.,
            ),
        ),
        (0.5, Sweep),
    ]
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

/// A page in a folder outside the project, with a stylesheet and a picture
/// beside it (scripts/walk.sh puts them there).
fn outside_page() -> &'static str {
    let dir = std::env::var("DENT_WALK_OUTSIDE").unwrap_or_else(|_| "/nonexistent".into());
    leak(format!("write {dir}/page.html with assets"))
}

thread_local! {
    /// Whether the sidebar showed before a scene toggled it.
    static SIDEBAR_WAS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the open right-click menu has a row saying `label`.
fn menu_has(d: &Dent, label: &str) -> bool {
    d.context
        .as_ref()
        .is_some_and(|m| m.items.iter().any(|(l, _)| l.as_ref() == label))
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

/// Where the latest frame drew a probe.
fn drawn(d: &Dent, name: &str) -> Option<Bounds<Pixels>> {
    let fresh = crate::ui::PROBE_FRAME
        .with(|f| f.borrow().get(name).copied())
        .is_some_and(|at| at + 2 >= d.frames);
    fresh
        .then(|| d.probes.borrow().get(name).copied())
        .flatten()
}

/// Scroll the conversation so its last reply starts at the top.
fn to_reply(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    let at = current(d).and_then(|s| s.items.iter().rposition(|i| matches!(i, Item::Text(_))));
    if let Some(ix) = at {
        d.transcript.scroll_to(ListOffset {
            item_ix: ix,
            offset_in_item: px(0.),
        });
        cx.notify();
    }
}

fn to_end(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    d.transcript.scroll_to_end();
    cx.notify();
}

type Saved = (Option<ListOffset>, Option<Option<ClipboardItem>>);

thread_local! {
    /// The conversation's scroll position before a swipe, and the clipboard
    /// before a copy, to compare against and put back.
    static SAVED: std::cell::RefCell<Saved> = Default::default();
}

fn save_scroll(d: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
    let top = d.transcript.logical_scroll_top();
    SAVED.with(|s| s.borrow_mut().0 = Some(top));
}

fn chat_still(d: &Dent) -> bool {
    let top = d.transcript.logical_scroll_top();
    SAVED.with(|s| {
        s.borrow().0.is_some_and(|was| {
            was.item_ix == top.item_ix && (was.offset_in_item - top.offset_in_item).abs() < px(0.5)
        })
    })
}

fn table_top(d: &Dent) -> f32 {
    d.mdv.table_scroll().map_or(0., |(top, _)| top)
}

fn clipboard_text(cx: &App) -> String {
    cx.read_from_clipboard()
        .and_then(|c| c.text())
        .unwrap_or_default()
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
    v.extend(typing());
    v.extend([
        (0.3, Key("cmd-n")),
        (
            0.1,
            Until(
                "the model menu names versions before any session starts",
                |d, _| {
                    crate::bridge::model_name(&d.models, d.defaults.model.as_deref())
                        == "Default · Sonnet 5.5"
                        && d.models.iter().any(|m| m.name == "Fable 5.1")
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
                    let top = d.transcript.logical_scroll_top().item_ix;
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
        (0.05, MoveBy(-250., 0.)),
        (0.05, Release),
        (
            0.1,
            Until(
                "and back out wider, the page returning",
                |d, _| d.panel_width > 520. && !d.dragging && d.preview_visible(),
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
        (0.1, Until("a dropped picture goes with the message", |d, _| has(d, |i| matches!(i, Item::User { text: t, .. } if t.contains("Attached") && t.contains("red.png"))) && status_is(d, Status::Done), 10.)),
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
                                    icon: "circle-alert",
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
                && has(d, |i| matches!(i, Item::Tools { summary, lines } if summary.contains("Subagent:") && summary.contains("Survey the tests · 3 steps") && !lines.iter().any(|l| l.text.contains("Survey the tests"))))
        }, 10.),
    ), (
        0.1,
        Until("a subagent's own subagent sits a level deeper, with its report", |d, _| {
            has(d, |i| matches!(i, Item::Tools { lines, .. }
                if lines.iter().any(|l| l.depth == 0 && l.text.contains("Time the db tests"))
                && lines.iter().any(|l| l.depth == 1 && l.text.contains("cargo test db"))
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
        (0.3, Shot("plan-folded")),
        (
            0.6,
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
    v
}

/// Maths, a diagram and a tall table in a reply: drawn, scrolled, copied.
fn rich_reply() -> Vec<(f32, Act)> {
    let mut v = new_session("show me some math, a diagram and a big table");
    v.extend([
        (
            0.1,
            Until(
                "a reply with maths, a diagram and a big table finishes",
                |d, _| done_saying(d, "all sixty"),
                20.,
            ),
        ),
        (0.3, Call(to_reply)),
        (
            0.1,
            Until(
                "display and inline maths are drawn",
                |d, _| drawn(d, "md-math").is_some() && drawn(d, "md-inline-math").is_some(),
                5.,
            ),
        ),
        (
            0.1,
            Until(
                "a diagram is drawn off the UI thread",
                |d, _| drawn(d, "md-diagram").is_some_and(|b| b.size.width > px(100.)),
                10.,
            ),
        ),
        (0.4, Shot("maths")),
        (0.2, Call(to_end)),
        (
            0.3,
            Until(
                "a tall table shows its header and 10½ rows",
                |d, _| {
                    drawn(d, "md-table-body").is_some_and(|b| {
                        (b.size.height.as_f32() - 10.5 * crate::markdown::ROW_H).abs() < 1.
                    }) && d.mdv.table_scroll().is_some_and(|(_, max)| max > 100.)
                },
                5.,
            ),
        ),
        (0.3, Shot("table")),
        (0.1, Call(save_scroll)),
        (0.1, Wheel("md-table-body", -90.)),
        (0.1, Wheel("md-table-body", -90.)),
        (
            0.2,
            Until(
                "the wheel scrolls the table, not the chat",
                |d, _| table_top(d) > 100. && chat_still(d),
                3.,
            ),
        ),
        (0.1, Call(save_scroll)),
        (0.1, Wheel("md-table-body", 60.)),
        (
            0.2,
            Until(
                "scrolling back up moves the table before the chat",
                |d, _| (table_top(d) - 120.).abs() < 1. && chat_still(d),
                3.,
            ),
        ),
        (0.1, Wheel("md-table-body", 300.)),
        (0.1, Call(save_scroll)),
        (0.1, Wheel("md-table-body", 60.)),
        (
            0.2,
            Until(
                "at the table's top, the wheel carries on to the chat",
                |d, _| table_top(d) < 0.5 && !chat_still(d),
                3.,
            ),
        ),
        (0.2, Call(to_end)),
        (
            0.3,
            Call(|_, _, cx| {
                let was = cx.read_from_clipboard();
                SAVED.with(|s| s.borrow_mut().1 = Some(was));
            }),
        ),
        (0.1, Click("md-copy-csv")),
        (
            0.1,
            Until(
                "Copy CSV puts the whole table on the clipboard as CSV",
                |_, cx| {
                    let t = clipboard_text(cx);
                    t.starts_with("#,Event,Duration\n1,event number 1,1.1 ms\n")
                        && t.lines().count() == 61
                        && t.ends_with("60,event number 60,6.4 ms")
                },
                3.,
            ),
        ),
        (0.2, Shot("table-copied")),
        (0.1, Click("md-copy-md")),
        (
            0.1,
            Until(
                "Copy puts it on the clipboard as Markdown",
                |_, cx| {
                    let t = clipboard_text(cx);
                    t.starts_with("| # | Event | Duration |\n| ---: | --- | ---: |\n| 1 |")
                        && t.lines().count() == 62
                },
                3.,
            ),
        ),
        (
            0.1,
            Call(|_, _, cx| {
                // Put back whatever was on the clipboard before.
                if let Some(was) = SAVED.with(|s| s.borrow_mut().1.take()) {
                    cx.write_to_clipboard(
                        was.unwrap_or_else(|| ClipboardItem::new_string(String::new())),
                    );
                }
            }),
        ),
    ]);
    v
}

/// Only the rich reply, for working on how Markdown renders.
/// The text box's own keys, quick to run alone.
fn typing() -> Vec<(f32, Act)> {
    vec![
        (
            0.2,
            Call(|d, w, cx| {
                d.starter.update(cx, |s, cx| {
                    s.set_value("", w, cx);
                    s.focus(w, cx);
                })
            }),
        ),
        (
            0.1,
            Until(
                "the new-session box has the keyboard",
                |d, cx| d.starter.read(cx).value().is_empty(),
                1.,
            ),
        ),
        (0.1, Keys("a")),
        (0.1, Key("alt-enter")),
        (0.1, Keys("b")),
        (0.1, Key("shift-enter")),
        (0.1, Keys("c")),
        (
            0.1,
            Until(
                "⌥ Return and Shift-Return start a new line, not a session",
                |d, cx| d.starter.read(cx).value() == "a\nb\nc",
                1.,
            ),
        ),
        (
            0.1,
            Call(|d, w, cx| d.starter.update(cx, |s, cx| s.set_value("", w, cx))),
        ),
    ]
}

fn markdown() -> Vec<(f32, Act)> {
    let mut v: Vec<(f32, Act)> = vec![(
        0.,
        Until(
            "connects to the daemon, starting it",
            |d, _| d.connection.is_ok(),
            15.,
        ),
    )];
    v.extend(rich_reply());
    v
}

fn mine(d: &Dent) -> usize {
    current(d).map_or(0, |s| {
        s.items
            .iter()
            .filter(|i| matches!(i, Item::User { .. }))
            .count()
    })
}

fn composer_is(d: &Dent, cx: &App, text: &str) -> bool {
    d.composer.read(cx).value() == text
}

/// Walkthroughs don't save as they go; this saves once, as dent would.
fn save_now(d: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
    d.save_prefs();
}

/// ↑ brings back what you sent, a waiting message first; drafts are kept.
fn recall() -> Vec<(f32, Act)> {
    let mut v = new_session("first prompt");
    v.extend([
        (
            0.1,
            Until(
                "the first turn ends",
                |d, _| status_is(d, Status::Done),
                20.,
            ),
        ),
        (0.2, Type(Field::Composer, "second prompt")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "the second turn ends",
                |d, _| status_is(d, Status::Done) && mine(d) == 2,
                20.,
            ),
        ),
        (0.2, Key("up")),
        (
            0.1,
            Until(
                "↑ in an empty composer brings back the last message",
                |d, cx| composer_is(d, cx, "second prompt"),
                1.,
            ),
        ),
        (0.1, Key("up")),
        (
            0.1,
            Until(
                "↑ again goes further back",
                |d, cx| composer_is(d, cx, "first prompt"),
                1.,
            ),
        ),
        (0.1, Key("up")),
        (
            0.1,
            Until(
                "↑ at the first message stays on it",
                |d, cx| composer_is(d, cx, "first prompt"),
                1.,
            ),
        ),
        (0.1, Key("down")),
        (
            0.1,
            Until(
                "↓ comes forward",
                |d, cx| composer_is(d, cx, "second prompt"),
                1.,
            ),
        ),
        (0.1, Key("down")),
        (
            0.1,
            Until(
                "↓ past the newest empties the box",
                |d, cx| composer_is(d, cx, ""),
                1.,
            ),
        ),
        (0.1, Key("up")),
        (0.1, Keys(" edited")),
        (0.1, Key("up")),
        (
            0.1,
            Until(
                "once edited, ↑ moves the caret and leaves the text",
                |d, cx| composer_is(d, cx, "second prompt edited"),
                1.,
            ),
        ),
        (0.1, Type(Field::Composer, "line one\nline two")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "a message of two lines is answered",
                |d, _| status_is(d, Status::Done) && mine(d) == 3,
                20.,
            ),
        ),
        (0.2, Key("up")),
        (0.1, Key("up")),
        (
            0.1,
            Until(
                "↑ through a message of several lines walks its lines first",
                |d, cx| {
                    composer_is(d, cx, "line one\nline two") && d.composer.read(cx).cursor() < 9
                },
                1.,
            ),
        ),
        (0.1, Key("up")),
        (
            0.1,
            Until(
                "then goes on to the message before",
                |d, cx| composer_is(d, cx, "second prompt"),
                1.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
        (0.2, Type(Field::Composer, "a long answer please")),
        (0.1, Key("enter")),
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "change me")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "the message waits",
                |d, _| has(d, |i| matches!(i, Item::Queued { .. })),
                5.,
            ),
        ),
        (0.2, Key("up")),
        (
            0.1,
            Until(
                "↑ takes a waiting message back out of the queue to edit",
                |d, cx| {
                    composer_is(d, cx, "change me") && !has(d, |i| matches!(i, Item::Queued { .. }))
                },
                3.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
        (
            0.1,
            Until(
                "taken back and cleared, it's never sent",
                |d, _| status_is(d, Status::Done) && mine(d) == 4,
                20.,
            ),
        ),
        (0.5, Wait),
        (0.1, Type(Field::Composer, "half a thought")),
        (0.1, Call(save_now)),
        (
            0.1,
            Until(
                "an unsent draft is saved for the next launch",
                |d, _| {
                    let id = d.selected.clone().unwrap_or_default();
                    mine(d) == 4
                        && crate::prefs::load().drafts.get(&id).map(String::as_str)
                            == Some("half a thought")
                },
                2.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
    ]);
    v
}

fn find_text(d: &Dent, cx: &App) -> String {
    d.find_input.read(cx).value().to_string()
}

fn find_at(d: &Dent) -> Option<(usize, usize)> {
    d.find.as_ref().map(|f| (f.at, f.hits.len()))
}

thread_local! {
    /// A number a step notes for later steps to check against.
    static MARK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn mark() -> usize {
    MARK.with(|m| m.get())
}

/// The turn is over and a reply follows your last message.
fn answered(d: &Dent) -> bool {
    status_is(d, Status::Done)
        && current(d).is_some_and(|s| {
            let last_mine = s.items.iter().rposition(|i| matches!(i, Item::User { .. }));
            let last_reply = s.items.iter().rposition(|i| matches!(i, Item::Text(_)));
            last_reply > last_mine
        })
}

/// Send `text` and wait until it's answered.
fn ask_and_wait(text: &'static str) -> Vec<(f32, Act)> {
    vec![
        (0.2, Type(Field::Composer, text)),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                leak(format!("“{text}” is answered")),
                |d, _| answered(d),
                20.,
            ),
        ),
    ]
}

/// ⌘F finds in the session: counts, steps round, says when nothing matches.
fn find() -> Vec<(f32, Act)> {
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

fn find_scope(d: &Dent) -> Option<crate::find::Scope> {
    d.find.as_ref().map(|f| f.scope)
}

thread_local! {
    /// Whether the current ⌘F match was on screen when last looked.
    static SHOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Look whether the current match is on screen, for the next step.
fn look_shown(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>) {
    SHOWN.with(|v| v.set(match_shown(d, w, cx)));
}

fn shown(_: &Dent, _: &App) -> bool {
    SHOWN.with(|v| v.get())
}

/// The current match is on screen, as last drawn.
fn match_shown(d: &Dent, w: &Window, cx: &App) -> bool {
    let Some(f) = d.find.as_ref() else {
        return false;
    };
    let (Some(s), Some(&(ix, k))) = (current(d), f.hits.get(f.at)) else {
        return false;
    };
    let needle = d.find_input.read(cx).value().to_string();
    crate::select::in_view(d.scope_area(s, f.scope), ix as u32, k, &needle, w)
}

fn doc_top(d: &Dent) -> f32 {
    current(d)
        .and_then(|s| match d.tab(s) {
            Tab::Art(id) => d.doc_scrolls.get(id),
            _ => None,
        })
        .map_or(0., |h| -h.offset().y.as_f32())
}

/// A long document, with a word three times far down it.
fn write_long_doc(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
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
fn find_viewer() -> Vec<(f32, Act)> {
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

fn noted(d: &Dent, start: &str) -> bool {
    current(d).is_some_and(|s| {
        s.items
            .iter()
            .any(|i| matches!(i, Item::Note { text, .. } if text.starts_with(start)))
    })
}

/// Approvals and plans answer to keys on their card, and only there.
fn answer_keys() -> Vec<(f32, Act)> {
    fn waiting(d: &Dent) -> bool {
        has(d, |i| matches!(i, Item::Approval { .. })) && status_is(d, Status::Needs)
    }
    let mut v = new_session("please run it");
    v.extend([
        (0.1, Until("an approval waits", |d, _| waiting(d), 10.)),
        (0.2, Keys("1")),
        (
            0.3,
            Until(
                "a 1 typed in the reply box is just a 1: the approval still waits",
                |d, cx| waiting(d) && composer_is(d, cx, "1"),
                1.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
        (0.1, Key("escape")),
        (
            0.1,
            Until(
                "Esc from the empty box takes the keyboard to the card",
                |d, _| d.ask_keyed,
                1.,
            ),
        ),
        (0.3, Shot("approval-keys")),
        (0.8, Key("1")),
        (
            0.1,
            Until(
                "1 on the card allows once",
                |d, _| {
                    done_saying(d, "Ran it")
                        && noted(d, "Allowed")
                        && !noted(d, "Allowed echo hello > hello.txt for")
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "the keyboard goes back to the reply box",
                |d, _| !d.ask_keyed,
                1.,
            ),
        ),
        (0.2, Type(Field::Composer, "run it again")),
        (0.1, Key("enter")),
        (0.1, Until("another approval", |d, _| waiting(d), 10.)),
        (0.1, Key("escape")),
        (0.2, Key("2")),
        (
            0.1,
            Until(
                "2 allows for this session",
                |d, _| {
                    status_is(d, Status::Done)
                        && noted(d, "Allowed echo hello > hello.txt for this session")
                },
                10.,
            ),
        ),
    ]);
    v.extend(new_session("run it, please"));
    v.extend([
        (0.1, Until("a third approval", |d, _| waiting(d), 10.)),
        (0.1, Key("escape")),
        (0.2, Key("3")),
        (
            0.1,
            Until(
                "3 allows it in this project from now on",
                |d, _| {
                    status_is(d, Status::Done)
                        && noted(d, "Allowed echo hello > hello.txt in this project")
                        && std::fs::read_to_string(in_project(".claude/settings.local.json"))
                            .is_ok_and(|s| s.contains("Bash(echo hello > hello.txt)"))
                },
                10.,
            ),
        ),
    ]);
    v.extend(new_session("run it once more"));
    v.extend([
        (
            0.1,
            Until(
                "and a later session runs it without asking",
                |d, _| done_saying(d, "Ran it") && !has(d, |i| matches!(i, Item::Approval { .. })),
                10.,
            ),
        ),
        // Leave the project as it was, so later checks are asked again.
        (
            0.1,
            Call(|_, _, _| {
                let _ = std::fs::remove_file(in_project(".claude/settings.local.json"));
            }),
        ),
    ]);
    v.extend(new_session("write keep.md please"));
    v.extend([
        (0.1, Until("a write waits", |d, _| waiting(d), 10.)),
        (0.1, Key("escape")),
        (0.2, Key("3")),
        (
            0.3,
            Until(
                "with no rule to save, 3 does nothing and says why on hover",
                |d, _| waiting(d) && drawn(d, "allow-project").is_some(),
                1.,
            ),
        ),
        (0.2, Shot("allow-project-off")),
        (0.8, Key("1")),
        (
            0.1,
            Until("it's written", |d, _| status_is(d, Status::Done), 10.),
        ),
    ]);
    v.extend(new_session("run it, please"));
    v.extend([
        (0.1, Until("a fourth approval", |d, _| waiting(d), 10.)),
        (
            0.1,
            Until(
                "a new session's reply box has the keyboard",
                |d, _| d.composer_keyed,
                1.,
            ),
        ),
        (0.1, Key("escape")),
        (
            0.1,
            Until("and Esc takes it to the card", |d, _| d.ask_keyed, 1.),
        ),
        (0.2, Key("4")),
        (
            0.1,
            Until(
                "4 denies",
                |d, _| status_is(d, Status::Done) && noted(d, "Denied"),
                10.,
            ),
        ),
    ]);
    v.extend(new_session("present a plan"));
    v.extend([
        (
            0.1,
            Until(
                "a plan waits",
                |d, _| has(d, |i| matches!(i, Item::Plan { .. })),
                10.,
            ),
        ),
        (0.1, Key("escape")),
        (0.2, Key("enter")),
        (
            0.1,
            Until(
                "Return on its card starts building",
                |d, _| done_saying(d, "Building it now") && noted(d, "Approved the plan"),
                10.,
            ),
        ),
    ]);
    v
}

/// When the session's latest turn ended, in ms.
fn last_end(d: &Dent) -> u64 {
    current(d).map_or(0, |s| {
        s.items
            .iter()
            .rev()
            .find_map(|i| match i {
                Item::TurnEnd { at, .. } => Some(*at),
                _ => None,
            })
            .unwrap_or(0)
    })
}

fn note_end(d: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
    MARK.with(|m| m.set(last_end(d) as usize));
}

/// Under a finished reply: its time, Copy, and Try again.
fn reply_actions() -> Vec<(f32, Act)> {
    let mut v = vec![(0., Call(save_clipboard))];
    v.extend(new_session("check the parser"));
    v.extend([
        (0.1, Until("it answers", |d, _| answered(d), 20.)),
        (
            0.2,
            Until(
                "the latest reply shows Copy and Try again",
                |d, _| drawn(d, "copy-reply-1").is_some() && drawn(d, "try-again").is_some(),
                2.,
            ),
        ),
        (0.3, Shot("reply-actions")),
        (0.8, Click("copy-reply-1")),
        (
            0.1,
            Until(
                "Copy takes the whole reply, as Markdown",
                |_, cx| copied(cx).starts_with("Here's what I'd do about **check the parser**"),
                1.,
            ),
        ),
        (0.1, Call(note_end)),
        (0.2, Click("try-again")),
        (
            0.1,
            Until(
                "Try again, with no files changed, sends the message again in its place",
                |d, _| answered(d) && mine(d) == 1 && last_end(d) as usize > mark(),
                20.,
            ),
        ),
    ]);
    v.extend(new_session("write redo.md please"));
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
                "the file is written",
                |d, _| status_is(d, Status::Done) && in_project("redo.md").exists(),
                10.,
            ),
        ),
        (0.3, Click("try-again")),
        (
            0.1,
            Until(
                "when the turn changed files, Try again asks about them first",
                |d, _| {
                    drawn(d, "try-again-files").is_some() && drawn(d, "try-again-keep").is_some()
                },
                2.,
            ),
        ),
        (0.3, Shot("try-again-files")),
        (0.8, Click("try-again-files")),
        (
            0.1,
            Until(
                "undoing them takes the file away, and the message goes again",
                |d, _| {
                    !in_project("redo.md").exists()
                        && has(d, |i| matches!(i, Item::Approval { .. }))
                        && mine(d) == 1
                },
                20.,
            ),
        ),
        (0.2, Click("allow")),
        (
            0.1,
            Until(
                "and it's written again",
                |d, _| status_is(d, Status::Done) && in_project("redo.md").exists(),
                10.,
            ),
        ),
        (0.1, Call(restore_clipboard)),
    ]);
    v
}

fn titled(d: &Dent, title: &str) -> bool {
    current(d).is_some_and(|s| s.title == title)
}

/// Letting go of Ctrl, which a walkthrough can't do with a key.
fn let_go(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>) {
    d.switch_finish(w, cx);
}

/// Ctrl+Tab: a tap goes back to the last session, held it steps further.
fn switching() -> Vec<(f32, Act)> {
    let mut v = Vec::new();
    for t in ["alpha notes", "beta notes", "gamma notes"] {
        v.extend(new_session(t));
        v.push((
            0.1,
            Until(leak(format!("“{t}” answers")), |d, _| answered(d), 20.),
        ));
    }
    v.extend([
        (0.2, Key("ctrl-tab")),
        (0.05, Call(let_go)),
        (
            0.1,
            Until(
                "a tap of Ctrl+Tab goes back to the session before",
                |d, _| titled(d, "beta notes") && d.switcher.is_none(),
                1.,
            ),
        ),
        (0.2, Key("ctrl-tab")),
        (0.05, Call(let_go)),
        (
            0.1,
            Until(
                "another tap comes back again",
                |d, _| titled(d, "gamma notes"),
                1.,
            ),
        ),
        (
            0.05,
            Until(
                "a quick tap never shows the list",
                |d, _| drawn(d, "switch-0").is_none(),
                0.2,
            ),
        ),
        (0.2, Key("ctrl-tab")),
        (0.1, Key("ctrl-tab")),
        (
            0.4,
            Until(
                "held, the list shows, most recent first, the one two back lit",
                |d, _| {
                    drawn(d, "switch-2").is_some() && d.switcher.as_ref().is_some_and(|s| s.at == 2)
                },
                1.,
            ),
        ),
        (0.2, Shot("switcher")),
        (0.8, Call(let_go)),
        (
            0.1,
            Until(
                "letting go opens it",
                |d, _| titled(d, "alpha notes") && drawn(d, "switch-0").is_none(),
                1.,
            ),
        ),
        (0.2, Key("ctrl-tab")),
        (0.3, Key("escape")),
        (
            0.1,
            Until(
                "Esc stays where you are",
                |d, _| titled(d, "alpha notes") && d.switcher.is_none(),
                1.,
            ),
        ),
        (0.2, Key("ctrl-shift-tab")),
        (
            0.05,
            Until(
                "Ctrl+Shift+Tab goes the other way, to the last in the list",
                |d, _| {
                    d.switcher
                        .as_ref()
                        .is_some_and(|s| s.at + 1 == s.order.len() && s.order.len() > 2)
                },
                1.,
            ),
        ),
        (0.05, Call(let_go)),
        (
            0.1,
            Until(
                "and letting go opens it",
                |d, _| d.switcher.is_none() && !titled(d, "alpha notes"),
                1.,
            ),
        ),
    ]);
    v
}

fn zoom_is(z: f32) -> bool {
    (crate::ds::zoom() - z).abs() < 0.001
}

/// ⇧Tab's modes, text size, appearance and the shortcuts sheet.
fn view_choices() -> Vec<(f32, Act)> {
    use dent_protocol::Permission as P;
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (0.2, Type(Field::Starter, "")),
        (0.1, Key("shift-tab")),
        (
            0.1,
            Until(
                "⇧Tab on a new session steps the mode to Edits",
                |d, _| d.defaults.permission == P::Edits,
                1.,
            ),
        ),
        (0.1, Key("shift-tab")),
        (0.1, Key("shift-tab")),
        (
            0.1,
            Until(
                "on through Plan and round to Ask, never to Full",
                |d, _| d.defaults.permission == P::Ask,
                1.,
            ),
        ),
    ];
    v.extend(new_session("check the parser"));
    v.extend([
        (0.1, Until("it answers", |d, _| answered(d), 20.)),
        (0.2, Type(Field::Composer, "")),
        (0.1, Key("shift-tab")),
        (
            0.1,
            Until(
                "in a session, ⇧Tab changes the session's own mode",
                |d, _| current(d).is_some_and(|s| s.permission == P::Edits),
                3.,
            ),
        ),
        (0.1, Key("shift-tab")),
        (0.1, Key("shift-tab")),
        (
            0.1,
            Until(
                "and round again",
                |d, _| current(d).is_some_and(|s| s.permission == P::Ask),
                3.,
            ),
        ),
        (0.2, Key("cmd-=")),
        (0.1, Key("cmd-=")),
        (
            0.1,
            Until(
                "⌘+ makes the text bigger, a step at a time",
                |_, _| zoom_is(1.2),
                1.,
            ),
        ),
        (0.4, Shot("text-bigger")),
        (0.8, Key("cmd-0")),
        (0.1, Until("⌘0 puts it back", |_, _| zoom_is(1.), 1.)),
        (0.1, Key("cmd--")),
        (0.1, Until("⌘− makes it smaller", |_, _| zoom_is(0.92), 1.)),
        (0.1, Key("cmd-0")),
        (
            0.1,
            Call(|d, w, cx| d.set_appearance(crate::prefs::Appearance::Dark, w, cx)),
        ),
        (
            0.1,
            Until("Dark Appearance wears Carbon", |d, _| d.t.dark, 1.),
        ),
        (
            0.1,
            Call(|d, w, cx| d.set_appearance(crate::prefs::Appearance::Light, w, cx)),
        ),
        (
            0.1,
            Until("Light Appearance wears Paper", |d, _| !d.t.dark, 1.),
        ),
        (0.1, Key("cmd-/")),
        (
            0.1,
            Until(
                "⌘/ shows every shortcut",
                |d, _| d.keys_open && drawn(d, "keys").is_some(),
                1.,
            ),
        ),
        (0.4, Shot("keys")),
        (0.8, Key("escape")),
        (0.1, Until("Esc puts it away", |d, _| !d.keys_open, 1.)),
        (0.1, Keys("?")),
        (
            0.1,
            Until(
                "? typed in the reply box is just a ?",
                |d, cx| !d.keys_open && composer_is(d, cx, "?"),
                1.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
    ]);
    v
}

fn pin_first(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    // The oldest session: the one furthest down.
    if let Some(id) = d.order().last().cloned() {
        MARK.with(|m| m.set(d.sessions.iter().position(|s| s.id == id).unwrap_or(0)));
        d.toggle_pin(&id, cx);
    }
}

fn marked_id(d: &Dent) -> String {
    d.sessions
        .get(mark())
        .map(|s| s.id.clone())
        .unwrap_or_default()
}

/// Pinning keeps a session at the top; a file's Open offers Open with.
fn pins() -> Vec<(f32, Act)> {
    let mut v = Vec::new();
    for t in ["first notes", "second notes"] {
        v.extend(new_session(t));
        v.push((
            0.1,
            Until(leak(format!("“{t}” answers")), |d, _| answered(d), 20.),
        ));
    }
    v.extend([
        (0.2, Call(pin_first)),
        (
            0.2,
            Until(
                "a pinned session moves to the top, under Pinned",
                |d, _| d.order().first() == Some(&marked_id(d)) && d.pinned.len() == 1,
                1.,
            ),
        ),
        (0.3, Shot("pinned")),
        (
            0.8,
            Call(|d, _, cx| {
                let id = marked_id(d);
                d.toggle_pin(&id, cx);
            }),
        ),
        (
            0.2,
            Until(
                "unpinned, it goes back to its section",
                |d, _| d.pinned.is_empty() && d.order().first() != Some(&marked_id(d)),
                1.,
            ),
        ),
    ]);
    v.extend(new_session("write about.md please"));
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
                "the file opens with Open and Open with beside it",
                |d, _| drawn(d, "open-doc").is_some() && drawn(d, "open-doc-with").is_some(),
                10.,
            ),
        ),
        (0.3, Shot("open-with")),
    ]);
    v
}

/// Something that went wrong can be tried again, unless a used-up limit
/// means it would only fail again.
fn failures() -> Vec<(f32, Act)> {
    let mut v = new_session("crash halfway please");
    v.extend([
        (
            0.1,
            Until(
                "when Claude Code stops unexpectedly, the note offers Try again",
                |d, _| {
                    has(d, |i| matches!(i, Item::Note { icon: "circle-alert", text } if text.contains("stopped unexpectedly")))
                        && (drawn(d, "retry-1").is_some() || drawn(d, "retry-2").is_some())
                },
                15.,
            ),
        ),
    ]);
    v.extend(new_session("hit the limit now"));
    v.extend([
        (
            0.1,
            Until(
                "a used-up limit says which and when it resets, and Try again waits for it",
                |d, _| {
                    !current(d).is_some_and(|s| s.running())
                        && d.limit_hit().is_some_and(|(w, _)| w == "5-hour limit")
                        && has(d, |i| {
                            matches!(
                                i,
                                Item::Note {
                                    icon: "circle-alert",
                                    ..
                                }
                            )
                        })
                },
                10.,
            ),
        ),
        (0.4, Shot("limit")),
        (0.8, Wait),
    ]);
    v
}

fn queued_ids(d: &Dent) -> Vec<(usize, String, String)> {
    current(d).map_or(Vec::new(), |s| {
        s.items
            .iter()
            .enumerate()
            .filter_map(|(ix, i)| match i {
                Item::Queued { id, text } => Some((ix, id.clone(), text.clone())),
                _ => None,
            })
            .collect()
    })
}

fn sent_order(d: &Dent) -> Vec<String> {
    current(d).map_or(Vec::new(), |s| {
        s.items
            .iter()
            .filter_map(|i| match i {
                Item::User { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    })
}

/// Waiting messages can be put in a different order before they go.
/// How many chat paragraphs contain `needle`, the one streaming included.
fn times_said(d: &Dent, needle: &str) -> usize {
    current(d).map_or(0, |s| {
        s.items
            .iter()
            .filter(|i| matches!(i, Item::Text(t) if t.contains(needle)))
            .count()
    })
}

fn detail_is(d: &Dent, text: &str) -> bool {
    current(d).is_some_and(|s| s.detail == text)
}

/// A turn that talks between calls: each paragraph shows once, the
/// next streams on its own, and the working line says what's going on
/// while a long file is written out.
fn narration() -> Vec<(f32, Act)> {
    const FIRST: &str = "First I'll read the handler";
    const SECOND: &str = "Now I'll write up";
    let mut v = new_session("Narrate as you go");
    v.extend([
        (
            0.1,
            Until(
                "the first paragraph streams",
                |d, _| says(d, FIRST) && status_is(d, Status::Working),
                15.,
            ),
        ),
        (
            0.,
            Until(
                "the second streams on its own, and the first shows once",
                |d, _| {
                    current(d).is_some_and(
                        |s| matches!(s.items.last(), Some(Item::Text(t)) if t.starts_with(SECOND)),
                    ) && times_said(d, FIRST) == 1
                },
                15.,
            ),
        ),
        (
            0.,
            Until(
                "writing the file, the line names it",
                |d, _| detail_is(d, "Writing handler-notes.md"),
                10.,
            ),
        ),
        (0., Shot("narrate-writing")),
        (
            0.,
            Until(
                "nothing streamed shows twice meanwhile",
                |d, _| times_said(d, FIRST) == 1 && times_said(d, SECOND) == 1,
                0.1,
            ),
        ),
        (
            0.1,
            Until("it finishes", |d, _| done_saying(d, "All done"), 20.),
        ),
        (
            0.,
            Until(
                "every paragraph once, none run together",
                |d, _| {
                    times_said(d, FIRST) == 1
                        && times_said(d, SECOND) == 1
                        && times_said(d, "All done") == 1
                        && times_said(d, "arrive.Now") == 0
                },
                0.1,
            ),
        ),
    ]);
    v.extend(new_session("Research it in parallel"));
    v.extend([
        (
            0.,
            Until(
                "a background subagent stays at work after the turn ends",
                |d, _| {
                    status_is(d, Status::Waiting)
                        && has(d, |i| {
                            matches!(i, Item::Tools { summary, .. }
                                if summary.starts_with("Subagent working: Survey competitors"))
                        })
                },
                15.,
            ),
        ),
        (
            0.,
            Until(
                "and the line says what it's doing",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.detail == "Subagent: Survey competitors · Searched the web for Monarch free trial"
                    })
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "once done, its report is what it found, not the launch receipt",
                |d, _| {
                    done_saying(d, "The research is in")
                        && has(d, |i| {
                            matches!(i, Item::Tools { summary, lines }
                                if summary.starts_with("Subagent: Survey competitors")
                                    && lines.iter().any(|l| l.report.as_deref() == Some("Monarch starts everyone on a trial before linking."))
                                    && !lines.iter().any(|l| l.report.as_deref().is_some_and(|r| r.contains("Async"))))
                        })
                },
                20.,
            ),
        ),
    ]);
    v.extend(new_session("Use a subagent"));
    v.push((
        0.,
        Until(
            "a subagent's latest step shows beside its task",
            |d, _| {
                current(d)
                    .is_some_and(|s| s.detail.starts_with("Subagent: Survey the tests · Read "))
            },
            10.,
        ),
    ));
    v
}

thread_local! {
    static MARKED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn files_attached(d: &Dent) -> Vec<String> {
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
fn file_links() -> Vec<(f32, Act)> {
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
        (0.2, RightClick("tool-row")),
        (
            0.1,
            Until(
                "right-clicking a file's tool row offers its path",
                |d, _| menu_has(d, "Copy Path"),
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
                let body: String = (1..=80).map(|n| format!("// line {n}\n")).collect();
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
    ]);
    v
}

fn attaching() -> Vec<(f32, Act)> {
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
                "⌘K in your terminal clears it, and search stays shut",
                |d, _| {
                    d.palette.is_none()
                        && current(d)
                            .and_then(|s| d.shells.get(&s.id))
                            .is_some_and(|sh| !sh.text().contains("my notes.txt"))
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
            Call(|d, w, cx| d.composer.update(cx, |c, cx| c.focus(w, cx))),
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

fn queue_order() -> Vec<(f32, Act)> {
    let mut v = new_session("a long answer please");
    v.extend([
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "first later")),
        (0.1, Key("enter")),
        (0.2, Type(Field::Composer, "second later")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "two wait, and only the second can move up",
                |d, _| {
                    let q = queued_ids(d);
                    q.len() == 2
                        && drawn(d, &format!("move-up-{}", q[1].0)).is_some()
                        && drawn(d, &format!("move-up-{}", q[0].0)).is_none()
                },
                5.,
            ),
        ),
        (
            0.,
            Call(|d, _, cx| {
                let q = queued_ids(d);
                let sid = d.selected.clone().unwrap_or_default();
                if q.len() == 2 {
                    d.requeue(&sid, &q[1].1, Some(q[0].1.clone()), cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "Move up puts it first",
                |d, _| {
                    let q = queued_ids(d);
                    q.len() == 2 && q[0].2 == "second later"
                },
                3.,
            ),
        ),
        (0., Shot("queue")),
        (
            0.1,
            Until(
                "and they go in that order",
                |d, _| {
                    let o = sent_order(d);
                    status_is(d, Status::Done)
                        && queued_ids(d).is_empty()
                        && o.ends_with(&["second later".to_string(), "first later".to_string()])
                },
                40.,
            ),
        ),
    ]);
    v
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

fn diff_paths(d: &Dent) -> Vec<String> {
    current(d).map_or(Vec::new(), |s| {
        s.diff.iter().map(|f| f.path.clone()).collect()
    })
}

fn review(d: &mut Dent, turn: usize, path: &str, cx: &mut Context<Dent>) {
    let sid = d.selected.clone().unwrap_or_default();
    let t = my_ids(d).get(turn).cloned();
    d.review_turn(&sid, t, Some(path.to_string()), cx);
}

fn git_in_project(args: &[&str]) {
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(in_project(""))
        .args(["-c", "user.name=walk", "-c", "user.email=walk@example.com"])
        .args(args)
        .output();
}

const TWELVE: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n";

/// One turn's changes on their own, and putting back a file or a hunk.
fn turn_changes() -> Vec<(f32, Act)> {
    let mut v = new_session("write turnone.md please");
    v.extend([
        (0.1, Until("asks to write", |d, _| has(d, |i| matches!(i, Item::Approval { .. })), 10.)),
        (0.2, Click("allow")),
        (0.1, Until("the first turn writes", |d, _| status_is(d, Status::Done) && in_project("turnone.md").exists(), 10.)),
        (0.2, Type(Field::Composer, "write turntwo.md please")),
        (0.1, Key("enter")),
        (0.1, Until("asks again", |d, _| has(d, |i| matches!(i, Item::Approval { .. })), 10.)),
        (0.2, Click("allow")),
        (0.1, Until("the second turn writes", |d, _| status_is(d, Status::Done) && in_project("turntwo.md").exists(), 10.)),
        (0.2, Call(|d, _, cx| review(d, 1, "turntwo.md", cx))),
        (
            0.1,
            Until(
                "a turn's Review shows only what that turn changed",
                |d, _| d.changes_turn.len() == 1 && diff_paths(d) == ["turntwo.md"] && drawn(d, "all-changes").is_some(),
                5.,
            ),
        ),
        (0.3, Shot("turn-changes")),
        (0.8, Click("discard-file")),
        (0.1, Until("Discard asks first", |d, _| drawn(d, "discard-file-yes").is_some(), 1.)),
        (0.3, Shot("discard-ask")),
        (0.8, Click("discard-file-yes")),
        (
            0.1,
            Until(
                "discarding puts it back, and with the turn's changes gone Changes shows everything",
                |d, _| {
                    !in_project("turntwo.md").exists()
                        && in_project("turnone.md").exists()
                        && d.changes_turn.is_empty()
                        && diff_paths(d).contains(&"turnone.md".to_string())
                },
                5.,
            ),
        ),
        (0.2, Call(|d, _, cx| review(d, 0, "turnone.md", cx))),
        (
            0.1,
            Until(
                "an older turn's changes can't be discarded from its own view",
                |d, _| d.changes_turn.len() == 1 && drawn(d, "discard-file").is_some(),
                5.,
            ),
        ),
        (0.2, Click("discard-file")),
        (
            0.3,
            Until(
                "and pressing it does nothing",
                |d, _| d.discarding.is_none() && in_project("turnone.md").exists(),
                1.,
            ),
        ),
        // A committed file with two changes far apart: one hunk goes back.
        (
            0.2,
            Call(|_, _, _| {
                let _ = std::fs::write(in_project("twelve.txt"), TWELVE);
                git_in_project(&["add", "twelve.txt"]);
                git_in_project(&["commit", "-qm", "twelve"]);
                let _ = std::fs::write(
                    in_project("twelve.txt"),
                    TWELVE.replace("2\n3", "TWO\n3").replace("11\n", "ELEVEN\n"),
                );
            }),
        ),
        (0.2, Call(|d, _, cx| {
            let sid = d.selected.clone().unwrap_or_default();
            d.review_turn(&sid, None, Some("twelve.txt".into()), cx);
        })),
        (
            0.1,
            Until(
                "all changes show the file's two hunks",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.diff.iter().any(|f| {
                            f.path == "twelve.txt"
                                && f.lines.iter().filter(|l| l.kind == crate::data::Line::Hunk).count() == 2
                        })
                    })
                },
                5.,
            ),
        ),
        (
            0.2,
            Call(|d, _, cx| {
                let sid = d.selected.clone().unwrap_or_default();
                d.discarding = Some((sid, "twelve.txt".into(), Some(1)));
                cx.notify();
            }),
        ),
        (0.2, Click("discard-hunk-yes-1")),
        (
            0.1,
            Until(
                "discarding one hunk puts back just that change",
                |_, _| {
                    std::fs::read_to_string(in_project("twelve.txt"))
                        .is_ok_and(|s| s.contains("TWO") && !s.contains("ELEVEN") && s.contains("11\n"))
                },
                5.,
            ),
        ),
    ]);
    v
}

/// The chrome's motion, for a recording of the window: a first tooltip,
/// the next one while warm, the palette, a menu, a session's rows arriving,
/// a toast and a fold.
fn motion() -> Vec<(f32, Act)> {
    let mut v: Vec<(f32, Act)> = vec![
        (
            0.,
            Until("connects to the daemon", |d, _| d.connection.is_ok(), 15.),
        ),
        (1.0, Hover("new-session", 0., 0.)),
        (1.5, Hover("new-session", -crate::ds::size::CONTROL, 0.)),
        (1.2, Hover("starter", 0., 0.)),
        (1.0, Key("cmd-k")),
        (1.0, Key("escape")),
        (0.8, Click("chip-model")),
        (1.0, Key("escape")),
    ];
    v.extend(new_session("Fix the footer on small screens"));
    v.extend([
        (
            0.1,
            Until("the agent finishes", |d, _| status_is(d, Status::Done), 30.),
        ),
        (1.0, Key("cmd-w")),
        (1.5, Click("closed-toggle")),
        (1.0, Click("closed-toggle")),
        (1.0, Wait),
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
        (0.2, Click("fork")),
        (0.1, Until("a fork opens", |d, _| current(d).is_some_and(|s| s.title.starts_with("Fork of")), 10.)),
        (0.2, Type(Field::Composer, "Which drink did I pick earlier in this conversation? One word.")),
        (0.2, Key("enter")),
        (0.1, Until("the fork remembers the conversation it came from", |d, _| done(d) && says(d, "Tea"), 90.)),
        (0.5, Shot("real-fork")),
        // Plan mode: a plan to approve, then the work.
        (0.3, Key("cmd-n")),
        (0.2, Call(use_project)),
        (0.1, Call(|d, _, cx| {
            d.defaults.model = Some("haiku".into());
            d.defaults.permission = dent_protocol::Permission::Plan;
            cx.notify();
        })),
        (0.2, Type(Field::Starter, "Plan how to add a file plan.txt containing the word yes. Keep the plan to two short lines, then present it.")),
        (0.2, Click("start")),
        (0.1, Until("in plan mode it proposes a plan", |d, _| has(d, |i| matches!(i, Item::Plan { .. })), 90.)),
        (0.5, Shot("real-plan")),
        (0.2, Click("plan-go")),
        (0.1, Until("approving the plan lets it work; it asks before writing", |d, _| has(d, |i| matches!(i, Item::Approval { .. })) || in_project("plan.txt").exists(), 90.)),
        (0.2, ClickIfShown("allow")),
        (0.1, Until("the planned file is written", |d, _| done(d) && in_project("plan.txt").exists(), 90.)),
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
        (0.3, Keys("redis")),
        (
            0.1,
            Until(
                "search finds words inside a conversation",
                |d, cx| {
                    d.palette_results(cx)
                        .iter()
                        .any(|id| d.session(id).is_some_and(|s| s.title.contains("cache")))
                },
                3.,
            ),
        ),
        (0.6, Shot("search")),
        (0.2, Key("escape")),
    ]);
    v
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
