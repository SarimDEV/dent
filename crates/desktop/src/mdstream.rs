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
    /// The link reference definitions among the finished blocks, given to
    /// every later parse so a `[text][1]` after its definition is a link.
    defs: String,
    out: Rc<Vec<Block>>,
    /// How many of `committed` `out` starts with: each chunk changes only
    /// what follows, so the finished blocks aren't copied every time.
    shared: usize,
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
            let chunk = &tail[..cut];
            let found = definitions(chunk);
            if found.is_empty() {
                self.committed.extend(parse(&with(chunk, &self.defs)));
            } else {
                // A definition: what came before may use it, so the
                // finished part is read again, once, as a whole.
                self.defs.push_str(&found.join("\n"));
                self.defs.push('\n');
                self.committed = parse(&src[..self.done + cut]);
                self.shared = 0;
            }
            self.done += cut;
        }
        let tail = terminate(&src[self.done..]);
        if self.done > 0 && !definitions(&tail).is_empty() {
            // A definition still in the unfinished part (a footnote's runs
            // on until the reply ends): read it with what came before.
            self.out = Rc::new(parse(&format!("{}{tail}", &src[..self.done])));
            self.shared = 0;
        } else {
            // Copied only if a drawing still holds the last result.
            let out = Rc::make_mut(&mut self.out);
            out.truncate(self.shared);
            out.extend(self.committed[self.shared..].iter().cloned());
            out.extend(parse(&with(&tail, &self.defs)));
            self.shared = self.committed.len();
        }
        self.src = src.to_string();
        self.out.clone()
    }
}

/// `src` with the definitions after it, which add no blocks of their own.
fn with<'a>(src: &'a str, defs: &str) -> Cow<'a, str> {
    if defs.is_empty() {
        Cow::Borrowed(src)
    } else {
        Cow::Owned(format!("{src}\n\n{defs}"))
    }
}

/// The lines in `src` that define a link reference (`[1]: https://…`) or
/// a footnote (`[^1]: …`). Only link references are returned: a footnote's
/// definition is a block of its own and is found only by reading the
/// whole again.
fn definitions(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut any = false;
    for line in src.lines() {
        let l = line.trim_start();
        if line.len() - l.len() > 3 || !l.starts_with('[') {
            continue;
        }
        let Some(end) = l.find("]:") else { continue };
        if end < 2 || l[1..end].contains(['[', ']']) {
            continue;
        }
        any = true;
        if !l.starts_with("[^") {
            out.push(line);
        }
    }
    if any && out.is_empty() {
        // A footnote alone still needs the read again.
        out.push("");
    }
    out
}

