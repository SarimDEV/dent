//! Markdown to Field Notes elements, enough for replies and the doc viewer:
//! headings, paragraphs, lists and task lists, quotes, code, tables, rules,
//! maths (drawn natively, see `math`) and Mermaid diagrams (see `diagram`).
//!
//! Parsing is pure. Rendering keeps a little state across frames in [`Md`]:
//! typeset maths, finished diagrams, scroll positions of wide blocks and
//! tall tables, and which Copy button was just pressed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ui::Press;

use crate::ui::RevealOnHover;
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use ratex_types::display_item::DisplayList;

use crate::diagram;
use crate::ds::{CODE, DISPLAY, DISPLAY_COMPACT, LABEL, READING, Type, Typed, UI, radius, space};
use crate::math;
use crate::theme::{self, Theme};

type Probes = Rc<RefCell<HashMap<String, Bounds<Pixels>>>>;

#[derive(Clone, Copy, Default)]
struct Style {
    strong: bool,
    emph: bool,
    code: bool,
    link: bool,
    strike: bool,
}

/// One paragraph's text with its styled runs.
#[derive(Default, Clone)]
pub struct Inline {
    pub text: String,
    runs: Vec<(usize, Style)>,
    /// Byte ranges that are links, and where they go.
    links: Vec<(std::ops::Range<usize>, String)>,
    /// Where the link being read goes.
    href: Option<String>,
    /// Inline maths: where in `text` it sits (always between runs) and its TeX.
    math: Vec<(usize, String)>,
    /// Pictures read while this paragraph had no text yet.
    images: Vec<(String, String)>,
}

/// How an inline's text is set: its family, size and colour, and what it
/// sits on (for the edge fade of maths too wide for a line).
#[derive(Clone, Copy)]
pub struct Face {
    pub family: &'static str,
    pub size: f32,
    pub ink: Hsla,
    pub bg: Hsla,
    /// The text's own weight. Runs carry their fonts, so a weight set on
    /// the element around them never reaches them; it goes here.
    pub weight: FontWeight,
}

impl Face {
    pub fn of(ty: Type, family: &'static str, ink: Hsla, bg: Hsla) -> Self {
        Self {
            family,
            size: ty.size,
            ink,
            bg,
            weight: FontWeight::NORMAL,
        }
    }

    pub fn weighted(self, weight: FontWeight) -> Self {
        Self { weight, ..self }
    }
}

/// Inline maths is set a little larger than the text around it, as TeX
/// fonts run small: 17 px against 15 px reading text.
const INLINE_EM: f32 = 17. / 15.;
/// Display maths at reading size.
const DISPLAY_EM: f32 = 19.;
/// Inline maths wider than this (at reading size) gets a line of its own,
/// which scrolls.
const INLINE_MAX: f32 = 260.;
/// Diagrams shrink to fit, but not below this scale; then they scroll.
const DIAGRAM_MIN_SCALE: f32 = 0.75;

impl Inline {
    fn push(&mut self, s: &str, st: Style) {
        if s.is_empty() {
            return;
        }
        if st.link
            && let Some(url) = &self.href
        {
            let at = self.text.len();
            match self.links.last_mut() {
                Some((r, u)) if r.end == at && u == url => r.end += s.len(),
                _ => self.links.push((at..at + s.len(), url.clone())),
            }
        }
        self.text.push_str(s);
        self.runs.push((s.len(), st));
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.math.is_empty()
    }

    /// The text as written, with maths back in dollars. `code` keeps code
    /// spans in backticks, for Markdown.
    fn plain(&self, code: bool) -> String {
        let mut out = String::new();
        let mut at = 0;
        let mut maths = self.math.iter().peekable();
        for (len, st) in &self.runs {
            while let Some((_, tex)) = maths.next_if(|(m, _)| *m <= at) {
                out.push_str(&format!("${tex}$"));
            }
            let piece = &self.text[at..at + len];
            if code && st.code {
                out.push_str(&format!("`{piece}`"));
            } else {
                out.push_str(piece);
            }
            at += len;
        }
        for (_, tex) in maths {
            out.push_str(&format!("${tex}$"));
        }
        out
    }

    /// The part of this inline between two byte offsets (at run boundaries).
    fn slice(&self, from: usize, to: usize) -> Inline {
        let mut out = Inline::default();
        let mut at = 0;
        for (len, st) in &self.runs {
            let (s, e) = (at.max(from), (at + len).min(to));
            if s < e {
                out.text.push_str(&self.text[s..e]);
                out.runs.push((e - s, *st));
            }
            at += len;
        }
        out.links = self
            .links
            .iter()
            .filter(|(r, _)| r.start < to && r.end > from)
            .map(|(r, u)| (r.start.max(from) - from..r.end.min(to) - from, u.clone()))
            .collect();
        out.math = self
            .math
            .iter()
            .filter(|(m, _)| *m >= from && *m <= to)
            .map(|(m, tex)| (m - from, tex.clone()))
            .collect();
        out
    }

