//! Spike: GPUI chat list and diff view at 100,000 events on pinned
//! gpui-component, in the Field Notes design.
//!
//! `just dev spike-gpui-chat [--dark] [--bench] [--events N] [--shots DIR]`
//!
//! `--bench` runs a scripted interaction (open the session, scroll, jump,
//! stream, scroll the diff and the sidebar), records GPUI's own frame timings
//! for each phase, takes screenshots, prints a JSON report and quits.
//! `--demo NAME [--record FILE.mov]` plays a scripted scene of the Field Notes
//! micro-interactions (stream, approval, scroll, theme) and can record it.
//! All data is generated locally: no agents, no usage.

mod anim;
mod bench;
mod demo;
mod devices;
mod ds;
mod fake;
mod specimen;
mod theme;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use ds::{CODE, DISPLAY, DISPLAY_COMPACT, LABEL, READING, Typed, UI, motion, radius, size, space};
use fake::{DiffLine, LineKind, Row, SessionRow};
use gpui_kit::prelude::*;
use gpui_kit::*;
use theme::{Status, Theme, status_square, status_square_at};

pub static PROCESS_START: OnceLock<Instant> = OnceLock::new();
/// Time spent generating fake data, which a real client would not do.
pub static GEN_MS: OnceLock<f64> = OnceLock::new();

pub struct Opts {
    pub dark: bool,
    pub bench: bool,
    pub events: usize,
    pub diff_lines: usize,
    pub shots: Option<std::path::PathBuf>,
    pub fonts: String,
    pub shot_only: bool,
    pub font_dir: Option<std::path::PathBuf>,
    pub demo: Option<String>,
    pub record: Option<std::path::PathBuf>,
    pub fps: bool,
    pub specimen: bool,
    pub stress: bool,
    pub verify: bool,
    pub compare: bool,
    pub devices: bool,
    pub size: Option<(f32, f32)>,
}

fn opts() -> Opts {
    let mut o = Opts {
        dark: false,
        bench: false,
        events: 100_000,
        diff_lines: 100_000,
        shots: None,
        fonts: "book".into(),
        shot_only: false,
        font_dir: None,
        demo: None,
        record: None,
        fps: false,
        specimen: false,
        stress: false,
        verify: false,
        compare: false,
        devices: false,
        size: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dark" => o.dark = true,
            "--bench" => o.bench = true,
            "--events" => o.events = it.next().and_then(|v| v.parse().ok()).unwrap_or(o.events),
            "--diff-lines" => {
                o.diff_lines = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(o.diff_lines)
            }
            "--shots" => o.shots = it.next().map(Into::into),
            "--shot" => {
                o.bench = true;
                o.shot_only = true;
            }
            "--fonts" => o.fonts = it.next().unwrap_or_default(),
            "--font-dir" => o.font_dir = it.next().map(Into::into),
            "--demo" => o.demo = it.next(),
            "--record" => o.record = it.next().map(Into::into),
            "--fps" => o.fps = true,
            "--specimen" => o.specimen = true,
            "--devices" => o.devices = true,
            "--compare" => {
                o.specimen = true;
                o.compare = true;
            }
            "--verify" => {
                o.specimen = true;
                o.stress = true;
                o.verify = true;
            }
            "--stress" => {
                o.specimen = true;
                o.stress = true;
            }
            "--size" => {
                o.size = it.next().and_then(|v| {
                    let (w, h) = v.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                })
            }
            other => eprintln!("ignoring unknown flag {other}"),
        }
    }
    o
}

pub const WINDOW: (f32, f32, f32, f32) = (40., 40., 1360., 780.);

/// `--size` overrides the window size; screenshots follow it.
pub static SIZE: std::sync::OnceLock<(f32, f32)> = std::sync::OnceLock::new();

pub fn window() -> (f32, f32, f32, f32) {
    let (x, y, w, h) = WINDOW;
    let (w, h) = SIZE.get().copied().unwrap_or((w, h));
    (x, y, w, h)
}

pub struct Spike {
    pub theme: Theme,
    pub rows: Rc<Vec<Row>>,
    pub chat: ListState,
    pub diff: Rc<Vec<DiffLine>>,
    pub diff_scroll: UniformListScrollHandle,
    pub sessions: Rc<Vec<SessionRow>>,
    pub sidebar_scroll: UniformListScrollHandle,
    /// Text being streamed into the last assistant row.
    pub streaming: Option<String>,
    /// Byte range and arrival time of each streamed word, for the Stream fade.
    pub stream_words: Vec<(usize, usize, Instant)>,
    pub bench: Option<bench::Bench>,
    pub demo: Option<demo::Demo>,
    /// Rows that appeared recently (Arrive, Resolve, Confirm).
    pub appeared: HashMap<usize, Instant>,
    /// Rows that finished in place: only their square pops (Confirm).
    pub confirmed: HashMap<usize, Instant>,
    /// An approval button that was just pressed (row, when).
    pub pressed: Option<(usize, Instant)>,
    /// An approval folding into a one-line tool call (row, when, command).
    pub folding: Option<(usize, Instant, SharedString)>,
    /// A running tool line (row, started, expected ms), for Progress.
    pub running: Option<(usize, Instant, f32)>,
    /// Status of the open session (sidebar row 0).
    pub current: Status,
    pub blocked: usize,
    pub usage: u32,
    ticks: Ticks,
    /// 0 = full title at the live tail, 1 = compact while reading history.
    title: anim::Spring,
    pub show_fps: bool,
    /// A theme colour tween in flight: started, from, to.
    pub theme_shift: Option<(Instant, Theme, Theme)>,
    /// How much of the Carbon grain shows (0 = Paper's), tweened with the theme.
    carbon: f32,
    pub grain: (Arc<Image>, Arc<Image>),
    last_frame: Instant,
}