/// Where the last top-level block of `tail` starts, when everything before
/// it is finished: the line that starts it is complete, so a half-typed
/// line ("2" before "2.") can't still join the block above. While that
/// line is still being typed, everything before the block above it is
/// finished: a line only ever changes the block just before it (a list it
/// continues, a paragraph it underlines or runs on from).
fn commit_point(tail: &str) -> Option<usize> {
    let mut depth = 0usize;
    // The last two top-level blocks' starts.
    let (mut prev, mut last) = (None, None);
    // A top-level block starts its line: an indented code block's own
    // range starts after its indent, and a cut there would leave the
    // indent behind and make the code a paragraph.
    let mut start = |at: usize| {
        let at = tail[..at].rfind('\n').map_or(0, |n| n + 1);
        if last != Some(at) {
            prev = last;
            last = Some(at);
        }
    };
    for (ev, range) in Parser::new_ext(tail, options()).into_offset_iter() {
        match ev {
            Event::Start(_) => {
                if depth == 0 {
                    start(range.start);
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            _ if depth == 0 => start(range.start),
            _ => {}
        }
    }
    let at = last?;
    if at > 0 && tail[at..].contains('\n') {
        Some(at)
    } else {
        prev.filter(|&p| p > 0)
    }
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
    // The block being written: from the last blank line, list item or
    // heading, or the line after a heading. What opened in an earlier block
    // ended with it.
    let mut block = 0;
    let mut at = 0;
    let mut after_heading = false;
    for line in src[..keep].split_inclusive('\n') {
        if content(line).trim().is_empty() {
            block = at + line.len();
            after_heading = false;
        } else {
            if after_heading || starts_block(line) {
                block = at;
            }
            after_heading = heading(line);
        }
        at += line.len();
    }
    // A table's header shows once its delimiter row is in: a row ending in
    // a pipe with nothing under it yet waits, after a line of text too.
    // Lines that only start with a pipe ("|x| is…") show.
    let mut rows: Vec<(usize, &str)> = Vec::new();
    let mut at = block;
    for line in src[block..keep].split_inclusive('\n') {
        if content(line).starts_with('|') {
            rows.push((at, line));
        } else {
            rows.clear();
        }
        at += line.len();
    }
    if let [(start, head)] = rows[..] {
        let h = content(head).trim_end();
        if h.len() > 1 && h.ends_with('|') {
            keep = start;
        }
    }
    let block = block.min(keep);
    let (cut, closers) = close_inline(&src[block..keep]);
    keep = block + cut;
    if keep == src.len() && closers.is_empty() {
        return Cow::Borrowed(src);
    }
    let body = &src[..keep];
    let words = body.trim_end();
    Cow::Owned(format!("{words}{closers}{}", &body[words.len()..]))
}

/// A line that starts a block of its own: a list item or a heading.
fn starts_block(line: &str) -> bool {
    let mut quoted = line.trim_start();
    while let Some(r) = quoted.strip_prefix('>') {
        quoted = r.trim_start();
    }
    content(line).len() != quoted.len() || heading(line)
}

/// An ATX heading line: one to six `#`, then a space or nothing.
fn heading(line: &str) -> bool {
    let l = content(line);
    let n = l.bytes().take_while(|b| *b == b'#').count();
    (1..=6).contains(&n) && l[n..].chars().next().is_none_or(char::is_whitespace)
}

/// Whether a `$` before `rest` plausibly opens maths: not a price (`$5`),
/// a shell variable (`$HOME`, `$name then`) or substitution (`$(pwd)`).
fn maths_opens(rest: &str) -> bool {
    let Some(c) = rest.chars().next() else {
        // A lone `$` at the end waits for what follows.
        return true;
    };
    if c.is_whitespace() || c.is_ascii_digit() || c == '(' || c == '{' {
        return false;
    }
    let word = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    if word >= 2 {
        let w = &rest[..word];
        if w.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        {
            return false;
        }
        return match rest[word..].chars().next() {
            None => true,
            Some(n) => !(n.is_whitespace() || "/.,;:!?\"')]".contains(n)),
        };
    }
    true
}

/// What closes the block `p`'s open inline spans, and where to cut it
/// first to drop what can't be shown yet. Only a delimiter that could
/// plausibly open a span counts: one after a space or an opening mark and
/// before a word, so globs (`*.rs`), sums (`2*3`) and paths (`/_x`) stay
/// text. A `$` or `[` that hasn't resolved by the end of its line (or, for
/// `[`, within 80 bytes or before a `)`) was text.
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
        // A line break ends what a `$` or `[` might have started.
        if c == b'\n' {
            math = None;
            if let Some((_, _, false)) = link {
                link = None;
            }
        }
        // Brackets that run long or close with `)` aren't a link's words.
        if let Some((start, None, false)) = link
            && (i - start > 80 || c == b')')
        {
            link = None;
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
            // An autolink or a tag: nothing inside it is formatting.
            b'<' if b.get(i + 1).is_some_and(u8::is_ascii_alphabetic) => {
                let rest = &p[i + 1..];
                match rest.find(|c: char| c == '>' || c.is_whitespace()) {
                    Some(e) if rest[e..].starts_with('>') => {
                        i += e + 2;
                        continue;
                    }
                    Some(_) => {}
                    None => break,
                }
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
                // Opens only where a span plausibly starts.
                let opens = before.is_none_or(|a| a.is_whitespace() || "([{\"'*_~".contains(a))
                    && after.is_none_or(|a| {
                        a.is_alphanumeric()
                            || "*_~`[(\"'\\".contains(a)
                            || (!a.is_ascii() && !a.is_whitespace())
                    });
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
                None if maths_opens(&p[i + 1..]) => math = Some(i),
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
/// when `now` isn't more of `was` (a new reply started streaming). Words
/// end at a space or a line break, as the drawn text counts them (a line
/// break inside a paragraph is drawn as a space).
pub fn new_words(was: &str, now: &str) -> Option<usize> {
    let more = now.strip_prefix(was)?;
    Some(more.matches([' ', '\n']).count())
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

    /// Text that only looks like the start of formatting shows as it comes,
    /// and nothing opened in one block closes in the next.
    #[test]
    fn plain_text_that_looks_like_formatting_shows() {
        for s in [
            // Globs, arithmetic and paths aren't emphasis.
            "Match *.rs files and then",
            "2*3 = 6 and so",
            "**/*.rs is a glob and",
            "See https://x.com/_private and more",
            "<https://x.com/*foo> and bar",
            // Opened in one block, never closed in the next.
            "- Use *.rs files\n- Next item",
            "- Use *it here\n- Next item",
            "# Title with *star\nThen text",
            "- a `b \n- c",
            // Shell variables, command substitution and ranges aren't maths
            // or links.
            "Set $HOME then run make and",
            "- Set $PATH\n- second item\n- third",
            "Use $(pwd) as the root and",
            "Set $name then run make",
            "Ranges like [0, 10) are half-open and",
            "a [note\nthat goes on",
            // Pipes that aren't a table.
            "|x| is the absolute value\nof x and",
        ] {
            assert_eq!(t(s), s, "{s:?}");
        }
        // Real formatting still closes, in its own block.
        assert_eq!(t("- one\n- an *aside"), "- one\n- an *aside*");
        assert_eq!(t("## Use `cargo"), "## Use `cargo`");
        assert_eq!(t("Area is $\\pi r^2"), "Area is ");
        assert_eq!(t("so $x"), "so ");
        assert_eq!(t("see $O(n)"), "see ");
    }

    #[test]
    fn a_table_header_waits_even_after_a_line() {
        assert_eq!(t("Intro:\n| a | b |\n"), "Intro:\n");
        assert_eq!(
            t("Intro:\n| a | b |\n|---|---|\n"),
            "Intro:\n| a | b |\n|---|---|\n"
        );
    }

    /// A reference link or footnote whose definition comes later turns into
    /// a link once the definition is in, not only when the reply ends.
    #[test]
    fn references_resolve_once_defined() {
        let mut s = Stream::default();
        let doc = "See [the docs][1] now.\n\nMore text.\n\n[1]: https://x.y\n\nAnd [again][1] here.\n\nEnd";
        let mut out = s.update(&doc[..1]);
        for (i, _) in doc.char_indices().skip(2) {
            out = s.update(&doc[..i]);
        }
        let texts: Vec<_> = out
            .iter()
            .filter_map(|b| match b {
                Block::Para(i) => Some(i.text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts[0], "See the docs now.");
        assert_eq!(texts[2], "And again here.");
        // A footnote's mark, likewise.
        let mut s = Stream::default();
        let doc = "Fact[^1] here.\n\nMore.\n\n[^1]: Source.\n\nEnd";
        let mut out = s.update(&doc[..1]);
        for (i, _) in doc.char_indices().skip(2) {
            out = s.update(&doc[..i]);
        }
        let first = match &out[0] {
            Block::Para(i) => i.text.clone(),
            _ => String::new(),
        };
        let whole = match &parse(doc)[0] {
            Block::Para(i) => i.text.clone(),
            _ => String::new(),
        };
        assert_eq!(first, whole);
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

    /// A block, written out whole: kind, words, and what's inside, so two
    /// parses compare block by block.
    fn sig(b: &Block) -> String {
        let all = |bs: &[Block]| bs.iter().map(sig).collect::<Vec<_>>().join("|");
        match b {
            Block::Heading(l, i) => format!("h{l}({})", i.text),
            Block::Para(i) => format!("p({})", i.text),
            Block::List(items) => format!(
                "list[{}]",
                items
                    .iter()
                    .map(|it| format!("{:?}{:?}:{}", it.number, it.task, all(&it.blocks)))
                    .collect::<Vec<_>>()
                    .join(";")
            ),
            Block::Quote(c, bs) => format!("quote{c:?}[{}]", all(bs)),
            Block::Image(a, b) => format!("img({a},{b})"),
            Block::Code(l, c, closed) => format!("code({l},{c},{closed})"),
            Block::Math(m, closed) => format!("math({m},{closed})"),
            Block::Diagram(d, closed) => format!("diagram({d},{closed})"),
            Block::Table(t) => format!("table({})", t.markdown()),
            Block::Rule => "rule".into(),
        }
    }

    /// Replies with what streaming gets wrong if anything does: lists that
    /// go on after a gap, fences, tables growing a row at a time, quotes
    /// and callouts, maths and diagrams, HTML, a footnote and a link
    /// definition read after their use.
    const CORPUS: &[&str] = &[
        "# Plan\n\nFirst **bold** and *it* with `code` and [a link](https://x.y).\n\n\
         1. one\n2. two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n\
         ```rust\nfn main() {}\n```\n\n> quoted **text**\n\nLast line.",
        "Intro.\n\n1. one\n\n2. two after a gap\n   - nested\n   - more\n\n3. three\n\nAfter.",
        "- [ ] todo\n- [x] done\n\n* other\n\n---\n\n## Next\n\ntext",
        "Code:\n\n```python\ndef f():\n    return 1\n\n\nprint(f())\n```\n\n~~~\ntilde\n~~~\n\n    indented\n    code\n\nend",
        "> [!NOTE]\n> a callout\n> over lines\n\n> plain\n>\n> two paras\n\nout",
        "| x | y | z |\n|:--|:-:|--:|\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |\n| 7 | 8 | 9 |\n\nunder",
        "Maths $a+b$ inline.\n\n$$\n\\frac{1}{2}\n$$\n\n```math\nx^2\n```\n\n```mermaid\ngraph TD; A-->B\n```\n\ndone",
        "<details>\n<summary>more</summary>\n\nhidden\n\n</details>\n\nvisible",
        "See [the docs][d] and a note[^1].\n\nMore text.\n\n[d]: https://d.example\n\n[^1]: The note.",
        "Unclosed at the end:\n\n```sh\necho hi\n",
    ];

    /// Every reply above, streamed a character at a time: every prefix
    /// parses without a panic, blocks once finished never change (except
    /// when a link or footnote definition arrives, which reads the finished
    /// part again, by design), and the end matches the plain parse block by
    /// block. Decision 0013 rests on this.
    #[test]
    fn streaming_ends_where_parsing_does() {
        for doc in CORPUS {
            let mut s = Stream::default();
            let mut finished: Vec<String> = Vec::new();
            let mut kept = 0;
            for (i, _) in doc.char_indices().skip(1) {
                let defs = s.defs.len();
                let _ = s.update(&doc[..i]);
                let now: Vec<String> = s.committed.iter().map(sig).collect();
                if s.defs.len() == defs {
                    assert!(
                        now.len() >= finished.len(),
                        "{doc:?} at {i}: blocks un-finished"
                    );
                    assert_eq!(
                        &now[..finished.len()],
                        &finished[..],
                        "{doc:?} at {i}: a finished block changed"
                    );
                }
                kept = kept.max(now.len());
                finished = now;
            }
            let streamed: Vec<String> = s.update(doc).iter().map(sig).collect();
            let whole: Vec<String> = parse(doc).iter().map(sig).collect();
            assert_eq!(streamed, whole, "{doc:?}");
            // The first, of separate blocks, keeps most as it goes.
            if std::ptr::eq(doc, &CORPUS[0]) {
                assert!(kept > 3, "finished blocks were kept");
            }
        }
    }

    /// While a long line is being typed, the blocks before the one above it
    /// are already finished: a chunk's cost doesn't grow with the reply.
    #[test]
    fn finished_blocks_commit_while_a_line_is_typed() {
        let mut s = Stream::default();
        s.update("One.\n\nTwo.\n\nThree is being");
        assert_eq!(s.done, "One.\n\n".len());
        assert_eq!(s.committed.len(), 1);
        // The block just above can still change ("2" continuing a list).
        let mut s = Stream::default();
        s.update("Intro.\n\n1. one\n2");
        assert_eq!(s.done, "Intro.\n\n".len());
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
        // A line break inside a paragraph is a word's end, as drawn.
        assert_eq!(new_words("one", "one\ntwo three"), Some(2));
    }

    /// What one chunk of a long reply costs, near its end: run with
    /// `cargo test --release -p dent-desktop stream_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn stream_cost() {
        let para = "Some words with **bold**, `code` and [a link](https://x.y) in them. ";
        let mut doc = String::new();
        for i in 0..400 {
            doc.push_str(&format!(
                "## Part {i}\n\n{}\n\n- one\n- two\n\n",
                para.repeat(4)
            ));
        }
        let mut s = Stream::default();
        let step = 30;
        let mut at = 0;
        let mut times = Vec::new();
        while at < doc.len() {
            at = (at + step).min(doc.len());
            while !doc.is_char_boundary(at) {
                at += 1;
            }
            let t = std::time::Instant::now();
            std::hint::black_box(s.update(&doc[..at]));
            times.push(t.elapsed());
        }
        let last: std::time::Duration = times[times.len() - 100..]
            .iter()
            .sum::<std::time::Duration>()
            / 100;
        let first: std::time::Duration = times[..100].iter().sum::<std::time::Duration>() / 100;
        eprintln!(
            "reply {} KB, {} blocks: a chunk costs {first:?} at the start, {last:?} at the end (tail {} bytes)",
            doc.len() / 1024,
            s.out.len(),
            doc.len() - s.done
        );
    }
}
