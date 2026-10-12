//! Assistant messages from local agent transcripts. Read in place, never copied
//! or committed: the spike only reports counts and timings from them.

use std::path::{Path, PathBuf};

use serde_json::Value;

pub struct Message {
    pub agent: &'static str,
    pub text: String,
}

/// Every assistant text message in `~/.claude/projects` and `~/.codex/sessions`.
pub fn load() -> Vec<Message> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let mut out = Vec::new();
    for file in jsonl_files(&home.join(".claude/projects")) {
        read_lines(&file, |v| {
            if v["type"] == "assistant"
                && let Some(parts) = v["message"]["content"].as_array()
            {
                for p in parts {
                    if p["type"] == "text"
                        && let Some(t) = p["text"].as_str()
                    {
                        push(&mut out, "claude", t);
                    }
                }
            }
        });
    }
    for file in jsonl_files(&home.join(".codex/sessions")) {
        read_lines(&file, |v| {
            let p = &v["payload"];
            if v["type"] == "response_item"
                && p["type"] == "message"
                && p["role"] == "assistant"
                && let Some(parts) = p["content"].as_array()
            {
                for c in parts {
                    if c["type"] == "output_text"
                        && let Some(t) = c["text"].as_str()
                    {
                        push(&mut out, "codex", t);
                    }
                }
            }
        });
    }
    out
}

fn push(out: &mut Vec<Message>, agent: &'static str, text: &str) {
    if !text.trim().is_empty() {
        out.push(Message {
            agent,
            text: text.to_string(),
        });
    }
}

fn read_lines(path: &Path, mut f: impl FnMut(&Value)) {
    let Ok(data) = std::fs::read_to_string(path) else {
        return;
    };
    for line in data.lines() {
        // Cheap prefilter: most lines are tool output.
        if !line.contains("\"assistant\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            f(&v);
        }
    }
}

fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "jsonl") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}