struct Ticks {
    events: anim::Tick,
    blocked: anim::Tick,
    usage: anim::Tick,
}

/// How long a row keeps animating after it appears (Arrive + Confirm).
const SETTLE_MS: f32 = 500.;
/// The approval card's fixed height, so Fold can collapse it without measuring.
const CARD_H: f32 = 110.;

impl Spike {
    fn new(o: &Opts) -> Self {
        let t0 = Instant::now();
        let rows = fake::chat(o.events, 7);
        let diff = fake::diff(o.diff_lines, 11);
        let sessions = fake::sessions(1_000, 3);
        GEN_MS.get_or_init(|| t0.elapsed().as_secs_f64() * 1e3);
        eprintln!(
            "generated {} events, {} diff lines in {:?}",
            rows.len(),
            diff.len(),
            t0.elapsed()
        );
        let chat = ListState::new(rows.len(), ListAlignment::Bottom, px(1200.));
        // Chat follows the live tail, stops when you scroll up, and re-engages at the bottom.
        chat.set_follow_mode(FollowMode::Tail);
        let blocked = 2;
        let usage = 5;
        Self {
            theme: if o.dark {
                Theme::carbon()
            } else {
                Theme::paper()
            },
            ticks: Ticks {
                events: anim::Tick::new(group(rows.len())),
                blocked: anim::Tick::new(blocked.to_string()),
                usage: anim::Tick::new(format!("{usage}%")),
            },
            rows: Rc::new(rows),
            chat,
            diff: Rc::new(diff),
            diff_scroll: UniformListScrollHandle::new(),
            sessions: Rc::new(sessions),
            sidebar_scroll: UniformListScrollHandle::new(),
            streaming: None,
            stream_words: Vec::new(),
            demo: o
                .demo
                .as_deref()
                .map(|name| demo::Demo::new(name, o.record.clone())),
            appeared: HashMap::new(),
            confirmed: HashMap::new(),
            pressed: None,
            folding: None,
            running: None,
            current: Status::Done,
            blocked,
            usage,
            title: anim::Spring::snappy(0.),
            show_fps: o.fps,
            theme_shift: None,
            carbon: if o.dark { 1. } else { 0. },
            grain: (
                Arc::new(Image::from_bytes(
                    ImageFormat::Png,
                    include_bytes!("../../../assets/textures/grain-paper.png").to_vec(),
                )),
                Arc::new(Image::from_bytes(
                    ImageFormat::Png,
                    include_bytes!("../../../assets/textures/grain-carbon.png").to_vec(),
                )),
            ),
            last_frame: Instant::now(),
            bench: o
                .bench
                .then(|| bench::Bench::new(o.shots.clone(), o.shot_only)),
        }
    }

    /// Append a row at the live tail; it arrives (or resolves, for approvals).
    pub fn push_row(&mut self, row: Row) {
        let ix = self.rows.len();
        Rc::make_mut(&mut self.rows).push(row);
        self.chat.splice(ix..ix, 1);
        self.appeared.insert(ix, Instant::now());
    }

    /// Replace a row in place (e.g. an approval folding into a tool line).
    pub fn replace_row(&mut self, ix: usize, row: Row) {
        Rc::make_mut(&mut self.rows)[ix] = row;
        self.chat.splice(ix..ix + 1, 1);
        self.appeared.insert(ix, Instant::now());
    }

    /// Finish a row in place: no arrival, just the Confirm pop.
    pub fn complete_row(&mut self, ix: usize, row: Row) {
        Rc::make_mut(&mut self.rows)[ix] = row;
        self.chat.splice(ix..ix + 1, 1);
        self.confirmed.insert(ix, Instant::now());
    }

    /// Switch Paper and Carbon with a colour tween. Switching again mid-tween
    /// retargets from the colours on screen.
    pub fn switch_theme(&mut self) {
        let to_dark = match &self.theme_shift {
            Some((_, _, to)) => !to.dark,
            None => !self.theme.dark,
        };
        let to = if to_dark {
            Theme::carbon()
        } else {
            Theme::paper()
        };
        self.theme_shift = Some((Instant::now(), self.theme, to));
    }

