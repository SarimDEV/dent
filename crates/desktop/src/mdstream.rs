//! Markdown as it streams in ([chat-rendering.md § Streaming]).
//!
//! A reply still arriving is parsed in two parts: the blocks already
//! finished, parsed once and kept, and the tail, re-parsed on each chunk
//! after a terminator has tidied its unfinished end so it doesn't flip from
//! raw to formatted when the next characters land. Finished text goes
//! through a bounded cache keyed by the whole text; the prefixes of a reply
//! never do.
//!
//! [chat-rendering.md § Streaming]: ../../../docs/chat-rendering.md#streaming

use std::borrow::Cow;
use std::rc::Rc;

use pulldown_cmark::{Event, Parser};

use crate::lru::Lru;
use crate::markdown::{Block, options, parse};

/// Parsed Markdown, remembered: finished texts by their hash, and each reply
/// streaming in by the place it's drawn.
pub struct Cache {
    done: Lru<u64, Rc<Vec<Block>>>,
    live: Vec<(String, Stream)>,
}

impl Default for Cache {
    fn default() -> Self {
        Cache {
            done: Lru::new(512),
            live: Vec::new(),
        }
    }
}

/// How many replies may stream at once (one per session drawn).
const LIVE: usize = 4;

impl Cache {
    /// Finished Markdown, parsed once.
    pub fn get(&mut self, src: &str) -> Rc<Vec<Block>> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        src.hash(&mut h);
        let key = h.finish();
        if let Some(b) = self.done.get(&key) {
            return b;
        }
        let b = Rc::new(parse(src));
        self.done.insert(key, b.clone());
        b
    }

    /// A reply still streaming in, drawn at `at`: only its unfinished end is
    /// parsed again.
    pub fn live(&mut self, at: &str, src: &str) -> Rc<Vec<Block>> {
        let ix = match self.live.iter().position(|(k, _)| k == at) {
            Some(ix) => ix,
            None => {
                if self.live.len() >= LIVE {
                    self.live.remove(0);
                }
                self.live.push((at.to_string(), Stream::default()));
                self.live.len() - 1
            }
        };
        self.live[ix].1.update(src)
    }

    pub fn clear(&mut self) {
        self.done.clear();
        self.live.clear();
    }
}

/// One reply streaming in: what's been committed, and the latest result.
#[derive(Default)]
pub struct Stream {
    src: String,
    /// Bytes of `src` whose blocks are finished.
    done: usize,
    committed: Vec<Block>,
    out: Rc<Vec<Block>>,
}

impl Stream {
    pub fn update(&mut self, src: &str) -> Rc<Vec<Block>> {
        if src == self.src && !src.is_empty() {
            return self.out.clone();
        }
        // Something other than more of the same: start over.
        if !src.starts_with(self.src.as_str()) {
            *self = Stream::default();
        }
        let tail = &src[self.done..];
        if let Some(cut) = commit_point(tail) {
            self.committed.extend(parse(&tail[..cut]));
            self.done += cut;
        }
        let mut blocks = self.committed.clone();
        blocks.extend(parse(&terminate(&src[self.done..])));
        self.src = src.to_string();
        self.out = Rc::new(blocks);
        self.out.clone()
    }
}

