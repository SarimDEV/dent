//! The stable-prefix streaming layer (decision 0013).
//!
//! Text arrives in chunks. Only the tail after the last committed block is
//! re-parsed; every top-level block except the last is committed and never
//! re-parsed again, so finished blocks cannot move. The unfinished tail goes
//! through a terminator that closes open inline syntax and holds back an
//! ambiguous last line, so nothing flickers between raw and rendered.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser};

pub fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
        | Options::ENABLE_DEFINITION_LIST
}

/// One top-level block: its source range and its events in a comparable form.
#[derive(Clone, PartialEq, Debug)]
pub struct Block {
    pub range: Range<usize>,
    pub events: Vec<Event<'static>>,
}

/// Split `src` into top-level blocks. Ranges are offset by `base`.
pub fn blocks(src: &str, base: usize) -> Vec<Block> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut events = Vec::new();
    for (ev, r) in Parser::new_ext(src, options()).into_offset_iter() {
        if depth == 0 {
            start = r.start;
        }
        match &ev {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        events.push(dollars(ev).into_static());
        if depth == 0 {
            out.push(Block {
                range: base + start..base + r.end,
                events: std::mem::take(&mut events),
            });
        }
    }
    out
}

/// Single-dollar math is only math when it looks like TeX. In real agent
/// output every `$…$` pair found was two prices ("$10/mo ($30/mo"), so a span
/// that starts with a digit or space and has no TeX syntax stays text.
pub fn dollars(ev: Event<'_>) -> Event<'_> {
    match ev {
        Event::InlineMath(s)
            if s.starts_with(|c: char| c.is_ascii_digit() || c.is_whitespace())
                && !s.contains(['\\', '^', '_', '{']) =>
        {
            Event::Text(format!("${s}$").into())
        }
        ev => ev,
    }
}

#[derive(Default)]
pub struct Streamer {
    pub text: String,
    /// Bytes before this are committed.
    stable: usize,
    pub committed: Vec<Block>,
    /// The rendered tail: not committed, may change on the next chunk.
    pub tail: Vec<Block>,
    /// Whether the terminator is applied (off only to measure its effect).
    pub terminate: bool,
}

impl Streamer {
    pub fn new(terminate: bool) -> Self {
        Self {
            terminate,
            ..Default::default()
        }
    }

    pub fn push(&mut self, chunk: &str) {
        self.text.push_str(chunk);
        let tail = &self.text[self.stable..];
        let mut parsed = blocks(tail, self.stable);
        // Commit every block whose successor starts on a complete line: a
        // half-typed line ("2" before "2. ") can still join the block above.
        let complete = self.stable + tail.rfind('\n').map_or(0, |i| i + 1);
        let keep = (1..parsed.len())
            .rev()
            .find(|&i| parsed[i].range.start < complete)
            .unwrap_or(0);
        if keep > 0 {
            let next = parsed[keep].range.start;
            let rest = parsed.split_off(keep);
            self.committed.extend(parsed);
            self.stable = next;
            parsed = rest;
        }
        self.tail = if self.terminate {
            let src = terminate(&self.text[self.stable..]);
            blocks(&src, self.stable)
        } else {
            parsed
        };
    }

    /// The message is complete: commit the tail as parsed, unterminated.
    pub fn finish(&mut self) {
        let rest = blocks(&self.text[self.stable..], self.stable);
        self.committed.extend(rest);
        self.tail.clear();
        self.stable = self.text.len();
    }

    pub fn visible(&self) -> impl Iterator<Item = &Block> {
        self.committed.iter().chain(&self.tail)
    }
}

/// Make the unfinished tail render as its final form will: hold back an
/// ambiguous last line and close inline syntax left open.
pub fn terminate(tail: &str) -> String {
    let (done, last) = match tail.rfind('\n') {
        Some(i) => tail.split_at(i + 1),
        None => ("", tail),
    };
    let mut s = done.to_string();
    if !held(last) {
        s.push_str(last);
    }
    // A table header without its delimiter row yet is held as well.
    if let Some(prev) = s.lines().last()
        && prev.trim_start().starts_with('|')
        && !in_table(&s)
    {
        let cut = s.trim_end_matches('\n').rfind('\n').map_or(0, |i| i + 1);
        s.truncate(cut);
    }
    if in_fence(&s) || in_display_math(&s) {
        return s;
    }
    // Holding back text can leave a bare marker behind ("- " under a line
    // reads as a heading underline), so re-check the last line until stable.
    for _ in 0..3 {
        close_inline(&mut s);
        let start = s.rfind('\n').map_or(0, |i| i + 1);
        if start < s.len() && held(&s[start..]) {
            s.truncate(start);
        } else {
            break;
        }
    }
    s
}

/// Lines whose meaning depends on what comes next: a fence or table row
/// still being typed, a bare list or heading marker, a setext underline.
/// Judged without delimiters still waiting for content ("- `").
fn held(last: &str) -> bool {
    let t = last
        .trim_start()
        .trim_end_matches(['*', '_', '~', '`', '[', '$', ' '])
        .trim_end();
    t.starts_with("```")
        || t.starts_with("~~~")
        || t.starts_with('|')
        || t.starts_with("$$")
        || t.chars().all(|c| "-=*_#>+`~ \t".contains(c))
        || is_bare_marker(t)
        || (t.len() < 10 && t.chars().all(|c| c.is_ascii_digit()))
}

/// "1." or "-" or "##" with nothing after it yet.
fn is_bare_marker(t: &str) -> bool {
    let digits = t.trim_end_matches(['.', ')']);
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) && digits.len() < t.len())
        || t.chars().all(|c| c == '#')
}