    fn animating(&self) -> bool {
        self.appeared
            .values()
            .chain(self.confirmed.values())
            .any(|s| anim::progress(*s, SETTLE_MS) < 1.)
            || self.theme_shift.is_some()
            || self.pressed.is_some()
            || self.folding.is_some()
            || self.running.is_some()
            || self.streaming.is_some()
            || self.ticks.events.animating()
            || self.ticks.blocked.animating()
            || self.ticks.usage.animating()
    }

    fn at_live_tail(&self) -> bool {
        self.chat.is_following_tail()
    }

    /// Advance springs and one-shot motions; returns true while anything moves.
    fn advance(&mut self) -> bool {
        let dt = self.last_frame.elapsed().as_secs_f32().min(0.05);
        self.last_frame = Instant::now();
        let target = if self.at_live_tail() { 0. } else { 1. };
        let title_moving = self.title.step(target, dt);
        self.appeared
            .retain(|_, s| anim::progress(*s, SETTLE_MS) < 1.);
        self.confirmed
            .retain(|_, s| anim::progress(*s, SETTLE_MS) < 1.);
        if let Some((_, s)) = self.pressed
            && s.elapsed() > Duration::from_millis(600)
        {
            self.pressed = None;
        }
        // Fold finished: the card becomes a tool line, which arrives.
        if let Some((ix, s, cmd)) = self.folding.clone()
            && anim::progress(s, motion::FAST * 0.5 + motion::SLOW) >= 1.
        {
            self.folding = None;
            self.replace_row(
                ix,
                Row::Tool {
                    name: "Bash".into(),
                    detail: cmd,
                    result: "allowed once".into(),
                    ok: true,
                },
            );
        }
        if let Some((s, from, to)) = self.theme_shift {
            let k = anim::ease_out(anim::progress(s, motion::THEME));
            self.theme = Theme::mix(&from, &to, k);
            let (a, b) = (from.dark as u8 as f32, to.dark as u8 as f32);
            self.carbon = a + (b - a) * k;
            if k >= 1. {
                self.theme = to;
                self.theme_shift = None;
            }
        }
        // Rows mid-motion are remeasured each frame; settled rows stay cached.
        let moving: Vec<usize> = self
            .appeared
            .keys()
            .chain(self.confirmed.keys())
            .copied()
            .chain(self.pressed.map(|p| p.0))
            .chain(self.folding.as_ref().map(|f| f.0))
            .chain(self.running.map(|r| r.0))
            .collect();
        for ix in moving {
            if ix < self.rows.len() {
                self.chat.splice(ix..ix + 1, 1);
            }
        }
        self.ticks.events.set(group(self.rows.len()));
        self.ticks.blocked.set(self.blocked.to_string());
        self.ticks.usage.set(format!("{}%", self.usage));
        title_moving || self.animating()
    }

    fn header(&self) -> Div {
        div()
            .h(px(size::HEADER))
            .flex_none()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(self.theme.hairline)
    }