    /// `fade` gives each byte offset an opacity, for words streaming in.
    pub fn element(
        &self,
        key: &str,
        t: &Theme,
        face: Face,
        fade: Option<&dyn Fn(usize) -> f32>,
        md: &Md,
        window: &mut Window,
    ) -> AnyElement {
        let em = face.size * INLINE_EM;
        let lists: Vec<Option<Rc<DisplayList>>> = self
            .math
            .iter()
            .map(|(_, tex)| md.typeset(tex, false))
            .collect();
        // Maths too wide for a line gets one of its own, between the text
        // before and after it.
        let max = INLINE_MAX * face.size / READING.size;
        if let Some(i) = lists
            .iter()
            .position(|l| l.as_ref().is_some_and(|l| l.width as f32 * em > max))
        {
            let at = self.math[i].0;
            let mut before = self.slice(0, at);
            before.math = self.math[..i].to_vec();
            let mut after = self.slice(at, self.text.len());
            after.math = self.math[i + 1..]
                .iter()
                .map(|(m, tex)| (m - at, tex.clone()))
                .collect();
            let list = lists[i].clone();
            let mut col = div().flex().flex_col();
            if !before.is_empty() {
                col = col.child(before.element(&format!("{key}<"), t, face, None, md, window));
            }
            if let Some(list) = list {
                col = col.child(math_line(md, list, em, face));
            }
            if !after.is_empty() {
                col = col.child(after.element(&format!("{key}>"), t, face, None, md, window));
            }
            return col.into_any_element();
        }

        // Maths sits in the text as invisible word characters exactly as
        // wide as the formula, so the line can't break inside it; a canvas
        // then draws it at the baseline.
        let mut text = String::with_capacity(self.text.len());
        let mut runs = Vec::new();
        // Where each formula sits in the text, and the slack either side of
        // it in its stand-in.
        let mut placed: Vec<(usize, Rc<DisplayList>, f32)> = Vec::new();
        // (original offset, bytes inserted there) to move link ranges.
        let mut shifts: Vec<(usize, usize)> = Vec::new();
        // Stand-ins copy as the TeX they stand for.
        let mut swaps: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        let mut maths = self.math.iter().zip(&lists).peekable();
        let mut at = 0;
        let mut insert = |at: usize, text: &mut String, runs: &mut Vec<TextRun>, until: usize| {
            while let Some(((_, tex), list)) = maths.next_if(|((m, _), _)| *m <= until) {
                let start = text.len();
                match list {
                    Some(list) => {
                        let w = list.width as f32 * em;
                        let (fill, fw) = filler(&*window, face, w);
                        placed.push((start, list.clone(), (fw - w) / 2.));
                        text.push_str(&fill);
                        swaps.push((start..text.len(), format!("${tex}$")));
                        runs.push(plain_run(
                            fill.len(),
                            font(face.family),
                            face.ink.opacity(0.),
                        ));
                    }
                    // Not TeX RaTeX can read: show it as written.
                    None => {
                        let s = format!("${tex}$");
                        text.push_str(&s);
                        runs.push(run(
                            s.len(),
                            font(theme::mono()),
                            face.ink,
                            &Style {
                                code: true,
                                ..Style::default()
                            },
                            t,
                        ));
                    }
                }
                shifts.push((at, text.len() - start));
            }
        };
        for (len, st) in &self.runs {
            insert(at, &mut text, &mut runs, at);
            let mut f = font(if st.code { theme::mono() } else { face.family });
            f.weight = face.weight;
            if st.strong {
                // A step heavier than the text around it.
                f.weight = if face.weight.0 >= FontWeight::MEDIUM.0 {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                };
            }
            if st.emph {
                f.style = FontStyle::Italic;
            }
            let color = if st.link { t.ink } else { face.ink };
            let piece = &self.text[at..at + len];
            text.push_str(piece);
            // Split a run where the fade changes, word by word.
            match fade {
                Some(fade) => {
                    let mut start = 0;
                    for (i, _) in piece
                        .match_indices(' ')
                        .map(|(i, s)| (i + 1, s))
                        .chain([(piece.len(), "")])
                    {
                        if i <= start {
                            continue;
                        }
                        runs.push(run(
                            i - start,
                            f.clone(),
                            color.opacity(fade(at + start)),
                            st,
                            t,
                        ));
                        start = i;
                    }
                }
                None => runs.push(run(*len, f, color, st, t)),
            }
            at += len;
        }
        insert(at, &mut text, &mut runs, usize::MAX);
        let shift = |o: usize, inclusive: bool| {
            o + shifts
                .iter()
                .filter(|(s, _)| if inclusive { *s <= o } else { *s < o })
                .map(|(_, n)| n)
                .sum::<usize>()
        };
        // Links open in the browser; paths are made whole from the document's
        // folder, and open in the viewer when it can show them.
        let links = LINK_BASE.with(|b| {
            let b = b.borrow();
            self.links
                .iter()
                .map(|(r, u)| {
                    (
                        shift(r.start, true)..shift(r.end, false),
                        resolve(u, b.as_deref()),
                    )
                })
                .collect()
        });
        let styled = crate::select::styled(
            SharedString::from(format!("md-{key}")),
            text.clone(),
            runs,
            t,
        )
        .links(links)
        .swaps(swaps);
        let layout = styled.layout();
        let el = styled.into_any_element();
        if placed.is_empty() {
            return el;
        }
        let probes = md.probes();
        div()
            .relative()
            .child(el)
            .child(
                canvas(
                    move |bounds, _, _| {
                        crate::ui::record(&probes, "md-inline-math".into(), bounds);
                    },
                    move |_, _, window, _| {
                        let id = window.text_system().resolve_font(&font(face.family));
                        let size = px(face.size);
                        let ascent = window.text_system().ascent(id, size);
                        // GPUI reports descent as a negative number.
                        let descent = window.text_system().descent(id, size).abs();
                        let lh = layout.line_height();
                        for (ix, list, slack) in &placed {
                            let Some(p) = layout.position_for_index(*ix) else {
                                continue;
                            };
                            let baseline = p.y + (lh - ascent - descent) / 2. + ascent;
                            let origin =
                                point(p.x + px(*slack), baseline - px(list.height as f32 * em));
                            math::paint(window, list, origin, em, face.ink);
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

/// Invisible word characters measured to about `width` plus a point either
/// side, so a line never breaks inside the formula they stand in for.
/// Returns them and how wide they really are, to centre the formula.
fn filler(window: &Window, face: Face, width: f32) -> (String, f32) {
    let ts = window.text_system();
    let id = ts.resolve_font(&font(face.family));
    let size = px(face.size);
    let want = px(width + 2.);
    let mut out = String::new();
    let mut left = want;
    let mut last = px(0.);
    for ch in ['m', 'n', 'o', 'i', '.'] {
        let w = ts.layout_width(id, size, ch);
        while w > px(0.) && left >= w {
            out.push(ch);
            left -= w;
        }
        last = w;
    }
    // What's left over rounds to the nearest narrow character.
    if out.is_empty() || left > last / 2. {
        out.push('.');
        left -= last;
    }
    (out, (want - left).as_f32())
}

fn plain_run(len: usize, font: Font, color: Hsla) -> TextRun {
    TextRun {
        len,
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

fn run(len: usize, font: Font, color: Hsla, st: &Style, t: &Theme) -> TextRun {
    TextRun {
        len,
        font,
        color,
        // Code reads by its face alone: a tint hugging the letters looked
        // like a selection.
        background_color: None,
        underline: st.link.then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(t.quiet.opacity(0.6)),
            wavy: false,
        }),
        strikethrough: st.strike.then(|| StrikethroughStyle {
            thickness: px(1.),
            color: Some(color),
        }),
    }
}

/// One item of a list: its number (if ordered), checkbox (if a task), and
/// what it holds, nested lists, code and all.
#[derive(Clone)]
pub struct ListItem {
    pub number: Option<u64>,
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

/// GitHub's callouts: > [!NOTE] and friends.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Callout {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Clone)]
pub enum Block {
    Heading(u8, Inline),
    Para(Inline),
    List(Vec<ListItem>),
    /// A quote, or a callout, holding its own blocks.
    Quote(Option<Callout>, Vec<Block>),
    /// A picture on a line of its own: where it is, and its words.
    Image(String, String),
    Code(String, String),
    /// Display maths, and whether it is finished (a fence still streaming isn't).
    Math(String, bool),
    /// A Mermaid diagram, and whether its fence has closed.
    Diagram(String, bool),
    Table(Rc<Table>),
    Rule,
}

pub struct Table {
    pub head: Vec<Inline>,
    pub rows: Vec<Vec<Inline>>,
    pub aligns: Vec<Alignment>,
}

impl Table {
    /// GitHub-flavoured Markdown, normalised from the cells.
    pub fn markdown(&self) -> String {
        let line = |cells: &[Inline]| {
            let parts: Vec<String> = cells
                .iter()
                .map(|c| c.plain(true).replace('|', "\\|"))
                .collect();
            format!("| {} |", parts.join(" | "))
        };
        let rule: Vec<&str> = (0..self.head.len())
            .map(|c| match self.aligns.get(c) {
                Some(Alignment::Left) => ":---",
                Some(Alignment::Center) => ":---:",
                Some(Alignment::Right) => "---:",
                _ => "---",
            })
            .collect();
        let mut out = vec![line(&self.head), format!("| {} |", rule.join(" | "))];
        out.extend(self.rows.iter().map(|r| line(r)));
        out.join("\n")
    }

    pub fn csv(&self) -> String {
        let field = |s: String| {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s
            }
        };
        std::iter::once(&self.head)
            .chain(&self.rows)
            .map(|r| {
                r.iter()
                    .map(|c| field(c.plain(false)))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Head, rows, the row being read, and whether we are still in the head.
type TableState = (Vec<Inline>, Vec<Vec<Inline>>, Vec<Inline>, Vec<Alignment>);

/// Text with bare web addresses made into links.
fn push_linking(cur: &mut Inline, s: &str, st: Style) {
    let mut rest = s;
    while let Some(at) = rest.find("https://").or_else(|| rest.find("http://")) {
        cur.push(&rest[..at], st);
        let tail = &rest[at..];
        let mut end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        // Trailing punctuation belongs to the sentence, not the address.
        while end > 0 && tail[..end].ends_with(['.', ',', ')', ';', ':', '!', '?', '"', '\'']) {
            end -= 1;
        }
        let url = &tail[..end];
        cur.href = Some(url.to_string());
        cur.push(url, Style { link: true, ..st });
        cur.href = None;
        rest = &tail[end..];
    }
    cur.push(rest, st);
}

/// Single-dollar maths is only maths when it looks like TeX. In real agent
/// output every `$…$` pair found was two prices ("$10/mo ($30/mo"), so a
/// span that starts with a digit or space and has no TeX syntax stays text.
pub fn is_price(span: &str) -> bool {
    span.starts_with(|c: char| c.is_ascii_digit() || c.is_whitespace())
        && !span.contains(['\\', '^', '_', '{'])
}

/// Whether the fenced code block written as `src` has its closing fence:
/// a reply still streaming may stop inside one.
fn fence_closed(src: &str) -> bool {
    let src = src.trim_end_matches(['\n', '\r']);
    let mut lines = src.lines();
    let Some(open) = lines.next().map(str::trim_start) else {
        return false;
    };
    let Some(ch) = open.chars().next().filter(|c| *c == '`' || *c == '~') else {
        return false;
    };
    let n = open.chars().take_while(|c| *c == ch).count();
    lines.next_back().is_some_and(|last| {
        let last = last.trim();
        last.len() >= n && last.chars().all(|c| c == ch)
    })
}

/// What the parser is inside, innermost last.
enum Frame {
    Root(Vec<Block>),
    Quote(Option<Callout>, Vec<Block>),
    List(Vec<ListItem>, Option<u64>),
    Item(ListItem),
}

impl Frame {
    /// Where finished blocks go: a list keeps them in its open item.
    fn blocks(&mut self) -> Option<&mut Vec<Block>> {
        match self {
            Frame::Root(b) | Frame::Quote(_, b) => Some(b),
            Frame::Item(i) => Some(&mut i.blocks),
            Frame::List(..) => None,
        }
    }
}

fn push_block(stack: &mut [Frame], b: Block) {
    if let Some(v) = stack.last_mut().and_then(Frame::blocks) {
        v.push(b);
    }
}

/// Text waiting for its paragraph to end becomes one now, before a block
/// that follows it (a tight list item's text has no paragraph of its own).
fn flush(stack: &mut [Frame], cur: &mut Inline) {
    if cur.is_empty() {
        for (src, alt) in std::mem::take(&mut cur.images) {
            push_block(stack, Block::Image(src, alt));
        }
    } else {
        cur.images.clear();
        let p = std::mem::take(cur);
        push_block(stack, Block::Para(p));
    }
}

pub fn parse(src: &str) -> Vec<Block> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_MATH);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_GFM);
    let mut stack: Vec<Frame> = vec![Frame::Root(Vec::new())];
    let mut cur = Inline::default();
    let mut st = Style::default();
    // Language, body, and whether the fence has closed.
    let mut code: Option<(String, String, bool)> = None;
    let mut table: Option<TableState> = None;
    // A picture being read: where it is, and its words so far.
    let mut image: Option<(String, String)> = None;
    for (ev, range) in Parser::new_ext(src, opts).into_offset_iter() {
        match ev {
            Event::Start(Tag::Strong) => st.strong = true,
            Event::End(TagEnd::Strong) => st.strong = false,
            Event::Start(Tag::Emphasis) => st.emph = true,
            Event::End(TagEnd::Emphasis) => st.emph = false,
            Event::Start(Tag::Strikethrough) => st.strike = true,
            Event::End(TagEnd::Strikethrough) => st.strike = false,
            Event::Start(Tag::Link { dest_url, .. }) => {
                st.link = true;
                cur.href = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                st.link = false;
                cur.href = None;
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                image = Some((dest_url.to_string(), String::new()));
            }
            Event::End(TagEnd::Image) => {
                if let Some((src_url, alt)) = image.take() {
                    // On its own line it's a picture; amid text, a link to it.
                    let alone = cur.is_empty();
                    if alone {
                        cur.images.push((src_url, alt));
                    } else {
                        let words = if alt.is_empty() {
                            "picture".into()
                        } else {
                            alt
                        };
                        cur.href = Some(src_url);
                        cur.push(&words, Style { link: true, ..st });
                        cur.href = None;
                    }
                }
            }
            // A file named in code is a link to it, as long as it isn't one already.
            Event::Code(c) if !st.link && looks_like_path(&c) => {
                cur.href = Some(c.to_string());
                cur.push(
                    &c,
                    Style {
                        code: true,
                        link: true,
                        ..st
                    },
                );
                cur.href = None;
            }
            Event::Code(c) => cur.push(&c, Style { code: true, ..st }),
            Event::InlineMath(m) if is_price(&m) => cur.push(&format!("${m}$"), st),
            Event::InlineMath(m) => cur.math.push((cur.text.len(), m.to_string())),
            // Display maths stands as its own block, splitting a paragraph
            // around it; inside a list, quote or table it sits inline.
            Event::DisplayMath(m) => {
                if stack.len() == 1 && table.is_none() {
                    flush(&mut stack, &mut cur);
                    push_block(&mut stack, Block::Math(m.trim().to_string(), true));
                } else {
                    cur.math.push((cur.text.len(), m.to_string()));
                }
            }
            Event::Text(s) => match (&mut code, &mut image) {
                (Some((_, body, _)), _) => body.push_str(&s),
                (None, Some((_, alt))) => alt.push_str(&s),
                (None, None) if st.link => cur.push(&s, st),
                (None, None) => push_linking(&mut cur, &s, st),
            },
            // Raw HTML in the text shows as written: `Vec<String>` stays.
            Event::InlineHtml(h) => cur.push(&h, st),
            Event::Html(h) => {
                if code.is_none() {
                    flush(&mut stack, &mut cur);
                    let h = h.trim_end();
                    if !h.is_empty() {
                        push_block(&mut stack, Block::Code("html".into(), h.to_string()));
                    }
                }
            }
            Event::SoftBreak => cur.push(" ", st),
            Event::HardBreak => cur.push("\n", st),
            Event::FootnoteReference(label) => {
                cur.push(&format!("[{label}]"), Style { code: false, ..st })
            }
            Event::Start(Tag::FootnoteDefinition(label)) => {
                flush(&mut stack, &mut cur);
                cur.push(&format!("[{label}] "), Style { strong: true, ..st });
            }
            Event::TaskListMarker(done) => {
                if let Some(Frame::Item(i)) = stack.last_mut() {
                    i.task = Some(done);
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut stack, &mut cur);
                let (lang, closed) = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(l) => {
                        (l.to_string(), fence_closed(&src[range]))
                    }
                    _ => (String::new(), true),
                };
                code = Some((lang, String::new(), closed));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, body, closed)) = code.take() {
                    let body = body.trim_end().to_string();
                    push_block(
                        &mut stack,
                        match lang.as_str() {
                            "math" => Block::Math(body, closed),
                            "mermaid" => Block::Diagram(body, closed),
                            _ => Block::Code(lang, body),
                        },
                    );
                }
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut stack, &mut cur);
                stack.push(Frame::List(Vec::new(), start));
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut stack, &mut cur);
                if let Some(Frame::List(items, _)) = stack.pop() {
                    push_block(&mut stack, Block::List(items));
                }
            }
            Event::Start(Tag::Item) => {
                let number = match stack.last() {
                    Some(Frame::List(_, n)) => *n,
                    _ => None,
                };
                stack.push(Frame::Item(ListItem {
                    number,
                    task: None,
                    blocks: Vec::new(),
                }));
            }
            Event::End(TagEnd::Item) => {
                flush(&mut stack, &mut cur);
                if let Some(Frame::Item(item)) = stack.pop()
                    && let Some(Frame::List(items, n)) = stack.last_mut()
                {
                    items.push(item);
                    if let Some(n) = n {
                        *n += 1;
                    }
                }
            }
            Event::Start(Tag::BlockQuote(kind)) => {
                flush(&mut stack, &mut cur);
                use pulldown_cmark::BlockQuoteKind as K;
                let callout = kind.map(|k| match k {
                    K::Note => Callout::Note,
                    K::Tip => Callout::Tip,
                    K::Important => Callout::Important,
                    K::Warning => Callout::Warning,
                    K::Caution => Callout::Caution,
                });
                stack.push(Frame::Quote(callout, Vec::new()));
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut stack, &mut cur);
                if let Some(Frame::Quote(c, blocks)) = stack.pop() {
                    push_block(&mut stack, Block::Quote(c, blocks));
                }
            }
            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::FootnoteDefinition) => {
                // A paragraph that was only a picture is the picture.
                if cur.text.is_empty() && cur.math.is_empty() && !cur.images.is_empty() {
                    for (src_url, alt) in std::mem::take(&mut cur.images) {
                        push_block(&mut stack, Block::Image(src_url, alt));
                    }
                } else {
                    // Pictures that shared a line with text were links.
                    cur.images.clear();
                    flush(&mut stack, &mut cur);
                }
            }
            Event::Start(Tag::Heading { .. }) => flush(&mut stack, &mut cur),
            Event::End(TagEnd::Heading(level)) => {
                let h = std::mem::take(&mut cur);
                push_block(&mut stack, Block::Heading(level as u8, h));
            }
            Event::Start(Tag::Table(aligns)) => {
                flush(&mut stack, &mut cur);
                table = Some((Vec::new(), Vec::new(), Vec::new(), aligns))
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(tb) = &mut table {
                    tb.0 = std::mem::take(&mut tb.2);
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(tb) = &mut table {
                    tb.2.push(std::mem::take(&mut cur));
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(tb) = &mut table {
                    let row = std::mem::take(&mut tb.2);
                    tb.1.push(row);
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some((head, rows, _, aligns)) = table.take() {
                    push_block(
                        &mut stack,
                        Block::Table(Rc::new(Table { head, rows, aligns })),
                    );
                }
            }
            Event::Rule => {
                flush(&mut stack, &mut cur);
                push_block(&mut stack, Block::Rule);
            }
            _ => {}
        }
    }
    // A reply still streaming may end mid-list or mid-quote: close what's open.
    flush(&mut stack, &mut cur);
    while stack.len() > 1 {
        match stack.pop() {
            Some(Frame::Item(item)) => {
                if let Some(Frame::List(items, _)) = stack.last_mut() {
                    items.push(item);
                }
            }
            Some(Frame::List(items, _)) => push_block(&mut stack, Block::List(items)),
            Some(Frame::Quote(c, blocks)) => push_block(&mut stack, Block::Quote(c, blocks)),
            _ => {}
        }
    }
    match stack.pop() {
        Some(Frame::Root(b)) => b,
        _ => Vec::new(),
    }
}

// ---- Render state ----

enum Diagram {
    Pending,
    Ready(Rc<diagram::Drawn>),
    Failed(String),
}

/// A table's measured columns, kept while its rows don't change.
struct TableCache {
    table: Weak<Table>,
    data: Rc<TableData>,
    /// Each row's measurements by its text, so a table streaming in measures
    /// only its new rows.
    rows: Vec<(u64, Rc<[Measure]>)>,
}

#[derive(Default)]
struct State {
    probes: Probes,
    math: HashMap<(String, bool), Option<Rc<DisplayList>>>,
    diagrams: HashMap<(String, bool), Diagram>,
    hscrolls: HashMap<String, ScrollHandle>,
    lists: HashMap<String, ListState>,
    tables: HashMap<String, TableCache>,
    copied: Option<(String, Instant)>,
    /// The capped table drawn last, for the walkthrough's checks.
    last_table: Option<String>,
}

/// What Markdown rendering remembers between frames. Cheap to clone; every
/// clone is the same state.
#[derive(Clone, Default)]
pub struct Md(Rc<RefCell<State>>);

impl Md {
    pub fn new(probes: Probes) -> Self {
        let md = Self::default();
        md.0.borrow_mut().probes = probes;
        md
    }

    fn probes(&self) -> Probes {
        self.0.borrow().probes.clone()
    }

    fn typeset(&self, tex: &str, display: bool) -> Option<Rc<DisplayList>> {
        let mut s = self.0.borrow_mut();
        if s.math.len() > 2048 {
            s.math.clear();
        }
        s.math
            .entry((tex.to_string(), display))
            .or_insert_with(|| math::typeset(tex, display).map(Rc::new))
            .clone()
    }

    fn hscroll_handle(&self, key: &str) -> ScrollHandle {
        let mut s = self.0.borrow_mut();
        if s.hscrolls.len() > 4096 {
            s.hscrolls.clear();
        }
        s.hscrolls.entry(key.to_string()).or_default().clone()
    }

    fn copied(&self, key: &str) -> bool {
        self.0
            .borrow()
            .copied
            .as_ref()
            .is_some_and(|(k, at)| k == key && at.elapsed() < COPIED)
    }

    /// The scroll position and end of the capped table drawn last.
    pub fn table_scroll(&self) -> Option<(f32, f32)> {
        let s = self.0.borrow();
        let state = s.lists.get(s.last_table.as_ref()?)?;
        Some((
            -state.scroll_px_offset_for_scrollbar().y.as_f32(),
            state.max_offset_for_scrollbar().y.as_f32(),
        ))
    }
}

/// How long "Copied" stays up.
const COPIED: Duration = Duration::from_millis(1300);

/// Everything rendering needs from the frame it runs in.
pub struct R<'a> {
    pub md: Md,
    pub window: &'a mut Window,
    pub cx: &'a mut App,
    /// The view to redraw when a diagram finishes or "Copied" ends.
    pub owner: EntityId,
    /// Names this run of blocks, so scroll positions stay with it while it
    /// streams: a session and item, or a document.
    pub scope: String,
    /// What the blocks sit on, for edge fades.
    pub bg: Hsla,
    /// The folder relative links and pictures are found from: the
    /// document's own, or the session's.
    pub base: Option<std::path::PathBuf>,
}

thread_local! {
    /// The folder links resolve from while blocks are being built.
    static LINK_BASE: RefCell<Option<std::path::PathBuf>> = const { RefCell::new(None) };
}

/// Whether a code span names a file: `notes/plan.md`, `/abs/x.rs`, `~/a.txt`.
fn looks_like_path(c: &str) -> bool {
    if c.contains(char::is_whitespace) || c.len() > 300 || c.contains("://") {
        return false;
    }
    let last = c.rsplit('/').next().unwrap_or(c);
    let ext_ok = last.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && (1..=8).contains(&ext.len())
            && ext.chars().all(|x| x.is_ascii_alphanumeric())
    });
    ext_ok && (c.contains('/') || c.starts_with('~'))
}

