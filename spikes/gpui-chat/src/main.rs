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
mod fake;
mod theme;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use fake::{DiffLine, LineKind, Row, SessionRow};
use gpui_kit::prelude::*;
use gpui_kit::*;
use theme::{Status, Theme, body, label, mono, sans, serif, status_square};

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
            other => eprintln!("ignoring unknown flag {other}"),
        }
    }
    o
}

pub const WINDOW: (f32, f32, f32, f32) = (40., 40., 1360., 780.);

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
    pub bench: Option<bench::Bench>,
    pub demo: Option<demo::Demo>,
    /// Rows that appeared recently, for the dither dissolve.
    pub appeared: HashMap<usize, Instant>,
    /// An approval button that was just pressed (row, when).
    pub pressed: Option<(usize, Instant)>,
    /// Status of the open session (sidebar row 0).
    pub current: Status,
    pub blocked: usize,
    /// 0 = full serif title at the live tail, 1 = compact while reading history.
    pub title_t: f32,
    pub show_fps: bool,
    /// Old surface colour dissolving away after a theme switch.
    pub theme_fade: Option<(Instant, Hsla)>,
    pub grain: (Arc<Image>, Arc<Image>),
    last_frame: Instant,
}

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
        Self {
            theme: if o.dark {
                Theme::carbon()
            } else {
                Theme::paper()
            },
            rows: Rc::new(rows),
            chat,
            diff: Rc::new(diff),
            diff_scroll: UniformListScrollHandle::new(),
            sessions: Rc::new(sessions),
            sidebar_scroll: UniformListScrollHandle::new(),
            streaming: None,
            demo: o
                .demo
                .as_deref()
                .map(|name| demo::Demo::new(name, o.record.clone())),
            appeared: HashMap::new(),
            pressed: None,
            current: Status::Done,
            blocked: 2,
            title_t: 0.,
            show_fps: o.fps,
            theme_fade: None,
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

    /// Append a row at the live tail; it resolves in with the dither dissolve.
    pub fn push_row(&mut self, row: Row) {
        let ix = self.rows.len();
        Rc::make_mut(&mut self.rows).push(row);
        self.chat.splice(ix..ix, 1);
        self.appeared.insert(ix, Instant::now());
    }

    /// Replace a row in place (e.g. an approval resolving into a tool line).
    pub fn replace_row(&mut self, ix: usize, row: Row) {
        Rc::make_mut(&mut self.rows)[ix] = row;
        self.chat.splice(ix..ix + 1, 1);
        self.appeared.insert(ix, Instant::now());
    }

    pub fn switch_theme(&mut self) {
        self.theme_fade = Some((Instant::now(), self.theme.surface));
        self.theme = if self.theme.dark {
            Theme::paper()
        } else {
            Theme::carbon()
        };
    }

    fn animating(&self) -> bool {
        let target = if self.at_live_tail() { 0. } else { 1. };
        self.appeared
            .values()
            .any(|s| anim::progress(*s, anim::DISSOLVE_MS) < 1.)
            || self.theme_fade.is_some()
            || self.pressed.is_some()
            || (self.title_t - target).abs() > 0.001
            || self.streaming.is_some()
    }

    fn at_live_tail(&self) -> bool {
        self.chat.is_following_tail()
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
                div()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .font_family(label())
                    .text_size(px(10.5))
                    .text_color(t.quiet)
                    .child("PROJECTS · 1,000 SESSIONS"),
            )
            .child(
                uniform_list("sessions", sessions.len(), move |range, _, _| {
                    range
                        .map(|i| {
                            let s = &sessions[i];
                            div()
                                .id(i)
                                .h(px(30.))
                                .px_4()
                                .flex()
                                .items_center()
                                .gap_2()
                                .when(i == 0, |d| d.bg(t.raised))
                                .child(status_square(if i == 0 { current } else { s.status }, &t))
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .text_color(t.ink)
                                        .child(s.title.clone()),
                                )
                                .child(
                                    div()
                                        .font_family(label())
                                        .text_size(px(10.))
                                        .text_color(t.quiet)
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
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(t.hairline)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .font_family(label())
                    .text_size(px(10.5))
                    .text_color(t.quiet)
                    .child(machine_row("laptop", Status::Done, "connected", &t))
                    .child(machine_row("mac-mini", Status::Working, "connected", &t))
                    .child(machine_row("ec2-runner", Status::Idle, "asleep · 12m", &t)),
            )
    }

    fn chat(&self, streaming: Option<&str>) -> impl IntoElement {
        let t = self.theme;
        let rows = self.rows.clone();
        let last = rows.len().saturating_sub(1);
        let stream: Option<SharedString> = streaming.map(|s| s.to_string().into());
        // Row effects are captured by value so the list closure stays 'static.
        let appeared: HashMap<usize, f32> = self
            .appeared
            .iter()
            .map(|(ix, s)| (*ix, anim::progress(*s, anim::DISSOLVE_MS)))
            .filter(|(_, p)| *p < 1.)
            .collect();
        let pressed = self.pressed.map(|(ix, s)| (ix, s.elapsed().as_secs_f32()));
        // The serif title shrinks while reading history and returns at the live tail.
        let k = anim::ease_out(self.title_t);
        let title_size = 30. - 11. * k;
        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .min_w_0()
            .child(
                div()
                    .px_8()
                    .pt(px(20. - 8. * k))
                    .pb(px(12. - 4. * k))
                    .border_b_1()
                    .border_color(t.hairline)
                    .flex()
                    .items_end()
                    .gap_3()
                    .child(
                        div()
                            .font_family(serif())
                            .text_size(px(title_size))
                            .line_height(px(title_size * 1.1))
                            .text_color(t.ink)
                            .child("Event log compaction"),
                    )
                    .child(
                        div()
                            .pb(px(4. - 2. * k))
                            .font_family(label())
                            .text_size(px(10.5))
                            .text_color(t.quiet)
                            .child(format!("CLAUDE · MAC-MINI · {} EVENTS", group(rows.len()))),
                    )
                    .child(div().flex_1())
                    .when(k > 0.02, |d| {
                        d.child(
                            div()
                                .relative()
                                .mb(px(2.))
                                .px_2()
                                .py(px(3.))
                                .rounded(px(4.))
                                .bg(t.ink)
                                .text_color(t.surface)
                                .font_family(label())
                                .text_size(px(10.))
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(status_square(self.current, &t))
                                .child("LIVE ↓")
                                .child(anim::dissolve(k, t.surface, 2.)),
                        )
                    }),
            )
            .child(
                list(self.chat.clone(), move |i, _, _| {
                    let row = &rows[i];
                    let fx = RowFx {
                        stream: if i == last { stream.clone() } else { None },
                        appear: appeared.get(&i).copied(),
                        pressed: pressed.and_then(|(ix, secs)| (ix == i).then_some(secs)),
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
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(t.hairline)
                    .flex()
                    .justify_between()
                    .font_family(mono())
                    .text_size(px(11.))
                    .child(
                        div()
                            .text_color(t.ink)
                            .child("crates/daemon/src/event_log.rs"),
                    )
                    .child(
                        div()
                            .text_color(t.quiet)
                            .child(format!("{} LINES", group(lines.len()))),
                    ),
            )
            .child(
                uniform_list("diff", lines.len(), move |range, _, _| {
                    range
                        .map(|i| {
                            let l = &lines[i];
                            let (bg, sign) = match l.kind {
                                LineKind::Added => (Some(t.added), "+"),
                                LineKind::Removed => (Some(t.removed), "−"),
                                LineKind::Context => (None, " "),
                            };
                            let num = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
                            div()
                                .id(i)
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .font_family(mono())
                                .text_size(px(12.))
                                .when_some(bg, |d, bg| d.bg(bg))
                                .child(
                                    div()
                                        .w(px(48.))
                                        .pr_2()
                                        .flex()
                                        .justify_end()
                                        .text_color(t.quiet)
                                        .child(num(l.old)),
                                )
                                .child(
                                    div()
                                        .w(px(48.))
                                        .pr_2()
                                        .flex()
                                        .justify_end()
                                        .text_color(t.quiet)
                                        .child(num(l.new)),
                                )
                                .child(div().w(px(16.)).text_color(t.quiet).child(sign))
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
        div()
            .h(px(28.))
            .px_4()
            .flex()
            .items_center()
            .gap_4()
            .border_t_1()
            .border_color(t.hairline)
            .font_family(label())
            .text_size(px(10.5))
            .text_color(t.quiet)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_square(Status::Working, &t))
                    .child("CLAUDE 5H 5% · WEEK 11%"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_square(Status::Waiting, &t))
                    .child("CODEX 30D 1%"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_square(
                        if self.blocked > 0 {
                            Status::Needs
                        } else {
                            Status::Done
                        },
                        &t,
                    ))
                    .child(if self.blocked > 0 {
                        format!("{} BLOCKED", self.blocked)
                    } else {
                        "NONE BLOCKED".to_string()
                    }),
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

fn machine_row(
    name: &'static str,
    status: Status,
    state: &'static str,
    t: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(status_square(status, t))
        .child(div().flex_1().child(name))
        .child(state)
}

/// Per-row animation state.
pub struct RowFx {
    /// Text streaming into this row, if it is the live tail.
    pub stream: Option<SharedString>,
    /// Dissolve progress if the row appeared recently.
    pub appear: Option<f32>,
    /// Seconds since the approval's primary button was pressed.
    pub pressed: Option<f32>,
}

fn chat_row(row: &Row, fx: RowFx, t: &Theme) -> Div {
    let el = chat_row_inner(row, &fx, t);
    match fx.appear {
        Some(p) => el.relative().child(anim::dissolve(p, t.surface, 3.)),
        None => el,
    }
}

fn chat_row_inner(row: &Row, fx: &RowFx, t: &Theme) -> Div {
    let stream = fx.stream.clone();
    let column = div().w_full().flex().justify_center().px_8();
    let inner = div().w_full().max_w(px(720.));
    match row {
        Row::User(text) => column.py_2().child(
            inner.flex().justify_end().child(
                div()
                    .max_w(px(560.))
                    .px_4()
                    .py_2()
                    .rounded(px(10.))
                    .bg(t.raised)
                    .border_1()
                    .border_color(t.hairline)
                    .font_family(body())
                    .text_size(px(15.))
                    .text_color(t.ink)
                    .child(text.clone()),
            ),
        ),
        Row::Assistant(paras) => column.py_2().child(
            inner
                .flex()
                .flex_col()
                .gap_3()
                .font_family(body())
                .text_size(px(15.))
                .line_height(px(23.))
                .text_color(t.ink)
                .children(match stream {
                    // A block cursor in the working colour while the reply streams.
                    Some(s) => {
                        vec![
                            div().child(StyledText::new(format!("{s} ▍")).with_highlights([(
                                s.len() + 1..s.len() + 1 + "▍".len(),
                                HighlightStyle {
                                    color: Some(t.working.0),
                                    ..Default::default()
                                },
                            )])),
                        ]
                    }
                    None => paras.iter().map(|p| div().child(p.clone())).collect(),
                }),
        ),
        Row::Tool {
            name,
            detail,
            result,
            ok,
        } => column.py(px(3.)).child(
            inner
                .flex()
                .items_center()
                .gap_2()
                .font_family(mono())
                .text_size(px(12.))
                .text_color(t.quiet)
                .child("├─")
                .child(div().text_color(t.ink).child(name.clone()))
                .child(div().flex_1().truncate().child(detail.clone()))
                .child(status_square(
                    if *ok { Status::Done } else { Status::Needs },
                    t,
                ))
                .child(result.clone()),
        ),
        Row::Approval(cmd) => column.py_2().child(
            inner.child(
                div()
                    .p_3()
                    .rounded(px(8.))
                    .bg(t.raised)
                    .border_1()
                    .border_color(t.needs.0)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .font_family(label())
                            .text_size(px(10.5))
                            .text_color(t.quiet)
                            .child(status_square(Status::Needs, t))
                            .child("NEEDS YOU · RUN COMMAND"),
                    )
                    .child(
                        div()
                            .font_family(mono())
                            .text_size(px(13.))
                            .text_color(t.ink)
                            .child(cmd.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .text_size(px(12.))
                            .child(match fx.pressed {
                                // Pressed: the button sinks and flashes the done colour.
                                Some(secs) => {
                                    let k = (secs / 0.18).clamp(0., 1.);
                                    button("Allow once", true, t)
                                        .bg(t.done.0)
                                        .border_color(t.done.0)
                                        .mt(px(1. - k))
                                        .child(
                                            div()
                                                .font_family(label())
                                                .text_size(px(10.))
                                                .child(" ✓"),
                                        )
                                }
                                None => button("Allow once", true, t),
                            })
                            .child(button("Allow for session", false, t))
                            .child(button("Deny…", false, t)),
                    ),
            ),
        ),
    }
}

fn button(label: &'static str, primary: bool, t: &Theme) -> Div {
    let d = div()
        .px_3()
        .py_1()
        .rounded(px(6.))
        .border_1()
        .border_color(t.hairline)
        .child(label);
    if primary {
        d.bg(t.ink).text_color(t.surface)
    } else {
        d.text_color(t.ink)
    }
}

fn composer(t: &Theme) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .justify_center()
        .px_8()
        .pb_4()
        .pt_2()
        .child(
            div()
                .w_full()
                .max_w(px(720.))
                .px_4()
                .py_3()
                .rounded(px(12.))
                .bg(t.raised)
                .border_1()
                .border_color(t.hairline)
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .font_family(body())
                        .text_size(px(15.))
                        .text_color(t.quiet)
                        .child("Reply, or Cmd+Enter to steer…"),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .font_family(label())
                        .text_size(px(10.5))
                        .text_color(t.quiet)
                        .child("OPUS · HIGH")
                        .child("ASK")
                        .child(div().flex_1())
                        .child("CONTEXT 38%"),
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
        // Ease the title towards its target (critically damped, ~180 ms).
        let dt = self.last_frame.elapsed().as_secs_f32().min(0.05);
        self.last_frame = Instant::now();
        let target = if self.at_live_tail() { 0. } else { 1. };
        self.title_t += (target - self.title_t) * (1. - (-dt / 0.06).exp());
        if (target - self.title_t).abs() < 0.001 {
            self.title_t = target;
        }
        self.appeared
            .retain(|_, s| anim::progress(*s, anim::DISSOLVE_MS) < 1.);
        if let Some((_, s)) = self.pressed
            && s.elapsed() > Duration::from_millis(600)
        {
            self.pressed = None;
        }
        let fade = self.theme_fade.map(|(s, c)| (anim::progress(s, 450.), c));
        if matches!(fade, Some((p, _)) if p >= 1.) {
            self.theme_fade = None;
        }
        if self.animating() || self.demo.is_some() {
            window.request_animation_frame();
        }

        let t = self.theme;
        let streaming = self.streaming.clone();
        let grain = if t.dark {
            self.grain.1.clone()
        } else {
            self.grain.0.clone()
        };
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .text_color(t.ink)
            .font_family(sans())
            .text_size(px(13.))
            // Static grain, tiled once behind everything (no live blur, no per-frame cost).
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .overflow_hidden()
                    .flex()
                    .flex_wrap()
                    .children((0..24).map(move |_| img(grain.clone()).size(px(256.)).flex_none())),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar())
                    .child(self.chat(streaming.as_deref()))
                    .child(self.diff_panel()),
            )
            .child(self.status_bar())
            .when_some(fade, |d, (p, old)| {
                d.child(anim::dissolve(anim::ease_out(p), old, 4.))
            })
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
    if o.bench {
        profiler::set_trace_enabled(true);
    }
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        load_fonts(cx, o.font_dir.as_deref());
        let (x, y, w, h) = WINDOW;
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(x), px(y)),
                size: size(px(w), px(h)),
            })),
            titlebar: Some(TitlebarOptions {
                title: Some("dent · spike 03".into()),
                ..Default::default()
            }),
            focus: true,
            ..Default::default()
        };
        let (_, view) = gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Spike::new(&o)))
            .expect("open window");
        // Repaint slowly so the working square drifts even when nothing else changes.
        let view = view.downgrade();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(theme::DRIFT_MS as u64))
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
