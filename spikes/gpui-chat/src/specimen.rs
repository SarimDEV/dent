//! Spike 04 in GPUI: a message streams through the stable-prefix layer and
//! renders as a block tree, with TeX drawn natively from RaTeX's display
//! list and Mermaid rasterised off the UI thread.
//!
//! `just dev spike-gpui-chat --specimen [--dark] [--bench] [--shots DIR]`

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
use crate::theme::{self, Theme};

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

enum Diagram {
    Pending,
    Ready(Arc<RenderImage>, f32, f32),
    Failed(String),
}

pub struct Specimen {
    theme: Theme,
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
    shot: u8,
}

impl Specimen {
    pub fn new(dark: bool, bench: bool, shots: Option<PathBuf>) -> Self {
        Self {
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
        }
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
        let want = ((self.start.elapsed().as_secs_f32() - 0.4).max(0.) * RATE) as usize;
        let mut at = self.streamer.text.len();
        if at >= MESSAGE.len() {
            if self.done_at.is_none() {
                self.streamer.finish();
                self.done_at = Some(Instant::now());
            }
            return false;
        }
        let mut end = want.min(MESSAGE.len());
        while !MESSAGE.is_char_boundary(end) {
            end += 1;
        }
        if end > at {
            self.streamer.push(&MESSAGE[at..end]);
            at = end;
        }
        at < MESSAGE.len()
    }
}

/// Inline content of one paragraph-like block, ready for a StyledText.
#[derive(Default)]
struct Inline {
    text: String,
    runs: Vec<TextRun>,
    math: Vec<(usize, Rc<DisplayList>)>,
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
    fn push_math(&mut self, list: Rc<DisplayList>, window: &Window, t: &Theme) {
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

fn display_math(list: Rc<DisplayList>, t: &Theme) -> AnyElement {
    let w = list.width as f32 * DISPLAY_EM;
    let h = (list.height + list.depth) as f32 * DISPLAY_EM;
    let ink = t.ink;
    div()
        .w_full()
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
            .w(px(w))
            .h(px(h)),
        )
        .into_any_element()
}

/// One container being built while walking a block's events.
struct Frame {
    tag: Option<Tag<'static>>,
    children: Vec<AnyElement>,
    inline: Inline,
    rows: Vec<Vec<AnyElement>>,
}

impl Frame {
    fn new(tag: Option<Tag<'static>>) -> Self {
        Self {
            tag,
            children: Vec::new(),
            inline: Inline::default(),
            rows: Vec::new(),
        }
    }
    fn flush(&mut self, t: &Theme) {
        let inline = std::mem::take(&mut self.inline);
        if !inline.text.is_empty() {
            self.children.push(inline.element(t));
        }
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
                            top.flush(&t);
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
                        f.flush(&t);
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
                    if let Some(top) = stack.last_mut() {
                        match list {
                            Some(l) => top.inline.push_math(l, window, &t),
                            None => {
                                top.inline
                                    .push(&format!("${s}$"), Style { code: true, ..st }, &t)
                            }
                        }
                    }
                }
                Event::DisplayMath(s) => {
                    let list = self.typeset(s, true);
                    if let Some(top) = stack.last_mut() {
                        top.flush(&t);
                        top.children.push(match list {
                            Some(l) => display_math(l, &t),
                            None => code_block(s, "tex", &t),
                        });
                    }
                }
                Event::SoftBreak => {
                    if let Some(top) = stack.last_mut() {
                        top.inline.push(" ", st, &t);
                    }
                }
                Event::HardBreak => {
                    if let Some(top) = stack.last_mut() {
                        top.flush(&t);
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
        root.flush(&t);
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
                    div()
                        .flex()
                        .gap(px(space::S2))
                        .child(div().w(px(18.)).flex_none().text_color(t.quiet).child(mark))
                        .child(div().flex_1().min_w_0().child(item))
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
            Some(Tag::Item) => Built::El(
                div()
                    .flex()
                    .flex_col()
                    .children(f.children)
                    .into_any_element(),
            ),
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
                            .child(code_block(code, lang, &t))
                            .into_any_element(),
                    )
                } else {
                    Built::El(code_block(code, lang, &t))
                }
            }
            Some(Tag::TableCell) => Built::Cell(
                div()
                    .flex_1()
                    .px(px(space::S2))
                    .py(px(space::S1))
                    .children(f.children)
                    .into_any_element(),
            ),
            Some(Tag::TableHead) => Built::Row(f.rows.into_iter().flatten().collect()),
            Some(Tag::TableRow) => Built::Row(f.rows.into_iter().flatten().collect()),
            Some(Tag::Table(_)) => {
                let n = f.rows.len();
                Built::El(
                    div()
                        .flex()
                        .flex_col()
                        .border_1()
                        .border_color(t.hairline)
                        .rounded(px(radius::MD))
                        .overflow_hidden()
                        .children(f.rows.into_iter().enumerate().map(|(i, cells)| {
                            div()
                                .flex()
                                .when(i == 0, |d| {
                                    d.bg(t.ink.opacity(0.04)).font_weight(FontWeight::MEDIUM)
                                })
                                .when(i + 1 < n, |d| d.border_b_1().border_color(t.hairline))
                                .children(cells)
                        }))
                        .into_any_element(),
                )
            }
            _ => Built::El(div().children(f.children).into_any_element()),
        }
    }

    fn mermaid(&mut self, src: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        self.diagram(src, cx);
        match self.diagrams.get(src) {
            Some(Diagram::Ready(img_, w, h)) => div()
                .w_full()
                .flex()
                .justify_center()
                .child(img(ImageSource::Render(img_.clone())).w(px(*w)).h(px(*h)))
                .into_any_element(),
            Some(Diagram::Failed(e)) => {
                div()
                    .flex()
                    .flex_col()
                    .child(code_block(src, "mermaid", &t))
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
                .child(code_block(src, "mermaid", &t))
                .into_any_element(),
        }
    }
}