/// Where a link or picture in a document points: a web address as it is,
/// a path made whole from the document's folder (`~` is home).
pub fn resolve(href: &str, base: Option<&std::path::Path>) -> String {
    if href.contains("://") || href.starts_with("mailto:") || href.starts_with('#') {
        return href.to_string();
    }
    let path = href.split(['#', '?']).next().unwrap_or(href);
    let path = crate::web::percent_decode(path);
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(h) = std::env::var_os("HOME")
    {
        return std::path::Path::new(&h).join(rest).display().to_string();
    }
    if path.starts_with('/') {
        return path;
    }
    match base {
        Some(b) => {
            // Tidy ./ and ../ so the path reads cleanly.
            let mut out = b.to_path_buf();
            for part in std::path::Path::new(&path).components() {
                match part {
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir => {
                        out.pop();
                    }
                    p => out.push(p),
                }
            }
            out.display().to_string()
        }
        None => path,
    }
}

/// A picture on its own line: a local file is shown, fitted to the column;
/// a web picture is a link to it.
fn image_block(src: &str, alt: &str, t: &Theme, key: &str, r: &mut R) -> AnyElement {
    let at = resolve(src, r.base.as_deref());
    if at.starts_with('/') && std::path::Path::new(&at).is_file() {
        div()
            .id(SharedString::from(format!("{key}/img")))
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .child(
                img(std::path::PathBuf::from(&at))
                    .max_w_full()
                    .max_h(px(520.))
                    .object_fit(ObjectFit::Contain)
                    .rounded(px(radius::MD)),
            )
            .when(!alt.is_empty(), |d| {
                d.child(div().ty(LABEL).text_color(t.quiet).child(alt.to_string()))
            })
            .into_any_element()
    } else {
        let mut i = Inline {
            href: Some(at),
            ..Inline::default()
        };
        let words = if alt.is_empty() {
            src.to_string()
        } else {
            alt.to_string()
        };
        i.push(
            &words,
            Style {
                link: true,
                ..Style::default()
            },
        );
        let f = Face::of(READING, theme::body(), t.ink, r.bg);
        div()
            .ty(READING)
            .child(i.element(key, t, f, None, &r.md, r.window))
            .into_any_element()
    }
}