fn in_fence(s: &str) -> bool {
    s.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("```") || t.starts_with("~~~")
        })
        .count()
        % 2
        == 1
}

fn in_display_math(s: &str) -> bool {
    s.matches("$$").count() % 2 == 1
}

fn in_table(s: &str) -> bool {
    s.lines().any(|l| {
        let t = l.trim();
        t.starts_with('|') && t.contains("--") && t.chars().all(|c| "|-: ".contains(c))
    })
}

/// Close emphasis, strikethrough, code spans and links that the last
/// paragraph opened. Text whose meaning is still unknown is held back: a
/// delimiter that hasn't got content yet, unclosed math, and a `[` that may
/// or may not become a link.
fn close_inline(s: &mut String) {
    // The last paragraph starts after a blank line or a closed fence.
    let fence_end = s
        .match_indices('\n')
        .map(|(i, _)| i + 1)
        .rfind(|&i| {
            let line = s[..i - 1].rsplit('\n').next().unwrap_or("").trim_start();
            line.starts_with("```") || line.starts_with("~~~")
        })
        .unwrap_or(0);
    let para_start = s
        .rfind("\n\n")
        .map_or(0, |i| i + 2)
        .max(fence_end)
        .min(s.len());
    let base = s
        .trim_end_matches(['*', '_', '~', '`'])
        .len()
        .max(para_start);
    if base < s.len() {
        let run = s[base..].to_string();
        let mut open = scan(&s[para_start..base]);
        let after_word = base > para_start && !s[..base].ends_with(char::is_whitespace);
        // Does the run close what is open ("**bold**")? Otherwise it is an
        // opener with no content yet, or a closer still being typed.
        let mut rest = run.as_str();
        while let Some(&(t, _)) = open.last() {
            match rest.strip_prefix(t) {
                Some(r) => {
                    rest = r;
                    open.pop();
                }
                None => break,
            }
        }
        if !(after_word && rest.is_empty()) {
            s.truncate(base);
        }
    }
    let mut stack = scan(&s[para_start..]);
    // Hold back from the earliest unknown: open math or an unresolved "[".
    if let Some(cut) = stack
        .iter()
        .filter(|t| t.0 == "$" || t.0 == "[")
        .map(|t| t.1)
        .min()
    {
        s.truncate(para_start + cut);
        stack.retain(|t| t.1 < cut);
        // Openers left with nothing after them go too ("**[" → "").
        while let Some(&(t, at)) = stack.last() {
            if s[para_start + at + t.len()..].trim().is_empty() {
                s.truncate(para_start + at);
                stack.pop();
            } else {
                break;
            }
        }
    }
    // A closer can't follow a space.
    if stack.iter().any(|t| !t.0.starts_with('`') && t.0 != "](") {
        let t = s.trim_end().len().max(para_start);
        s.truncate(t);
    }
    for (t, _) in stack.iter().rev() {
        match *t {
            "](" => s.push(')'),
            t => s.push_str(t),
        }
    }
}

