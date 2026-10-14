//! Syntax highlighting for code blocks, through arborium's tree-sitter
//! grammars ([decision 0016]). Each block is highlighted once and cached;
//! colours come from the theme, so it stays within the Field Notes palette.
//!
//! [decision 0016]: ../../docs/decisions/0016-highlighting-arborium.md

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::*;

use crate::theme::Theme;

/// The kinds of token that get their own look.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tag {
    Keyword,
    String,
    Comment,
    Function,
    Type,
    Number,
}

impl Tag {
    fn of(capture: &str) -> Option<Tag> {
        let head = capture.split('.').next().unwrap_or(capture);
        Some(match head {
            "keyword" | "conditional" | "repeat" | "include" | "exception" | "storageclass" => {
                Tag::Keyword
            }
            "string" | "character" | "escape" => Tag::String,
            "comment" => Tag::Comment,
            "function" | "method" | "constructor" | "macro" => Tag::Function,
            "type" | "tag" | "attribute" => Tag::Type,
            "number" | "float" | "boolean" | "constant" => Tag::Number,
            _ => return None,
        })
    }

    pub fn color(self, t: &Theme) -> Hsla {
        // The ink, part way toward a status hue: present, never loud.
        let toward = |h: Hsla, k: f32| crate::theme::mix_pub(t.ink, h, k);
        match self {
            Tag::Keyword => t.ink,
            Tag::String => toward(t.done.0, 0.45),
            Tag::Comment => t.quiet,
            Tag::Function => toward(t.working.0, 0.5),
            Tag::Type => toward(t.waiting.0, 0.5),
            // Never toward "needs you": that hue is for what needs you.
            Tag::Number => toward(t.idle.0, 0.55),
        }
    }
}

/// The grammar for a fence's language name, if there is one.
fn grammar(lang: &str) -> Option<&'static str> {
    Some(match lang.trim().to_ascii_lowercase().as_str() {
        "rust" | "rs" => "rust",
        "python" | "py" => "python",
        "javascript" | "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "typescript" | "ts" => "typescript",
        "tsx" => "tsx",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "bash" | "sh" | "shell" | "zsh" | "console" => "bash",
        "html" | "xml" | "svg" => "html",
        "css" | "scss" => "css",
        "go" | "golang" => "go",
        "swift" => "swift",
        "sql" => "sql",
        "c" | "h" => "c",
        "cpp" | "c++" | "cc" | "hpp" => "cpp",
        "java" => "java",
        "ruby" | "rb" => "ruby",
        "diff" | "patch" => "diff",
        "markdown" | "md" => "markdown",
        _ => return None,
    })
}

type Spans = Rc<Vec<(Range<usize>, Tag)>>;

thread_local! {
    static HL: RefCell<Option<arborium::Highlighter>> = const { RefCell::new(None) };
    static CACHE: RefCell<HashMap<u64, Spans>> = RefCell::new(HashMap::new());
    static LINES: RefCell<HashMap<u64, Lines>> = RefCell::new(HashMap::new());
}

/// Each line's runs of (length, tag), for a whole block.
pub type Lines = Rc<Vec<Vec<(usize, Option<Tag>)>>>;

/// A block's lines in runs, worked out in one pass and remembered: a long
/// block isn't rescanned for every line, every frame.
pub fn block_lines(lang: &str, code: &str) -> Lines {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (lang, code).hash(&mut h);
    let key = h.finish();
    if let Some(l) = LINES.with(|c| c.borrow().get(&key).cloned()) {
        return l;
    }
    let spans = spans(lang, code);
    let mut tags: Vec<Option<Tag>> = vec![None; code.len()];
    for (r, tag) in spans.iter() {
        for t in &mut tags[r.start.min(code.len())..r.end.min(code.len())] {
            *t = Some(*tag);
        }
    }
    let mut out = Vec::new();
    let mut at = 0;
    for line in code.split('\n') {
        let mut runs: Vec<(usize, Option<Tag>)> = Vec::new();
        for (ix, ch) in line.char_indices() {
            let (tag, w) = (tags[at + ix], ch.len_utf8());
            match runs.last_mut() {
                Some((len, t)) if *t == tag => *len += w,
                _ => runs.push((w, tag)),
            }
        }
        out.push(runs);
        at += line.len() + 1;
    }
    let out = Rc::new(out);
    LINES.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 256 {
            c.clear();
        }
        c.insert(key, out.clone());
    });
    out
}

