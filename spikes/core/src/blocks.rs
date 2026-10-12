//! The block tree each client draws, flattened. A reply becomes a list of
//! blocks (paragraphs, headings, list items, code, quotes, table rows), each
//! a run of styled spans. Flat because every UI lays chat out as a list, and
//! because recursive types cross FFI badly.

use pulldown_cmark::{CodeBlockKind, Event, Tag, TagEnd};
use serde::Serialize;

use crate::stream;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Block {
    pub kind: BlockKind,
    /// List nesting: 0 outside lists, 1 for a top-level item.
    pub depth: u32,
    /// Quote nesting.
    pub quote: u32,
    pub spans: Vec<Span>,
    /// Code and display math keep their text here, unstyled.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum BlockKind {
    Paragraph,
    Heading {
        level: u8,
    },
    /// `marker` is "•", "3." or a task box ("☐", "☑").
    Item {
        marker: String,
    },
    Code {
        lang: String,
    },
    Math,
    Rule,
    /// Cells are told apart by `Span::cell`.
    Row {
        header: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub math: bool,
    pub link: Option<String>,
    /// Table cell index, for rows.
    pub cell: u32,
}

#[derive(Default)]
struct Style {
    bold: u32,
    italic: u32,
    strike: u32,
    links: Vec<String>,
}

struct Walker {
    out: Vec<Block>,
    /// The block taking inline content.
    open: Option<usize>,
    style: Style,
    /// Ordered lists' next number, or None for bullets.
    lists: Vec<Option<u64>>,
    quote: u32,
    cell: u32,
    in_head: bool,
    /// An item whose first paragraph hasn't arrived yet.
    item_waiting: bool,
}

impl Walker {
    fn block(&mut self, kind: BlockKind) {
        self.out.push(Block {
            kind,
            depth: self.lists.len() as u32,
            quote: self.quote,
            spans: Vec::new(),
            text: String::new(),
        });
        self.open = Some(self.out.len() - 1);
    }

    fn span(&mut self, text: &str, code: bool, math: bool) {
        if text.is_empty() {
            return;
        }
        if self.open.is_none() {
            self.block(BlockKind::Paragraph);
        }
        let Some(b) = self.open.and_then(|i| self.out.get_mut(i)) else {
            return;
        };
        if matches!(b.kind, BlockKind::Code { .. } | BlockKind::Math) {
            b.text.push_str(text);
            return;
        }
        let s = Span {
            text: text.to_string(),
            bold: self.style.bold > 0,
            italic: self.style.italic > 0,
            code,
            strike: self.style.strike > 0,
            math,
            link: self.style.links.last().cloned(),
            cell: self.cell,
        };
        match b.spans.last_mut() {
            Some(l) if same_style(l, &s) => l.text.push_str(&s.text),
            _ => b.spans.push(s),
        }
    }

    fn event(&mut self, ev: &Event<'static>) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    if !self.item_waiting {
                        self.block(BlockKind::Paragraph);
                    }
                    self.item_waiting = false;
                }
                Tag::Heading { level, .. } => {
                    self.item_waiting = false;
                    self.block(BlockKind::Heading {
                        level: *level as u8,
                    })
                }
                Tag::BlockQuote(_) => self.quote += 1,
                Tag::List(start) => self.lists.push(*start),
                Tag::Item => {
                    let marker = match self.lists.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}.");
                            *n += 1;
                            m
                        }
                        _ => "•".to_string(),
                    };
                    self.block(BlockKind::Item { marker });
                    self.item_waiting = true;
                }
                Tag::CodeBlock(kind) => {
                    self.item_waiting = false;
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => {
                            l.split_whitespace().next().unwrap_or("").into()
                        }
                        CodeBlockKind::Indented => String::new(),
                    };
                    self.block(BlockKind::Code { lang });
                }
                Tag::Table(_) => {}
                Tag::TableHead => {
                    self.in_head = true;
                    self.cell = 0;
                    self.block(BlockKind::Row { header: true });
                }
                Tag::TableRow => {
                    self.cell = 0;
                    self.block(BlockKind::Row { header: false });
                }
                Tag::TableCell => {}
                Tag::Emphasis => self.style.italic += 1,
                Tag::Strong => self.style.bold += 1,
                Tag::Strikethrough => self.style.strike += 1,
                Tag::Link { dest_url, .. } => self.style.links.push(dest_url.to_string()),
                Tag::Image { .. } => self.span("[image: ", false, false),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock => self.open = None,
                TagEnd::BlockQuote(_) => self.quote = self.quote.saturating_sub(1),
                TagEnd::List(_) => {
                    self.lists.pop();
                    self.open = None;
                }
                TagEnd::Item => {
                    self.item_waiting = false;
                    self.open = None;
                }
                TagEnd::TableHead => {
                    self.in_head = false;
                    self.open = None;
                }
                TagEnd::TableRow => self.open = None,
                TagEnd::TableCell => self.cell += 1,
                TagEnd::Emphasis => self.style.italic = self.style.italic.saturating_sub(1),
                TagEnd::Strong => self.style.bold = self.style.bold.saturating_sub(1),
                TagEnd::Strikethrough => self.style.strike = self.style.strike.saturating_sub(1),
                TagEnd::Link => {
                    self.style.links.pop();
                }
                TagEnd::Image => self.span("]", false, false),
                _ => {}
            },
            Event::Text(t) => self.span(t, false, false),
            Event::Code(t) => self.span(t, true, false),
            Event::InlineMath(t) => self.span(t, false, true),
            Event::DisplayMath(t) => {
                self.block(BlockKind::Math);
                self.span(t, false, true);
                self.open = None;
            }
            Event::Html(t) | Event::InlineHtml(t) => self.span(t, false, false),
            Event::FootnoteReference(t) => self.span(&format!("[{t}]"), false, false),
            Event::SoftBreak => self.span(" ", false, false),
            Event::HardBreak => self.span("\n", false, false),
            Event::Rule => {
                self.block(BlockKind::Rule);
                self.open = None;
            }
            Event::TaskListMarker(done) => {
                if let Some(Block {
                    kind: BlockKind::Item { marker },
                    ..
                }) = self.open.and_then(|i| self.out.get_mut(i))
                {
                    *marker = if *done { "☑" } else { "☐" }.into();
                }
            }
        }
    }
}

