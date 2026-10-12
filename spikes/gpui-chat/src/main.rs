//! Spike: GPUI chat list and diff view at 100,000 events on pinned
//! gpui-component, in the Field Notes design.
//!
//! `just dev spike-gpui-chat [--dark] [--bench] [--events N] [--shots DIR]`
//!
//! `--bench` runs a scripted interaction (open the session, scroll, jump,
//! stream, scroll the diff and the sidebar), records GPUI's own frame timings
//! for each phase, takes screenshots, prints a JSON report and quits.
//! All data is generated locally: no agents, no usage.

mod bench;
mod fake;
mod theme;

use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;

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
            bench: o
                .bench
                .then(|| bench::Bench::new(o.shots.clone(), o.shot_only)),
        }
    }

    fn sidebar(&self) -> impl IntoElement {
        let t = self.theme;
        let sessions = self.sessions.clone();
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
                                .child(status_square(s.status, &t))
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
        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .min_w_0()
            .child(
                div()
                    .px_8()
                    .pt_5()
                    .pb_3()
                    .border_b_1()
                    .border_color(t.hairline)
                    .flex()
                    .items_end()
                    .gap_3()
                    .child(
                        div()
                            .font_family(serif())
                            .text_size(px(30.))
                            .text_color(t.ink)
                            .child("Event log compaction"),
                    )
                    .child(
                        div()
                            .pb_1()
                            .font_family(label())
                            .text_size(px(10.5))
                            .text_color(t.quiet)
                            .child(format!("CLAUDE · MAC-MINI · {} EVENTS", group(rows.len()))),
                    ),
            )
            .child(
                list(self.chat.clone(), move |i, _, _| {
                    let row = &rows[i];
                    let override_text = if i == last { stream.clone() } else { None };
                    chat_row(row, override_text, &t).into_any_element()
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
                    .child(status_square(Status::Needs, &t))
                    .child("2 BLOCKED"),
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

fn chat_row(row: &Row, stream: Option<SharedString>, t: &Theme) -> Div {
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
                    Some(s) => vec![div().child(s)],
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
                            .child(button("Allow once", true, t))
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
        let t = self.theme;
        let streaming = self.streaming.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .text_color(t.ink)
            .font_family(sans())
            .text_size(px(13.))
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
        gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Spike::new(&o)))
            .expect("open window");
        cx.activate(true);
    });
}