/// The inline constructs left open at the end of `para`, with their offsets.
fn scan(para: &str) -> Vec<(&'static str, usize)> {
    let mut stack: Vec<(&'static str, usize)> = Vec::new();
    let mut i = 0;
    while i < para.len() {
        let rest = &para[i..];
        let step = rest.chars().next().map_or(1, char::len_utf8);
        let top = stack.last().map(|t| t.0);
        // A backslash escapes the next character, except inside code.
        if rest.starts_with('\\') && !top.is_some_and(|t| t.starts_with('`')) {
            i += 1 + rest[1..].chars().next().map_or(0, char::len_utf8);
            continue;
        }
        // Code spans and math are opaque until they close; a code span
        // closes only on a backtick run of the same length.
        if let Some(t) = top.filter(|t| t.starts_with('`') || *t == "$") {
            let run = rest.len() - rest.trim_start_matches('`').len();
            if (t == "$" && rest.starts_with('$')) || (run > 0 && run == t.len()) {
                stack.pop();
            }
            i += run.max(step);
            continue;
        }
        let ticks = rest.len() - rest.trim_start_matches('`').len();
        // Runs of three or more emphasis characters are left as text.
        let long = ['*', '_']
            .into_iter()
            .any(|c| rest.len() - rest.trim_start_matches(c).len() >= 3);
        if long {
            i += rest.len() - rest.trim_start_matches(['*', '_']).len();
            continue;
        }
        let tok = if ticks > 0 {
            ["`", "``", "```"].get(ticks - 1).copied().or(Some("```"))
        } else {
            ["**", "__", "~~", "*", "_", "$", "[", "](", "]"]
                .into_iter()
                .find(|t| rest.starts_with(t))
        };
        let len = if ticks > 0 {
            ticks
        } else {
            tok.map_or(step, str::len)
        };
        let prev = para[..i].chars().last();
        let next = rest[len..].chars().next();
        // CommonMark flanking: the end of the text counts as whitespace.
        let ws = |c: Option<char>| c.is_none_or(char::is_whitespace);
        let punct = |c: Option<char>| c.is_some_and(|c| c.is_ascii_punctuation());
        let left = !ws(next) && (!punct(next) || ws(prev) || punct(prev));
        let right = !ws(prev) && (!punct(prev) || ws(next) || punct(next));
        match tok {
            Some("[") => stack.push(("[", i)),
            Some("](") if top == Some("[") => {
                stack.pop();
                stack.push(("](", i));
            }
            // "]" at the very end might still become "](": keep holding.
            Some("]") if top == Some("[") && next.is_some() => {
                stack.pop();
            }
            Some(t @ ("*" | "**" | "~~" | "_" | "__")) => {
                let under = t.starts_with('_');
                let opens = left && (!under || !right || punct(prev));
                let closes = right && (!under || !left || punct(next));
                if top == Some(t) && closes {
                    stack.pop();
                } else if opens {
                    stack.push((t, i));
                }
            }
            Some(t) if t.starts_with('`') => stack.push((t, i)),
            // Same rule as `dollars`: "$10" is a price, not math.
            Some("$") if !ws(next) && !next.is_some_and(|c| c.is_ascii_digit()) => {
                stack.push(("$", i))
            }
            None if rest.starts_with(')') && top == Some("](") => {
                stack.pop();
            }
            _ => {}
        }
        i += len;
    }
    stack
}
