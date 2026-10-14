//! Markdown to Field Notes elements, enough for replies and the doc viewer:
//! headings, paragraphs, lists and task lists, quotes, code, tables, rules.

use gpui_kit::prelude::*;
use gpui_kit::*;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::ds::{CODE, DISPLAY, DISPLAY_COMPACT, LABEL, READING, Typed, UI, radius, space};
use crate::theme::{self, Theme};

#[derive(Clone, Copy, Default)]
struct Style {
    strong: bool,
    emph: bool,
    code: bool,
    link: bool,
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
}

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

    /// `fade` gives each byte offset an opacity, for words streaming in.
    pub fn element(
        &self,
        t: &Theme,
        base: &'static str,
        ink: Hsla,
        fade: Option<&dyn Fn(usize) -> f32>,
    ) -> AnyElement {
        let mut runs = Vec::new();
        let mut at = 0;
        for (len, st) in &self.runs {
            let mut f = font(if st.code { theme::mono() } else { base });
            if st.strong {
                f.weight = FontWeight::MEDIUM;
            }
            if st.emph {
                f.style = FontStyle::Italic;
            }
            let color = if st.link { t.ink } else { ink };
            // Split a run where the fade changes, word by word.
            match fade {
                Some(fade) => {
                    let piece = &self.text[at..at + len];
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
        let text = StyledText::new(self.text.clone()).with_runs(runs);
        if self.links.is_empty() {
            return text.into_any_element();
        }
        // Links open in the browser; paths open in their default app.
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.text.hash(&mut h);
        let urls: Vec<String> = self.links.iter().map(|(_, u)| u.clone()).collect();
        InteractiveText::new(SharedString::from(format!("md-{:x}", h.finish())), text)
            .on_click(
                self.links.iter().map(|(r, _)| r.clone()).collect(),
                move |ix, _, cx| {
                    if let Some(url) = urls.get(ix) {
                        if url.starts_with('/') {
                            cx.open_url(&format!("file://{url}"));
                        } else {
                            cx.open_url(url);
                        }
                    }
                },
            )
            .into_any_element()
    }
}

fn run(len: usize, font: Font, color: Hsla, st: &Style, t: &Theme) -> TextRun {
    TextRun {
        len,
        font,
        color,
        background_color: st.code.then(|| t.ink.opacity(0.06)),
        underline: st.link.then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(t.quiet.opacity(0.6)),
            wavy: false,
        }),
        strikethrough: None,
    }
}

/// Depth, number (if ordered), checkbox (if a task), text.
pub type ListItem = (usize, Option<u64>, Option<bool>, Inline);

#[derive(Clone)]
pub enum Block {
    Heading(u8, Inline),
    Para(Inline),
    /// Items with their depth, number (if ordered) and checkbox (if a task).
    List(Vec<ListItem>),
    Quote(Vec<Inline>),
    Code(String, String),
    Table(Vec<Inline>, Vec<Vec<Inline>>),
    Rule,
}

/// Head, rows, the row being read, and whether we are still in the head.
type TableState = (Vec<Inline>, Vec<Vec<Inline>>, Vec<Inline>, bool);

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