    fn sidebar(&self) -> impl IntoElement {
        let t = self.theme;
        let sessions = self.sessions.clone();
        let current = self.current;
        div()
            .w(px(260.))
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.hairline)
            .child(
                self.header()
                    .px(px(space::S4))
                    .justify_between()
                    .ty(LABEL)
                    .child(div().text_color(t.quiet).child("PROJECTS"))
                    .child(
                        div()
                            .text_color(t.faint)
                            .child(format!("{} sessions", group(sessions.len()))),
                    ),
            )
            .child(
                uniform_list("sessions", sessions.len(), move |range, _, _| {
                    range
                        .map(|i| {
                            let s = &sessions[i];
                            div()
                                .id(i)
                                .w_full()
                                .overflow_hidden()
                                .h(px(size::ROW))
                                .px(px(space::S4))
                                .flex()
                                .items_center()
                                .gap(px(space::S2))
                                .when(i == 0, |d| d.bg(t.selected()))
                                .hover(|d| d.bg(t.hover()))
                                .child(status_square(if i == 0 { current } else { s.status }, &t))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .ty(UI)
                                        .text_color(t.ink)
                                        .child(s.title.clone()),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .ty(LABEL)
                                        .text_color(t.faint)
                                        .child(s.machine.clone()),
                                )
                        })
                        .collect()
                })
                .track_scroll(&self.sidebar_scroll)
                .flex_1(),
            )
            .child(
                div()
                    .px(px(space::S4))
                    .py(px(space::S3))
                    .border_t_1()
                    .border_color(t.hairline)
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .ty(LABEL)
                    .child(
                        div()
                            .text_color(t.quiet)
                            .pb(px(space::S1))
                            .child("MACHINES"),
                    )
                    .child(machine_row("laptop", Status::Done, "connected", &t))
                    .child(machine_row("mac-mini", Status::Working, "connected", &t))
                    .child(machine_row("ec2-runner", Status::Idle, "asleep 12m", &t)),
            )
    }

    fn chat(&self) -> impl IntoElement {
        let t = self.theme;
        let rows = self.rows.clone();
        let last = rows.len().saturating_sub(1);
        let stream: Option<SharedString> = self.streaming.as_ref().map(|s| s.clone().into());
        let words: Rc<Vec<(usize, usize, f32)>> = Rc::new(
            self.stream_words
                .iter()
                .map(|(a, b, s)| (*a, *b, s.elapsed().as_secs_f32() * 1000.))
                .filter(|w| w.2 < motion::BASE)
                .collect(),
        );
        // Row effects are captured by value so the list closure stays 'static.
        let appeared: HashMap<usize, f32> = self
            .appeared
            .iter()
            .map(|(ix, s)| (*ix, s.elapsed().as_secs_f32() * 1000.))
            .collect();
        let confirmed: HashMap<usize, f32> = self
            .confirmed
            .iter()
            .map(|(ix, s)| (*ix, s.elapsed().as_secs_f32() * 1000.))
            .collect();
        let pressed = self
            .pressed
            .map(|(ix, s)| (ix, s.elapsed().as_secs_f32() * 1000.));
        let folding = self
            .folding
            .as_ref()
            .map(|(ix, s, _)| (*ix, s.elapsed().as_secs_f32() * 1000.));
        let running = self
            .running
            .map(|(ix, s, exp)| (ix, s.elapsed().as_secs_f32() * 1000., exp));
        // The serif title compacts while reading history; the live pill arrives.
        let k = self.title.value().clamp(0., 1.);
        let title = lerp_type(DISPLAY, DISPLAY_COMPACT, k);
        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .min_w_0()
            .child(
                self.header()
                    .px(px(space::S8))
                    .gap(px(space::S3))
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap(px(space::S3))
                            .child(
                                div()
                                    .ty(title)
                                    .text_color(t.ink)
                                    .child("Event log compaction"),
                            )
                            .child(
                                div()
                                    .pb(px(5. - 2. * k))
                                    .flex()
                                    .gap(px(space::S1H))
                                    .ty(LABEL)
                                    .text_color(t.quiet)
                                    .child("claude")
                                    .child(div().text_color(t.faint).child("·"))
                                    .child("mac-mini")
                                    .child(div().text_color(t.faint).child("·"))
                                    .child(self.ticks.events.render(LABEL, t.quiet))
                                    .child("events"),
                            ),
                    )
                    .child(div().flex_1())
                    .when(k > 0.01, |d| {
                        d.child(
                            div()
                                .relative()
                                .top(px((1. - k) * motion::NUDGE))
                                .opacity(k)
                                .h(px(size::CONTROL_SM))
                                .px(px(space::S2))
                                .rounded(px(radius::SM))
                                .bg(t.raised)
                                .border_1()
                                .border_color(t.hairline)
                                .flex()
                                .items_center()
                                .gap(px(space::S1H))
                                .ty(LABEL)
                                .text_color(t.ink)
                                .child(status_square(self.current, &t))
                                .child("live ↓"),
                        )
                    }),
            )
            .child(
                list(self.chat.clone(), move |i, _, _| {
                    let row = &rows[i];
                    let fx = RowFx {
                        stream: if i == last { stream.clone() } else { None },
                        words: words.clone(),
                        age: appeared.get(&i).copied(),
                        confirmed: confirmed.get(&i).copied(),
                        pressed: pressed.and_then(|(ix, ms)| (ix == i).then_some(ms)),
                        fold: folding.and_then(|(ix, ms)| (ix == i).then_some(ms)),
                        running: running.and_then(|(ix, ms, exp)| (ix == i).then_some((ms, exp))),
                    };
                    chat_row(row, fx, &t).into_any_element()
                })
                .flex_1(),
            )
            .child(composer(&t))
    }

    fn diff_panel(&self) -> impl IntoElement {
        let t = self.theme;
        let lines = self.diff.clone();
        div()
            .w(px(480.))
            .h_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .border_l_1()
            .border_color(t.hairline)
            .child(
                self.header()
                    .px(px(space::S4))
                    .justify_between()
                    .child(
                        div()
                            .ty(CODE)
                            .text_color(t.ink)
                            .child("crates/daemon/src/event_log.rs"),
                    )
                    .child(
                        div()
                            .ty(LABEL)
                            .text_color(t.faint)
                            .child(format!("{} lines", group(lines.len()))),
                    ),
            )
            .child(
                uniform_list("diff", lines.len(), move |range, _, _| {
                    range
                        .map(|i| {
                            let l = &lines[i];
                            let (bg, bar, sign) = match l.kind {
                                LineKind::Added => (Some(t.added), Some(t.done.0), "+"),
                                LineKind::Removed => (Some(t.removed), Some(t.needs.0), "−"),
                                LineKind::Context => (None, None, " "),
                            };
                            let num = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
                            div()
                                .id(i)
                                .h(px(size::LINE))
                                .flex()
                                .items_center()
                                .ty(CODE)
                                .when_some(bg, |d, bg| d.bg(bg))
                                .child(div().w(px(2.)).h_full().when_some(bar, |d, bar| d.bg(bar)))
                                .child(
                                    div()
                                        .w(px(40.))
                                        .pr(px(space::S2))
                                        .flex()
                                        .justify_end()
                                        .text_color(t.faint)
                                        .child(num(l.old)),
                                )
                                .child(
                                    div()
                                        .w(px(40.))
                                        .pr(px(space::S2))
                                        .flex()
                                        .justify_end()
                                        .text_color(t.faint)
                                        .child(num(l.new)),
                                )
                                .child(div().w(px(space::S4)).text_color(t.faint).child(sign))
                                .child(
                                    div()
                                        .flex_1()
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .text_color(t.ink)
                                        .child(l.text.clone()),
                                )
                        })
                        .collect()
                })
                .track_scroll(&self.diff_scroll)
                .flex_1(),
            )
    }

    fn status_bar(&self) -> impl IntoElement {
        let t = self.theme;
        let item = || div().flex().items_center().gap(px(space::S1H));
        div()
            .h(px(size::STATUS))
            .flex_none()
            .px(px(space::S4))
            .flex()
            .items_center()
            .gap(px(space::S6))
            .border_t_1()
            .border_color(t.hairline)
            .ty(LABEL)
            .text_color(t.quiet)
            .child(
                item()
                    .child(status_square(Status::Working, &t))
                    .child("claude 5h")
                    .child(meter(self.usage as f32 / 100., &t))
                    .child(self.ticks.usage.render(LABEL, t.quiet))
                    .child(div().text_color(t.faint).child("·"))
                    .child("week 11%"),
            )
            .child(
                item()
                    .child(status_square(Status::Waiting, &t))
                    .child("codex 30d")
                    .child(meter(0.01, &t))
                    .child("1%"),
            )
            .child(div().flex_1())
            .child(
                item()
                    .child(status_square(
                        if self.blocked > 0 {
                            Status::Needs
                        } else {
                            Status::Done
                        },
                        &t,
                    ))
                    .child(self.ticks.blocked.render(LABEL, t.quiet))
                    .child("blocked"),
            )
    }
}

fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn lerp_type(a: ds::Type, b: ds::Type, k: f32) -> ds::Type {
    ds::Type {
        family: a.family,
        size: a.size + (b.size - a.size) * k,
        line: a.line + (b.line - a.line) * k,
    }
}

/// A 32 × 4 usage meter.
fn meter(frac: f32, t: &Theme) -> impl IntoElement {
    div()
        .w(px(32.))
        .h(px(4.))
        .bg(t.hairline)
        .child(div().h_full().w(px(32. * frac.clamp(0.04, 1.))).bg(t.quiet))
}

fn machine_row(
    name: &'static str,
    status: Status,
    state: &'static str,
    t: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(space::S2))
        .text_color(t.quiet)
        .child(status_square(status, t))
        .child(div().flex_1().child(name))
        .child(div().text_color(t.faint).child(state))
}

/// Per-row animation state, captured by value.
pub struct RowFx {
    /// Text streaming into this row, if it is the live tail.
    pub stream: Option<SharedString>,
    /// Recently streamed words: byte range and age in ms.
    pub words: Rc<Vec<(usize, usize, f32)>>,
    /// Ms since the row appeared.
    pub age: Option<f32>,
    /// Ms since the row finished in place.
    pub confirmed: Option<f32>,
    /// Ms since the approval's primary button was pressed.
    pub pressed: Option<f32>,
    /// Ms since the approval started folding.
    pub fold: Option<f32>,
    /// Running tool: ms elapsed and ms expected.
    pub running: Option<(f32, f32)>,
}

fn chat_row(row: &Row, fx: RowFx, t: &Theme) -> Div {
    let el = chat_row_inner(row, &fx, t);
    let Some(age) = fx.age else { return el };
    match row {
        // Approvals are rare and important: they Resolve through the dither.
        Row::Approval(_) => el.relative().child(anim::dissolve(
            anim::ease_out(age / motion::SLOW),
            t.surface,
            3.,
        )),
        // Everything else Arrives: fade and a small rise, no relayout.
        _ => {
            let k = anim::ease_out(age / motion::BASE);
            el.relative().top(px((1. - k) * motion::RISE)).opacity(k)
        }
    }
}

fn column(t: &Theme) -> (Div, Div) {
    let _ = t;
    (
        div().w_full().flex().justify_center().px(px(space::S8)),
        div().w_full().max_w(px(size::COLUMN)),
    )
}

