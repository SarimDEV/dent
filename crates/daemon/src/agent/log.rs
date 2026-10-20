//! What agent output becomes in dent's log, the same for every agent: tool
//! output cut to its start and end, pictures saved beside the session, a
//! subagent's report kept longer, and the thought on the working line.

use dent_protocol::{Block, Body, Section, Task, runs_subagent};
use serde_json::Value;

use super::{AssistantBlock, BackgroundTask, ToolResult};

/// Tool output kept in the log: its start and its end, where a build or a
/// test run says how it went. The middle stays in the agent's transcript.
const HEAD: usize = 8 * 1024;
const TAIL: usize = 24 * 1024;
/// A subagent's report is its whole answer, in Markdown: kept far longer.
const REPORT_HEAD: usize = 56 * 1024;
const REPORT_TAIL: usize = 8 * 1024;

/// The last finished sentence of thinking still streaming (or its last
/// line), at most 140 characters, for the working line. None until one
/// has finished.
pub fn latest_sentence(thinking: &str) -> Option<String> {
    // Where sentences end: a full stop, ! or ? with a space after it (so
    // "file.rs" and "e.g." mid-word don't count), or a line's end.
    let b = thinking.as_bytes();
    let ends: Vec<usize> = (0..b.len())
        .filter(|&i| {
            b[i] == b'\n'
                || matches!(b[i], b'.' | b'!' | b'?')
                    && b.get(i + 1).is_some_and(u8::is_ascii_whitespace)
        })
        .collect();
    let end = *ends.last()?;
    let start = ends.iter().rev().nth(1).map_or(0, |i| i + 1);
    let s = thinking[start..=end].trim();
    if s.chars().count() < 4 {
        return None;
    }
    let mut out: String = s.chars().take(140).collect();
    if s.chars().count() > 140 {
        out.push('…');
    }
    Some(out)
}

#[cfg(test)]
#[test]
fn the_working_line_shows_the_last_finished_thought() {
    assert_eq!(latest_sentence("Looking at main.rs now"), None);
    assert_eq!(
        latest_sentence("Looking at main.rs now. The parser drops a toke"),
        Some("Looking at main.rs now.".into())
    );
    assert_eq!(
        latest_sentence("First. Then the second one! And"),
        Some("Then the second one!".into())
    );
}

/// Calls that run a subagent and whose report hasn't come yet: a report,
/// being the subagent's whole answer, keeps more of itself (`keep`).
#[derive(Default)]
pub struct Reports(std::collections::HashSet<String>);

impl Reports {
    /// Note the subagent calls in an assistant message as logged (live,
    /// imported, or read back from the log after a restart).
    pub fn note(&mut self, blocks: &[Block]) {
        for b in blocks {
            if let Block::Tool { id, name, .. } = b
                && runs_subagent(name)
            {
                self.0.insert(id.clone());
            }
        }
    }

    /// Whether call `id`'s result is a report; it's answered now.
    pub fn answered(&mut self, id: &str) -> bool {
        self.0.remove(id)
    }
}

/// What an assistant message becomes in the log: its blocks, then the
/// results of server tools that came with it. The same for a live session
/// and one picked up from the terminal.
pub fn assistant_bodies(
    parent: Option<String>,
    uuid: Option<String>,
    blocks: Vec<AssistantBlock>,
    results: Vec<ToolResult>,
    reports: &mut Reports,
) -> Vec<Body> {
    let blocks = to_blocks(blocks);
    reports.note(&blocks);
    let mut out = Vec::new();
    if !blocks.is_empty() {
        out.push(Body::Assistant {
            parent: parent.clone(),
            blocks,
            uuid,
        });
    }
    out.extend(result_bodies(parent, results, reports, None));
    out
}

