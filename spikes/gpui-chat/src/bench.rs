//! Scripted interaction with frame timing from GPUI's own trace.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::profiler::{FrameEvent, FrameTimingCollector};
use gpui_kit::*;

use crate::fake::{self, Row};
use crate::{GEN_MS, PROCESS_START, Spike, WINDOW};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Let the first frames settle; cold start is measured from process start.
    Warmup,
    /// Re-open the 100k session from scratch (fresh ListState) and draw it.
    Open,
    Shot(&'static str),
    /// Scroll the chat upward from the bottom, 60px per frame.
    ScrollChat,
    /// Jump to far-away, never-measured rows.
    JumpChat,
    /// Stream words into the last message while pinned to the bottom.
    Stream,
    ScrollDiff,
    ScrollSidebar,
    Done,
}

/// Screenshot only: settle, scroll a little so a mix of rows shows, shoot.
const SHOT_PLAN: &[(Phase, u32)] = &[
    (Phase::Warmup, 30),
    (Phase::Shot("chat"), 40),
    (Phase::Done, 1),
];

const PLAN: &[(Phase, u32)] = &[
    (Phase::Warmup, 30),
    (Phase::Shot("chat-bottom"), 40),
    (Phase::Open, 20),
    (Phase::ScrollChat, 600),
    (Phase::Shot("chat-scrolled"), 40),
    (Phase::JumpChat, 120),
    (Phase::Stream, 600),
    (Phase::Shot("streaming"), 40),
    (Phase::ScrollDiff, 600),
    (Phase::Shot("diff-scrolled"), 40),
    (Phase::ScrollSidebar, 300),
    (Phase::Done, 1),
];

pub struct Bench {
    plan: &'static [(Phase, u32)],
    step: usize,
    frame_in_step: u32,
    collector: FrameTimingCollector,
    /// Draw durations (ms) per phase label.
    samples: Vec<(String, Vec<f64>)>,
    first_frame_ms: Option<f64>,
    open: Option<(Instant, Option<f64>)>,
    shots: Option<PathBuf>,
    words: Box<dyn Iterator<Item = &'static str>>,
    jump_rng: fake::Rng,
}

impl Bench {
    pub fn new(shots: Option<PathBuf>, shot_only: bool) -> Self {
        Self {
            plan: if shot_only { SHOT_PLAN } else { PLAN },
            step: 0,
            frame_in_step: 0,
            collector: FrameTimingCollector::new(),
            samples: Vec::new(),
            first_frame_ms: None,
            open: None,
            shots,
            words: Box::new(fake::stream_words(5)),
            jump_rng: fake::Rng::new(99),
        }
    }

    fn label(&self) -> String {
        format!("{:?}", self.plan[self.step].0)
    }

    /// Attribute frames drawn since the last poll to the current phase.
    fn collect(&mut self) {
        let start = *PROCESS_START.get().unwrap_or(&Instant::now());
        let label = self.label();
        for ev in self.collector.collect_unseen() {
            if let FrameEvent::Draw(f) = ev {
                if self.first_frame_ms.is_none() {
                    self.first_frame_ms =
                        Some(f.draw_end.duration_since(start).as_secs_f64() * 1e3);
                }
                if let Some((asked, None)) = self.open
                    && f.draw_start >= asked
                {
                    self.open = Some((
                        asked,
                        Some(f.draw_end.duration_since(asked).as_secs_f64() * 1e3),
                    ));
                }
                let ms = f.draw_duration().as_secs_f64() * 1e3;
                match self.samples.last_mut() {
                    Some((l, v)) if *l == label => v.push(ms),
                    _ => self.samples.push((label.clone(), vec![ms])),
                }
            }
        }
    }
}