/// Render blocks in the reading column. `doc` sets headings for a document;
/// replies in chat never have big headings.
pub fn render(blocks: &[Block], t: &Theme, doc: bool, r: &mut R) -> Div {
    render_with(blocks, t, doc, false, r)
}

/// Blocks in the quiet ink: words that sit inside something else, like a
/// subagent's report, and shouldn't read as the answer.
pub fn render_with(blocks: &[Block], t: &Theme, doc: bool, quiet: bool, r: &mut R) -> Div {
    let prev = LINK_BASE.with(|b| b.replace(r.base.clone()));
    let out = render_in(blocks, t, doc, r, space::S3, quiet, None, false);
    LINK_BASE.with(|b| *b.borrow_mut() = prev);
    out
}

thread_local! {
    /// What each copied line inside the blocks being laid out starts with:
    /// a list item's indent, a quote's `> `.
    static COPY_PREFIX: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Lays out `f` with `more` added to the copy prefix.
fn prefixed<T>(more: &str, f: impl FnOnce() -> T) -> T {
    let saved = COPY_PREFIX.with(|p| {
        let saved = p.borrow().clone();
        p.borrow_mut().push_str(more);
        saved
    });
    let out = f();
    COPY_PREFIX.with(|p| *p.borrow_mut() = saved);
    out
}

/// Blocks with `gap` between them; `quiet` sets their text in the quiet
/// ink (a quote, or a task that's done). They copy a blank line apart, or
/// line by line when `tight` (inside a list item); `first` is what the first
/// one copies with, like its list marker.
#[allow(clippy::too_many_arguments)]
fn render_in(
    blocks: &[Block],
    t: &Theme,
    doc: bool,
    r: &mut R,
    gap: f32,
    quiet: bool,
    first: Option<String>,
    tight: bool,
) -> Div {
    let ink = if quiet { t.quiet } else { t.ink };
    let reading = theme::body();
    let mut out = Vec::with_capacity(blocks.len());
    let bg = r.bg;
    let face = |ty: Type, family: &'static str, ink: Hsla| Face::of(ty, family, ink, bg);
    for (ix, b) in blocks.iter().enumerate() {
        let key = format!("{}/{ix}", r.scope);
        let prefix = COPY_PREFIX.with(|p| p.borrow().clone());
        let lead = match &first {
            Some(f) if ix == 0 => f.clone(),
            _ if tight => prefix.clone(),
            _ => format!("{}\n{prefix}", prefix.trim_end()),
        };
        crate::select::lead_next(lead.clone());
        let el = match b {
            Block::Heading(level, i) => {
                let (ty, family) = match (doc, level) {
                    (true, 1) => (DISPLAY, theme::serif()),
                    (true, 2) => (DISPLAY_COMPACT, theme::serif()),
                    (true, _) => (READING, reading),
                    // In chat, headings are the interface face, so they
                    // read as structure rather than another paragraph.
                    (false, _) => (READING, theme::sans()),
                };
                let weight = if !doc || *level > 2 {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                };
                let f = face(ty, family, t.ink).weighted(weight);
                div()
                    .when(*level > 1 || !doc, |d| d.pt(px(space::S3)))
                    .ty(ty)
                    .text_color(t.ink)
                    .child(i.element(&key, t, f, None, &r.md, r.window))
                    .into_any_element()
            }
            Block::Para(i) => {
                let f = face(READING, reading, ink);
                div()
                    .ty(READING)
                    .text_color(ink)
                    .child(i.element(&key, t, f, None, &r.md, r.window))
                    .into_any_element()
            }
            Block::List(items) => {
                let mut rows = Vec::with_capacity(items.len());
                for (li, item) in items.iter().enumerate() {
                    let marker: AnyElement = match (item.task, item.number) {
                        (Some(done), _) => checkbox(done, t).into_any_element(),
                        (None, Some(n)) => div()
                            .ty(READING)
                            .text_color(t.quiet)
                            .child(format!("{n}."))
                            .into_any_element(),
                        (None, None) => div()
                            .ty(READING)
                            .text_color(t.faint)
                            .child("–")
                            .into_any_element(),
                    };
                    // What the item holds, nested lists and code included,
                    // sits in its own column beside the marker.
                    let done = matches!(item.task, Some(true));
                    let mark = match (item.task, item.number) {
                        (Some(true), _) => "- [x] ".to_string(),
                        (Some(false), _) => "- [ ] ".to_string(),
                        (None, Some(n)) => format!("{n}. "),
                        (None, None) => "- ".to_string(),
                    };
                    let item_lead = format!("{}{mark}", if li == 0 { &lead } else { &prefix });
                    let saved = std::mem::replace(&mut r.scope, format!("{key}.{li}"));
                    let body = prefixed(&" ".repeat(mark.len()), || {
                        render_in(
                            &item.blocks,
                            t,
                            doc,
                            r,
                            space::S2,
                            done || quiet,
                            Some(item_lead),
                            true,
                        )
                    });
                    r.scope = saved;
                    rows.push(
                        div()
                            .relative()
                            .pl(px(24.))
                            .ty(READING)
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .h(px(READING.line))
                                    .flex()
                                    .items_center()
                                    .child(marker),
                            )
                            .child(body),
                    );
                }
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .children(rows)
                    .into_any_element()
            }
            Block::Quote(callout, blocks) => {
                let saved = std::mem::replace(&mut r.scope, format!("{key}.q"));
                // A quote reads quieter; a callout is meant to be read.
                let body = prefixed("> ", || {
                    render_in(
                        blocks,
                        t,
                        doc,
                        r,
                        space::S2,
                        callout.is_none() || quiet,
                        Some(format!("{lead}> ")),
                        false,
                    )
                });
                r.scope = saved;
                match callout {
                    None => div()
                        .pl(px(space::S4))
                        .border_l_2()
                        .border_color(t.hairline)
                        .child(body)
                        .into_any_element(),
                    Some(c) => {
                        // Signals are fills, never text, and the needs-you
                        // magenta means only that: the colour marks the edge
                        // and icon; the words stay ink.
                        let (label, icon_name, ink) = match c {
                            Callout::Note => ("Note", "info", t.working.0),
                            Callout::Tip => ("Tip", "lightbulb", t.done.0),
                            Callout::Important => {
                                ("Important", "message-square-warning", t.waiting.0)
                            }
                            Callout::Warning => ("Warning", "triangle-alert", t.waiting.0),
                            Callout::Caution => ("Caution", "octagon-alert", t.waiting.0),
                        };
                        div()
                            .pl(px(space::S4))
                            .pr(px(space::S3))
                            .py(px(space::S2))
                            .rounded(px(radius::SM))
                            .bg(t.wash(ink))
                            .border_l_2()
                            .border_color(ink)
                            .flex()
                            .flex_col()
                            .gap(px(space::S1))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S1H))
                                    .ty(UI)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.ink)
                                    .child(crate::ui::icon(icon_name, 14., ink))
                                    .child(label),
                            )
                            .child(body)
                            .into_any_element()
                    }
                }
            }
            Block::Image(src, alt) => image_block(src, alt, t, &key, r),
            Block::Code(lang, body) => code_block(lang, body, t, &key, r),
            Block::Math(tex, closed) => display_math(tex, *closed, t, &key, r),
            Block::Diagram(src, closed) => diagram_block(src, *closed, t, &key, r),
            Block::Table(table) => table_block(table, t, &key, r),
            Block::Rule => div().h(px(1.)).bg(t.hairline).into_any_element(),
        };
        crate::select::drop_lead();
        out.push(el);
    }
    div().flex().flex_col().gap(px(gap)).children(out)
}