/// What tool results become in the log, a subagent's report keeping more.
/// Pictures in a result are saved under `media` (the session's
/// attachments), so the chat can show them; without it, or if one can't be
/// saved, the output says a picture was there.
pub fn result_bodies(
    parent: Option<String>,
    results: Vec<ToolResult>,
    reports: &mut Reports,
    media: Option<&std::path::Path>,
) -> Vec<Body> {
    results
        .into_iter()
        .map(|r| {
            let report = reports.answered(&r.tool_use_id);
            let mut output = r.output;
            let mut images = Vec::new();
            for (n, source) in r.images.iter().enumerate() {
                let name = format!("tool-{}-{}", safe_name(&r.tool_use_id), n + 1);
                match media.and_then(|m| save_picture(source, m, &name)) {
                    Some(path) => images.push(path),
                    None => {
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        output.push_str("[image]");
                    }
                }
            }
            Body::ToolResult {
                tool: r.tool_use_id,
                error: r.is_error,
                output: keep(output, report),
                parent: parent.clone(),
                images,
            }
        })
        .collect()
}

/// Letters, digits, `-` and `_` only: an id made safe for a file name.
fn safe_name(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// A base64 picture (an image block's `source`) written to `media` as
/// `name` and its kind's extension. Its path, or None if it isn't a kind
/// that shows or can't be written.
pub fn save_picture(source: &Value, media: &std::path::Path, name: &str) -> Option<String> {
    let data = source["data"].as_str()?;
    let bytes = dent_protocol::b64::decode(data)?;
    // By what the bytes are, not what they're labelled: an empty or broken
    // picture would show as a blank frame, and a mislabelled one wouldn't
    // open in Preview.
    let ext = match picture_kind(&bytes)? {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        _ => "webp",
    };
    std::fs::create_dir_all(media).ok()?;
    let path = media.join(format!("{name}.{ext}"));
    std::fs::write(&path, bytes).ok()?;
    Some(path.display().to_string())
}

pub fn to_blocks(blocks: Vec<AssistantBlock>) -> Vec<Block> {
    blocks
        .into_iter()
        // Some models send empty thinking blocks; there's nothing to keep.
        .filter(|b| !matches!(b, AssistantBlock::Thinking(t) if t.trim().is_empty()))
        .map(|b| match b {
            AssistantBlock::Text(text) => Block::Text { text },
            AssistantBlock::Thinking(text) => Block::Thinking { text },
            AssistantBlock::ToolUse { id, name, input } => Block::Tool {
                id,
                name,
                input: input.to_string(),
            },
        })
        .collect()
}

/// The agent's background tasks as dent's, each with the call that started
/// it when `calls` knows it.
pub fn tasks(
    tasks: &[BackgroundTask],
    calls: &std::collections::HashMap<String, String>,
) -> Vec<Task> {
    tasks
        .iter()
        .map(|t| Task {
            id: t.task_id.clone(),
            description: t.description.clone(),
            kind: t.task_type.clone(),
            call: calls.get(&t.task_id).cloned(),
        })
        .collect()
}

/// What of a tool's output goes in the log. Long output keeps its start
/// and its end, cut at line breaks where it can (so never inside a colour
/// code), with a line between them saying how much was left out. A
/// subagent's report (`report`) keeps far more.
pub fn keep(text: String, report: bool) -> String {
    let (head, tail) = if report {
        (REPORT_HEAD, REPORT_TAIL)
    } else {
        (HEAD, TAIL)
    };
    if text.len() <= head + tail {
        return text;
    }
    let head_end = cut_back(&text, head);
    let tail_start = cut_forward(&text, text.len() - tail);
    let gone = &text[head_end..tail_start];
    let lines = gone.matches('\n').count();
    let what = if lines > 0 {
        format!("{} line{}", group(lines), if lines == 1 { "" } else { "s" })
    } else {
        format!("{} characters", group(gone.chars().count()))
    };
    // A colour left on at the cut doesn't run into the line.
    let reset = if text.contains('\x1b') { "\x1b[0m" } else { "" };
    let mut out = String::with_capacity(head + tail + 64);
    out.push_str(text[..head_end].trim_end_matches('\n'));
    out.push_str(&format!("{reset}\n… {what} not kept …\n"));
    out.push_str(text[tail_start..].trim_start_matches('\n'));
    out
}

/// Where the kept start ends: after the last line break before `at` if
/// one is near, else at a character boundary outside any escape sequence
/// (before the one it falls in).
fn cut_back(text: &str, at: usize) -> usize {
    let mut end = at;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if let Some(nl) = text[..end].rfind('\n')
        && end - nl < 2048
    {
        return nl + 1;
    }
    crate::escapes::around(text, end).map_or(end, |r| r.start)
}

/// Where the kept end starts: after the first line break from `at` if one
/// is near, else at the next character boundary outside any escape
/// sequence (after the one it falls in), so the tail never starts inside
/// a colour code.
fn cut_forward(text: &str, at: usize) -> usize {
    let mut start = at;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    match text[start..].find('\n') {
        Some(nl) if nl < 2048 => start + nl + 1,
        _ => crate::escapes::around(text, start).map_or(start, |r| r.end),
    }
}

/// 18402 as "18,402".
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A picture's media type from its first bytes, for the kinds the model reads.
pub fn picture_kind(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// A sheet in plain words, for clients that don't draw one.
pub fn sheet_text(sections: &[Section]) -> String {
    let mut out = String::new();
    for s in sections {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&s.title);
        out.push('\n');
        if !s.note.is_empty() {
            out.push_str(&format!("  {}\n", s.note));
        }
        for r in &s.rows {
            match r.value.is_empty() {
                true => out.push_str(&format!("  {}\n", r.label)),
                false => out.push_str(&format!("  {}: {}\n", r.label, r.value)),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_output_keeps_its_start_and_end() {
        // Short output is kept whole.
        assert_eq!(keep("ok\n".into(), false), "ok\n");
        // Numbered lines: the first and the last are kept, the middle is
        // counted.
        let text: String = (1..=20000).map(|i| format!("{i}\n")).collect();
        let out = keep(text.clone(), false);
        assert!(out.len() <= HEAD + TAIL + 64);
        assert!(out.starts_with("1\n2\n"));
        assert!(out.ends_with("19999\n20000\n"));
        let marker = out.lines().find(|l| l.contains("not kept")).unwrap();
        let kept = out.lines().filter(|l| !l.contains("not kept")).count();
        let n: usize = marker
            .trim_matches(['…', ' '])
            .split(' ')
            .next()
            .unwrap()
            .replace(',', "")
            .parse()
            .unwrap();
        assert_eq!(kept + n, 20000, "{marker}");
        // Every kept line is whole.
        for l in out.lines().filter(|l| !l.contains("not kept")) {
            assert!(l.parse::<u32>().is_ok(), "{l:?}");
        }
        // A report keeps much more.
        let report = "word ".repeat(10_000);
        assert_eq!(keep(report.clone(), true), report);
    }

    #[test]
    fn a_cut_never_splits_a_character_or_an_escape() {
        // One long line: no line break to cut at.
        let long = "é".repeat(HEAD + TAIL);
        let out = keep(long, false);
        assert!(out.contains("characters not kept"));
        // Colour codes all the way: the cut lands before an escape, never
        // inside one, and colour is reset before the marker line.
        let coloured = "\x1b[31mred\x1b[0m ".repeat(HEAD);
        let out = keep(coloured, false);
        let (head, _) = out.split_once("\x1b[0m\n…").unwrap();
        assert!(
            head.ends_with(' ') || head.ends_with('m'),
            "{:?}",
            &head[head.len() - 8..]
        );
        let parsed_ok = |s: &str| !s.contains("\x1b[3\n") && !s.contains("\x1b\n");
        assert!(parsed_ok(&out));
        // The tail side too: it starts at an escape or between them, never
        // inside one, whatever the offset the cut first lands on.
        for pad in 0..12 {
            let coloured = format!(
                "{}{}",
                "x".repeat(pad),
                "\x1b[38;2;255;0;0mred ".repeat(HEAD)
            );
            let out = keep(coloured, false);
            let (_, tail) = out.split_once("not kept …\n").unwrap();
            assert!(
                tail.starts_with('\x1b') || tail.starts_with("red") || tail.starts_with(' '),
                "{pad}: {:?}",
                &tail[..12]
            );
        }
    }

    #[test]
    fn group_puts_in_thousands_separators() {
        assert_eq!(group(5), "5");
        assert_eq!(group(1234), "1,234");
        assert_eq!(group(18402), "18,402");
        assert_eq!(group(1_000_000), "1,000,000");
    }
}
