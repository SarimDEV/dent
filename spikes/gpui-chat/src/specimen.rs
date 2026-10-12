//! Spike 04 in GPUI: a message streams through the stable-prefix layer and
//! renders as a block tree, with TeX drawn natively from RaTeX's display
//! list and Mermaid rasterised off the UI thread.
//!
//! `just dev spike-gpui-chat --specimen [--dark] [--bench] [--shots DIR]`
//! `just dev spike-gpui-chat --stress [--size WxH]`: every block at its limits.
//! `STRESS_ROWS=n` sets the tall table's rows; `STRESS_FILE=path` replaces
//! the stress page with any Markdown file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::*;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Tag, TagEnd};
use ratex_types::display_item::{DisplayItem, DisplayList};
use ratex_types::path_command::PathCommand;
use spike_markdown::stream::{Block, Streamer};

use crate::ds::{CODE, LABEL, READING, Typed, radius, space};

#[path = "chrome.rs"]
mod chrome;
#[path = "reveal.rs"]
pub mod reveal;
#[path = "verify.rs"]
mod verify;

pub use reveal::{Compare, Reveal};

use crate::theme::{self, Theme};
use chrome::{CopyText, Probes, TableData};

/// Written for the spike; no transcript content.
pub const MESSAGE: &str = r#"## Why the frame budget holds

The chat list only measures what it shows, so opening a long session costs the same as a short one. With $n$ events and a viewport of $k$ rows, layout is $O(k)$ rather than $O(n)$, and streaming re-measures one row.

Three things keep a frame under **8.3 ms**:

1. Finished blocks are *committed* and never parsed again.
2. Only the tail is re-parsed, with `pulldown-cmark`, in about 5 µs.
3. Math and diagrams render off the UI thread and are cached by hash.

| Scene | p50 | p99 |
| --- | --- | --- |
| Stream | 4.7 ms | 5.3 ms |
| Scroll | 4.5 ms | 5.4 ms |
| Jump | 5.3 ms | 5.8 ms |

The attention score for a query against $d_k$-dimensional keys is

$$
\mathrm{softmax}\!\left(\frac{QK^\top}{\sqrt{d_k}}\right)V, \qquad \sum_{i=1}^{n} \alpha_i = 1
$$

and a frame is on time when $t_{\text{draw}} \le \frac{1}{120}\,\mathrm{s}$.

```rust
fn push(&mut self, chunk: &str) {
    self.text.push_str(chunk);
    let tail = &self.text[self.stable..];
    self.reparse(tail);
}
```

```mermaid
flowchart LR
  A[Chunk arrives] --> B[Re-parse tail]
  B --> C{Block closed?}
  C -- yes --> D[Commit]
  C -- no --> E[Terminate and draw]
```

> Nothing flickers between raw and rendered: an open `**` closes itself, and a half-written table row waits for its newline.
"#;

/// Characters per second while streaming, roughly an agent's pace.
const RATE: f32 = 260.;

/// Every block at its limits: too wide, too tall, too long, too deep.
/// Written for the spike; no transcript content.
pub fn stress() -> String {
    let part = std::env::var("STRESS_PART").unwrap_or_default();
    if let Ok(f) = std::env::var("STRESS_FILE") {
        return std::fs::read_to_string(f).unwrap_or_default();
    }
    if part == "head" {
        return STRESS_HEAD.into();
    }
    if part == "tail" {
        return STRESS_TAIL.into();
    }
    let mut s = String::from(STRESS_HEAD);
    s.push_str("\n### A table wider than any window\n\n|");
    for c in 1..=14 {
        s.push_str(&format!(" Column {c} |"));
    }
    s.push_str("\n|");
    for _ in 1..=14 {
        s.push_str(" --- |");
    }
    for r in 1..=3 {
        s.push_str("\n|");
        for c in 1..=14 {
            s.push_str(&format!(" r{r}c{c} value |"));
        }
    }
    s.push_str("\n\n### A table taller than the window\n\n| # | Event | Duration |\n| ---: | --- | ---: |\n");
    let rows: usize = std::env::var("STRESS_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    for r in 1..=rows {
        s.push_str(&format!(
            "| {r} | event number {r} | {}.{} ms |\n",
            r % 9,
            r % 7
        ));
    }
    s.push_str(STRESS_TAIL);
    s
}

const STRESS_HEAD: &str = r#"# Stress: every block at its limits

A link with no spaces: https://example.com/a/very/long/path/that/never/breaks/because/it/has/no/spaces/at/all/and/keeps/going?query=parameters&and=more&still=more and a word that won't break: Pneumonoultramicroscopicsilicovolcanoconiosis.

## A heading that goes on far longer than any heading should, so it has to wrap across several lines when the window is narrow

Inline maths wider than a line: $\sum_{i=1}^{n} \sum_{j=1}^{m} \left( a_{ij} x_i y_j + b_{ij} x_i^2 + c_{ij} y_j^2 + d_{ij} \right) = \int_0^\infty e^{-t^2} \, dt \cdot \prod_{k=1}^{K} \frac{k}{k+1}$ and the sentence continues after it.

Display maths wider than the column:

$$
f(x) = a_0 + a_1 x + a_2 x^2 + a_3 x^3 + a_4 x^4 + a_5 x^5 + a_6 x^6 + a_7 x^7 + a_8 x^8 + a_9 x^9 + a_{10} x^{10} + a_{11} x^{11} + a_{12} x^{12} + a_{13} x^{13} + a_{14} x^{14} + a_{15} x^{15}
$$

Tall maths:

$$
x = a_0 + \cfrac{1}{a_1 + \cfrac{1}{a_2 + \cfrac{1}{a_3 + \cfrac{1}{a_4 + \cfrac{1}{a_5 + \cfrac{1}{a_6}}}}}}
\qquad
\begin{pmatrix} a_{11} & a_{12} & a_{13} & a_{14} \\ a_{21} & a_{22} & a_{23} & a_{24} \\ a_{31} & a_{32} & a_{33} & a_{34} \\ a_{41} & a_{42} & a_{43} & a_{44} \\ a_{51} & a_{52} & a_{53} & a_{54} \\ a_{61} & a_{62} & a_{63} & a_{64} \end{pmatrix}
$$

A table with long cells:

| Setting | What it does | Example |
| --- | --- | --- |
| `retry` | Retries a failed request with exponential backoff, starting at 100 ms and doubling until it reaches the ceiling set below, then gives up and reports the last error to every client that asked | `3` |
| `endpoint` | Where requests go | https://example.com/api/v2/sessions/0123456789abcdef0123456789abcdef/events?after=1000000 |
"#;

const STRESS_TAIL: &str = r#"
A code block with long lines:

```rust
fn configure(builder: &mut Builder) -> Result<Config, Error> { builder.with_timeout(Duration::from_secs(30)).with_retry(Retry::exponential(Duration::from_millis(100), Duration::from_secs(10))).with_endpoint("https://example.com/api/v2/sessions").build() }
let short = 1;
```

A diagram wider than the column:

```mermaid
flowchart LR
  A[Request] --> B[Parse] --> C[Validate] --> D[Authorise] --> E[Load session] --> F[Append event] --> G[Fan out] --> H[Persist] --> I[Acknowledge] --> J[Done]
```

A diagram taller than the window:

```mermaid
flowchart TD
  A[Start] --> B[Step one] --> C[Step two] --> D[Step three] --> E[Step four] --> F[Step five] --> G[Step six] --> H[Step seven] --> I[Step eight] --> J[Step nine] --> K[Step ten] --> L[Step eleven] --> M[Done]
```

Deep nesting:

- one
  - two
    - three
      - four
        - five
          - six, with a sentence long enough to wrap when it is indented this far in a narrow window

> A quote
> > inside a quote
> > > inside a quote, with `code` and **bold** and a long enough line to wrap at least once in the narrowest window.

The end.
"#;

enum Diagram {
    Pending,
    Ready(Arc<RenderImage>, f32, f32),
    Failed(String),
}

pub struct Specimen {
    theme: Theme,
    message: &'static str,
    rate: f32,
    start: Instant,
    streamer: Streamer,
    math: HashMap<(String, bool), Option<Rc<DisplayList>>>,
    diagrams: HashMap<String, Diagram>,
    bench: bool,
    shots: Option<PathBuf>,
    collector: profiler::FrameTimingCollector,
    draws: Vec<f64>,
    done_at: Option<Instant>,
    /// Whether the block being built is committed (its fences are closed).
    closed: bool,
    scroll: ScrollHandle,
    shot: usize,
    /// When the current page was scrolled into place.
    paged_at: Option<Instant>,
    offsets: Vec<f32>,
    /// Horizontal scroll state for wide content, by (block, nth in block).
    hscrolls: HashMap<(usize, usize), ScrollHandle>,
    block_ix: usize,
    hs_count: usize,
    builds: Vec<f64>,
    tables: HashMap<usize, Rc<TableData>>,
    table_lists: HashMap<usize, ListState>,
    panel: Option<usize>,
    panel_list: ListState,
    copied: Option<(String, Instant)>,
    probes: Probes,
    verify: Option<verify::Verify>,
    copy_texts: HashMap<String, CopyText>,
    /// Top-level tables built row by row, by where their block starts.
    table_rows: HashMap<usize, TableBuild>,
    pub reveal: Reveal,
    arrivals: Vec<(f32, usize)>,
    revealed: f32,
    last_tick: Option<Instant>,
    /// Per inline (block start, position): rendered length and when seen.
    seen: HashMap<(usize, usize), Vec<(usize, Instant)>>,
    inline_ix: usize,
    block_start: usize,
    /// Block mode: when each block was first shown.
    first_seen: HashMap<usize, Instant>,
    /// Something is mid-fade, so keep drawing.
    fading: bool,
    /// How many frames had been drawn when streaming finished.
    draws_at_done: Option<usize>,
}

impl Specimen {
    pub fn new(
        dark: bool,
        bench: bool,
        shots: Option<PathBuf>,
        stress: bool,
        verify: bool,
    ) -> Self {
        Self {
            message: if stress {
                Box::leak(self::stress().into_boxed_str())
            } else {
                MESSAGE
            },
            // The stress page streams too, just fast.
            rate: if verify {
                3_000.
            } else if stress {
                20_000.
            } else {
                RATE
            },
            theme: if dark {
                Theme::carbon()
            } else {
                Theme::paper()
            },
            start: Instant::now(),
            streamer: Streamer::new(true),
            math: HashMap::new(),
            diagrams: HashMap::new(),
            bench,
            shots,
            collector: profiler::FrameTimingCollector::new(),
            draws: Vec::new(),
            done_at: None,
            closed: false,
            scroll: ScrollHandle::new(),
            shot: 0,
            paged_at: None,
            offsets: Vec::new(),
            hscrolls: HashMap::new(),
            block_ix: 0,
            hs_count: 0,
            builds: Vec::new(),
            tables: HashMap::new(),
            table_lists: HashMap::new(),
            panel: None,
            panel_list: ListState::new(0, ListAlignment::Top, px(200.)),
            copied: None,
            probes: Probes::default(),
            verify: verify.then(verify::Verify::default),
            copy_texts: HashMap::new(),
            table_rows: HashMap::new(),
            reveal: Reveal::Rate,
            arrivals: Vec::new(),
            revealed: 0.,
            last_tick: None,
            seen: HashMap::new(),
            inline_ix: 0,
            block_start: 0,
            first_seen: HashMap::new(),
            fading: false,
            draws_at_done: None,
        }
    }

    /// A pane for `--compare`: the specimen message, revealed one way.
    pub fn pane(dark: bool, reveal: Reveal) -> Self {
        let mut s = Self::new(dark, false, None, false, false);
        s.reveal = reveal;
        s.arrivals = reveal::arrivals(MESSAGE);
        s
    }

    fn typeset(&mut self, src: &str, display: bool) -> Option<Rc<DisplayList>> {
        self.math
            .entry((src.to_string(), display))
            .or_insert_with(|| {
                spike_markdown::math::typeset(src, display)
                    .ok()
                    .map(|t| Rc::new(t.list))
            })
            .clone()
    }

    /// Kick off a diagram render on the background executor.
    fn diagram(&mut self, src: &str, cx: &mut Context<Self>) {
        if self.diagrams.contains_key(src) {
            return;
        }
        self.diagrams.insert(src.to_string(), Diagram::Pending);
        let key = src.to_string();
        let dark = self.theme.dark;
        let task = cx.background_executor().spawn(async move {
            let t = Instant::now();
            let pal = if dark {
                &spike_markdown::mermaid::CARBON
            } else {
                &spike_markdown::mermaid::PAPER
            };
            let (r, req) = spike_markdown::mermaid::renderer(pal).map_err(|e| e.to_string())?;
            let svg = spike_markdown::mermaid::svg(&r, &req, &key)?;
            let pm = spike_markdown::mermaid::raster(&svg, &spike_markdown::mermaid::fontdb(), 2.)?;
            let (w, h) = (pm.width(), pm.height());
            // tiny-skia is premultiplied RGBA; GPUI wants straight BGRA.
            let mut px = pm.take();
            for p in px.as_chunks_mut::<4>().0 {
                let a = p[3] as f32 / 255.;
                if a > 0. {
                    for c in &mut p[..3] {
                        *c = (*c as f32 / a).round().min(255.) as u8;
                    }
                }
                p.swap(0, 2);
            }
            let buf = image::RgbaImage::from_raw(w, h, px).ok_or("bad buffer")?;
            eprintln!("diagram ready in {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
            Ok::<_, String>((
                Arc::new(RenderImage::new(vec![image::Frame::new(buf)])),
                w as f32 / 2.,
                h as f32 / 2.,
            ))
        });
        let key = src.to_string();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |s, cx| {
                s.diagrams.insert(
                    key,
                    match result {
                        Ok((img, w, h)) => Diagram::Ready(img, w, h),
                        Err(e) => Diagram::Failed(e),
                    },
                );
                cx.notify();
            });
        })
        .detach();
    }

    fn advance(&mut self) -> bool {
        let msg = self.message;
        let want = self.reveal_target();
        let mut at = self.streamer.text.len();
        if at >= msg.len() {
            if self.done_at.is_none() {
                self.streamer.finish();
                self.done_at = Some(Instant::now());
                self.draws_at_done = Some(self.draws.len());
            }
            return false;
        }
        let mut end = want.min(msg.len());
        while !msg.is_char_boundary(end) {
            end += 1;
        }
        if end > at {
            self.streamer.push(&msg[at..end]);
            at = end;
        }
        at < msg.len()
    }
}