fn chat_row_inner(row: &Row, fx: &RowFx, t: &Theme) -> Div {
    let (column, inner) = column(t);
    match row {
        Row::User(text) => column.pt(px(space::S4)).pb(px(space::S2)).child(
            inner.flex().justify_end().child(
                div()
                    .max_w(px(560.))
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .rounded(px(radius::LG))
                    .bg(t.raised)
                    .border_1()
                    .border_color(t.hairline)
                    .ty(READING)
                    .text_color(t.ink)
                    .child(text.clone()),
            ),
        ),
        Row::Assistant(paras) => column.py(px(space::S2)).child(
            inner
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .ty(READING)
                .text_color(t.ink)
                .children(match &fx.stream {
                    Some(s) if s.is_empty() => vec![thinking(t)],
                    Some(s) => vec![streamed(s, &fx.words, t)],
                    None => paras.iter().map(|p| div().child(p.clone())).collect(),
                }),
        ),
        Row::Tool {
            name,
            detail,
            result,
            ok,
        } => {
            // Confirm: the square pops in when the line arrives.
            let pop = fx
                .confirmed
                .or(fx.age)
                .map(|ms| anim::pop(ms / 1000.))
                .unwrap_or(1.);
            column.child(
                inner
                    .relative()
                    .h(px(size::LINE))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .ty(CODE)
                    .text_color(t.quiet)
                    .child(div().text_color(t.faint).child("├─"))
                    .child(div().text_color(t.ink).child(name.clone()))
                    .child(div().flex_1().min_w_0().truncate().child(detail.clone()))
                    .map(|d| match fx.running {
                        // Progress: a hairline fill towards 90% of the expected time, then holds.
                        Some((ms, expected)) => d
                            .child(status_square(Status::Working, t))
                            .child(format_ms(ms))
                            .child(
                                div()
                                    .absolute()
                                    .bottom_0()
                                    .left_0()
                                    .h(px(1.))
                                    .w(relative((ms / expected).min(1.) * 0.9))
                                    .bg(t.working.0),
                            ),
                        None => d
                            .child(status_square_at(
                                if *ok { Status::Done } else { Status::Needs },
                                t,
                                0.6 + 0.4 * pop,
                            ))
                            .child(result.clone()),
                    }),
            )
        }
        Row::Approval(cmd) => {
            // Fold: content fades out, then the card collapses to one line.
            let (fade, collapse) = match fx.fold {
                Some(ms) => (
                    anim::ease_out(ms / motion::FAST),
                    // The collapse overlaps the fade so the card never sits empty.
                    anim::ease_in_out((ms - motion::FAST * 0.5) / motion::SLOW),
                ),
                None => (0., 0.),
            };
            let pad = space::S2 * (1. - collapse);
            column.py(px(pad)).child(
                inner.child(
                    div()
                        .h(px(CARD_H + (size::LINE - CARD_H) * collapse))
                        .overflow_hidden()
                        .rounded(px(radius::LG))
                        .bg(t.wash(t.needs.0).opacity(1. - collapse))
                        .border_1()
                        .border_color(t.edge(t.needs.0).opacity(1. - collapse))
                        .child(
                            div()
                                .opacity(1. - fade)
                                .p(px(space::S3))
                                .flex()
                                .flex_col()
                                .gap(px(space::S2))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(space::S1H))
                                        .ty(LABEL)
                                        .child(status_square(Status::Needs, t))
                                        .child(div().text_color(t.ink).child("NEEDS YOU"))
                                        .child(div().text_color(t.faint).child("·"))
                                        .child(div().text_color(t.quiet).child("run command")),
                                )
                                .child(div().ty(CODE).text_color(t.ink).child(cmd.clone()))
                                .child(
                                    div()
                                        .pt(px(space::S1))
                                        .flex()
                                        .gap(px(space::S2))
                                        .child(button("Allow once", Some("⏎"), true, fx.pressed, t))
                                        .child(button("Allow for session", None, false, None, t))
                                        .child(button("Deny", Some("esc"), false, None, t)),
                                ),
                        ),
                ),
            )
        }
    }
}

fn format_ms(ms: f32) -> String {
    if ms < 10_000. {
        format!("{} ms", (ms / 10.).round() as u32 * 10)
    } else {
        format!("{:.1} s", ms / 1000.)
    }
}

/// Streamed text: each new word fades in; a block cursor breathes at the end.
fn streamed(s: &SharedString, words: &[(usize, usize, f32)], t: &Theme) -> Div {
    let cursor = "▍";
    let text = format!("{s} {cursor}");
    let mut hl: Vec<(std::ops::Range<usize>, HighlightStyle)> = words
        .iter()
        .filter(|(_, end, _)| *end <= s.len())
        .map(|(a, b, ms)| {
            let k = anim::ease_out(ms / motion::BASE);
            (
                *a..*b,
                HighlightStyle {
                    color: Some(t.ink.opacity(0.2 + 0.8 * k)),
                    ..Default::default()
                },
            )
        })
        .collect();
    let secs = crate::PROCESS_START
        .get()
        .map(|p| p.elapsed().as_secs_f32())
        .unwrap_or(0.);
    let breathe = 0.55 + 0.45 * (secs * std::f32::consts::TAU / 1.1).cos();
    hl.push((
        s.len() + 1..s.len() + 1 + cursor.len(),
        HighlightStyle {
            color: Some(t.working.0.opacity(breathe)),
            ..Default::default()
        },
    ));
    div().child(StyledText::new(text).with_highlights(hl))
}