pub fn step(app: &mut Spike, window: &mut Window, cx: &mut Context<Spike>) {
    let Some(mut b) = app.bench.take() else {
        return;
    };
    b.collect();
    let (phase, frames) = b.plan[b.step];
    let n = b.frame_in_step;
    match phase {
        Phase::Warmup | Phase::Done => {}
        Phase::Shot(name) => {
            if n == 25
                && let Some(dir) = &b.shots
            {
                let suffix = if app.theme.dark { "carbon" } else { "paper" };
                let fonts = crate::theme::FONTS.get().map(|f| f.name).unwrap_or("book");
                shoot(dir.join(format!("{fonts}-{name}-{suffix}.png")));
            }
        }
        Phase::Open => {
            if n == 0 {
                app.chat = ListState::new(app.rows.len(), ListAlignment::Bottom, px(1200.));
                b.open = Some((Instant::now(), None));
            }
        }
        Phase::ScrollChat => {
            wheel(window, cx, point(px(700.), px(450.)), 60.);
            if n % 200 == 0 {
                eprintln!("chat scroll top: {:?}", app.chat.logical_scroll_top());
            }
        }
        Phase::JumpChat => {
            let ix = b.jump_rng.below(app.rows.len());
            app.chat.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(0.),
            });
        }
        Phase::Stream => {
            if n == 0 {
                app.chat.scroll_to_end();
                app.streaming = Some(String::new());
            }
            if let (Some(s), Some(w)) = (app.streaming.as_mut(), b.words.next()) {
                if !s.is_empty() {
                    s.push(' ');
                }
                s.push_str(w);
            }
            // Only the last row changed: re-measure just that one.
            let last = app.rows.len() - 1;
            app.chat.splice(last..last + 1, 1);
            app.chat.scroll_to_end();
        }
        Phase::ScrollDiff => wheel(window, cx, point(px(1100.), px(450.)), -80.),
        Phase::ScrollSidebar => wheel(window, cx, point(px(130.), px(450.)), -40.),
    }
    b.frame_in_step += 1;
    if b.frame_in_step >= frames {
        b.step += 1;
        b.frame_in_step = 0;
    }
    if b.plan[b.step.min(b.plan.len() - 1)].0 == Phase::Done && b.step >= b.plan.len() - 1 {
        report(&b, app);
        cx.quit();
        return;
    }
    app.bench = Some(b);
    window.request_animation_frame();
}

/// Feed the window a trackpad-style wheel event, exactly as real input arrives.
/// Positive `dy` scrolls content down (towards older messages in the chat).
fn wheel(window: &mut Window, cx: &mut Context<Spike>, position: Point<Pixels>, dy: f32) {
    window.defer(cx, move |window, cx| {
        window.dispatch_event(
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            }),
            cx,
        );
    });
}

fn shoot(path: PathBuf) {
    let (x, y, w, h) = WINDOW;
    // Include the title bar area above the content origin.
    let region = format!("{},{},{},{}", x, y, w, h + 28.);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        let ok = std::process::Command::new("screencapture")
            .args(["-x", "-R", &region])
            .arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        eprintln!(
            "screenshot {} {}",
            path.display(),
            if ok { "ok" } else { "FAILED" }
        );
    });
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        return 0.0;
    }
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn rss_mb() -> f64 {
    let pid = std::process::id().to_string();
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|kb| kb / 1024.0)
        .unwrap_or(0.0)
}

fn report(b: &Bench, app: &Spike) {
    let mut phases = Vec::new();
    for (label, v) in &b.samples {
        if label.starts_with("Shot") {
            continue;
        }
        let mut v = v.clone();
        let over8 = v.iter().filter(|&&x| x > 8.33).count();
        let over16 = v.iter().filter(|&&x| x > 16.0).count();
        phases.push(format!(
            "    {{\"phase\":\"{label}\",\"frames\":{},\"p50_ms\":{:.2},\"p95_ms\":{:.2},\"p99_ms\":{:.2},\"max_ms\":{:.2},\"over_8ms\":{over8},\"over_16ms\":{over16}}}",
            v.len(),
            pct(&mut v, 0.5),
            pct(&mut v, 0.95),
            pct(&mut v, 0.99),
            pct(&mut v, 1.0),
        ));
    }
    let streamed = app
        .streaming
        .as_ref()
        .map(|s| s.split(' ').count())
        .unwrap_or(0);
    let rows = app
        .rows
        .iter()
        .filter(|r| matches!(r, Row::Assistant(_)))
        .count();
    println!(
        "{{\n  \"theme\":\"{}\",\n  \"events\":{},\n  \"assistant_rows\":{rows},\n  \"diff_lines\":{},\n  \"cold_start_to_first_frame_ms\":{:.1},\n  \"of_which_fake_data_ms\":{:.1},\n  \"open_100k_session_ms\":{:.1},\n  \"streamed_words\":{streamed},\n  \"rss_mb\":{:.0},\n  \"phases\":[\n{}\n  ]\n}}",
        if app.theme.dark { "carbon" } else { "paper" },
        app.rows.len(),
        app.diff.len(),
        b.first_frame_ms.unwrap_or(-1.0),
        GEN_MS.get().copied().unwrap_or(0.0),
        b.open.and_then(|(_, d)| d).unwrap_or(-1.0),
        rss_mb(),
        phases.join(",\n")
    );
}