/// Inline content of one paragraph-like block, ready for a StyledText.
#[derive(Default, Clone)]
struct Inline {
    text: String,
    runs: Vec<TextRun>,
    math: Vec<(usize, Rc<DisplayList>)>,
    /// The text as written, with maths as `$…$`, for copying.
    plain: String,
}

#[derive(Clone, Copy, Default)]
struct Style {
    strong: bool,
    emph: bool,
    code: bool,
    strike: bool,
    link: bool,
}

const INLINE_EM: f32 = 17.;
const DISPLAY_EM: f32 = 19.;

impl Inline {
    fn push(&mut self, s: &str, st: Style, t: &Theme) {
        // Text resuming after a formula on its own line starts flush left.
        let s = if self.text.is_empty() {
            s.trim_start()
        } else {
            s
        };
        if s.is_empty() {
            return;
        }
        let mut f = font(if st.code {
            theme::mono()
        } else {
            theme::body()
        });
        if st.strong {
            f.weight = FontWeight::MEDIUM;
        }
        if st.emph {
            f.style = FontStyle::Italic;
        }
        self.text.push_str(s);
        self.plain.push_str(s);
        self.runs.push(TextRun {
            len: s.len(),
            font: f,
            color: if st.link { t.working.0 } else { t.ink },
            background_color: st.code.then(|| t.ink.opacity(0.06)),
            underline: st.link.then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(t.working.0.opacity(0.5)),
                wavy: false,
            }),
            strikethrough: st.strike.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: Some(t.quiet),
            }),
        });
    }

    /// Reserve exactly the formula's width with invisible word characters, so
    /// the line never breaks inside it, and remember where to draw it.
    fn push_math(&mut self, list: Rc<DisplayList>, src: &str, window: &Window, t: &Theme) {
        self.plain.push_str(&format!("${src}$"));
        let width = px(list.width as f32 * INLINE_EM + 2.);
        let f = font(theme::body());
        let ts = window.text_system();
        let id = ts.resolve_font(&f);
        let size = px(READING.size);
        let mut fill = String::new();
        let mut left = width;
        for ch in ['m', 'n', 'o', 'i', '.'] {
            let w = ts.layout_width(id, size, ch);
            while w > px(0.) && left >= w {
                fill.push(ch);
                left -= w;
            }
        }
        if fill.is_empty() {
            fill.push('.');
        }
        self.math.push((self.text.len(), list));
        self.text.push_str(&fill);
        self.runs.push(TextRun {
            len: fill.len(),
            font: f,
            color: t.ink.opacity(0.),
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }

    fn element(self, t: &Theme) -> AnyElement {
        if self.text.is_empty() {
            return div().into_any_element();
        }
        let styled = StyledText::new(self.text).with_runs(self.runs);
        let layout = styled.layout().clone();
        let maths = self.math;
        let ink = t.ink;
        div()
            .relative()
            .child(styled)
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
                        let f = font(theme::body());
                        let id = window.text_system().resolve_font(&f);
                        let size = px(READING.size);
                        let ascent = window.text_system().ascent(id, size);
                        let descent = window.text_system().descent(id, size);
                        let lh = layout.line_height();
                        for (ix, list) in &maths {
                            let Some(p) = layout.position_for_index(*ix) else {
                                continue;
                            };
                            // GPUI reports descent as a negative number.
                            let baseline = p.y + (lh - ascent - descent.abs()) / 2. + ascent;
                            let origin =
                                point(p.x + px(1.), baseline - px(list.height as f32 * INLINE_EM));
                            paint_math(window, cx, list, origin, INLINE_EM, ink);
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .into_any_element()
    }
}

/// Draw a RaTeX display list with GPUI primitives: glyphs in the KaTeX
/// fonts, rules as quads, delimiters and radicals as paths.
fn paint_math(
    window: &mut Window,
    _cx: &mut App,
    list: &DisplayList,
    origin: Point<Pixels>,
    em: f32,
    color: Hsla,
) {
    let at = |x: f64, y: f64| point(origin.x + px(x as f32 * em), origin.y + px(y as f32 * em));
    let hair = 1. / window.scale_factor();
    for item in &list.items {
        match item {
            DisplayItem::GlyphPath {
                x,
                y,
                scale,
                font: name,
                char_code,
                ..
            } => {
                let Some(ch) = char::from_u32(*char_code) else {
                    continue;
                };
                let size = px(em * *scale as f32);
                // GPUI skips fonts without an "m" glyph, which includes the
                // KaTeX Size fonts (big operators, stretchy delimiters), so
                // those glyphs are drawn from their outlines instead.
                if let Some(cmds) = outline(name, ch) {
                    paint_commands(
                        window,
                        &cmds,
                        at(*x, *y),
                        em * *scale as f32,
                        true,
                        color,
                        hair,
                    );
                    continue;
                }
                let (fam, style) = name.split_once('-').unwrap_or((name, "Regular"));
                let mut f = font(SharedString::from(format!("KaTeX_{fam}")));
                if style.contains("Bold") {
                    f.weight = FontWeight::BOLD;
                }
                if style.contains("Italic") {
                    f.style = FontStyle::Italic;
                }
                let s: SharedString = ch.to_string().into();
                let line = window.text_system().shape_line(
                    s.clone(),
                    size,
                    &[TextRun {
                        len: s.len(),
                        font: f,
                        color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                for run in &line.runs {
                    for g in &run.glyphs {
                        let _ = window.paint_glyph(
                            at(*x, *y) + point(g.position.x, px(0.)),
                            run.font_id,
                            g.id,
                            size,
                            color,
                        );
                    }
                }
            }
            DisplayItem::Line {
                x,
                y,
                width,
                thickness,
                ..
            } => {
                let t = (*thickness as f32 * em).max(hair);
                let o = at(*x, *y);
                window.paint_quad(fill(
                    Bounds {
                        origin: point(o.x, o.y - px(t / 2.)),
                        size: size(px(*width as f32 * em), px(t)),
                    },
                    color,
                ));
            }
            DisplayItem::Rect {
                x,
                y,
                width,
                height,
                color: c,
            } => {
                window.paint_quad(fill(
                    Bounds {
                        origin: at(*x, *y),
                        size: size(px(*width as f32 * em), px(*height as f32 * em)),
                    },
                    rgba_of(c),
                ));
            }
            DisplayItem::Path {
                x,
                y,
                commands,
                fill: filled,
                ..
            } => {
                paint_commands(window, commands, at(*x, *y), em, *filled, color, hair);
            }
        }
    }
}

/// Fill or stroke path commands given in em units from `origin`, one
/// subpath at a time because stretchy arrows mix windings.
fn paint_commands(
    window: &mut Window,
    commands: &[PathCommand],
    origin: Point<Pixels>,
    em: f32,
    filled: bool,
    color: Hsla,
    hair: f32,
) {
    let p = |x: f64, y: f64| point(origin.x + px(x as f32 * em), origin.y + px(y as f32 * em));
    let mut start = 0;
    for i in 1..=commands.len() {
        if i < commands.len() && !matches!(commands[i], PathCommand::MoveTo { .. }) {
            continue;
        }
        let mut b = if filled {
            PathBuilder::fill()
        } else {
            PathBuilder::stroke(px(hair.max(1.)))
        };
        for c in &commands[start..i] {
            match *c {
                PathCommand::MoveTo { x, y } => b.move_to(p(x, y)),
                PathCommand::LineTo { x, y } => b.line_to(p(x, y)),
                PathCommand::CubicTo {
                    x1,
                    y1,
                    x2,
                    y2,
                    x,
                    y,
                } => b.cubic_bezier_to(p(x, y), p(x1, y1), p(x2, y2)),
                PathCommand::QuadTo { x1, y1, x, y } => b.curve_to(p(x, y), p(x1, y1)),
                PathCommand::Close => b.close(),
            }
        }
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
        start = i;
    }
}

type Outlines = HashMap<(String, char), Option<Rc<Vec<PathCommand>>>>;

thread_local! {
    static OUTLINES: std::cell::RefCell<Outlines> =
        Default::default();
}

/// The glyph's outline in em units (y down, origin on the baseline), only
/// for KaTeX fonts GPUI won't load.
fn outline(name: &str, ch: char) -> Option<Rc<Vec<PathCommand>>> {
    OUTLINES.with(|cache| {
        cache
            .borrow_mut()
            .entry((name.to_string(), ch))
            .or_insert_with(|| {
                let bytes = ratex_katex_fonts::ttf_bytes(&format!("KaTeX_{name}.ttf"))?;
                let face = ttf_parser::Face::parse(&bytes, 0).ok()?;
                if face.glyph_index('m').is_some() {
                    return None;
                }
                let gid = face.glyph_index(ch)?;
                struct B(Vec<PathCommand>, f64);
                impl ttf_parser::OutlineBuilder for B {
                    fn move_to(&mut self, x: f32, y: f32) {
                        let k = self.1;
                        self.0.push(PathCommand::MoveTo {
                            x: x as f64 * k,
                            y: -y as f64 * k,
                        });
                    }
                    fn line_to(&mut self, x: f32, y: f32) {
                        let k = self.1;
                        self.0.push(PathCommand::LineTo {
                            x: x as f64 * k,
                            y: -y as f64 * k,
                        });
                    }
                    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
                        let k = self.1;
                        self.0.push(PathCommand::QuadTo {
                            x1: x1 as f64 * k,
                            y1: -y1 as f64 * k,
                            x: x as f64 * k,
                            y: -y as f64 * k,
                        });
                    }
                    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
                        let k = self.1;
                        self.0.push(PathCommand::CubicTo {
                            x1: x1 as f64 * k,
                            y1: -y1 as f64 * k,
                            x2: x2 as f64 * k,
                            y2: -y2 as f64 * k,
                            x: x as f64 * k,
                            y: -y as f64 * k,
                        });
                    }
                    fn close(&mut self) {
                        self.0.push(PathCommand::Close);
                    }
                }
                let mut b = B(Vec::new(), 1. / face.units_per_em() as f64);
                face.outline_glyph(gid, &mut b)?;
                Some(Rc::new(b.0))
            })
            .clone()
    })
}

fn rgba_of(c: &ratex_types::color::Color) -> Hsla {
    Rgba {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
    .into()
}

/// One container being built while walking a block's events.
struct Frame {
    tag: Option<Tag<'static>>,
    children: Vec<AnyElement>,
    inline: Inline,
    rows: Vec<Vec<Cell>>,
    /// A table cell's natural width and its longest word.
    natural: f32,
    min: f32,
}

/// A table cell before its column width is known.
struct Cell {
    inline: Inline,
    natural: f32,
    min: f32,
}

/// Columns never get wider than this before their text wraps…
const CELL_MAX: f32 = 320.;
/// …and a single long word never forces a column wider than this.
const CELL_MIN_CAP: f32 = 160.;
/// A column of prose never squeezes below this; the table scrolls first.
const CELL_PROSE_MIN: f32 = 140.;
/// Inline maths wider than this gets a line of its own, which scrolls.
const INLINE_MAX: f32 = 260.;
/// Diagrams shrink to fit, but not below this scale; then they scroll.
const DIAGRAM_MIN_SCALE: f32 = 0.75;

/// Natural width and longest-word width of a cell's inline content.
fn measure(inline: &Inline, window: &Window) -> (f32, f32) {
    if inline.text.is_empty() {
        return (0., 0.);
    }
    let size = px(READING.size);
    let ts = window.text_system();
    let natural = ts
        .shape_line(inline.text.clone().into(), size, &inline.runs, None)
        .width
        .as_f32();
    // Longest word, each measured in its own run's font (code is wider).
    let mut min = 0f32;
    let mut at = 0;
    for run in &inline.runs {
        let seg = &inline.text[at..at + run.len];
        at += run.len;
        for w in seg.split_whitespace() {
            let r = TextRun {
                len: w.len(),
                ..run.clone()
            };
            min = min.max(
                ts.shape_line(w.to_string().into(), size, &[r], None)
                    .width
                    .as_f32(),
            );
        }
    }
    // Shaping rounds differently from wrapping; a hair of slack keeps a
    // word that exactly fits from breaking.
    (natural + 2., min + 2.)
}

impl Frame {
    fn new(tag: Option<Tag<'static>>) -> Self {
        Self {
            tag,
            children: Vec::new(),
            inline: Inline::default(),
            rows: Vec::new(),
            natural: 0.,
            min: 0.,
        }
    }
    fn take_inline(&mut self) -> Option<Inline> {
        let inline = std::mem::take(&mut self.inline);
        (!inline.text.is_empty()).then_some(inline)
    }
}

impl Specimen {
    /// Turn a frame's pending inline text into an element, fading in what's
    /// new when revealing smoothly.
    fn flush_frame(&mut self, f: &mut Frame) {
        if let Some(inline) = f.take_inline() {
            let t = self.theme;
            let inline = self.fade_inline(inline);
            f.children.push(inline.element(&t));
        }
    }
}

/// A top-level table's rows, measured as they arrive. A complete row never
/// changes while the message streams, so each is measured once; a table
/// growing by one row costs one row.
#[derive(Default)]
struct TableBuild {
    head: Option<Rc<[Cell]>>,
    rows: Vec<Rc<[Cell]>>,
    data: Option<Rc<TableData>>,
}

impl Specimen {
    fn table_block(
        &mut self,
        b: &Block,
        aligns: &[pulldown_cmark::Alignment],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut build = self.table_rows.remove(&b.range.start).unwrap_or_default();
        let mut start = 0;
        let mut row = 0;
        for (i, ev) in b.events.iter().enumerate() {
            match ev {
                Event::Start(Tag::TableHead | Tag::TableRow) => start = i + 1,
                Event::End(TagEnd::TableHead) if build.head.is_none() => {
                    build.head = Some(self.cells(&b.events[start..i], true, window).into());
                }
                Event::End(TagEnd::TableRow) => {
                    if row >= build.rows.len() {
                        let cells = self.cells(&b.events[start..i], false, window);
                        build.rows.push(cells.into());
                    }
                    row += 1;
                }
                _ => {}
            }
        }
        build.rows.truncate(row);
        let data = match &build.data {
            Some(d) if d.rows.len() == build.rows.len() => d.clone(),
            _ => {
                let head = build.head.clone().unwrap_or_else(|| Rc::from(Vec::new()));
                Rc::new(TableData::new(head, build.rows.clone(), aligns))
            }
        };
        build.data = Some(data.clone());
        self.table_rows.insert(b.range.start, build);
        self.table_view(data, cx)
    }

    /// The cells of one table row, measured. Header cells are medium weight.
    fn cells(&mut self, events: &[Event<'static>], head: bool, window: &Window) -> Vec<Cell> {
        let t = self.theme;
        let mut cells = Vec::new();
        let mut cur = Inline::default();
        let mut st = Style::default();
        for ev in events {
            match ev {
                Event::Start(Tag::TableCell) => cur = Inline::default(),
                Event::End(TagEnd::TableCell) => {
                    let mut inline = std::mem::take(&mut cur);
                    if head {
                        for r in &mut inline.runs {
                            r.font.weight = FontWeight::MEDIUM;
                        }
                    }
                    let (natural, min) = measure(&inline, window);
                    cells.push(Cell {
                        inline,
                        natural,
                        min,
                    });
                }
                Event::Start(Tag::Strong) => st.strong = true,
                Event::End(TagEnd::Strong) => st.strong = false,
                Event::Start(Tag::Emphasis) => st.emph = true,
                Event::End(TagEnd::Emphasis) => st.emph = false,
                Event::Start(Tag::Strikethrough) => st.strike = true,
                Event::End(TagEnd::Strikethrough) => st.strike = false,
                Event::Start(Tag::Link { .. }) => st.link = true,
                Event::End(TagEnd::Link) => st.link = false,
                Event::Text(s) | Event::Html(s) | Event::InlineHtml(s) => cur.push(s, st, &t),
                Event::Code(s) => cur.push(s, Style { code: true, ..st }, &t),
                Event::InlineMath(s) => match self.typeset(s, false) {
                    Some(l) => cur.push_math(l, s, window, &t),
                    None => cur.push(&format!("${s}$"), Style { code: true, ..st }, &t),
                },
                Event::SoftBreak | Event::HardBreak => cur.push(" ", st, &t),
                _ => {}
            }
        }
        cells
    }
}

impl Specimen {
    fn block(
        &mut self,
        b: &Block,
        done: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.closed = done;
        self.hs_count = 0;
        self.block_start = b.range.start;
        self.inline_ix = 0;
        if let Some(Event::Start(Tag::Table(aligns))) = b.events.first() {
            return self.table_block(b, aligns, window, cx);
        }
        let t = self.theme;
        let mut stack = vec![Frame::new(None)];
        let mut st = Style::default();
        let mut code = String::new();
        let mut code_lang = String::new();
        for ev in &b.events {
            match ev {
                Event::Start(tag) => match tag {
                    Tag::Strong => st.strong = true,
                    Tag::Emphasis => st.emph = true,
                    Tag::Strikethrough => st.strike = true,
                    Tag::Link { .. } => st.link = true,
                    Tag::CodeBlock(kind) => {
                        code.clear();
                        code_lang = match kind {
                            CodeBlockKind::Fenced(l) => l.to_string(),
                            _ => String::new(),
                        };
                        stack.push(Frame::new(Some(tag.clone())));
                    }
                    _ => {
                        if let Some(top) = stack.last_mut() {
                            self.flush_frame(top);
                        }
                        stack.push(Frame::new(Some(tag.clone())));
                    }
                },
                Event::End(end) => match end {
                    TagEnd::Strong => st.strong = false,
                    TagEnd::Emphasis => st.emph = false,
                    TagEnd::Strikethrough => st.strike = false,
                    TagEnd::Link => st.link = false,
                    _ => {
                        let Some(mut f) = stack.pop() else { break };
                        if matches!(f.tag, Some(Tag::TableCell)) {
                            // Header cells are medium weight, so measure them that way.
                            let head = stack
                                .last()
                                .is_some_and(|p| matches!(p.tag, Some(Tag::TableHead)));
                            if head {
                                for r in &mut f.inline.runs {
                                    r.font.weight = FontWeight::MEDIUM;
                                }
                            }
                            (f.natural, f.min) = measure(&f.inline, window);
                        } else {
                            self.flush_frame(&mut f);
                        }
                        let el = self.finish(f, &code, &code_lang, window, cx);
                        if let Some(parent) = stack.last_mut() {
                            match el {
                                Built::Cell(c) => {
                                    if let Some(row) = parent.rows.last_mut() {
                                        row.push(c)
                                    } else {
                                        parent.rows.push(vec![c])
                                    }
                                }
                                Built::Row(cells) => parent.rows.push(cells),
                                Built::Rows(rows) => parent.rows.extend(rows),
                                Built::El(e) => parent.children.push(e),
                            }
                        }
                    }
                },
                Event::Text(s) => {
                    let in_code = stack
                        .last()
                        .is_some_and(|f| matches!(f.tag, Some(Tag::CodeBlock(_))));
                    if in_code {
                        code.push_str(s);
                    } else if let Some(top) = stack.last_mut() {
                        top.inline.push(s, st, &t);
                    }
                }
                Event::Code(s) => {
                    if let Some(top) = stack.last_mut() {
                        top.inline.push(s, Style { code: true, ..st }, &t);
                    }
                }
                Event::InlineMath(s) => {
                    let list = self.typeset(s, false);
                    // Too wide to sit in a line: give it its own, which scrolls.
                    if let Some(l) = list
                        .clone()
                        .filter(|l| l.width as f32 * INLINE_EM > INLINE_MAX)
                    {
                        let el = self.inline_math_line(l, &t);
                        if let Some(top) = stack.last_mut() {
                            self.flush_frame(top);
                            top.children.push(el);
                        }
                        continue;
                    }
                    if let Some(top) = stack.last_mut() {
                        match list {
                            Some(l) => top.inline.push_math(l, s, window, &t),
                            None => {
                                top.inline
                                    .push(&format!("${s}$"), Style { code: true, ..st }, &t)
                            }
                        }
                    }
                }
                Event::DisplayMath(s) => {
                    let list = self.typeset(s, true);
                    let el = match list {
                        Some(l) => self.display_math(l, s, cx),
                        None => self.code_block(s, "tex", cx),
                    };
                    if let Some(top) = stack.last_mut() {
                        self.flush_frame(top);
                        top.children.push(el);
                    }
                }
                Event::SoftBreak => {
                    if let Some(top) = stack.last_mut() {
                        top.inline.push(" ", st, &t);
                    }
                }
                Event::HardBreak => {
                    if let Some(top) = stack.last_mut() {
                        self.flush_frame(top);
                    }
                }
                Event::Rule => {
                    if let Some(top) = stack.last_mut() {
                        top.children.push(
                            div()
                                .h(px(1.))
                                .bg(t.hairline)
                                .my(px(space::S3))
                                .into_any_element(),
                        );
                    }
                }
                Event::TaskListMarker(done) => {
                    if let Some(top) = stack.last_mut() {
                        top.inline.push(if *done { "☑ " } else { "☐ " }, st, &t);
                    }
                }
                _ => {}
            }
        }
        let mut root = stack.pop().unwrap_or_else(|| Frame::new(None));
        self.flush_frame(&mut root);
        div()
            .flex()
            .flex_col()
            .children(root.children)
            .into_any_element()
    }

    fn finish(
        &mut self,
        f: Frame,
        code: &str,
        lang: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Built {
        let t = self.theme;
        match f.tag {
            Some(Tag::Paragraph) => Built::El(div().children(f.children).into_any_element()),
            Some(Tag::Heading { level, .. }) => {
                let size = match level {
                    HeadingLevel::H1 => 22.,
                    HeadingLevel::H2 => 19.,
                    _ => 16.,
                };
                Built::El(
                    div()
                        .pt(px(space::S2))
                        .font_weight(FontWeight::MEDIUM)
                        .text_size(px(size))
                        .line_height(px(size + 8.))
                        .children(f.children)
                        .into_any_element(),
                )
            }
            Some(Tag::List(start)) => {
                let items = f.children.into_iter().enumerate().map(|(i, item)| {
                    let mark = match start {
                        Some(n) => format!("{}.", n + i as u64),
                        None => "•".into(),
                    };
                    // A padded block with the marker out of flow, not a flex
                    // row: nested flex rows re-measure their text at every
                    // level, which doubles the cost per level of nesting.
                    div()
                        .relative()
                        .pl(px(18. + space::S2))
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .text_color(t.quiet)
                                .child(mark),
                        )
                        .child(item)
                });
                Built::El(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::S1))
                        .children(items)
                        .into_any_element(),
                )
            }
            Some(Tag::Item) => Built::El(div().children(f.children).into_any_element()),
            Some(Tag::BlockQuote(_)) => Built::El(
                div()
                    .border_l_2()
                    .border_color(t.hairline)
                    .pl(px(space::S3))
                    .text_color(t.quiet)
                    .children(f.children)
                    .into_any_element(),
            ),
            Some(Tag::CodeBlock(_)) => {
                // A diagram renders once its fence is closed; until then
                // its source shows dimmed.
                if lang == "mermaid" && self.closed {
                    Built::El(self.mermaid(code, cx))
                } else if lang == "mermaid" {
                    Built::El(
                        div()
                            .opacity(0.5)
                            .child(self.code_block(code, lang, cx))
                            .into_any_element(),
                    )
                } else {
                    Built::El(self.code_block(code, lang, cx))
                }
            }
            Some(Tag::TableCell) => Built::Cell(Cell {
                inline: f.inline,
                natural: f.natural,
                min: f.min,
            }),
            Some(Tag::TableHead) => Built::Row(f.rows.into_iter().flatten().collect()),
            Some(Tag::TableRow) => Built::Row(f.rows.into_iter().flatten().collect()),
            Some(Tag::Table(aligns)) => Built::El(self.table(f.rows, &aligns, cx)),
            _ => Built::El(div().children(f.children).into_any_element()),
        }
    }

    fn mermaid(&mut self, src: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        self.diagram(src, cx);
        match self.diagrams.get(src).map(|d| match d {
            Diagram::Ready(i, w, h) => Diagram::Ready(i.clone(), *w, *h),
            Diagram::Failed(e) => Diagram::Failed(e.clone()),
            Diagram::Pending => Diagram::Pending,
        }) {
            // Shrink to fit the column, but never so far that labels get
            // unreadable; past that the diagram scrolls sideways.
            Some(Diagram::Ready(img_, w, h)) => {
                let n = self.hs_count;
                let body = self.hscroll(
                    div()
                        .w_full()
                        .min_w(px(w * DIAGRAM_MIN_SCALE))
                        .flex()
                        .justify_center()
                        .child(
                            img(ImageSource::Render(img_))
                                .w_full()
                                .max_w(px(w))
                                .aspect_ratio(w / h),
                        ),
                    t.surface,
                );
                if self.reveal != Reveal::Rate {
                    return body;
                }
                let key = format!("copy-mermaid-{}", self.block_ix);
                let copy = self.copy_button(key, "Copy source", chrome::text(src.to_string()), cx);
                hover_actions(body, copy, self.block_ix, n)
            }
            Some(Diagram::Failed(e)) => {
                div()
                    .flex()
                    .flex_col()
                    .child(self.code_block(src, "mermaid", cx))
                    .child(
                        div()
                            .ty(LABEL)
                            .text_color(t.quiet)
                            .child(format!("diagram failed: {e}")),
                    )
            }
            .into_any_element(),
            _ => div()
                .opacity(0.5)
                .child(self.code_block(src, "mermaid", cx))
                .into_any_element(),
        }
    }
}

enum Built {
    El(AnyElement),
    Cell(Cell),
    Row(Vec<Cell>),
    #[allow(dead_code)]
    Rows(Vec<Vec<Cell>>),
}

impl Specimen {
    /// Wrap content that may be wider than the column in a horizontal
    /// scroller, with a fade on whichever edge has more to see.
    fn hscroll(&mut self, child: impl IntoElement, bg: Hsla) -> AnyElement {
        let key = (self.block_ix, self.hs_count);
        self.hs_count += 1;
        let handle = self.hscrolls.entry(key).or_default().clone();
        let max = handle.max_offset().x.as_f32();
        let x = -handle.offset().x.as_f32();
        let fade = |right: bool| {
            let (a, b) = if right {
                (bg.opacity(0.), bg)
            } else {
                (bg, bg.opacity(0.))
            };
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(28.))
                .when(right, |d| d.right_0())
                .when(!right, |d| d.left_0())
                .bg(linear_gradient(
                    90.,
                    linear_color_stop(a, 0.),
                    linear_color_stop(b, 1.),
                ))
        };
        div()
            .relative()
            .w_full()
            .child(
                div()
                    .id(SharedString::from(format!("h{}-{}", key.0, key.1)))
                    .track_scroll(&handle)
                    .overflow_x_scroll()
                    // Vertical wheel motion belongs to the chat, never turned sideways.
                    .restrict_scroll_to_axis()
                    .w_full()
                    .child(child),
            )
            .when(max > 0.5 && x < max - 0.5, |d| d.child(fade(true)))
            .when(x > 0.5, |d| d.child(fade(false)))
            .into_any_element()
    }

    fn display_math(
        &mut self,
        list: Rc<DisplayList>,
        src: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let w = list.width as f32 * DISPLAY_EM;
        let h = (list.height + list.depth) as f32 * DISPLAY_EM;
        let ink = t.ink;
        let n = self.hs_count;
        let body = self.hscroll(
            div()
                .w_full()
                .min_w(px(w))
                .flex()
                .justify_center()
                .py(px(space::S2))
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, cx| {
                            paint_math(window, cx, &list, bounds.origin, DISPLAY_EM, ink)
                        },
                    )
                    .flex_none()
                    .w(px(w))
                    .h(px(h)),
                ),
            t.surface,
        );
        // The comparison panes are watched, not used: no hover actions.
        if self.reveal != Reveal::Rate {
            return body;
        }
        let key = format!("copy-tex-{}-{n}", self.block_ix);
        let copy = self.copy_button(key, "Copy TeX", chrome::text(src.trim().to_string()), cx);
        hover_actions(body, copy, self.block_ix, n)
    }

    /// Inline maths too wide for a line, at text size on a line of its own.
    fn inline_math_line(&mut self, list: Rc<DisplayList>, t: &Theme) -> AnyElement {
        let w = list.width as f32 * INLINE_EM;
        let h = (list.height + list.depth) as f32 * INLINE_EM;
        let ink = t.ink;
        self.hscroll(
            div().py(px(space::S1)).child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        paint_math(window, cx, &list, bounds.origin, INLINE_EM, ink)
                    },
                )
                .flex_none()
                .w(px(w + 2.))
                .h(px(h)),
            ),
            t.surface,
        )
    }

    /// Code never wraps; long lines scroll sideways inside the block.
    fn code_block(&mut self, src: &str, lang: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let body = div()
            .px(px(space::S3))
            .py(px(space::S2))
            .ty(CODE)
            .text_color(t.ink)
            .children(
                src.trim_end_matches('\n')
                    .split('\n')
                    .map(|l| div().whitespace_nowrap().child(l.to_string())),
            );
        let key = format!("copy-code-{}-{}", self.block_ix, self.hs_count);
        let body = self.hscroll(body, t.raised);
        let copy = self.copy_button(
            key,
            "Copy",
            chrome::text(src.trim_end_matches('\n').to_string()),
            cx,
        );
        div()
            .flex()
            .flex_col()
            .bg(t.raised)
            .border_1()
            .border_color(t.hairline)
            .rounded(px(radius::LG))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(space::S3))
                    .pr(px(space::S1))
                    .py(px(2.))
                    .border_b_1()
                    .border_color(t.hairline)
                    .child(
                        div()
                            .ty(LABEL)
                            .text_color(t.quiet)
                            .child(if lang.is_empty() {
                                "code".to_string()
                            } else {
                                lang.to_string()
                            }),
                    )
                    .child(copy),
            )
            .child(body)
            .into_any_element()
    }
}