/// Before the first word: "Thinking" with a band of ink sweeping across it.
fn thinking(t: &Theme) -> Div {
    let label = "Thinking";
    let secs = crate::PROCESS_START
        .get()
        .map(|p| p.elapsed().as_secs_f32())
        .unwrap_or(0.);
    let center = (secs / 1.6).fract() * (label.len() as f32 + 6.) - 3.;
    let hl: Vec<_> = label
        .char_indices()
        .map(|(i, c)| {
            let k = (1. - ((i as f32 - center).abs() / 3.)).clamp(0., 1.);
            (
                i..i + c.len_utf8(),
                HighlightStyle {
                    color: Some(t.faint.blend(t.ink.opacity(k))),
                    ..Default::default()
                },
            )
        })
        .collect();
    div()
        .italic()
        .child(StyledText::new(label).with_highlights(hl))
}

/// Buttons press by insetting their background layer 1px (reads as 0.97), so
/// neighbours never move.
fn button(
    text: &'static str,
    key: Option<&'static str>,
    primary: bool,
    pressed_ms: Option<f32>,
    t: &Theme,
) -> Div {
    let inset = match pressed_ms {
        Some(ms) => {
            let down = anim::ease_out(ms / motion::FAST);
            let up = anim::ease_out((ms - 300.) / motion::BASE);
            down - up
        }
        None => 0.,
    };
    let (bg, fg, key_fg) = if primary {
        (t.ink, t.surface, t.surface.opacity(0.55))
    } else {
        (gpui_kit::transparent_black(), t.ink, t.faint)
    };
    div()
        .relative()
        .h(px(size::CONTROL))
        .px(px(space::S3))
        .flex()
        .items_center()
        .gap(px(space::S2))
        .ty(UI)
        .text_color(fg)
        .child(
            div()
                .absolute()
                .top(px(inset))
                .bottom(px(inset))
                .left(px(inset))
                .right(px(inset))
                .rounded(px(radius::SM))
                .bg(bg)
                .when(!primary, |d| {
                    d.border_1().border_color(t.edge(t.ink).opacity(0.5))
                })
                .when(pressed_ms.is_some(), |d| {
                    d.bg(if primary {
                        t.ink.opacity(0.85)
                    } else {
                        t.press()
                    })
                }),
        )
        .child(div().relative().child(text))
        .when_some(key, |d, k| {
            d.child(div().relative().ty(LABEL).text_color(key_fg).child(k))
        })
}

fn composer(t: &Theme) -> impl IntoElement {
    // The box is one gutter wider than the column so its text lines up with messages.
    div()
        .w_full()
        .flex()
        .justify_center()
        .px(px(space::S4))
        .pb(px(space::S4))
        .pt(px(space::S2))
        .child(
            div()
                .w_full()
                .max_w(px(size::COLUMN + 2. * space::S4))
                .px(px(space::S4))
                .py(px(space::S3))
                .rounded(px(radius::XL))
                .bg(t.raised)
                .border_1()
                .border_color(t.hairline)
                .flex()
                .flex_col()
                .gap(px(space::S2))
                .child(
                    div()
                        .ty(READING)
                        .text_color(t.faint)
                        .child("Reply, or ⌘⏎ to steer"),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(space::S3))
                        .ty(LABEL)
                        .text_color(t.quiet)
                        .child("opus · high")
                        .child("ask")
                        .child(div().flex_1())
                        .child(div().text_color(t.faint).child("context 38%")),
                ),
        )
}

impl Render for Spike {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.bench.is_some() {
            bench::step(self, window, cx);
        }
        if self.demo.is_some() {
            demo::step(self, window, cx);
        }
        if self.advance() || self.demo.is_some() {
            window.request_animation_frame();
        }
        let t = self.theme;
        // Static grain, tiled once behind everything (no live blur, no per-frame
        // cost). During a theme tween both grains show, crossfading.
        let grain_layer = |image: Arc<Image>, opacity: f32| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .overflow_hidden()
                .opacity(opacity)
                .flex()
                .flex_wrap()
                .children((0..24).map(move |_| img(image.clone()).size(px(256.)).flex_none()))
        };
        let (paper_grain, carbon_grain) = (self.grain.0.clone(), self.grain.1.clone());
        let carbon = self.carbon;
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .text_color(t.ink)
            .ty(UI)
            .when(carbon < 1., |d| {
                d.child(grain_layer(paper_grain, 1. - carbon))
            })
            .when(carbon > 0., |d| d.child(grain_layer(carbon_grain, carbon)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar())
                    .child(self.chat())
                    .child(self.diff_panel()),
            )
            .child(self.status_bar())
            .when(self.show_fps, |d| {
                d.child(gpui_fps::fps_monitor(window, cx))
            })
    }
}