enum Built {
    El(AnyElement),
    Cell(AnyElement),
    Row(Vec<AnyElement>),
    #[allow(dead_code)]
    Rows(Vec<Vec<AnyElement>>),
}

fn code_block(src: &str, lang: &str, t: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .bg(t.raised)
        .border_1()
        .border_color(t.hairline)
        .rounded(px(radius::LG))
        .child(
            div()
                .px(px(space::S3))
                .py(px(space::S1))
                .border_b_1()
                .border_color(t.hairline)
                .ty(LABEL)
                .text_color(t.quiet)
                .child(if lang.is_empty() {
                    "code".to_string()
                } else {
                    lang.to_string()
                }),
        )
        .child(
            div()
                .px(px(space::S3))
                .py(px(space::S2))
                .ty(CODE)
                .text_color(t.ink)
                .children(
                    src.trim_end_matches('\n')
                        .split('\n')
                        .map(|l| div().child(l.to_string())),
                ),
        )
        .into_any_element()
}

impl Render for Specimen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let streaming = self.advance();
        if self.bench {
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
        {
            window.request_animation_frame();
        }
        let t = self.theme;
        let n = self.streamer.committed.len();
        let blocks: Vec<Block> = self.streamer.visible().cloned().collect();
        let els: Vec<AnyElement> = blocks
            .iter()
            .enumerate()
            .map(|(i, b)| self.block(b, i < n, window, cx))
            .collect();
        div()
            .size_full()
            .bg(t.surface)
            .text_color(t.ink)
            .flex()
            .justify_center()
            .child(
                div()
                    .id("column")
                    .track_scroll(&self.scroll)
                    .overflow_y_scroll()
                    .w_full()
                    .max_w(px(crate::ds::size::COLUMN + 64.))
                    .px(px(space::S8))
                    .py(px(space::S6))
                    .ty(READING)
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .children(els),
            )
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
        if let Some(dir) = self.shots.clone() {
            let _ = std::fs::create_dir_all(&dir);
            let name = if self.theme.dark { "carbon" } else { "paper" };
            let t = done.elapsed().as_secs_f32();
            if self.shot == 0 {
                crate::demo::still(dir.join(format!("04-specimen-{name}-top.png")));
                self.shot = 1;
            } else if self.shot == 1 && t > 0.9 {
                let max = self.scroll.max_offset();
                self.scroll.set_offset(point(px(0.), -max.y));
                self.shot = 2;
            } else if self.shot == 2 && t > 1.2 {
                crate::demo::still(dir.join(format!("04-specimen-{name}-bottom.png")));
                self.shot = 3;
            }
            if self.shot < 3 || t < 1.5 {
                return;
            }
        } else if done.elapsed().as_secs_f32() < 1.2 {
            return;
        }
        let mut d = self.draws.clone();
        let over = d.iter().filter(|&&x| x > 8.3).count();
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