/// Actions that appear in the top-right corner while the pointer is over
/// a block with no header of its own (display maths, diagrams).
fn hover_actions(body: AnyElement, actions: AnyElement, block: usize, n: usize) -> AnyElement {
    let group = SharedString::from(format!("hover-{block}-{n}"));
    div()
        .group(group.clone())
        .relative()
        .child(body)
        .child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .opacity(0.)
                .group_hover(group, |s| s.opacity(1.))
                .child(actions),
        )
        .into_any_element()
}

impl Render for Specimen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let streaming = self.advance();
        if self.verify.is_some() {
            self.verify_tick(window, cx);
            window.request_animation_frame();
        } else if self.bench {
            for e in self.collector.collect_unseen() {
                if let profiler::FrameEvent::Draw(f) = e {
                    self.draws.push(f.draw_duration().as_secs_f64() * 1e3);
                }
            }
            self.report(window, cx);
        }
        if streaming
            || self
                .diagrams
                .values()
                .any(|d| matches!(d, Diagram::Pending))
            || self.bench
            || self
                .copied
                .as_ref()
                .is_some_and(|(_, at)| at.elapsed().as_secs_f32() < 1.3)
        {
            window.request_animation_frame();
        }
        // Probes and copy texts describe this frame only; stale entries from
        // streaming frames (when block numbers differed) would mislead.
        self.probes.borrow_mut().clear();
        self.copy_texts.clear();
        let t = self.theme;
        let n = self.streamer.committed.len();
        let built = Instant::now();
        // Borrow the blocks rather than copy their events every frame: the
        // streamer is set aside while elements are built, then put back.
        let streamer = std::mem::take(&mut self.streamer);
        self.fading = false;
        let blocks: Box<dyn Iterator<Item = &Block>> = if self.reveal == Reveal::Blocks {
            // A block is shown once another block has started after it: only
            // the one still being written is held back.
            let shown = streamer.tail.len().saturating_sub(1);
            Box::new(streamer.committed.iter().chain(&streamer.tail[..shown]))
        } else {
            Box::new(streamer.visible())
        };
        let els: Vec<AnyElement> = blocks
            .enumerate()
            .map(|(i, b)| {
                self.block_ix = i;
                let el = self.block(b, i < n, window, cx);
                if self.reveal == Reveal::Blocks {
                    let a = self.block_alpha(b.range.start);
                    div().opacity(a).child(el).into_any_element()
                } else {
                    el
                }
            })
            .collect();
        self.streamer = streamer;
        if self.fading {
            window.request_animation_frame();
        }
        // The panes follow the end of the reply, as a chat does.
        if self.reveal != Reveal::Rate
            && self.done_at.is_none_or(|d| d.elapsed().as_secs_f32() < 1.)
        {
            self.scroll.scroll_to_bottom();
        }
        self.builds.push(built.elapsed().as_secs_f64() * 1e3);
        let panel = self.panel(window, cx);
        div()
            .size_full()
            .relative()
            .bg(t.surface)
            .text_color(t.ink)
            .flex()
            .justify_center()
            .child(
                div()
                    .id("column")
                    .track_scroll(&self.scroll)
                    .overflow_y_scroll()
                    // A sideways swipe is for wide blocks, never turned into
                    // vertical scrolling of the chat.
                    .restrict_scroll_to_axis()
                    .w_full()
                    .max_w(px(crate::ds::size::COLUMN + 64.))
                    .px(px(if self.reveal == Reveal::Rate {
                        space::S8
                    } else {
                        space::S4
                    }))
                    .py(px(space::S6))
                    .ty(READING)
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .children(els),
            )
            .children(panel)
    }
}