fn load_fonts(cx: &mut App, extra: Option<&std::path::Path>) {
    let fonts: Vec<std::borrow::Cow<'static, [u8]>> = vec![
        include_bytes!("../../../assets/fonts/Literata-Regular.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/Literata-Italic.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/Literata-Medium.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/Literata-MediumItalic.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/CommitMono-400-Regular.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/CommitMono-400-Italic.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/CommitMono-700-Regular.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/InstrumentSerif-Regular.ttf")
            .as_slice()
            .into(),
        include_bytes!("../../../assets/fonts/InstrumentSerif-Italic.ttf")
            .as_slice()
            .into(),
    ];
    let mut fonts = fonts;
    // Candidate fonts for comparison, loaded from a folder at runtime.
    if let Some(dir) = extra {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("ttf" | "otf")
            ) && let Ok(bytes) = std::fs::read(&path)
            {
                fonts.push(bytes.into());
            }
        }
    }
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("font load failed: {e:#}");
    }
    let names = cx.text_system().all_font_names();
    let f = theme::FONTS.get().copied().unwrap_or(&theme::PAIRINGS[0]);
    for family in [f.sans, f.body, f.mono, f.label, f.serif] {
        if !names.iter().any(|n| n == family) {
            eprintln!("font family {family:?} not found");
        }
    }
}

fn main() {
    PROCESS_START.get_or_init(Instant::now);
    let o = opts();
    let pairing = theme::PAIRINGS
        .iter()
        .find(|p| p.name == o.fonts)
        .unwrap_or(&theme::PAIRINGS[0]);
    theme::FONTS.get_or_init(|| pairing);
    if o.bench || o.demo.is_some() || o.specimen {
        profiler::set_trace_enabled(true);
    }
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        load_fonts(cx, o.font_dir.as_deref());
        specimen::load_katex(cx);
        if let Some(s) = o.size {
            let _ = SIZE.set(s);
        }
        let (x, y, w, h) = window();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(x), px(y)),
                size: size(px(w), px(h)),
            })),
            titlebar: Some(TitlebarOptions {
                title: Some(
                    if o.devices {
                        "dent · devices"
                    } else if o.compare {
                        "dent · streaming, three ways"
                    } else if o.stress {
                        "dent · spike 04 stress"
                    } else if o.specimen {
                        "dent · spike 04"
                    } else {
                        "dent · spike 03"
                    }
                    .into(),
                ),
                ..Default::default()
            }),
            focus: true,
            ..Default::default()
        };
        if o.devices {
            let (dark, demo, record) = (o.dark, o.demo.clone(), o.record.clone());
            let secs = match demo.as_deref() {
                Some("away") => 50.,
                Some("upgrade") => 32.,
                _ => 25.,
            };
            gpui_kit::open_window(options, cx, move |window, cx| {
                let view = cx.new(|cx| devices::DevicesView::new(dark, demo, cx));
                let focus = view.read(cx).focus_handle();
                window.focus(&focus, cx);
                view
            })
            .expect("open window");
            if let Some(path) = record {
                demo::record(path, secs);
            }
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            cx.activate(true);
            return;
        }
        if o.compare {
            let (dark, record) = (o.dark, o.record.clone());
            gpui_kit::open_window(options, cx, move |_, cx| {
                use specimen::Reveal;
                let panes = [Reveal::Raw, Reveal::Smooth, Reveal::Blocks]
                    .into_iter()
                    .map(|r| cx.new(|_| specimen::Specimen::pane(dark, r)))
                    .collect();
                let theme = if dark {
                    theme::Theme::carbon()
                } else {
                    theme::Theme::paper()
                };
                cx.new(|_| specimen::Compare::new(panes, theme))
            })
            .expect("open window");
            if let Some(path) = record {
                demo::record(path, 10.5);
            }
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_secs_f32(12.))
                    .await;
                cx.update(|cx| cx.quit());
            })
            .detach();
            cx.activate(true);
            return;
        }
        if o.specimen {
            let (dark, bench, shots, stress, verify) =
                (o.dark, o.bench, o.shots.clone(), o.stress, o.verify);
            gpui_kit::open_window(options, cx, move |_, cx| {
                cx.new(|_| specimen::Specimen::new(dark, bench, shots, stress, verify))
            })
            .expect("open window");
            cx.activate(true);
            return;
        }
        let (_, view) = gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Spike::new(&o)))
            .expect("open window");
        // Step the working square's ripple (steps.pixel) even when nothing else changes.
        let view = view.downgrade();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs_f32(1. / ds::motion::PIXEL_FPS))
                    .await;
                if view.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.activate(true);
    });
}