pub fn parse(src: &str) -> Vec<Block> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let mut out = Vec::new();
    let mut cur = Inline::default();
    let mut st = Style::default();
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut items: Vec<ListItem> = Vec::new();
    let mut task: Option<bool> = None;
    let mut quote: Option<Vec<Inline>> = None;
    let mut code: Option<(String, String)> = None;
    let mut table: Option<TableState> = None;
    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(Tag::Strong) => st.strong = true,
            Event::End(TagEnd::Strong) => st.strong = false,
            Event::Start(Tag::Emphasis) => st.emph = true,
            Event::End(TagEnd::Emphasis) => st.emph = false,
            Event::Start(Tag::Link { dest_url, .. }) => {
                st.link = true;
                cur.href = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                st.link = false;
                cur.href = None;
            }
            Event::Code(c) => cur.push(&c, Style { code: true, ..st }),
            Event::Text(s) => match &mut code {
                Some((_, body)) => body.push_str(&s),
                None if st.link => cur.push(&s, st),
                None => push_linking(&mut cur, &s, st),
            },
            Event::SoftBreak | Event::HardBreak => cur.push(" ", st),
            Event::TaskListMarker(done) => task = Some(done),
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(l) => l.to_string(),
                    _ => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, body)) = code.take() {
                    out.push(Block::Code(lang, body.trim_end().to_string()));
                }
            }
            Event::Start(Tag::List(start)) => {
                if !cur.text.is_empty() && !lists.is_empty() {
                    let n = lists.last().copied().flatten();
                    items.push((lists.len() - 1, n, task.take(), std::mem::take(&mut cur)));
                }
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
                if lists.is_empty() {
                    out.push(Block::List(std::mem::take(&mut items)));
                }
            }
            Event::End(TagEnd::Item) => {
                if !cur.text.is_empty() {
                    let depth = lists.len().saturating_sub(1);
                    let n = lists.last().copied().flatten();
                    items.push((depth, n, task.take(), std::mem::take(&mut cur)));
                }
                if let Some(Some(n)) = lists.last_mut() {
                    *n += 1;
                }
            }
            Event::Start(Tag::BlockQuote(_)) => quote = Some(Vec::new()),
            Event::End(TagEnd::BlockQuote(_)) => {
                if let Some(q) = quote.take() {
                    out.push(Block::Quote(q));
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if !lists.is_empty() {
                    continue;
                }
                let p = std::mem::take(&mut cur);
                match &mut quote {
                    Some(q) => q.push(p),
                    None => out.push(Block::Para(p)),
                }
            }
            Event::End(TagEnd::Heading(level)) => {
                out.push(Block::Heading(level as u8, std::mem::take(&mut cur)));
            }
            Event::Start(Tag::Table(_)) => table = Some((Vec::new(), Vec::new(), Vec::new(), true)),
            Event::End(TagEnd::TableHead) => {
                if let Some(tb) = &mut table {
                    tb.0 = std::mem::take(&mut tb.2);
                    tb.3 = false;
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
                if let Some((head, rows, _, _)) = table.take() {
                    out.push(Block::Table(head, rows));
                }
            }
            Event::Rule => out.push(Block::Rule),
            _ => {}
        }
    }
    out
}

