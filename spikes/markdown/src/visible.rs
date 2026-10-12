//! What a reader sees, as characters tagged with their style and block kind.
//! Streaming is flicker-free when every frame's visible text is a prefix of the
//! final message's visible text: text only ever appears, it never changes
//! style, changes block, or vanishes.

use pulldown_cmark::{Event, Tag, TagEnd};

use crate::stream::Block;

pub type Styled = Vec<(char, u32)>;

const STRONG: u32 = 1;
const EMPH: u32 = 1 << 1;
const STRIKE: u32 = 1 << 2;
const CODE: u32 = 1 << 3;
const LINK: u32 = 1 << 4;
const MATH: u32 = 1 << 5;
const CODE_BLOCK: u32 = 1 << 6;
const QUOTE: u32 = 1 << 7;
const TABLE: u32 = 1 << 8;
const HEAD: u32 = 1 << 9;
const ITEM: u32 = 1 << 10;
const HEADING_SHIFT: u32 = 12; // 3 bits of heading level
const LIST_SHIFT: u32 = 16; // list depth

/// Tight and loose lists render the same in dent, so a list turning loose
/// mid-stream is not a visible change: paragraphs directly in an item are
/// ignored.
pub fn styled<'a>(blocks: impl Iterator<Item = &'a Block>) -> Styled {
    let mut out = Styled::new();
    let mut flags = 0u32;
    let mut lists = 0u32;
    for b in blocks {
        for ev in &b.events {
            let f = flags | (lists.min(7) << LIST_SHIFT);
            match ev {
                Event::Start(t) => match t {
                    Tag::Strong => flags |= STRONG,
                    Tag::Emphasis => flags |= EMPH,
                    Tag::Strikethrough => flags |= STRIKE,
                    Tag::Link { .. } => flags |= LINK,
                    Tag::CodeBlock(_) => flags |= CODE_BLOCK,
                    Tag::BlockQuote(_) => flags |= QUOTE,
                    Tag::Table(_) => flags |= TABLE,
                    Tag::TableHead => flags |= HEAD,
                    Tag::Item => flags |= ITEM,
                    Tag::List(_) => lists += 1,
                    Tag::Heading { level, .. } => flags |= (*level as u32) << HEADING_SHIFT,
                    _ => {}
                },
                Event::End(t) => {
                    match t {
                        TagEnd::Strong => flags &= !STRONG,
                        TagEnd::Emphasis => flags &= !EMPH,
                        TagEnd::Strikethrough => flags &= !STRIKE,
                        TagEnd::Link => flags &= !LINK,
                        TagEnd::CodeBlock => flags &= !CODE_BLOCK,
                        TagEnd::BlockQuote(_) => flags &= !QUOTE,
                        TagEnd::Table => flags &= !TABLE,
                        TagEnd::TableHead => flags &= !HEAD,
                        TagEnd::Item => flags &= !ITEM,
                        TagEnd::List(_) => lists = lists.saturating_sub(1),
                        TagEnd::Heading(_) => flags &= !(7 << HEADING_SHIFT),
                        _ => {}
                    }
                    let sep = match t {
                        TagEnd::TableCell => Some('\t'),
                        TagEnd::Paragraph if flags & ITEM != 0 => None,
                        TagEnd::Paragraph
                        | TagEnd::Heading(_)
                        | TagEnd::Item
                        | TagEnd::TableRow
                        | TagEnd::TableHead
                        | TagEnd::CodeBlock => Some('\n'),
                        _ => None,
                    };
                    // A separator closes a unit; the open tail never has one,
                    // so a growing paragraph stays a prefix of its final form.
                    if let Some(c) = sep {
                        out.push((c, 0));
                    }
                }
                Event::Text(s) => out.extend(s.chars().map(|c| (c, f))),
                Event::Code(s) => out.extend(s.chars().map(|c| (c, f | CODE))),
                // Math appears whole or not at all: one placeholder per formula.
                Event::InlineMath(s) | Event::DisplayMath(s) => {
                    out.push(('∑', f | MATH | (s.len() as u32 & 0xff) << 24))
                }
                Event::SoftBreak => out.push((' ', f)),
                Event::HardBreak => out.push(('\n', f)),
                Event::TaskListMarker(done) => out.push((if *done { '☑' } else { '☐' }, f)),
                Event::Rule => out.push(('—', 0)),
                Event::Html(s) | Event::InlineHtml(s) => out.extend(s.chars().map(|c| (c, f))),
                Event::FootnoteReference(s) => out.extend(s.chars().map(|c| (c, f | LINK))),
            }
        }
    }
    out
}

/// Whether `now` is a prefix of `last`, ignoring the trailing separator a
/// growing tail can't have yet and the whitespace at the open end.
pub fn is_prefix(now: &Styled, last: &Styled) -> bool {
    let mut n = now.as_slice();
    while let Some(((c, _), rest)) = n.split_last() {
        if c.is_whitespace() {
            n = rest;
        } else {
            break;
        }
    }
    n.len() <= last.len() && last[..n.len()] == *n
}
