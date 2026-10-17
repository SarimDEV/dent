//! Terminal escape sequences in text the daemon keeps or reads: one
//! scanner, so taking them out (a terminal session's first line) and
//! cutting long output between them agree on where each starts and ends.
//! (The desktop reads them with the terminal's own parser.)

use std::ops::Range;

/// Where each escape sequence in `text` is, in order: CSI (`ESC [`, its
/// parameters and intermediates, then a final byte), OSC and other
/// strings (`ESC ]`, `ESC P`, `ESC X`, `ESC ^`, `ESC _`, up to BEL or
/// `ESC \`), and the rest (`ESC`, intermediates, a final byte). A sequence
/// that's cut off ends where the text does, or at a byte that can't be in
/// it.
pub fn find(text: &str) -> Vec<Range<usize>> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != 0x1b {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        match b.get(i) {
            Some(b'[') => {
                i += 1;
                while i < b.len() && (0x20..=0x3f).contains(&b[i]) {
                    i += 1;
                }
                if i < b.len() && (0x40..=0x7e).contains(&b[i]) {
                    i += 1;
                }
            }
            Some(b']' | b'P' | b'X' | b'^' | b'_') => {
                i += 1;
                while i < b.len() {
                    if b[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if b[i] == 0x1b {
                        i += usize::from(b.get(i + 1) == Some(&b'\\')) + 1;
                        break;
                    }
                    i += 1;
                }
            }
            Some(_) => {
                while i < b.len() && (0x20..=0x2f).contains(&b[i]) {
                    i += 1;
                }
                if i < b.len() && (0x30..=0x7e).contains(&b[i]) {
                    i += 1;
                }
            }
            None => {}
        }
        out.push(start..i);
    }
    out
}

/// `text` without its escape sequences.
pub fn strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for r in find(text) {
        out.push_str(&text[at..r.start]);
        at = r.end;
    }
    out.push_str(&text[at..]);
    out
}

/// The escape sequence `at` falls strictly inside, if any.
pub fn around(text: &str, at: usize) -> Option<Range<usize>> {
    find(text).into_iter().find(|r| r.start < at && at < r.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_found_whole() {
        let t = "a\x1b[1;31mb\x1b]0;title\x07c\x1b]8;;x\x1b\\d\x1b(Be\x1b[>4;2mf";
        assert_eq!(strip(t), "abcdef");
        // An OSC title isn't cut short at its first letter.
        assert_eq!(strip("\x1b]0;my title\x07x"), "x");
        // Cut off at the end: it ends with the text.
        assert_eq!(strip("x\x1b[3"), "x");
        // Inside a colour code, and not.
        let t = "ab\x1b[38;2;1;2;3mcd";
        assert_eq!(around(t, 5), Some(2..15));
        assert_eq!(around(t, 2), None);
        assert_eq!(around(t, 15), None);
    }
}