impl Specimen {
    fn report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(done) = self.done_at else { return };
        let settled = !self
            .diagrams
            .values()
            .any(|d| matches!(d, Diagram::Pending));
        if !settled || done.elapsed().as_secs_f32() < 0.6 {
            return;
        }
        // Capture the page a viewport at a time, top to bottom, so the
        // whole document can be stitched back together.
        if let Some(dir) = self.shots.clone() {
            let _ = std::fs::create_dir_all(&dir);
            let name = format!(
                "{}-{}-{}x{}",
                if self.rate > RATE {
                    "04-stress"
                } else {
                    "04-specimen"
                },
                if self.theme.dark { "carbon" } else { "paper" },
                crate::window().2,
                crate::window().3
            );
            let max = self.scroll.max_offset().y.as_f32();
            let view = self.scroll.bounds().size.height.as_f32();
            match self.paged_at {
                None => {
                    // Pages overlap a little so window corners never land in the stitch.
                    let y = (self.shot as f32 * (view - 60.)).min(max);
                    self.scroll.set_offset(point(px(0.), px(-y)));
                    self.offsets.push(y);
                    self.paged_at = Some(Instant::now());
                    return;
                }
                Some(at) if at.elapsed().as_secs_f32() < 0.3 => return,
                Some(at) if at.elapsed().as_secs_f32() < 0.5 => {
                    if self.offsets.len() == self.shot + 1 {
                        crate::demo::still(dir.join(format!("{name}-p{:02}.png", self.shot)));
                        self.offsets.push(f32::NAN);
                    }
                    return;
                }
                Some(_) => {
                    self.offsets.pop();
                    let y = self.offsets[self.shot];
                    self.shot += 1;
                    self.paged_at = None;
                    if y < max {
                        return;
                    }
                    let meta = format!(
                        "{{\"view\": {view}, \"max\": {max}, \"offsets\": {:?}}}",
                        self.offsets
                    );
                    let _ = std::fs::write(dir.join(format!("{name}.json")), meta);
                }
            }
        } else if done.elapsed().as_secs_f32() < 1.2 {
            return;
        }
        let mut d = self.draws.clone();
        let over = d.iter().filter(|&&x| x > 8.3).count();
        let after = self.draws_at_done.unwrap_or(0).min(d.len());
        let mut settled = d[after..].to_vec();
        eprintln!(
            "streaming: {} frames p50 {:.2} p99 {:.2} · finished: {} frames p50 {:.2} p99 {:.2} max {:.2}",
            after,
            spike_markdown::pct(&mut d[..after].to_vec(), 50.),
            spike_markdown::pct(&mut d[..after].to_vec(), 99.),
            settled.len(),
            spike_markdown::pct(&mut settled.clone(), 50.),
            spike_markdown::pct(&mut settled.clone(), 99.),
            spike_markdown::pct(&mut settled, 100.)
        );
        let mut bl = self.builds.clone();
        eprintln!(
            "build p50 {:.2} ms p99 {:.2} ms",
            spike_markdown::pct(&mut bl.clone(), 50.),
            spike_markdown::pct(&mut bl, 99.)
        );
        println!(
            "{{\"frames\": {}, \"p50\": {:.2}, \"p95\": {:.2}, \"p99\": {:.2}, \"max\": {:.2}, \"over_8_3ms\": {over}}}",
            d.len(),
            spike_markdown::pct(&mut d.clone(), 50.),
            spike_markdown::pct(&mut d.clone(), 95.),
            spike_markdown::pct(&mut d.clone(), 99.),
            spike_markdown::pct(&mut d, 100.)
        );
        let _ = window;
        cx.quit();
    }
}

