//! Terminal output in the chat: colour codes become styles, other escape
//! sequences go, and a carriage return overwrites its line as a terminal
//! would (progress bars end as their last state). Long output is clipped
//! by lines and by characters, so one huge line can't stall a frame.

use std::ops::Range;

/// A colour as the program asked for it: one of the 16, one of 256, or
/// exact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Colour {
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Sgr {
    pub fg: Option<Colour>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

/// `s` as plain text, with the style of each stretch that has one.
pub fn parse(s: &str) -> (String, Vec<(Range<usize>, Sgr)>) {
    let mut out = String::with_capacity(s.len());
    let mut spans: Vec<(Range<usize>, Sgr)> = Vec::new();
    let mut cur = Sgr::default();
    // Where the line being written starts, for a carriage return.
    let mut line = 0;
    let mut chars = s.char_indices().peekable();
    // Where the plain text not yet copied out starts.
    let mut plain_from = None::<usize>;
    // Copy the plain text before `to` out, in the current style.
    macro_rules! flush {
        ($to:expr) => {
            if let Some(f) = plain_from.take() {
                let text = &s[f..$to];
                let at = out.len();
                out.push_str(text);
                if cur != Sgr::default() {
                    match spans.last_mut() {
                        Some((r, st)) if r.end == at && *st == cur => r.end = out.len(),
                        _ => spans.push((at..out.len(), cur)),
                    }
                }
            }
        };
    }
    while let Some((i, c)) = chars.next() {
        match c {
            '\x1b' => {
                flush!(i);
                match chars.peek().map(|&(_, c)| c) {
                    // CSI: parameters, then a final byte. Something that
                    // can't be in one (a cut-off sequence) ends it, and shows.
                    Some('[') => {
                        chars.next();
                        let mut params = String::new();
                        let mut fin = None;
                        while let Some(&(j, c)) = chars.peek() {
                            if ('\x40'..='\x7e').contains(&c) {
                                chars.next();
                                fin = Some(c);
                                break;
                            }
                            if !('\x20'..='\x3f').contains(&c) {
                                plain_from = Some(j);
                                break;
                            }
                            chars.next();
                            params.push(c);
                        }
                        // A private mode (`ESC[>4;2m`) isn't a style.
                        if fin == Some('m') && !params.starts_with(['<', '=', '>', '?']) {
                            sgr(&params, &mut cur);
                        }
                    }
                    // OSC (titles, links): up to BEL or ST.
                    Some(']') => {
                        chars.next();
                        while let Some((_, c)) = chars.next() {
                            if c == '\x07' {
                                break;
                            }
                            if c == '\x1b' {
                                if chars.peek().map(|&(_, c)| c) == Some('\\') {
                                    chars.next();
                                }
                                break;
                            }
                        }
                    }
                    // Charset choices and the like take one more byte.
                    Some('(' | ')' | '*' | '+') => {
                        chars.next();
                        if chars.peek().is_some_and(|&(_, c)| c != '\n') {
                            chars.next();
                        }
                    }
                    // A stray ESC before a line break leaves the break.
                    Some('\n' | '\r') => {}
                    Some(_) => {
                        chars.next();
                    }
                    None => {}
                }
            }
            // A line end, however many returns come before it (a PTY's
            // `\r\r\n`), is just a line end.
            '\r' if s[i..].trim_start_matches('\r').starts_with('\n') => {
                flush!(i);
            }
            // Back to the start of the line: what follows overwrites it.
            '\r' => {
                flush!(i);
                // Only when something follows on this line.
                if chars.peek().is_some_and(|&(_, c)| c != '\n') {
                    out.truncate(line);
                    spans.retain_mut(|(r, _)| {
                        r.end = r.end.min(line);
                        r.start < r.end
                    });
                }
            }
            '\n' => {
                flush!(i);
                out.push('\n');
                line = out.len();
            }
            // Other control characters (bell, backspace) draw nothing.
            c if c.is_control() && c != '\t' => {
                flush!(i);
            }
            _ => {
                plain_from.get_or_insert(i);
            }
        }
    }
    flush!(s.len());
    (out, spans)
}

/// Plain text and its styles.
pub type Parsed = std::rc::Rc<(String, Vec<(Range<usize>, Sgr)>)>;

thread_local! {
    static PARSED: std::cell::RefCell<crate::lru::Lru<u64, Parsed>> =
        std::cell::RefCell::new(crate::lru::Lru::new(64));
}

/// [`parse`], remembered: an opened call's output isn't parsed every frame.
pub fn parsed(s: &str) -> Parsed {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    let key = h.finish();
    if let Some(p) = PARSED.with(|c| c.borrow_mut().get(&key)) {
        return p;
    }
    let p = std::rc::Rc::new(parse(s));
    PARSED.with(|c| c.borrow_mut().insert(key, p.clone()));
    p
}

/// Apply one SGR sequence's parameters to `st`. Parameters are split by
/// `;`; a parameter's own sub-parameters follow it after `:` (`4:3`, a
/// curly underline; `38:2::255:0:0`, a colour).
fn sgr(params: &str, st: &mut Sgr) {
    let groups: Vec<Vec<u16>> = if params.is_empty() {
        vec![vec![0]]
    } else {
        params
            .split(';')
            .map(|g| g.split(':').map(|p| p.parse().unwrap_or(0)).collect())
            .collect()
    };
    // The flat list, for the `38;5;n` and `38;2;r;g;b` forms.
    let nums: Vec<u16> = groups.iter().map(|g| g[0]).collect();
    let mut i = 0;
    while i < groups.len() {
        let sub = &groups[i][1..];
        match nums[i] {
            0 => *st = Sgr::default(),
            1 => st.bold = true,
            2 => st.dim = true,
            3 => st.italic = true,
            4 => st.underline = sub.first().is_none_or(|&n| n != 0),
            22 => {
                st.bold = false;
                st.dim = false;
            }
            23 => st.italic = false,
            24 => st.underline = false,
            n @ 30..=37 => st.fg = Some(Colour::Indexed((n - 30) as u8)),
            n @ 90..=97 => st.fg = Some(Colour::Indexed((n - 90 + 8) as u8)),
            39 => st.fg = None,
            38 if !sub.is_empty() => st.fg = colour(sub).or(st.fg),
            38 => match nums.get(i + 1) {
                Some(5) => {
                    st.fg = nums.get(i + 2).map(|&n| Colour::Indexed(n.min(255) as u8));
                    i += 2;
                }
                Some(2) => {
                    let c = |k: usize| nums.get(i + k).copied().unwrap_or(0).min(255) as u8;
                    st.fg = Some(Colour::Rgb(c(2), c(3), c(4)));
                    i += 4;
                }
                _ => {}
            },
            // Backgrounds are left to the page.
            48 if !sub.is_empty() => {}
            48 => match nums.get(i + 1) {
                Some(5) => i += 2,
                Some(2) => i += 4,
                _ => {}
            },
            _ => {}
        }
        i += 1;
    }
}

/// A colour from `38`'s sub-parameters: `5:n`, or `2:r:g:b` with or
/// without the colour space before the three (`2::r:g:b`).
fn colour(sub: &[u16]) -> Option<Colour> {
    let c = |n: u16| n.min(255) as u8;
    match sub {
        [5, n, ..] => Some(Colour::Indexed(c(*n))),
        [2, _, r, g, b, ..] => Some(Colour::Rgb(c(*r), c(*g), c(*b))),
        [2, r, g, b] => Some(Colour::Rgb(c(*r), c(*g), c(*b))),
        _ => None,
    }
}

/// Folded, output shows this many lines…
pub const LINES: usize = 60;
/// …each up to this many characters…
pub const LINE_CHARS: usize = 300;
/// …and this many in all. Shown whole, lines and the whole are still held
/// to these, so one enormous output can't stall the chat.
pub const ALL_LINE_CHARS: usize = 20_000;
pub const ALL_CHARS: usize = 200_000;

/// What of `text` shows: the byte ranges kept, in order, each followed by
/// a line break unless it's the last; and how much was left out (lines,
/// characters). `all` is the output opened up in full.
pub fn clip(text: &str, all: bool) -> (Vec<Range<usize>>, usize, usize) {
    let (max_lines, max_line, max_all) = if all {
        (usize::MAX, ALL_LINE_CHARS, ALL_CHARS)
    } else {
        (LINES, LINE_CHARS, LINE_CHARS * 30)
    };
    let text_end = text.trim_end().len();
    let mut kept = Vec::new();
    let (mut lines_left, mut chars_left) = (0usize, 0usize);
    let mut budget = max_all;
    let mut at = 0;
    for (n, line) in text[..text_end].split('\n').enumerate() {
        let start = at;
        at += line.len() + 1;
        let count = line.chars().count();
        if n >= max_lines || budget == 0 {
            lines_left += 1;
            continue;
        }
        let take = count.min(max_line).min(budget);
        let end = line.char_indices().nth(take).map_or(line.len(), |(i, _)| i);
        chars_left += count - take;
        budget -= take;
        kept.push(start..start + end);
    }
    (kept, lines_left, chars_left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_become_styles() {
        let (text, spans) = parse("\x1b[1;31merror\x1b[0m: bad\n\x1b[32mok\x1b[m");
        assert_eq!(text, "error: bad\nok");
        assert_eq!(spans.len(), 2);
        assert_eq!(&text[spans[0].0.clone()], "error");
        assert!(spans[0].1.bold);
        assert_eq!(spans[0].1.fg, Some(Colour::Indexed(1)));
        assert_eq!(&text[spans[1].0.clone()], "ok");
        let (_, spans) = parse("\x1b[38;5;208mx\x1b[38;2;1;2;3my");
        assert_eq!(spans[0].1.fg, Some(Colour::Indexed(208)));
        assert_eq!(spans[1].1.fg, Some(Colour::Rgb(1, 2, 3)));
    }

    #[test]
    fn other_sequences_go_and_returns_overwrite() {
        let (text, _) = parse("\x1b]0;title\x07\x1b[2K\x1b[1Gdone\x1b(B");
        assert_eq!(text, "done");
        let (text, _) = parse("first\n 10%\r 50%\r100%\nnext\r\nlast");
        assert_eq!(text, "first\n100%\nnext\nlast");
        // Never splits a character.
        let (text, _) = parse("héllo\x1b[31mwörld\x1b");
        assert_eq!(text, "héllowörld");
    }

    #[test]
    fn long_output_clips_by_lines_and_characters() {
        let huge = "x".repeat(1_000_000);
        let (kept, lines, chars) = clip(&huge, false);
        assert_eq!((kept.len(), kept[0].clone()), (1, 0..LINE_CHARS));
        assert_eq!((lines, chars), (0, 1_000_000 - LINE_CHARS));
        let (kept, _, chars) = clip(&huge, true);
        assert_eq!((kept.len(), kept[0].clone()), (1, 0..ALL_LINE_CHARS));
        assert!(chars > 0);
        let many: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let (kept, lines, chars) = clip(&many, false);
        assert_eq!((kept.len(), lines, chars), (LINES, 40, 0));
        let (kept, lines, _) = clip(&many, true);
        assert_eq!((kept.len(), lines), (100, 0));
        let (kept, _, _) = clip("é".repeat(400).as_str(), false);
        assert_eq!(kept[0].end, LINE_CHARS * 2);
    }

    #[test]
    fn edge_cases_from_real_output() {
        // A PTY's \r\r\n is one line end, not an overwrite.
        assert_eq!(parse("hello\r\r\nnext").0, "hello\nnext");
        // A stray ESC before a line break doesn't join the lines.
        assert_eq!(parse("a\x1b\nb").0, "a\nb");
        // A sequence cut off by something that can't be in one ends there:
        // what cut it shows.
        assert_eq!(parse("x\x1b[3…").0, "x…");
        // Colon sub-parameters belong to their parameter: a curly
        // underline isn't italic, and colon truecolour keeps its colour.
        let (_, spans) = parse("\x1b[4:3mx");
        assert!(spans[0].1.underline && !spans[0].1.italic);
        let (_, spans) = parse("\x1b[4:0mx\x1b[1my");
        assert!(!spans[0].1.underline);
        let (_, spans) = parse("\x1b[38:2::255:0:0mx");
        assert_eq!(spans[0].1.fg, Some(Colour::Rgb(255, 0, 0)));
        let (_, spans) = parse("\x1b[38:2:1:2:3mx");
        assert_eq!(spans[0].1.fg, Some(Colour::Rgb(1, 2, 3)));
        let (_, spans) = parse("\x1b[38:5:208mx");
        assert_eq!(spans[0].1.fg, Some(Colour::Indexed(208)));
        // A private-mode `m` (`ESC[>4;2m`, keyboard modes) isn't a style.
        let (text, spans) = parse("\x1b[>4;2mx");
        assert_eq!((text.as_str(), spans.len()), ("x", 0));
    }
}