/// Token ranges for `code` in `lang`, innermost last. Empty for languages
/// without a grammar.
pub fn spans(lang: &str, code: &str) -> Spans {
    let Some(g) = grammar(lang) else {
        return Rc::default();
    };
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (g, code).hash(&mut h);
    let key = h.finish();
    if let Some(s) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return s;
    }
    let found = HL.with(|hl| {
        let mut hl = hl.borrow_mut();
        let hl = hl.get_or_insert_with(arborium::Highlighter::new);
        hl.highlight_spans(g, code).unwrap_or_default()
    });
    let mut out: Vec<(Range<usize>, Tag)> = found
        .into_iter()
        .filter_map(|s| {
            let tag = Tag::of(&s.capture)?;
            let r = s.start as usize..(s.end as usize).min(code.len());
            (r.start < r.end).then_some((r, tag))
        })
        .collect();
    // Outer spans first, so inner ones paint over them.
    out.sort_by_key(|(r, _)| (r.start, std::cmp::Reverse(r.end)));
    let out = Rc::new(out);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 512 {
            c.clear();
        }
        c.insert(key, out.clone());
    });
    out
}

/// The tag for each byte of `line` (which starts at `at` in the block).
pub fn line_runs(
    spans: &[(Range<usize>, Tag)],
    at: usize,
    line: &str,
) -> Vec<(usize, Option<Tag>)> {
    let mut tags: Vec<Option<Tag>> = vec![None; line.len()];
    for (r, tag) in spans {
        if r.end <= at || r.start >= at + line.len() {
            continue;
        }
        let s = r.start.saturating_sub(at);
        let e = (r.end - at).min(line.len());
        for t in &mut tags[s..e] {
            *t = Some(*tag);
        }
    }
    // Merge into runs, never splitting a character.
    let mut runs: Vec<(usize, Option<Tag>)> = Vec::new();
    for (ix, ch) in line.char_indices() {
        let (tag, w) = (tags[ix], ch.len_utf8());
        match runs.last_mut() {
            Some((len, t)) if *t == tag => *len += w,
            _ => runs.push((w, tag)),
        }
    }
    runs
}

/// A line's runs in its tokens' colours.
pub fn runs(spans: &[(Range<usize>, Tag)], line: &str, t: &Theme) -> Vec<TextRun> {
    line_runs(spans, 0, line)
        .into_iter()
        .map(|(len, tag)| {
            let mut f = font(crate::theme::mono());
            // The code face comes in regular and bold only.
            if tag == Some(Tag::Keyword) {
                f.weight = FontWeight::BOLD;
            }
            if tag == Some(Tag::Comment) {
                f.style = FontStyle::Italic;
            }
            TextRun {
                len,
                font: f,
                color: tag.map(|g| g.color(t)).unwrap_or(t.ink),
                background_color: None,
                underline: None,
                strikethrough: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn rust_gets_keywords_and_strings() {
        let code = "fn main() {\n    let s = \"hi\"; // greet\n}";
        let sp = super::spans("rs", code);
        let has = |tag, text: &str| {
            sp.iter()
                .any(|(r, t)| *t == tag && &code[r.clone()] == text)
        };
        assert!(has(super::Tag::Keyword, "fn"), "{sp:?}");
        assert!(has(super::Tag::String, "\"hi\""), "{sp:?}");
        assert!(sp.iter().any(|(_, t)| *t == super::Tag::Comment));
        assert!(super::spans("brainfudge", code).is_empty());
    }
}