fn same_style(a: &Span, b: &Span) -> bool {
    a.bold == b.bold
        && a.italic == b.italic
        && a.code == b.code
        && a.strike == b.strike
        && a.math == b.math
        && a.link == b.link
        && a.cell == b.cell
}

/// One top-level block from the streamer, as UI blocks.
pub fn convert(b: &stream::Block) -> Vec<Block> {
    let mut w = Walker {
        out: Vec::new(),
        open: None,
        style: Style::default(),
        lists: Vec::new(),
        quote: 0,
        cell: 0,
        in_head: false,
        item_waiting: false,
    };
    for ev in &b.events {
        w.event(ev);
    }
    w.out
}

/// A whole text, parsed at once.
pub fn parse(text: &str) -> Vec<Block> {
    stream::blocks(text, 0).iter().flat_map(convert).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        let b = parse(
            "# Title\n\nSome **bold** and `code` with a [link](https://x).\n\n1. one\n2. two\n   - nested\n\n> quoted\n\n```rust\nfn x() {}\n```\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n---\n- [x] done\n",
        );
        let kinds: Vec<String> = b
            .iter()
            .map(|b| format!("{:?}/{}", b.kind, b.depth))
            .collect();
        assert_eq!(
            kinds,
            [
                "Heading { level: 1 }/0",
                "Paragraph/0",
                "Item { marker: \"1.\" }/1",
                "Item { marker: \"2.\" }/1",
                "Item { marker: \"•\" }/2",
                "Paragraph/0",
                "Code { lang: \"rust\" }/0",
                "Row { header: true }/0",
                "Row { header: false }/0",
                "Rule/0",
                "Item { marker: \"☑\" }/1",
            ]
        );
        assert_eq!(b[1].spans.len(), 7);
        assert!(b[1].spans[1].bold);
        assert!(b[1].spans[3].code);
        assert_eq!(b[1].spans[5].link.as_deref(), Some("https://x"));
        assert_eq!(b[1].spans[6].link, None);
        assert_eq!(b[5].quote, 1);
        assert_eq!(b[6].text, "fn x() {}\n");
        assert_eq!(
            b[8].spans.iter().map(|s| s.cell).collect::<Vec<_>>(),
            [0, 1]
        );
    }
}