/// Render blocks in the reading column. `doc` sets headings for a document;
/// replies in chat never have big headings.
pub fn render(blocks: &[Block], t: &Theme, doc: bool) -> Div {
    let reading = theme::body();
    div()
        .flex()
        .flex_col()
        .gap(px(space::S3))
        .children(blocks.iter().map(|b| -> AnyElement {
            match b {
                Block::Heading(level, i) => {
                    let (ty, family) = match (doc, level) {
                        (true, 1) => (DISPLAY, theme::serif()),
                        (true, 2) => (DISPLAY_COMPACT, theme::serif()),
                        (true, _) => (READING, reading),
                        // In chat, headings are the interface face, so they
                        // read as structure rather than another paragraph.
                        (false, _) => (READING, theme::sans()),
                    };
                    div()
                        .when(*level > 1 || !doc, |d| d.pt(px(space::S3)))
                        .ty(ty)
                        .text_color(t.ink)
                        .when(!doc || *level > 2, |d| d.font_weight(FontWeight::MEDIUM))
                        .child(i.element(t, family, t.ink, None))
                        .into_any_element()
                }
                Block::Para(i) => div()
                    .ty(READING)
                    .text_color(t.ink)
                    .child(i.element(t, reading, t.ink, None))
                    .into_any_element(),
                Block::List(items) => div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .children(items.iter().map(|(depth, n, task, i)| {
                        let marker: AnyElement = match (task, n) {
                            (Some(done), _) => checkbox(*done, t).into_any_element(),
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
                        let done = matches!(task, Some(true));
                        div()
                            .relative()
                            .pl(px(24. + *depth as f32 * 20.))
                            .ty(READING)
                            .child(
                                div()
                                    .absolute()
                                    .left(px(*depth as f32 * 20.))
                                    .top_0()
                                    .h(px(READING.line))
                                    .flex()
                                    .items_center()
                                    .child(marker),
                            )
                            .child(i.element(t, reading, if done { t.quiet } else { t.ink }, None))
                    }))
                    .into_any_element(),
                Block::Quote(ps) => div()
                    .pl(px(space::S4))
                    .border_l_2()
                    .border_color(t.hairline)
                    .flex()
                    .flex_col()
                    .gap(px(space::S2))
                    .children(ps.iter().map(|i| {
                        div()
                            .ty(READING)
                            .text_color(t.quiet)
                            .child(i.element(t, reading, t.quiet, None))
                    }))
                    .into_any_element(),
                Block::Code(lang, body) => div()
                    .rounded(px(radius::LG))
                    .border_1()
                    .border_color(t.hairline)
                    .bg(t.raised)
                    .overflow_hidden()
                    .child(
                        div()
                            .px(px(space::S3))
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
                                lang.clone()
                            })
                            .child({
                                let text = body.clone();
                                div()
                                    .id(SharedString::from(format!("copy-{}-{}", body.len(), lang)))
                                    .px(px(space::S1H))
                                    .rounded(px(radius::SM))
                                    .cursor_pointer()
                                    .hover(|d| d.bg(t.hover()).text_color(t.ink))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            text.clone(),
                                        ))
                                    })
                                    .child("Copy")
                            }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("code-{}", body.len())))
                            .overflow_x_scroll()
                            .px(px(space::S3))
                            .py(px(space::S2))
                            .ty(CODE)
                            .text_color(t.ink)
                            .children(body.lines().map(|l| {
                                div().whitespace_nowrap().child(if l.is_empty() {
                                    " ".to_string()
                                } else {
                                    l.to_string()
                                })
                            })),
                    )
                    .into_any_element(),
                Block::Table(head, rows) => {
                    let cols = head.len().max(1);
                    let cell = |i: &Inline, header: bool| {
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .px(px(space::S3))
                            .py(px(space::S2))
                            .ty(UI)
                            .text_color(if header { t.quiet } else { t.ink })
                            .when(header, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(i.element(
                                t,
                                theme::sans(),
                                if header { t.quiet } else { t.ink },
                                None,
                            ))
                    };
                    div()
                        .rounded(px(radius::LG))
                        .border_1()
                        .border_color(t.hairline)
                        .overflow_hidden()
                        .child(
                            div()
                                .flex()
                                .bg(t.raised)
                                .border_b_1()
                                .border_color(t.hairline)
                                .children(head.iter().map(|h| cell(h, true))),
                        )
                        .children(rows.iter().enumerate().map(|(r, row)| {
                            div()
                                .flex()
                                .when(r > 0, |d| d.border_t_1().border_color(t.hairline))
                                .children(
                                    (0..cols).map(|c| {
                                        cell(row.get(c).unwrap_or(&Inline::default()), false)
                                    }),
                                )
                        }))
                        .into_any_element()
                }
                Block::Rule => div().h(px(1.)).bg(t.hairline).into_any_element(),
            }
        }))
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
    #[test]
    fn links_and_bare_addresses_are_found() {
        let blocks = super::parse("See [the docs](https://a.example/x) or https://b.example/y.");
        let super::Block::Para(p) = &blocks[0] else {
            panic!()
        };
        let urls: Vec<&str> = p.links.iter().map(|(_, u)| u.as_str()).collect();
        assert_eq!(urls, ["https://a.example/x", "https://b.example/y"]);
        let (r, _) = &p.links[1];
        assert_eq!(&p.text[r.clone()], "https://b.example/y");
    }
}
