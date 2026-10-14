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
        Sweep => {
            advance = sweep_step(&me, window, cx);
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

/// A page in a folder outside the project, with a stylesheet and a picture
/// beside it (scripts/walk.sh puts them there).
fn outside_page() -> &'static str {
    let dir = std::env::var("DENT_WALK_OUTSIDE").unwrap_or_else(|_| "/nonexistent".into());
    leak(format!("write {dir}/page.html with assets"))
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
        (0.2, Type(Field::Composer, "look at @read")),
        (
            0.1,
            Until(
                "typing @ offers the project's files",
                |d, cx| d.completions(cx).1.iter().any(|c| c.name == "README.md"),
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
        (0.1, Until("the turn's edits get a Review line", |d, _| has(d, |i| matches!(i, Item::Changed { paths } if paths.iter().any(|p| p == "site/page.html"))), 5.)),
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
            if let Some(ix) = s.items.iter().position(|i| matches!(i, Item::Tools { lines, .. } if lines.iter().any(|l| l.report.is_some()))) {
                d.open_tools.insert((s.id.clone(), ix));
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