/// Where the last top-level block of `tail` starts, when everything before
/// it is finished: the line that starts it is complete, so a half-typed
/// line ("2" before "2.") can't still join the block above.
fn commit_point(tail: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut last = None;
    for (ev, range) in Parser::new_ext(tail, options()).into_offset_iter() {
        match ev {
            Event::Start(_) => {
                if depth == 0 {
                    last = Some(range.start);
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            _ if depth == 0 => last = Some(range.start),
            _ => {}
        }
    }
    let at = last.filter(|&at| at > 0)?;
    tail[at..].contains('\n').then_some(at)
}

/// A line without what holds it: indentation, quote markers, a list marker.
fn content(line: &str) -> &str {
    let mut l = line.trim_start();
    loop {
        if let Some(r) = l.strip_prefix('>') {
            l = r.trim_start();
            continue;
        }
        let digits = l.bytes().take_while(u8::is_ascii_digit).count();
        let marker = match l.as_bytes().get(digits) {
            Some(b'.' | b')') if digits > 0 => digits + 1,
            Some(b'-' | b'*' | b'+') if digits == 0 => 1,
            _ => 0,
        };
        if marker > 0 && l[marker..].starts_with([' ', '\t']) {
            l = l[marker..].trim_start();
            continue;
        }
        return l;
    }
}

/// The fence a line opens or closes: its character and length.
fn fence(line: &str) -> Option<(char, usize)> {
    let l = content(line);
    let ch = l.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = l.chars().take_while(|c| *c == ch).count();
    (n >= 3).then_some((ch, n))
}

/// A line that says nothing yet and could still turn into something else:
/// a bare list or heading marker, a setext underline or rule, a number
/// that may become a list item.
fn undecided(line: &str) -> bool {
    let l = line.trim();
    if l.is_empty() {
        return false;
    }
    let first = l.chars().next().unwrap_or(' ');
    let same = |c: char| l.chars().all(|x| x == c || x == ' ');
    if matches!(first, '-' | '=' | '*' | '_' | '#' | '+') && same(first) {
        return true;
    }
    if l.len() <= 9
        && l.bytes()
            .all(|b| b.is_ascii_digit() || b == b'.' || b == b')')
    {
        return true;
    }
    // A list item whose words haven't started, task box and all.
    let c = content(line).trim();
    c.len() < l.len() && matches!(c, "" | "[" | "[ " | "[ ]" | "[x" | "[x]" | "[X" | "[X]")
}

/// A table's delimiter row: `|---|:--:|`.
fn delimiter_row(line: &str) -> bool {
    let l = content(line).trim();
    l.contains('-') && l.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// The unfinished end of a reply, tidied so it parses as it will once
/// finished: open emphasis, code spans and link targets are closed;
/// delimiters with nothing after them yet, an unresolved `[` or `$`, a fence
/// or table row being typed, a bare list or heading marker and a setext
/// underline are held back. Inside an open code fence only a closing fence
/// being typed is held back.
pub fn terminate(src: &str) -> Cow<'_, str> {
    let last_start = src.rfind('\n').map_or(0, |i| i + 1);
    let last = &src[last_start..];
    // Inside an open fence, the code shows as it comes.
    let mut open: Option<(char, usize)> = None;
    for line in src[..last_start].lines() {
        match (open, fence(line)) {
            (None, Some(f)) => open = Some(f),
            (Some((c, n)), Some((c2, n2)))
                if c == c2 && n2 >= n && content(line).trim_end().chars().all(|x| x == c) =>
            {
                open = None
            }
            _ => {}
        }
    }
    if let Some((ch, _)) = open {
        // Its closing fence being typed waits, so it doesn't show as code.
        let l = content(last).trim_end();
        return if !l.is_empty() && l.chars().all(|x| x == ch) {
            Cow::Borrowed(&src[..last_start])
        } else {
            Cow::Borrowed(src)
        };
    }
    let mut keep = src.len();
    let c = content(last);
    if !last.is_empty() && (undecided(last) || fence(last).is_some() || c.starts_with('|')) {
        keep = last_start;
    }
    // The paragraph being written: from the last blank line.
    let mut para = 0;
    let mut at = 0;
    for line in src[..keep].split_inclusive('\n') {
        if content(line).trim().is_empty() {
            para = at + line.len();
        }
        at += line.len();
    }
    // A table's header shows once its delimiter row is in.
    if content(&src[para..keep]).starts_with('|') {
        let mut lines = src[para..keep].lines();
        let _head = lines.next();
        if !lines.next().is_some_and(delimiter_row) {
            keep = para;
        }
    }
    let (cut, closers) = close_inline(&src[para..keep]);
    keep = para + cut;
    if keep == src.len() && closers.is_empty() {
        return Cow::Borrowed(src);
    }
    let body = &src[..keep];
    let words = body.trim_end();
    Cow::Owned(format!("{words}{closers}{}", &body[words.len()..]))
}

/// What closes the paragraph `p`'s open inline spans, and where to cut it
/// first to drop what can't be shown yet.
fn close_inline(p: &str) -> (usize, String) {
    let b = p.as_bytes();
    let ch_before = |i: usize| p[..i].chars().next_back();
    let ch_after = |i: usize| p.get(i..).and_then(|s| s.chars().next());
    // An open code span: where, and its backtick run.
    let mut code: Option<(usize, usize)> = None;
    // Open emphasis and strikethrough: the delimiter, and where.
    let mut stack: Vec<(&'static str, usize)> = Vec::new();
    // An open link: where its `[` is, where its `]` is, and whether its
    // target has begun.
    let mut link: Option<(usize, Option<usize>, bool)> = None;
    let mut math: Option<usize> = None;
    let mut display: Option<usize> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let run = |c: u8| b[i..].iter().take_while(|x| **x == c).count();
        if let Some((_, n)) = code {
            if c == b'`' {
                let r = run(b'`');
                if r == n {
                    code = None;
                }
                i += r;
            } else {
                i += 1;
            }
            continue;
        }
        // A `]` not followed by `(` was just brackets.
        if let Some((_, Some(close), false)) = link
            && i == close + 1
            && c != b'('
        {
            link = None;
        }
        if let Some((_, _, true)) = link {
            if c == b')' {
                link = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'\\' => {
                i += 2;
                continue;
            }
            b'`' => {
                let r = run(b'`');
                code = Some((i, r));
                i += r;
                continue;
            }
            b'*' | b'_' | b'~' => {
                let r = run(c);
                let (before, after) = (ch_before(i), ch_after(i + r));
                let opens = after.is_none_or(|a| !a.is_whitespace());
                let closes = before.is_some_and(|a| !a.is_whitespace());
                let intraword = c == b'_'
                    && before.is_some_and(char::is_alphanumeric)
                    && after.is_some_and(char::is_alphanumeric);
                if !intraword {
                    let delims: &[&'static str] = match (c, r) {
                        (b'~', 2) => &["~~"],
                        (b'~', _) => &[],
                        (b'*', 1) => &["*"],
                        (b'*', 2) => &["**"],
                        (b'*', _) => &["**", "*"],
                        (_, 1) => &["_"],
                        (_, 2) => &["__"],
                        _ => &["__", "_"],
                    };
                    let mut at = i;
                    for d in delims {
                        match stack.iter().rposition(|(x, _)| x == d) {
                            Some(pos) if closes => stack.truncate(pos),
                            _ if opens => stack.push((*d, at)),
                            _ => {}
                        }
                        at += d.len();
                    }
                }
                i += r;
                continue;
            }
            b'[' if link.is_none() => {
                let start = if i > 0 && b[i - 1] == b'!' { i - 1 } else { i };
                link = Some((start, None, false));
            }
            b']' => {
                if let Some((_, close @ None, _)) = &mut link {
                    *close = Some(i);
                }
            }
            b'(' => {
                if let Some((_, Some(close), target)) = &mut link
                    && *close + 1 == i
                {
                    *target = true;
                }
            }
            b'$' if b.get(i + 1) == Some(&b'$') => {
                display = match display {
                    Some(_) => None,
                    None => Some(i),
                };
                i += 2;
                continue;
            }
            b'$' => match math {
                Some(_) => math = None,
                // A price is text, never maths.
                None if !ch_after(i + 1)
                    .is_some_and(|a| a.is_ascii_digit() || a.is_whitespace()) =>
                {
                    math = Some(i)
                }
                None => {}
            },
            _ => {}
        }
        i += 1;
    }
    let mut cut = p.len();
    let mut close_link = false;
    match link {
        Some((_, _, true)) => close_link = true,
        Some((start, _, false)) => cut = cut.min(start),
        None => {}
    }
    if let Some(at) = math {
        cut = cut.min(at);
    }
    if let Some(at) = display {
        cut = cut.min(at);
    }
    let mut closers = String::new();
    match code {
        // Nothing in it yet: the backticks wait.
        Some((at, n)) if p[at + n..cut.max(at + n)].trim().is_empty() => cut = cut.min(at),
        Some((at, n)) if at < cut => closers.push_str(&"`".repeat(n)),
        _ => {}
    }
    if close_link && link.is_some_and(|(s, ..)| s < cut) {
        closers.push(')');
    }
    stack.retain(|(_, at)| *at < cut);
    // A delimiter with nothing after it yet waits.
    while let Some(&(d, at)) = stack.last() {
        if closers.is_empty() && p.get(at + d.len()..cut).is_none_or(|s| s.trim().is_empty()) {
            cut = at;
            stack.pop();
        } else {
            break;
        }
    }
    for (d, _) in stack.iter().rev() {
        closers.push_str(d);
    }
    (cut, closers)
}

/// Words that arrived since `was` was drawn, for fading them in; `None`
/// when `now` isn't more of `was` (a new reply started streaming).
pub fn new_words(was: &str, now: &str) -> Option<usize> {
    let more = now.strip_prefix(was)?;
    Some(more.matches(' ').count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> String {
        terminate(s).into_owned()
    }

    #[test]
    fn open_spans_close() {
        assert_eq!(t("some **bold"), "some **bold**");
        assert_eq!(t("an *aside and **more"), "an *aside and **more***");
        assert_eq!(t("run `cargo te"), "run `cargo te`");
        assert_eq!(
            t("see [the docs](https://exa"),
            "see [the docs](https://exa)"
        );
        assert_eq!(t("~~gone"), "~~gone~~");
        // Closed already: untouched.
        assert_eq!(t("**done** and `x`"), "**done** and `x`");
        assert_eq!(t("snake_case_name"), "snake_case_name");
        // Closers go before trailing space, where they still close.
        assert_eq!(t("**bold "), "**bold** ");
    }

    #[test]
    fn what_cant_show_yet_waits() {
        assert_eq!(t("then **"), "then ");
        assert_eq!(t("a `"), "a ");
        assert_eq!(t("see [the do"), "see ");
        assert_eq!(t("an ![pic"), "an ");
        assert_eq!(t("costs $5 and $x^"), "costs $5 and ");
        assert_eq!(t("Intro\n- "), "Intro\n");
        assert_eq!(t("Intro\n2"), "Intro\n");
        assert_eq!(t("Intro\n##"), "Intro\n");
        assert_eq!(t("Title\n---"), "Title\n");
        assert_eq!(t("Steps\n- [ "), "Steps\n");
        assert_eq!(t("Text\n```ru"), "Text\n");
        // A table waits for its delimiter row, then shows complete rows.
        assert_eq!(t("| a | b |\n|--"), "");
        assert_eq!(t("| a | b |\n|---|---|\n| 1 | 2"), "| a | b |\n|---|---|\n");
    }

    #[test]
    fn code_streams_as_it_comes() {
        assert_eq!(t("```rust\nfn main() {"), "```rust\nfn main() {");
        assert_eq!(t("```rust\nfn main() {}\n``"), "```rust\nfn main() {}\n");
        // No emphasis games inside code.
        assert_eq!(t("```\nlet a = *b"), "```\nlet a = *b");
        assert_eq!(t("> ```\n> x = **y"), "> ```\n> x = **y");
    }

    #[test]
    fn never_splits_a_character() {
        for s in [
            "héllo **wörld",
            "日本語の`コード",
            "[リンク](ht",
            "€ $é",
            "**é",
        ] {
            for (i, _) in s.char_indices() {
                let _ = terminate(&s[..i]);
            }
            let _ = terminate(s);
        }
    }

    /// Streamed a character at a time, every prefix parses without a panic,
    /// finished blocks never change, and the end matches the plain parse.
    #[test]
    fn streaming_ends_where_parsing_does() {
        let doc = "# Plan\n\nFirst **bold** and *it* with `code` and [a link](https://x.y).\n\n\
                   1. one\n2. two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n\
                   ```rust\nfn main() {}\n```\n\n> quoted **text**\n\nLast line.";
        let mut s = Stream::default();
        let mut prev_committed = 0;
        for (i, _) in doc.char_indices().skip(1) {
            let _ = s.update(&doc[..i]);
            assert!(s.committed.len() >= prev_committed);
            prev_committed = s.committed.len();
        }
        let streamed = s.update(doc);
        let whole = parse(doc);
        let shape = |b: &[Block]| -> Vec<String> {
            b.iter()
                .map(|b| match b {
                    Block::Heading(l, i) => format!("h{l}:{}", i.text),
                    Block::Para(i) => format!("p:{}", i.text),
                    Block::List(items) => format!("list:{}", items.len()),
                    Block::Quote(_, b) => format!("quote:{}", b.len()),
                    Block::Code(l, c, closed) => format!("code:{l}:{c}:{closed}"),
                    Block::Table(t) => format!("table:{}", t.rows.len()),
                    _ => "other".into(),
                })
                .collect()
        };
        assert_eq!(shape(&streamed), shape(&whole));
        assert!(prev_committed > 3, "finished blocks were kept");
    }

    #[test]
    fn a_new_reply_starts_over() {
        let mut s = Stream::default();
        s.update("First reply.\n\nMore");
        let b = s.update("Second");
        assert_eq!(b.len(), 1);
        assert!(matches!(&b[0], Block::Para(i) if i.text == "Second"));
    }

    #[test]
    fn words_fade_in_only_as_more_of_the_same() {
        assert_eq!(new_words("Hello", "Hello there you"), Some(2));
        assert_eq!(new_words("héllo wörld", "€uro"), None);
        assert_eq!(new_words("", "a b"), Some(1));
    }
}
