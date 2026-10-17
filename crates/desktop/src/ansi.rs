//! Terminal output in the chat: colour codes become styles (read by the
//! terminal tab's own parser, and coloured as it colours them), other escape
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

/// `s` as plain text, with the style of each stretch that has one. The
/// escapes are read by the same parser the terminal tab uses (vte, through
/// alacritty_terminal): only colours and text styles are kept.
pub fn parse(s: &str) -> (String, Vec<(Range<usize>, Sgr)>) {
    use alacritty_terminal::vte;
    let mut p = Plain {
        out: String::with_capacity(s.len()),
        spans: Vec::new(),
        cur: Sgr::default(),
        line: 0,
        cr: false,
    };
    vte::Parser::new().advance(&mut p, s.as_bytes());
    (p.out, p.spans)
}

/// What [`parse`] builds as the parser reads.
struct Plain {
    out: String,
    spans: Vec<(Range<usize>, Sgr)>,
    cur: Sgr,
    /// Where the line being written starts, for a carriage return.
    line: usize,
    /// A carriage return came: what's printed next overwrites the line
    /// (a line end first makes it just a line end, as a PTY's `\r\r\n`).
    cr: bool,
}

impl Plain {
    fn push(&mut self, c: char) {
        if std::mem::take(&mut self.cr) {
            let line = self.line;
            self.out.truncate(line);
            self.spans.retain_mut(|(r, _)| {
                r.end = r.end.min(line);
                r.start < r.end
            });
        }
        let at = self.out.len();
        self.out.push(c);
        if self.cur != Sgr::default() {
            match self.spans.last_mut() {
                Some((r, st)) if r.end == at && *st == self.cur => r.end = self.out.len(),
                _ => self.spans.push((at..self.out.len(), self.cur)),
            }
        }
    }
}

impl alacritty_terminal::vte::Perform for Plain {
    fn print(&mut self, c: char) {
        self.push(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.cr = false;
                self.out.push('\n');
                self.line = self.out.len();
            }
            // Back to the start of the line: what follows overwrites it.
            b'\r' => self.cr = true,
            b'\t' => self.push('\t'),
            // Other control characters (bell, backspace) draw nothing.
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &alacritty_terminal::vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        // A private mode (`ESC[>4;2m`, keyboard modes) isn't a style.
        if action == 'm' && intermediates.is_empty() && !ignore {
            let groups: Vec<Vec<u16>> = params.iter().map(<[u16]>::to_vec).collect();
            sgr(&groups, &mut self.cur);
        }
    }
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

/// Apply one SGR sequence's parameters to `st`: each parameter with its
/// own sub-parameters, those after `:` (`4:3`, a curly underline;
/// `38:2::255:0:0`, a colour).
fn sgr(groups: &[Vec<u16>], st: &mut Sgr) {
    let reset = [vec![0]];
    let groups: &[Vec<u16>] = if groups.is_empty() { &reset } else { groups };
    // The flat list, for the `38;5;n` and `38;2;r;g;b` forms.
    let nums: Vec<u16> = groups
        .iter()
        .map(|g| g.first().copied().unwrap_or(0))
        .collect();
    let mut i = 0;
    while i < groups.len() {
        let sub = groups[i].get(1..).unwrap_or(&[]);
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
/// …and this many in all. Shown whole, a line is still held to
/// `ALL_LINE_CHARS` (cut with `…`; Copy still takes it whole), so one
/// enormous line can't stall the chat. `ALL_CHARS` is a guard only: the
/// daemon keeps at most 64 KB of any output.
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
        // A stray ESC before a line break doesn't join the lines (what
        // the ESC takes as its final byte goes, as in the terminal tab).
        assert_eq!(parse("a\x1b\nb\nc").0, "a\n\nc");
        // A sequence cut off by something that can't be in one is dropped
        // as a terminal drops it, and what follows shows.
        assert_eq!(parse("x\x1b[3…\x1b[0mok").0, "xok");
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