/// A small text button that copies `text` and says so for a moment. `text`
/// runs only when clicked, so a big table's Markdown and CSV are never
/// built just to draw the button.
fn copy_button(
    key: String,
    label: &'static str,
    text: impl Fn() -> String + 'static,
    t: &Theme,
    r: &R,
) -> Stateful<Div> {
    let copied = r.md.copied(&key);
    let md = r.md.clone();
    let owner = r.owner;
    let probes = r.md.probes();
    div()
        .id(SharedString::from(key.clone()))
        .relative()
        .px(px(space::S1H))
        .rounded(px(radius::SM))
        .ty(LABEL)
        .text_color(if copied { t.ink } else { t.quiet })
        .cursor_pointer()
        .hover(|d| d.bg(t.hover()).text_color(t.ink))
        .child(if copied { "Copied" } else { label })
        .child(crate::ui::probe(&probes, key.clone()))
        .on_press(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text()));
            md.0.borrow_mut().copied = Some((key.clone(), Instant::now()));
            cx.notify(owner);
            // Back to its label once "Copied" has been read; no frames meanwhile.
            cx.spawn(async move |cx| {
                cx.background_executor().timer(COPIED).await;
                cx.update(|cx| cx.notify(owner));
            })
            .detach();
        })
}

/// Actions in the top-right corner while the pointer is over a block with
/// no header of its own (display maths, diagrams, small tables).
fn hover_actions(body: impl IntoElement, actions: impl IntoElement, key: &str, t: &Theme) -> Div {
    let group = SharedString::from(format!("hover-{key}"));
    div().group(group.clone()).relative().child(body).child(
        div()
            .absolute()
            .top(px(space::S1))
            .right(px(space::S1))
            .flex()
            .gap(px(space::S1))
            .p(px(space::HALF))
            .rounded(px(radius::MD))
            .bg(t.raised)
            .border_1()
            .border_color(t.hairline)
            .reveal_on_hover(group)
            .child(actions),
    )
}

/// Content that may be wider than the column, in a sideways scroller with a
/// fade on whichever edge has more to see.
fn hscroll(md: &Md, key: &str, child: impl IntoElement, bg: Hsla) -> Div {
    let handle = md.hscroll_handle(key);
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
                .id(SharedString::from(format!("h-{key}")))
                .track_scroll(&handle)
                .overflow_x_scroll()
                // Vertical wheel motion belongs to the chat, never turned sideways.
                .restrict_scroll_to_axis()
                .w_full()
                .child(child),
        )
        .when(max > 0.5 && x < max - 0.5, |d| d.child(fade(true)))
        .when(x > 0.5, |d| d.child(fade(false)))
}

/// Inline maths too wide for a line, at text size on a line of its own.
fn math_line(md: &Md, list: Rc<DisplayList>, em: f32, face: Face) -> Div {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    list.width.to_bits().hash(&mut h);
    list.items.len().hash(&mut h);
    let w = list.width as f32 * em;
    let hgt = (list.height + list.depth) as f32 * em;
    let ink = face.ink;
    hscroll(
        md,
        &format!("im-{:x}", h.finish()),
        div().py(px(space::S1)).child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| math::paint(window, &list, bounds.origin, em, ink),
            )
            .flex_none()
            .w(px(w + 2.))
            .h(px(hgt)),
        ),
        face.bg,
    )
}

