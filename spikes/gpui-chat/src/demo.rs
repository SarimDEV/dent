//! Scripted scenes of the Field Notes micro-interactions, for screenshots and
//! clips. Time-based, so recordings play at real speed.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::*;

use crate::fake::Row;
use crate::theme::Status;
use crate::{Spike, WINDOW};

#[derive(Clone)]
enum Act {
    User(&'static str),
    Tool(&'static str, &'static str, &'static str, bool),
    /// Start streaming this reply, a word at a time.
    Stream(&'static str),
    /// Finish the stream: the text becomes a normal message.
    EndStream,
    Approval(&'static str),
    Press,
    /// The pressed approval resolves into a one-line tool call.
    Resolve(&'static str),
    /// A tool starts running: name, detail, expected ms.
    Run(&'static str, &'static str, f32),
    /// The running tool finishes with this result.
    Finish(&'static str),
    Usage(u32),
    Status(Status),
    Blocked(usize),
    /// A trackpad flick into history: wheel deltas that decay like momentum.
    Flick(f32),
    Live,
    Theme,
    Fps(bool),
    Still(&'static str),
}

pub struct Demo {
    name: String,
    timeline: Vec<(f32, Act)>,
    length: f32,
    next: usize,
    start: Option<Instant>,
    record: Option<PathBuf>,
    stream_words: Vec<&'static str>,
    stream_ix: usize,
    stream_next: f32,
    flick_v: f32,
    approval_row: Option<usize>,
    /// Draw times (ms) of every frame in the scene, reported on exit.
    collector: gpui_kit::profiler::FrameTimingCollector,
    draws: Vec<f64>,
}

const WORDS_PER_SEC: f32 = 16.;
/// Seconds of "Thinking" before a reply's first word.
const THINK: f32 = 0.9;
/// Seconds the recorder runs before the scene clock starts.
pub const LEAD: f32 = 1.5;
/// Seconds the window stays open after the scene when recording.
pub const HOLD: f32 = 1.5;

fn scene(name: &str) -> (Vec<(f32, Act)>, f32) {
    use Act::*;
    match name {
        "stream" => (
            vec![
                (
                    0.6,
                    User(
                        "Compact the event log once a session passes 50k events, and keep search working.",
                    ),
                ),
                (0.7, Status(crate::theme::Status::Working)),
                (
                    1.5,
                    Tool("Read", "crates/daemon/src/event_log.rs", "412 lines", true),
                ),
                (2.0, Tool("Grep", "\"fts5\" crates/", "9 matches", true)),
                (
                    2.6,
                    Stream(
                        "Compaction runs in the writer task, so reads never block. Deltas older than the last checkpoint fold into their final message events, and the FTS5 index is external-content, so search keeps working while the old rows are rewritten. I'll add a size trigger at 50,000 events and a test that replays a long session through it.",
                    ),
                ),
                (7.4, EndStream),
                (
                    7.6,
                    Tool("Edit", "crates/daemon/src/event_log.rs", "+84 −12", true),
                ),
                (
                    8.1,
                    Run("Bash", "cargo test -p dent-daemon compaction", 1800.),
                ),
                (9.7, Finish("41 passed · 1.6 s")),
                (9.8, Status(crate::theme::Status::Done)),
                (9.9, Usage(6)),
            ],
            11.2,
        ),
        "approval" => (
            vec![
                (0.6, Status(crate::theme::Status::Working)),
                (
                    0.7,
                    Tool("Edit", "crates/daemon/src/checkpoint.rs", "+31 −4", true),
                ),
                (1.3, Approval("git push origin compaction")),
                (1.35, Status(crate::theme::Status::Needs)),
                (1.35, Blocked(1)),
                (3.0, Press),
                (3.25, Resolve("git push origin compaction")),
                (3.3, Status(crate::theme::Status::Working)),
                (3.3, Blocked(0)),
                (
                    4.1,
                    Stream(
                        "Pushed. The PR is open and CI has started; I'll watch the checks and tell you if anything fails.",
                    ),
                ),
                (6.6, EndStream),
                (6.7, Status(crate::theme::Status::Done)),
            ],
            7.8,
        ),
        "scroll" => (
            vec![
                (0.2, Fps(true)),
                (0.8, Flick(340.)),
                (3.0, Still("history")),
                (3.6, Flick(300.)),
                (5.6, Live),
            ],
            7.2,
        ),
        "theme" => (
            vec![
                (1.0, Theme),
                (2.6, Still("carbon")),
                (3.2, Theme),
                (4.6, Still("paper")),
            ],
            5.4,
        ),
        // One frame of everything at rest, for stills.
        _ => (vec![(0.8, Still("rest"))], 1.6),
    }
}

impl Demo {
    pub fn new(name: &str, record: Option<PathBuf>) -> Self {
        let (timeline, length) = scene(name);
        Self {
            name: name.to_string(),
            timeline,
            length,
            next: 0,
            start: None,
            record,
            stream_words: Vec::new(),
            stream_ix: 0,
            stream_next: 0.,
            flick_v: 0.,
            approval_row: None,
            collector: gpui_kit::profiler::FrameTimingCollector::new(),
            draws: Vec::new(),
        }
    }
}

pub fn step(app: &mut Spike, window: &mut Window, cx: &mut Context<Spike>) {
    let Some(mut d) = app.demo.take() else { return };
    let start = *d.start.get_or_insert_with(|| {
        // Start the recorder once the window has drawn; the scene clock starts
        // after LEAD so the recording catches the first beat.
        if let Some(path) = d.record.clone() {
            hide_pointer(window, cx);
            record(path, d.length + LEAD);
        }
        Instant::now()
    });
    let lead = if d.record.is_some() { LEAD } else { 0. };
    let t = start.elapsed().as_secs_f32() - lead;
    if std::env::var("DENT_DEBUG_FOLLOW").is_ok() && (t * 10.).fract() < 0.1 {
        eprintln!(
            "t={t:.1} following={} top={:?} rows={}",
            app.chat.is_following_tail(),
            app.chat.logical_scroll_top(),
            app.rows.len()
        );
    }
    if t < 0. {
        app.demo = Some(d);
        window.request_animation_frame();
        return;
    }

    while d.next < d.timeline.len() && d.timeline[d.next].0 <= t {
        let act = d.timeline[d.next].1.clone();
        d.next += 1;
        match act {
            Act::User(text) => app.push_row(Row::User(text.into())),
            Act::Tool(name, detail, result, ok) => app.push_row(Row::Tool {
                name: name.into(),
                detail: detail.into(),
                result: result.into(),
                ok,
            }),
            Act::Stream(text) => {
                app.push_row(Row::Assistant(vec![]));
                app.streaming = Some(String::new());
                app.stream_words.clear();
                d.stream_words = text.split(' ').collect();
                d.stream_ix = 0;
                // A beat of "Thinking" before the first word.
                d.stream_next = t + THINK;
            }
            Act::EndStream => {
                if let Some(s) = app.streaming.take() {
                    let last = app.rows.len() - 1;
                    app.replace_row(last, Row::Assistant(vec![s.into()]));
                    app.appeared.remove(&last);
                    app.stream_words.clear();
                }
            }
            Act::Approval(cmd) => {
                app.push_row(Row::Approval(cmd.into()));
                d.approval_row = Some(app.rows.len() - 1);
            }
            Act::Press => {
                if let Some(ix) = d.approval_row {
                    app.pressed = Some((ix, Instant::now()));
                    app.chat.splice(ix..ix + 1, 1);
                }
            }
            Act::Resolve(cmd) => {
                if let Some(ix) = d.approval_row.take() {
                    app.folding = Some((ix, Instant::now(), cmd.into()));
                }
            }
            Act::Run(name, detail, expected) => {
                app.push_row(Row::Tool {
                    name: name.into(),
                    detail: detail.into(),
                    result: "".into(),
                    ok: true,
                });
                app.running = Some((app.rows.len() - 1, Instant::now(), expected));
            }
            Act::Finish(result) => {
                if let Some((ix, _, _)) = app.running.take()
                    && let Row::Tool { name, detail, .. } = app.rows[ix].clone()
                {
                    app.complete_row(
                        ix,
                        Row::Tool {
                            name,
                            detail,
                            result: result.into(),
                            ok: true,
                        },
                    );
                }
            }
            Act::Usage(pct) => app.usage = pct,
            Act::Status(s) => app.current = s,
            Act::Blocked(n) => app.blocked = n,
            Act::Flick(v) => d.flick_v = v,
            Act::Live => {
                d.flick_v = 0.;
                app.chat.set_follow_mode(FollowMode::Tail);
            }
            Act::Theme => app.switch_theme(),
            Act::Fps(on) => app.show_fps = on,
            Act::Still(name) => {
                if let Some(rec) = &d.record {
                    let theme = if app.theme.dark { "carbon" } else { "paper" };
                    let path = rec.with_file_name(format!("{}-{name}-{theme}.png", d.name));
                    still(path);
                }
            }
        }
    }

    // Streaming: reveal words at a steady reading pace.
    if let Some(s) = app.streaming.as_mut() {
        while d.stream_ix < d.stream_words.len() && d.stream_next <= t {
            if !s.is_empty() {
                s.push(' ');
            }
            let start = s.len();
            s.push_str(d.stream_words[d.stream_ix]);
            app.stream_words.push((start, s.len(), Instant::now()));
            d.stream_ix += 1;
            d.stream_next += 1. / WORDS_PER_SEC;
        }
        let last = app.rows.len() - 1;
        app.chat.splice(last..last + 1, 1);
    }

    // Momentum flick: decaying wheel deltas, like a real trackpad.
    if d.flick_v > 1. {
        crate::bench::wheel(window, cx, point(px(700.), px(420.)), d.flick_v);
        d.flick_v *= 0.955;
    }

    // Keep the window up until well after the recorder stops, so a clip can
    // never run past the window and capture whatever is behind it.
    for ev in d.collector.collect_unseen() {
        if let gpui_kit::profiler::FrameEvent::Draw(f) = ev {
            d.draws.push(f.draw_duration().as_secs_f64() * 1e3);
        }
    }
    let hold = if d.record.is_some() { HOLD } else { 0. };
    if t >= d.length + hold {
        report(&d.name, &mut d.draws);
        cx.quit();
        return;
    }
    app.demo = Some(d);
}

/// Frame draw times for the scene, so every motion is measured (design-system.md, Cost rules).
fn report(name: &str, draws: &mut [f64]) {
    if draws.is_empty() {
        return;
    }
    let slow: Vec<String> = draws
        .iter()
        .enumerate()
        .filter(|(_, m)| **m > 8.3)
        .map(|(i, m)| format!("{i}:{m:.1}"))
        .collect();
    eprintln!("frames over 8.3 ms (index:ms): {slow:?}");
    draws.sort_by(f64::total_cmp);
    let at = |q: f64| draws[((draws.len() - 1) as f64 * q) as usize];
    println!(
        "{{\"scene\":\"{name}\",\"frames\":{},\"p50_ms\":{:.2},\"p95_ms\":{:.2},\"p99_ms\":{:.2},\"max_ms\":{:.2},\"over_8ms\":{}}}",
        draws.len(),
        at(0.5),
        at(0.95),
        at(0.99),
        at(1.),
        draws.iter().filter(|m| **m > 8.3).count()
    );
}

/// GPUI hides the pointer on typing until the mouse moves; one synthetic
/// keypress keeps it out of recordings without touching the real mouse.
fn hide_pointer(window: &mut Window, cx: &mut Context<Spike>) {
    cx.set_cursor_hide_mode(CursorHideMode::OnTyping);
    window.defer(cx, |window, cx| {
        window.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke: Keystroke {
                    modifiers: Modifiers::default(),
                    key: "z".into(),
                    key_char: Some("z".into()),
                },
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
    });
}

/// The whole window, including the title bar.
fn region() -> String {
    let (x, y, w, h) = crate::window();
    format!("{},{},{},{}", x, y, w, h + 28.)
}

/// Record the window region at 60 fps without the mouse pointer.
fn record(path: PathBuf, secs: f32) {
    let (x, y, w, h) = WINDOW;
    let s = 2.; // Retina backing scale
    let crop = format!("crop={}:{}:{}:{}", w * s, (h + 28.) * s, x * s, y * s);
    let _ = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "avfoundation",
            "-capture_cursor",
            "0",
            "-framerate",
            "60",
            "-pixel_format",
            "nv12",
        ])
        .args([
            "-i",
            "Capture screen 0:none",
            "-t",
            &format!("{:.1}", secs + 0.3),
            "-vf",
            &crop,
        ])
        .args([
            "-c:v",
            "h264_videotoolbox",
            "-b:v",
            "40M",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .spawn();
}

pub(crate) fn still(path: PathBuf) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        let _ = std::process::Command::new("screencapture")
            .args(["-x", "-R", &region()])
            .arg(&path)
            .status();
    });
}