pub fn load_katex(cx: &mut App) {
    let names = [
        "AMS-Regular",
        "Caligraphic-Bold",
        "Caligraphic-Regular",
        "Fraktur-Bold",
        "Fraktur-Regular",
        "Main-Bold",
        "Main-BoldItalic",
        "Main-Italic",
        "Main-Regular",
        "Math-BoldItalic",
        "Math-Italic",
        "SansSerif-Bold",
        "SansSerif-Italic",
        "SansSerif-Regular",
        "Script-Regular",
        "Size1-Regular",
        "Size2-Regular",
        "Size3-Regular",
        "Size4-Regular",
        "Typewriter-Regular",
    ];
    let fonts = names
        .iter()
        .filter_map(|n| ratex_katex_fonts::ttf_bytes(&format!("KaTeX_{n}.ttf")))
        .collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("katex fonts failed: {e:#}");
    }
    if std::env::var("DENT_DEBUG_FONTS").is_ok() {
        let names = cx.text_system().all_font_names();
        eprintln!(
            "{:?}",
            names
                .iter()
                .filter(|n| n.contains("KaTeX"))
                .collect::<Vec<_>>()
        );
        let id = cx.text_system().resolve_font(&font(theme::body()));
        let s = px(READING.size);
        for fam in ["KaTeX_Main", "KaTeX_Size1", "KaTeX_Size2", "KaTeX_Size4"] {
            let f = font(fam);
            let id = cx.text_system().resolve_font(&f);
            let w = cx.text_system().layout_width(id, px(20.), '∑');
            let a = cx.text_system().ascent(id, px(20.));
            eprintln!("{fam}: id {id:?} ∑ width {w:?} ascent {a:?}");
        }
        eprintln!(
            "body ascent {:?} descent {:?}",
            cx.text_system().ascent(id, s),
            cx.text_system().descent(id, s)
        );
    }
}
