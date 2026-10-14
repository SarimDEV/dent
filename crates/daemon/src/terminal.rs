//! Sessions started with plain `claude` in a terminal, from Claude Code's
//! own store (`~/.claude/projects/*/<session>.jsonl`), so dent can list them
//! and carry one on with its history. Only the transcripts are read.

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use dent_protocol::{Body, RecentSession};
use serde_json::Value;

use crate::claude::{self, wire};

fn store() -> PathBuf {
    crate::paths::home().join(".claude/projects")
}

/// Recent interactive sessions, newest first, leaving out `owned` (dent's
/// own) and any touched in the last minute (likely still open).
pub fn recent(owned: &HashSet<String>, limit: usize) -> Vec<RecentSession> {
    let mut files: Vec<(PathBuf, u64)> = std::fs::read_dir(store())
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|d| std::fs::read_dir(d.path()).into_iter().flatten().flatten())
        .filter(|f| f.path().extension().is_some_and(|e| e == "jsonl"))
        .filter_map(|f| {
            let t = f.metadata().ok()?.modified().ok()?;
            let ms = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
            Some((f.path(), ms))
        })
        .collect();
    files.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    let now = crate::daemon::now_ms();
    let mut out = Vec::new();
    for (path, updated) in files {
        if out.len() >= limit {
            break;
        }
        if now.saturating_sub(updated) < 60_000 {
            continue;
        }
        let Some(id) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        if owned.contains(&id) {
            continue;
        }
        if let Some(r) = summary(&path, &id, updated) {
            out.push(r);
        }
    }
    out
}

/// The first prompt, the last one, and where it ran: from the head of the
/// file and its tail, so a long session costs no more than a short one.
fn summary(path: &Path, id: &str, updated: u64) -> Option<RecentSession> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let (mut cwd, mut title, mut cli) = (None, None, false);
    for line in BufReader::new(&f).lines().map_while(Result::ok).take(400) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        cli |= v["entrypoint"] == "cli";
        if cwd.is_none() {
            cwd = v["cwd"].as_str().map(str::to_owned);
        }
        if title.is_none() {
            title = prompt(&v);
        }
        if cli && cwd.is_some() && title.is_some() {
            break;
        }
    }
    let cwd = cwd?;
    // Only sessions a person ran in a terminal, in folders that still exist.
    if !cli || !Path::new(&cwd).is_dir() {
        return None;
    }
    // The latest prompt is near the end.
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(128 * 1024)))
        .ok()?;
    let mut tail = String::new();
    let _ = f.take(128 * 1024).read_to_string(&mut tail);
    let mut last = None;
    for line in tail.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(p) = prompt(&v) {
            last = Some(p);
        } else if v["type"] == "last-prompt"
            && let Some(p) = v["lastPrompt"].as_str()
        {
            last = Some(p.to_string());
        }
    }
    Some(RecentSession {
        agent_session: id.to_string(),
        cwd,
        title: one_line(&title?),
        last: last.map(|l| one_line(&l)).unwrap_or_default(),
        updated,
    })
}

/// A prompt the person typed: user text that isn't tooling.
fn prompt(v: &Value) -> Option<String> {
    if v["type"] != "user" || v["isMeta"] == true || v["isSidechain"] == true {
        return None;
    }
    let text = v["message"]["content"].as_str()?;
    let t = text.trim();
    (!t.is_empty() && !t.starts_with('<')).then(|| t.to_string())
}

fn one_line(s: &str) -> String {
    let l = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim();
    let mut out: String = l.chars().take(90).collect();
    if l.chars().count() > 90 {
        out.push('…');
    }
    out
}

/// A terminal session's history as dent events, with where it ran and its
/// first prompt.
pub fn import(agent_session: &str) -> Option<(String, String, Vec<Body>)> {
    let path = std::fs::read_dir(store())
        .ok()?
        .flatten()
        .map(|d| d.path().join(format!("{agent_session}.jsonl")))
        .find(|p| p.exists())?;
    let f = std::fs::File::open(path).ok()?;
    let (mut cwd, mut title) = (None, None);
    let mut bodies = Vec::new();
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if cwd.is_none() {
            cwd = v["cwd"].as_str().map(str::to_owned);
        }
        if v["isSidechain"] == true {
            continue;
        }
        if let Some(p) = prompt(&v) {
            title.get_or_insert_with(|| one_line(&p));
            bodies.push(Body::User {
                id: v["uuid"].as_str().unwrap_or_default().to_string(),
                text: p,
                queued: false,
                steer: false,
                attachments: Vec::new(),
            });
            continue;
        }
        match wire::parse(line.as_bytes()) {
            Ok(wire::Inbound::Assistant { uuid, blocks, .. }) => {
                let blocks = claude::blocks(blocks);
                if !blocks.is_empty() {
                    bodies.push(Body::Assistant {
                        parent: None,
                        blocks,
                        uuid: uuid.or_else(|| v["uuid"].as_str().map(str::to_owned)),
                    });
                }
            }
            Ok(wire::Inbound::ToolResults { results, .. }) => {
                for r in results {
                    bodies.push(Body::ToolResult {
                        tool: r.tool_use_id,
                        error: r.is_error,
                        output: r.output,
                        parent: None,
                    });
                }
            }
            _ => {}
        }
    }
    Some((
        cwd?,
        title.unwrap_or_else(|| "Terminal session".into()),
        bodies,
    ))
}