/// A code block: a header with the language and Copy, then the code, which
/// never wraps and scrolls sideways. `dim` shows source still waiting to
/// become maths or a diagram.
fn code_frame(lang: &str, body: &str, t: &Theme, key: &str, dim: bool, r: &R) -> Div {
    let text = body.to_string();
    let copy = copy_button(format!("copy-{key}"), "Copy", move || text.clone(), t, r);
    div()
        .rounded(px(radius::LG))
        .border_1()
        .border_color(t.hairline)
        .bg(t.raised)
        .overflow_hidden()
        .when(dim, |d| d.opacity(0.5))
        .child(
            div()
                .pl(px(space::S3))
                .pr(px(space::S1H))
                .h(px(28.))
                .flex()
                .items_center()
                .border_b_1()
                .border_color(t.hairline)
                .justify_between()
                .ty(LABEL)
                .text_color(t.quiet)
                .child(if lang.is_empty() {
                    "code".to_string()
                } else {
                    lang.to_string()
                })
                .child(copy),
        )
        .child(
            div()
                .id(SharedString::from(format!("code-{key}")))
                .overflow_x_scroll()
                .restrict_scroll_to_axis()
                .px(px(space::S3))
                .py(px(space::S2))
                .ty(CODE)
                .text_color(t.ink)
                .children({
                    // Highlighted once per block, line by line, then cached.
                    let lines = crate::highlight::block_lines(lang, body);
                    // Lines copy one to a line, blank ones kept, each with
                    // the block's prefix (inside a list or quote).
                    let prefix = COPY_PREFIX.with(|p| p.borrow().clone());
                    let mut first = Some(crate::select::take_lead().to_string());
                    let mut blanks = String::new();
                    body.split('\n')
                        .enumerate()
                        .map(|(n, l)| {
                            // A blank line still selects, so the drag can
                            // cross it; it copies as a lead on the next.
                            if l.is_empty() {
                                if first.is_none() {
                                    blanks = format!("{blanks}{}\n", prefix.trim_end());
                                }
                                return div().child(
                                    crate::select::text(
                                        SharedString::from(format!("code-{key}-{n}")),
                                        " ",
                                        t,
                                    )
                                    .lead(""),
                                );
                            }
                            let lead = first.take().unwrap_or_else(|| {
                                format!("{}{prefix}", std::mem::take(&mut blanks))
                            });
                            let runs: Vec<TextRun> = lines
                                .get(n)
                                .cloned()
                                .unwrap_or_default()
                                .into_iter()
                                .map(|(len, tag)| {
                                    let mut f = font(theme::mono());
                                    if tag == Some(crate::highlight::Tag::Keyword) {
                                        f.weight = FontWeight::BOLD;
                                    }
                                    if tag == Some(crate::highlight::Tag::Comment) {
                                        f.style = FontStyle::Italic;
                                    }
                                    plain_run(len, f, tag.map(|g| g.color(t)).unwrap_or(t.ink))
                                })
                                .collect();
                            div().whitespace_nowrap().child(
                                crate::select::styled(
                                    SharedString::from(format!("code-{key}-{n}")),
                                    l.to_string(),
                                    runs,
                                    t,
                                )
                                .lead(lead),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
        )
}

fn code_block(lang: &str, body: &str, t: &Theme, key: &str, r: &mut R) -> AnyElement {
    code_frame(lang, body, t, key, false, r).into_any_element()
}

/// Display maths, centred, scrolling sideways when wider than the column.
/// TeX that won't typeset shows as source.
fn display_math(tex: &str, closed: bool, t: &Theme, key: &str, r: &mut R) -> AnyElement {
    let list = closed.then(|| r.md.typeset(tex, true)).flatten();
    let Some(list) = list else {
        return code_frame("tex", tex, t, key, !closed, r).into_any_element();
    };
    let w = list.width as f32 * DISPLAY_EM;
    let h = (list.height + list.depth) as f32 * DISPLAY_EM;
    let ink = t.ink;
    let probes = r.md.probes();
    let body = hscroll(
        &r.md,
        key,
        div()
            .w_full()
            .min_w(px(w))
            .flex()
            .justify_center()
            .py(px(space::S2))
            .child(
                canvas(
                    move |bounds, _, _| {
                        crate::ui::record(&probes, "md-math".into(), bounds);
                    },
                    move |bounds, _, window, _| {
                        math::paint(window, &list, bounds.origin, DISPLAY_EM, ink)
                    },
                )
                .flex_none()
                .w(px(w))
                .h(px(h)),
            ),
        r.bg,
    );
    let src = tex.trim().to_string();
    let copy = copy_button(
        format!("copy-tex-{key}"),
        "Copy TeX",
        move || src.clone(),
        t,
        r,
    );
    hover_actions(body, copy, key, t).into_any_element()
}

/// A Mermaid diagram: rendered off the UI thread once its fence closes, its
/// source shown dimmed until then. Shrinks to fit the column, but never so
/// far that labels get unreadable; past that it scrolls sideways.
fn diagram_block(src: &str, closed: bool, t: &Theme, key: &str, r: &mut R) -> AnyElement {
    if !closed {
        return code_frame("mermaid", src, t, key, true, r).into_any_element();
    }
    let dark = t.dark;
    let ck = (src.to_string(), dark);
    let state = {
        let s = r.md.0.borrow();
        s.diagrams.get(&ck).map(|d| match d {
            Diagram::Pending => Err(None),
            Diagram::Ready(d) => Ok(d.clone()),
            Diagram::Failed(e) => Err(Some(e.clone())),
        })
    };
    match state {
        None => {
            start_diagram(ck, r);
            code_frame("mermaid", src, t, key, true, r).into_any_element()
        }
        Some(Err(None)) => code_frame("mermaid", src, t, key, true, r).into_any_element(),
        Some(Err(Some(e))) => div()
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .child(code_frame("mermaid", src, t, key, false, r))
            .child(
                div()
                    .ty(LABEL)
                    .text_color(t.quiet)
                    .child(format!("Couldn't draw this diagram: {e}")),
            )
            .into_any_element(),
        Some(Ok(d)) => {
            let (w, h) = (d.w, d.h);
            let probes = r.md.probes();
            let body = hscroll(
                &r.md,
                key,
                div()
                    .w_full()
                    .min_w(px(w * DIAGRAM_MIN_SCALE))
                    .relative()
                    .flex()
                    .justify_center()
                    .child(
                        img(ImageSource::Render(d.image.clone()))
                            .w_full()
                            .max_w(px(w))
                            .aspect_ratio(w / h),
                    )
                    .child(crate::ui::probe(&probes, "md-diagram")),
                r.bg,
            );
            let text = src.to_string();
            let copy = copy_button(
                format!("copy-mermaid-{key}"),
                "Copy source",
                move || text.clone(),
                t,
                r,
            );
            hover_actions(body, copy, key, t).into_any_element()
        }
    }
}

/// Render a diagram on the background executor, once per source and theme.
fn start_diagram(ck: (String, bool), r: &mut R) {
    {
        let mut s = r.md.0.borrow_mut();
        if s.diagrams.len() > 256 {
            s.diagrams.retain(|_, d| matches!(d, Diagram::Pending));
        }
        s.diagrams.insert(ck.clone(), Diagram::Pending);
    }
    let palette = diagram::Palette::of(&if ck.1 {
        Theme::carbon()
    } else {
        Theme::paper()
    });
    let src = ck.0.clone();
    let task =
        r.cx.background_executor()
            .spawn(async move { diagram::render(&src, &palette) });
    let md = r.md.clone();
    let owner = r.owner;
    r.cx.spawn(async move |cx| {
        let done = match task.await {
            Ok(d) => Diagram::Ready(Rc::new(d)),
            Err(e) => Diagram::Failed(e),
        };
        md.0.borrow_mut().diagrams.insert(ck, done);
        cx.update(|cx| cx.notify(owner));
    })
    .detach();
}

// ---- Tables ----

/// Body rows a table shows before it scrolls inside the chat…
const TABLE_ROWS: usize = 10;
/// …once that hides a meaningful number of rows.
const TABLE_SLACK: usize = 3;
/// Columns never get wider than this before their text wraps…
const CELL_MAX: f32 = 320.;
/// …a single long word never forces a column wider than this…
const CELL_MIN_CAP: f32 = 160.;
/// …and a column of prose never squeezes below this; the table scrolls first.
const CELL_PROSE_MIN: f32 = 140.;
/// Cell padding.
const CELL_X: f32 = space::S3;
const CELL_Y: f32 = space::S1H;
/// One single-line row: text, padding and the rule below it.
pub const ROW_H: f32 = UI.line + 2. * CELL_Y + 1.;
/// Room on the right of a scrolling table for its scrollbar.
const GUTTER: f32 = 10.;

/// A cell's natural width and its longest word.
#[derive(Clone, Copy)]
struct Measure {
    natural: f32,
    min: f32,
}

/// A table with its shared column widths, kept so every row lines up.
pub struct TableData {
    table: Rc<Table>,
    widths: Vec<f32>,
    mins: Vec<f32>,
    total: f32,
    total_min: f32,
}

impl TableData {
    fn capped(&self) -> bool {
        self.table.rows.len() > TABLE_ROWS + TABLE_SLACK
    }
}

/// Natural width and longest-word width of a cell, each word measured in its
/// own run's font (code is wider, headers are medium weight).
fn measure(i: &Inline, head: bool, md: &Md, window: &Window) -> Measure {
    let size = px(UI.size);
    let ts = window.text_system();
    let maths: Vec<f32> = i
        .math
        .iter()
        .filter_map(|(_, tex)| md.typeset(tex, false))
        .map(|l| l.width as f32 * UI.size * INLINE_EM + 2.)
        .collect();
    if i.text.is_empty() {
        let w = maths.iter().sum::<f32>();
        return Measure {
            natural: w,
            min: maths.iter().copied().fold(0., f32::max),
        };
    }
    let fonts: Vec<Font> = i
        .runs
        .iter()
        .map(|(_, st)| {
            let mut f = font(if st.code {
                theme::mono()
            } else {
                theme::sans()
            });
            if st.strong || head {
                f.weight = FontWeight::MEDIUM;
            }
            if st.emph {
                f.style = FontStyle::Italic;
            }
            f
        })
        .collect();
    let runs: Vec<TextRun> = i
        .runs
        .iter()
        .zip(&fonts)
        .map(|((len, _), f)| plain_run(*len, f.clone(), black()))
        .collect();
    let natural = ts
        .shape_line(i.text.clone().into(), size, &runs, None)
        .width
        .as_f32()
        + maths.iter().sum::<f32>();
    let mut min = maths.iter().copied().fold(0., f32::max);
    let mut at = 0;
    for ((len, _), f) in i.runs.iter().zip(&fonts) {
        for w in i.text[at..at + len].split_whitespace() {
            let r = plain_run(w.len(), f.clone(), black());
            min = min.max(
                ts.shape_line(w.to_string().into(), size, &[r], None)
                    .width
                    .as_f32(),
            );
        }
        at += len;
    }
    // Shaping rounds differently from wrapping; a hair of slack keeps a
    // word that exactly fits from breaking.
    Measure {
        natural: natural + 2.,
        min: min + 2.,
    }
}

/// The table's column widths, measured once and kept while it is unchanged.
/// While it streams in, only new or changed rows are measured.
fn table_data(table: &Rc<Table>, key: &str, md: &Md, window: &Window) -> Rc<TableData> {
    if let Some(c) = md.0.borrow().tables.get(key)
        && c.table.as_ptr() == Rc::as_ptr(table)
    {
        return c.data.clone();
    }
    use std::hash::{Hash, Hasher};
    let hash = |row: &[Inline], head: bool| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        head.hash(&mut h);
        for c in row {
            c.text.hash(&mut h);
            c.math.len().hash(&mut h);
        }
        h.finish()
    };
    let old =
        md.0.borrow_mut()
            .tables
            .remove(key)
            .map(|c| c.rows)
            .unwrap_or_default();
    let mut rows = Vec::with_capacity(table.rows.len() + 1);
    for (i, row) in std::iter::once(&table.head).chain(&table.rows).enumerate() {
        let h = hash(row, i == 0);
        let m = match old.get(i) {
            Some((oh, m)) if *oh == h => m.clone(),
            _ => row
                .iter()
                .map(|c| measure(c, i == 0, md, window))
                .collect::<Vec<_>>()
                .into(),
        };
        rows.push((h, m));
    }
    let pad = 2. * CELL_X;
    let cols = table.head.len().max(1);
    let mut widths = vec![0f32; cols];
    let mut mins = vec![0f32; cols];
    for (_, row) in &rows {
        for (c, m) in row.iter().enumerate().take(cols) {
            widths[c] = widths[c].max(m.natural.min(CELL_MAX) + pad);
            mins[c] = mins[c].max(m.min.min(CELL_MIN_CAP) + pad);
        }
    }
    for c in 0..cols {
        mins[c] = mins[c].max(widths[c].min(CELL_PROSE_MIN + pad));
        widths[c] = widths[c].max(mins[c]);
    }
    let data = Rc::new(TableData {
        table: table.clone(),
        total: widths.iter().sum::<f32>() + 2.,
        total_min: mins.iter().sum::<f32>() + 2.,
        widths,
        mins,
    });
    let mut s = md.0.borrow_mut();
    if s.tables.len() > 512 {
        s.tables.clear();
    }
    s.tables.insert(
        key.to_string(),
        TableCache {
            table: Rc::downgrade(table),
            data: data.clone(),
            rows,
        },
    );
    data
}

/// One table row. Every row uses the same flex basis, grow and minimum per
/// column, so columns line up without a grid. `gutter` keeps the last column
/// clear of a scrollbar, on the header too so columns stay aligned.
#[allow(clippy::too_many_arguments)]
fn row(
    key: &str,
    lead: &str,
    data: &TableData,
    cells: &[Inline],
    head: bool,
    rule: bool,
    gutter: bool,
    md: &Md,
    window: &mut Window,
    t: &Theme,
) -> Div {
    let pad = 2. * CELL_X;
    let ink = if head { t.quiet } else { t.ink };
    let face = Face::of(UI, theme::sans(), ink, t.surface).weighted(if head {
        FontWeight::MEDIUM
    } else {
        FontWeight::NORMAL
    });
    let empty = Inline::default();
    let mut out = Vec::with_capacity(data.widths.len());
    for c in 0..data.widths.len() {
        let cell = cells.get(c).unwrap_or(&empty);
        let w = data.widths[c];
        out.push(
            div()
                .flex_basis(px(w))
                .flex_grow(w)
                .flex_shrink(1.)
                .min_w(px(data.mins.get(c).copied().unwrap_or(pad)))
                .px(px(CELL_X))
                .py(px(CELL_Y))
                .map(|d| match data.table.aligns.get(c) {
                    Some(Alignment::Right) => d.text_right(),
                    Some(Alignment::Center) => d.text_center(),
                    _ => d,
                })
                .child({
                    // A row copies as one line, its cells a tab apart.
                    crate::select::lead_next(if c == 0 {
                        lead.to_string()
                    } else {
                        format!("{}\t", crate::select::SAME_LINE)
                    });
                    cell.element(&format!("{key}.{c}"), t, face, None, md, window)
                }),
        );
    }
    div()
        .flex()
        .w_full()
        .ty(UI)
        .text_color(ink)
        .when(gutter, |d| d.pr(px(GUTTER)))
        .when(head, |d| d.bg(t.raised))
        .when(rule, |d| d.border_b_1().border_color(t.hairline))
        .children(out)
}

/// Scroll handoff for a list nested in the scrolling chat. GPUI lets every
/// scroller under the pointer take the wheel, so without this the table and
/// the chat would scroll together. In the capture phase we note whether the
/// list can still move in the wheel's direction; in the bubble phase, after
/// the list has scrolled, we stop the event there. At either end the event
/// carries on to the chat.
///
/// Must be painted before the list so its bubble handler runs after the list's.
fn handoff(state: ListState) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let state = state.clone();
            let inner = Rc::new(std::cell::Cell::new(false));
            window.on_mouse_event(move |e: &ScrollWheelEvent, phase, _, cx| {
                if !bounds.contains(&e.position) {
                    return;
                }
                match phase {
                    DispatchPhase::Capture => {
                        let d = e.delta.pixel_delta(px(20.));
                        // GPUI's list merges a frame's wheel deltas by sign, and
                        // 0.0 counts as positive: the zero-delta event that ends a
                        // trackpad gesture would cancel that frame's scrolling.
                        if d.x.as_f32() == 0. && d.y.as_f32() == 0. {
                            cx.stop_propagation();
                            return;
                        }
                        let dy = d.y.as_f32();
                        let top = -state.scroll_px_offset_for_scrollbar().y.as_f32();
                        let max = state.max_offset_for_scrollbar().y.as_f32();
                        inner.set(if dy < 0. {
                            top < max - 0.5
                        } else if dy > 0. {
                            top > 0.5
                        } else {
                            false
                        });
                    }
                    DispatchPhase::Bubble => {
                        if inner.get() {
                            cx.stop_propagation();
                        }
                    }
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A scrollbar that is always visible while there is more to see, so a
/// capped table reads as scrollable before anyone touches it.
fn scrollbar(state: ListState, t: &Theme) -> impl IntoElement {
    let ink = t.ink;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let view = bounds.size.height.as_f32();
            let max = state.max_offset_for_scrollbar().y.as_f32();
            if max <= 0.5 {
                return;
            }
            let top = -state.scroll_px_offset_for_scrollbar().y.as_f32();
            let thumb = (view * view / (view + max)).max(24.);
            let y = (top / max).clamp(0., 1.) * (view - thumb);
            window.paint_quad(
                fill(
                    Bounds::new(
                        point(bounds.right() - px(7.), bounds.top() + px(y + 2.)),
                        size(px(4.), px(thumb - 4.)),
                    ),
                    ink.opacity(0.28),
                )
                .corner_radii(px(2.)),
            );
        },
    )
    .absolute()
    .top_0()
    .right_0()
    .w(px(GUTTER))
    .h_full()
}

fn vfade(top: bool, bg: Hsla) -> Div {
    let (a, b) = if top {
        (bg, bg.opacity(0.))
    } else {
        (bg.opacity(0.), bg)
    };
    div()
        .absolute()
        .left_0()
        .right_0()
        .h(px(28.))
        .when(top, |d| d.top_0())
        .when(!top, |d| d.bottom_0())
        .bg(linear_gradient(
            180.,
            linear_color_stop(a, 0.),
            linear_color_stop(b, 1.),
        ))
}

/// The list state behind a capped table, kept while its rows stream in.
fn list_for(md: &Md, key: &str, n: usize) -> ListState {
    let mut s = md.0.borrow_mut();
    if s.lists.len() > 512 {
        s.lists.clear();
    }
    let state = s
        .lists
        .entry(key.to_string())
        .or_insert_with(|| ListState::new(0, ListAlignment::Top, px(ROW_H * 4.)))
        .clone();
    let have = state.item_count();
    if have != n {
        if have < n {
            // Rows streaming in append; the view stays where the reader left it.
            state.splice(have..have, n - have);
        } else {
            state.reset(n);
        }
        // The list only knows the heights of rows it has drawn. Without a
        // hint its scroll end sits short of the real one, and the wheel
        // would hand off to the chat with rows still hidden.
        let _ = state.clone().with_uniform_item_height(px(ROW_H));
    }
    s.last_table = Some(key.to_string());
    state
}

/// A table: header, rows, and copying. Past 13 rows it shows 10½ of them
/// and scrolls inside the chat, with an always-visible scrollbar, fades and
/// a footer giving the row count. Wider than the column, it scrolls sideways.
fn table_block(table: &Rc<Table>, t: &Theme, key: &str, r: &mut R) -> AnyElement {
    let data = table_data(table, key, &r.md, r.window);
    let n = table.rows.len();
    // The header copies with the block's lead; each row on a line of its own.
    let head_lead = crate::select::take_lead().to_string();
    let prefix = COPY_PREFIX.with(|p| p.borrow().clone());
    let capped = data.capped();
    let bg = r.bg;
    // The header first: it reads, and copies, before the rows.
    let head = row(
        &format!("{key}/h"),
        &head_lead,
        &data,
        &table.head,
        true,
        true,
        capped,
        &r.md,
        r.window,
        t,
    );
    let body: AnyElement = if capped {
        let state = list_for(&r.md, key, n);
        let top = -state.scroll_px_offset_for_scrollbar().y.as_f32();
        let max = state.max_offset_for_scrollbar().y.as_f32();
        // Half a row peeks out at the bottom: a cut-off row says "more".
        let h = (TABLE_ROWS as f32 + 0.5) * ROW_H;
        let (rows, md, t2, k, pre) = (
            data.clone(),
            r.md.clone(),
            *t,
            key.to_string(),
            prefix.clone(),
        );
        // Rows are made as the list draws them, after this block: they keep
        // its place in the reading order.
        let slot = crate::select::slot();
        div()
            .relative()
            .h(px(h))
            .child(handoff(state.clone()))
            .child(
                list(state.clone(), move |ix, window, _| {
                    let _place = slot.enter(ix);
                    row(
                        &format!("{k}/r{ix}"),
                        &pre,
                        &rows,
                        &rows.table.rows[ix],
                        false,
                        ix + 1 < rows.table.rows.len(),
                        true,
                        &md,
                        window,
                        &t2,
                    )
                    .into_any_element()
                })
                .size_full(),
            )
            .when(top > 0.5, |d| d.child(vfade(true, bg)))
            .when(top < max - 0.5, |d| d.child(vfade(false, bg)))
            .child(scrollbar(state, t))
            .child(crate::ui::probe(&r.md.probes(), "md-table-body"))
            .into_any_element()
    } else {
        let mut rows = Vec::with_capacity(n);
        for (i, cells) in table.rows.iter().enumerate() {
            rows.push(row(
                &format!("{key}/r{i}"),
                &prefix,
                &data,
                cells,
                false,
                i + 1 < n,
                false,
                &r.md,
                r.window,
                t,
            ));
        }
        div().children(rows).into_any_element()
    };
    let gutter = if capped { GUTTER } else { 0. };
    let grid = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w(px(data.total_min + gutter))
        .max_w(px(data.total + gutter))
        .border_1()
        .border_color(t.hairline)
        .rounded(px(radius::LG))
        .overflow_hidden()
        .child(head)
        .child(body);
    let scroller = hscroll(&r.md, key, grid, bg);
    let (a, b) = (table.clone(), table.clone());
    let md = copy_button(
        format!("md-copy-md-{key}"),
        "Copy",
        move || a.markdown(),
        t,
        r,
    );
    let csv = copy_button(
        format!("md-copy-csv-{key}"),
        "Copy CSV",
        move || b.csv(),
        t,
        r,
    );
    if !capped {
        return hover_actions(scroller, div().flex().child(md).child(csv), key, t)
            .w_full()
            .max_w(px(data.total))
            .into_any_element();
    }
    // The footer spans the table, not the column, so its actions sit under
    // the table they act on.
    let probes = r.md.probes();
    div()
        .flex()
        .flex_col()
        .gap(px(space::HALF))
        .w_full()
        .max_w(px(data.total + gutter))
        .child(scroller)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .ty(LABEL)
                        .text_color(t.quiet)
                        .pl(px(space::S1))
                        .child(format!("{n} rows")),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(space::S1))
                        .child(md.child(crate::ui::probe(&probes, "md-copy-md")))
                        .child(csv.child(crate::ui::probe(&probes, "md-copy-csv"))),
                ),
        )
        .into_any_element()
}

fn checkbox(done: bool, t: &Theme) -> Div {
    div()
        .size(px(14.))
        .rounded(px(3.))
        .border_1()
        .border_color(if done { t.ink } else { t.faint })
        .when(done, |d| d.bg(t.ink))
        .flex()
        .items_center()
        .justify_center()
        .when(done, |d| {
            d.child(
                svg()
                    .path("icons/check.svg")
                    .size(px(10.))
                    .text_color(t.surface),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::{Block, Callout, is_price, parse, resolve};

    #[test]
    fn links_and_bare_addresses_are_found() {
        let blocks = parse("See [the docs](https://a.example/x) or https://b.example/y.");
        let Block::Para(p) = &blocks[0] else { panic!() };
        let urls: Vec<&str> = p.links.iter().map(|(_, u)| u.as_str()).collect();
        assert_eq!(urls, ["https://a.example/x", "https://b.example/y"]);
        let (r, _) = &p.links[1];
        assert_eq!(&p.text[r.clone()], "https://b.example/y");
    }

    #[test]
    fn prices_are_not_maths() {
        assert!(is_price("10/mo ("));
        assert!(is_price(" and "));
        assert!(!is_price("x"));
        assert!(!is_price("2^n"));
        assert!(!is_price(r"1 \le n"));
        assert!(!is_price("d_k"));
        assert!(!is_price("O(n)"));
    }

    #[test]
    fn dollars_follow_the_rule() {
        let blocks = parse("It costs $10/mo ($30/mo yearly), and $n$ events cost $O(k)$.");
        let Block::Para(p) = &blocks[0] else { panic!() };
        assert!(p.text.contains("$10/mo ($30/mo yearly)"), "{}", p.text);
        let maths: Vec<&str> = p.math.iter().map(|(_, m)| m.as_str()).collect();
        assert_eq!(maths, ["n", "O(k)"]);
        // Each formula sits where it was written.
        assert_eq!(
            &p.text[..p.math[0].0],
            "It costs $10/mo ($30/mo yearly), and "
        );
        assert_eq!(
            p.plain(false),
            "It costs $10/mo ($30/mo yearly), and $n$ events cost $O(k)$."
        );
    }

    #[test]
    fn display_maths_is_its_own_block() {
        let blocks = parse("Before\n\n$$\n\\frac{a}{b}\n$$\n\nAfter $$x^2$$ inline.");
        assert!(matches!(&blocks[0], Block::Para(p) if p.text == "Before"));
        assert!(matches!(&blocks[1], Block::Math(m, true) if m == "\\frac{a}{b}"));
        assert!(matches!(&blocks[2], Block::Para(p) if p.text.trim() == "After"));
        assert!(matches!(&blocks[3], Block::Math(m, true) if m == "x^2"));
        assert!(matches!(&blocks[4], Block::Para(p) if p.text.trim() == "inline."));
        assert_eq!(blocks.len(), 5);
    }

    #[test]
    fn maths_and_mermaid_fences() {
        let blocks = parse("```math\nE = mc^2\n```\n\n```mermaid\nflowchart LR\n  A --> B\n```\n");
        assert!(matches!(&blocks[0], Block::Math(m, true) if m == "E = mc^2"));
        assert!(matches!(&blocks[1], Block::Diagram(d, true) if d.starts_with("flowchart")));
        // Still streaming: the fence hasn't closed.
        let blocks = parse("```mermaid\nflowchart LR\n  A --> B\n");
        assert!(matches!(&blocks[0], Block::Diagram(_, false)));
        let blocks = parse("````mermaid\nA\n```\n");
        assert!(matches!(&blocks[0], Block::Diagram(_, false)));
        let blocks = parse("```rust\nfn main() {}\n```");
        assert!(matches!(&blocks[0], Block::Code(l, b) if l == "rust" && b == "fn main() {}"));
    }

    #[test]
    fn maths_in_lists_and_tables_stays_inline() {
        let blocks = parse("- a $x$ b\n\n| A | B |\n| --- | ---: |\n| $y^2$ | 3 |\n");
        let Block::List(items) = &blocks[0] else {
            panic!()
        };
        let Block::Para(p) = &items[0].blocks[0] else {
            panic!()
        };
        assert_eq!(p.math.len(), 1);
        let Block::Table(t) = &blocks[1] else {
            panic!()
        };
        assert_eq!(t.rows[0][0].math[0].1, "y^2");
        assert_eq!(t.aligns[1], pulldown_cmark::Alignment::Right);
    }

    #[test]
    fn blocks_stay_inside_their_list_item() {
        let blocks = parse(
            "1. Install:\n\n   ```bash\n   cargo test\n   ```\n\n   Then run it.\n2. Next\n   - nested\n",
        );
        assert_eq!(blocks.len(), 1);
        let Block::List(items) = &blocks[0] else {
            panic!()
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].number, Some(1));
        assert!(matches!(&items[0].blocks[0], Block::Para(p) if p.text == "Install:"));
        assert!(
            matches!(&items[0].blocks[1], Block::Code(l, b) if l == "bash" && b == "cargo test")
        );
        assert!(matches!(&items[0].blocks[2], Block::Para(p) if p.text == "Then run it."));
        assert_eq!(items[1].number, Some(2));
        assert!(matches!(&items[1].blocks[1], Block::List(n) if n.len() == 1));
    }

    #[test]
    fn quotes_callouts_breaks_and_html() {
        let blocks = parse(
            "> [!WARNING]\n> Careful\n> - one\n\nline one  \nline two\n\nReturns Vec<String> here.\n\n~~old~~\n",
        );
        assert!(matches!(&blocks[0], Block::Quote(Some(Callout::Warning), b) if b.len() == 2));
        assert!(matches!(&blocks[1], Block::Para(p) if p.text == "line one\nline two"));
        assert!(matches!(&blocks[2], Block::Para(p) if p.text == "Returns Vec<String> here."));
        assert!(matches!(&blocks[3], Block::Para(p) if p.runs[0].1.strike));
    }

    #[test]
    fn pictures_and_file_paths() {
        let blocks = parse("![chart](img/chart.png)\n\nSee `notes/plan.md` and `cargo test`.\n");
        assert!(matches!(&blocks[0], Block::Image(s, a) if s == "img/chart.png" && a == "chart"));
        let Block::Para(p) = &blocks[1] else { panic!() };
        assert_eq!(p.links.len(), 1);
        assert_eq!(p.links[0].1, "notes/plan.md");
        assert_eq!(
            resolve("../b.md#top", Some(std::path::Path::new("/p/docs"))),
            "/p/b.md"
        );
        assert_eq!(resolve("https://x.dev/a", None), "https://x.dev/a");
    }

    #[test]
    fn tables_copy_as_markdown_and_csv() {
        let blocks =
            parse("| Name | Note |\n| :--- | --- |\n| `a` | x, \"y\" |\n| b | $z$ \\| w |\n");
        let Block::Table(t) = &blocks[0] else {
            panic!()
        };
        assert_eq!(
            t.markdown(),
            "| Name | Note |\n| :--- | --- |\n| `a` | x, \"y\" |\n| b | $z$ \\| w |"
        );
        assert_eq!(t.csv(), "Name,Note\na,\"x, \"\"y\"\"\"\nb,$z$ | w");
    }
}
