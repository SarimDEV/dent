//! Sessions started with plain `claude` in a terminal, from Claude Code's
//! own store (`~/.claude/projects/*/<session>.jsonl`), so dent can list them
//! and carry one on with its history. Only the transcripts are read.

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use dent_protocol::{Body, Outcome, RecentSession};
use serde_json::Value;

use crate::claude::{self, wire};

fn store() -> PathBuf {
    crate::paths::home().join(".claude/projects")
}

/// How recently a transcript must have changed for its session to count as
/// still open in a terminal, when Claude Code keeps no record of it running.
const OPEN_MS: u64 = 60_000;

/// Sessions a Claude Code process is running now. Each running `claude`
/// keeps `~/.claude/sessions/<pid>.json` naming its session; a record whose
/// process is gone is stale.
fn running() -> HashSet<String> {
    running_in(&crate::paths::home().join(".claude/sessions"))
}

fn running_in(dir: &Path) -> HashSet<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|f| f.path().extension().is_some_and(|e| e == "json"))
        .filter_map(|f| {
            let v: Value = serde_json::from_slice(&std::fs::read(f.path()).ok()?).ok()?;
            let pid = v.get("pid")?.as_i64()?;
            alive(pid).then(|| v.get("sessionId")?.as_str().map(str::to_string))?
        })
        .collect()
}

#[cfg(unix)]
fn alive(pid: i64) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // Signal 0 only asks whether the process exists; EPERM means it does.
    pid > 0
        && (unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

#[cfg(not(unix))]
fn alive(pid: i64) -> bool {
    pid > 0
}

/// Recent interactive sessions, newest first by the last thing said in
/// them, leaving out `owned` (dent's own). One still being written is
/// listed, marked open.
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
    // Claude Code also writes bookkeeping into quiet transcripts, so the
    // file's time only narrows the search; the last message orders it.
    files.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    let now = crate::daemon::now_ms();
    let live = running();
    let mut out = Vec::new();
    for (path, changed) in files {
        if out.len() >= limit * 3 {
            break;
        }
        let Some(id) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        if owned.contains(&id) {
            continue;
        }
        if let Some(mut r) = summary(&path, &id, changed) {
            r.open = live.contains(&id) || now.saturating_sub(changed) < OPEN_MS;
            out.push(r);
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.updated));
    out.truncate(limit);
    out
}

/// Whether a terminal session looks open right now.
pub fn is_open(agent_session: &str) -> bool {
    running().contains(agent_session)
        || find(agent_session)
            .and_then(|p| p.metadata().ok()?.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_millis() < u128::from(OPEN_MS))
}

/// Unix milliseconds from a transcript timestamp like
/// `2026-10-13T23:56:30.647Z`.
fn timestamp_ms(s: &str) -> Option<u64> {
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let ms = s
        .get(19..)
        .and_then(|r| r.strip_prefix('.'))
        .map(|r| {
            r.chars()
                .take_while(char::is_ascii_digit)
                .take(3)
                .collect::<String>()
        })
        .and_then(|r| format!("{r:0<3}").parse::<i64>().ok())
        .unwrap_or(0);
    // Days from 1970-01-01 to the date, in the proleptic Gregorian calendar.
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let total = ((days * 24 + h) * 60 + mi) * 60 + sec;
    u64::try_from(total * 1000 + ms).ok()
}

/// The first prompt, the last one, and where it ran: from the head of the
/// file and its tail, so a long session costs no more than a short one.
fn summary(path: &Path, id: &str, updated: u64) -> Option<RecentSession> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let (mut cwd, mut title, mut cli) = (None, None, false);
    let (mut ai_title, mut custom_title): (Option<String>, Option<String>) = (None, None);
    for line in BufReader::new(&f).lines().map_while(Result::ok).take(400) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        cli |= v["entrypoint"] == "cli";
        if let Some(t) = v["aiTitle"].as_str() {
            ai_title = Some(t.to_string());
        }
        if let Some(t) = v["customTitle"].as_str() {
            custom_title = Some(t.to_string());
        }
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
    let mut said_at = None;
    for line in tail.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(t) = v["aiTitle"].as_str() {
            ai_title = Some(t.to_string());
        }
        if let Some(t) = v["customTitle"].as_str() {
            custom_title = Some(t.to_string());
        }
        // When something was last said, by either side.
        if matches!(v["type"].as_str(), Some("user" | "assistant"))
            && v["isMeta"] != true
            && let Some(at) = v["timestamp"].as_str().and_then(timestamp_ms)
        {
            said_at = Some(at);
        }
        if let Some(p) = prompt(&v) {
            last = Some(p);
        } else if v["type"] == "last-prompt"
            && let Some(p) = v["lastPrompt"].as_str()
        {
            last = Some(p.to_string());
        }
    }
    let first = title?;
    let last = last.map(|l| one_line(&l)).unwrap_or_default();
    // Named the way Claude Code's own resume list names it; failing that,
    // by the latest thing said, which is what's easiest to recognise.
    let title = custom_title
        .or(ai_title)
        .map(|t| one_line(&t))
        .unwrap_or_else(|| {
            if last.is_empty() {
                one_line(&first)
            } else {
                last.clone()
            }
        });
    Some(RecentSession {
        agent_session: id.to_string(),
        cwd,
        title,
        last,
        updated: said_at.unwrap_or(updated),
        open: false,
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

/// A terminal session's history, ready to become a dent session.
pub struct Imported {
    pub cwd: String,
    pub title: String,
    pub bodies: Vec<Body>,
}

/// Find a terminal session's transcript.
pub fn find(agent_session: &str) -> Option<PathBuf> {
    std::fs::read_dir(store())
        .ok()?
        .flatten()
        .map(|d| d.path().join(format!("{agent_session}.jsonl")))
        .find(|p| p.exists())
}

/// A transcript as dent events, with where it ran and its title. Pictures
/// the person pasted are saved under `media`, so they show as attachments.
pub fn import(path: &Path, media: &Path) -> Option<Imported> {
    let f = std::fs::File::open(path).ok()?;
    let (mut cwd, mut first, mut ai_title, mut custom_title) = (None, None, None, None);
    let mut bodies = Vec::new();
    // Each prompt opens a turn; the next one, or the end, closes it.
    let mut open = false;
    let close = |bodies: &mut Vec<Body>, open: &mut bool, outcome: Outcome| {
        if std::mem::take(open) {
            bodies.push(Body::TurnEnded { outcome });
        }
    };
    let mut pictures = 0;
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if cwd.is_none() {
            cwd = v["cwd"].as_str().map(str::to_owned);
        }
        match v["type"].as_str().unwrap_or_default() {
            "ai-title" => ai_title = v["aiTitle"].as_str().map(str::to_owned),
            "custom-title" => custom_title = v["customTitle"].as_str().map(str::to_owned),
            _ => {}
        }
        if v["isSidechain"] == true || v["isMeta"] == true {
            continue;
        }
        if v["type"] == "system" && v["subtype"] == "compact_boundary" {
            let m = &v["compactMetadata"];
            let m = if m.is_object() {
                m
            } else {
                &v["compact_metadata"]
            };
            bodies.push(Body::Compacted {
                auto: m["trigger"] == "auto",
                before: m["preTokens"]
                    .as_u64()
                    .or(m["pre_tokens"].as_u64())
                    .unwrap_or(0),
                after: m["postTokens"].as_u64().or(m["post_tokens"].as_u64()),
            });
            continue;
        }
        if v["type"] == "user" {
            if v["isCompactSummary"] == true {
                continue;
            }
            let said = said(&v, media, &mut pictures);
            if said.stopped {
                close(
                    &mut bodies,
                    &mut open,
                    Outcome::Interrupted { reason: None },
                );
            }
            if let Some(out) = said.output {
                bodies.push(Body::Notice { text: out });
            }
            if let Some((text, attachments)) = said.prompt {
                // Claude Code writes the summary before the /compact that
                // asked for it; dent shows them the way they happened.
                let compacted = matches!(bodies.last(), Some(Body::Compacted { .. }));
                if text.starts_with("/compact") && compacted {
                    let marker = bodies.pop();
                    close(&mut bodies, &mut open, Outcome::Done);
                    bodies.push(Body::User {
                        id: v["uuid"].as_str().unwrap_or_default().to_string(),
                        text,
                        queued: false,
                        steer: false,
                        attachments,
                    });
                    bodies.extend(marker);
                    continue;
                }
                close(&mut bodies, &mut open, Outcome::Done);
                first.get_or_insert_with(|| one_line(&text));
                bodies.push(Body::User {
                    id: v["uuid"].as_str().unwrap_or_default().to_string(),
                    text,
                    queued: false,
                    steer: false,
                    attachments,
                });
                bodies.push(Body::TurnStarted);
                open = true;
            }
        }
        match wire::parse(line.as_bytes()) {
            // A usage limit or API error, written by Claude Code: the turn
            // ended there with an error.
            Ok(wire::Inbound::Assistant {
                synthetic: true,
                blocks,
                ..
            }) => {
                let message = blocks
                    .into_iter()
                    .filter_map(|b| match b {
                        wire::AssistantBlock::Text(t) => Some(t),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                close(&mut bodies, &mut open, Outcome::Error { message });
            }
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
    close(&mut bodies, &mut open, Outcome::Done);
    let title = custom_title
        .or(ai_title)
        .or(first)
        .unwrap_or_else(|| "Terminal session".into());
    Some(Imported {
        cwd: cwd?,
        title: one_line(&title),
        bodies,
    })
}

/// What a person's line in a transcript says.
struct Said {
    /// What a local command (`/model`, `/login`) printed, in one line.
    output: Option<String>,
    /// Something they typed, and the pictures with it.
    prompt: Option<(String, Vec<String>)>,
    /// They stopped the agent here.
    stopped: bool,
}

fn said(v: &Value, media: &Path, pictures: &mut usize) -> Said {
    let mut out = Said {
        output: None,
        prompt: None,
        stopped: false,
    };
    let (mut texts, mut attachments) = (Vec::new(), Vec::new());
    match &v["message"]["content"] {
        Value::String(t) => texts.push(t.clone()),
        Value::Array(parts) => {
            for p in parts {
                match p["type"].as_str() {
                    Some("text") => texts.push(s(p, "text")),
                    Some("image") => {
                        if let Some(path) = save_picture(&p["source"], media, *pictures) {
                            *pictures += 1;
                            attachments.push(path);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    let mut kept = Vec::new();
    for t in texts {
        let t = t.trim();
        if t.starts_with("[Request interrupted") {
            out.stopped = true;
        } else if let Some(said) = between(t, "<local-command-stdout>", "</local-command-stdout>") {
            let line = strip_ansi(said.lines().find(|l| !l.trim().is_empty()).unwrap_or(""));
            // Compaction already has its own line.
            if !line.is_empty() && !line.starts_with("Compacted") {
                out.output = Some(line);
            }
        } else if let Some(t) = typed(t) {
            kept.push(t);
        }
    }
    if !kept.is_empty() || !attachments.is_empty() {
        out.prompt = Some((kept.join("\n\n"), attachments));
    }
    out
}

/// The part of a user line the person typed: plain text, a slash command,
/// a `!` shell command or a paste. Tooling (notifications, command output,
/// reminders) is left out.
fn typed(t: &str) -> Option<String> {
    if t.is_empty() {
        return None;
    }
    if !t.starts_with('<') {
        return Some(without_image_marks(t));
    }
    if let Some(name) = between(t, "<command-name>", "</command-name>") {
        let args = between(t, "<command-args>", "</command-args>").unwrap_or_default();
        let name = if name.starts_with('/') {
            name.to_string()
        } else {
            format!("/{name}")
        };
        return Some(format!("{name} {args}").trim().to_string());
    }
    if let Some(cmd) = between(t, "<bash-input>", "</bash-input>") {
        return Some(format!("! {cmd}"));
    }
    if t.starts_with("<pasted_content") {
        // Keep the person's words and the paste, without the wrapper.
        let mut out = String::new();
        for l in t.lines() {
            let tl = l.trim();
            if tl.starts_with("<pasted_content") || tl.starts_with("</pasted_content") {
                continue;
            }
            out.push_str(l);
            out.push('\n');
        }
        let out = out.trim().to_string();
        return (!out.is_empty()).then_some(out);
    }
    None
}

/// Text without the `[Image #2]` marks Claude Code leaves where a picture
/// was pasted; the pictures themselves come in as attachments.
fn without_image_marks(t: &str) -> String {
    let mut out = String::new();
    let mut rest = t;
    while let Some(at) = rest.find("[Image #") {
        let tail = &rest[at + 8..];
        let digits = tail.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && tail[digits..].starts_with(']') {
            out.push_str(&rest[..at]);
            rest = &tail[digits + 1..];
        } else {
            out.push_str(&rest[..at + 8]);
            rest = tail;
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Text without terminal colour codes.
fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ ... letter
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

fn between<'a>(t: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = t.find(open)? + open.len();
    let b = t[a..].find(close)? + a;
    Some(t[a..b].trim())
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

fn save_picture(source: &Value, media: &Path, n: usize) -> Option<String> {
    let data = source["data"].as_str()?;
    let ext = match source["media_type"].as_str()? {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => return None,
    };
    let bytes = dent_protocol::b64::decode(data)?;
    std::fs::create_dir_all(media).ok()?;
    let path = media.join(format!("picture-{}.{ext}", n + 1));
    std::fs::write(&path, bytes).ok()?;
    Some(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dent_protocol::Block;

    #[test]
    fn a_session_counts_as_running_while_its_process_lives() {
        let dir = std::env::temp_dir().join(format!("dent-running-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let me = std::process::id();
        std::fs::write(
            dir.join(format!("{me}.json")),
            format!(r#"{{"pid":{me},"sessionId":"live-one"}}"#),
        )
        .unwrap();
        // A pid far past any real one: its record is stale.
        std::fs::write(
            dir.join("99999999.json"),
            r#"{"pid":99999999,"sessionId":"gone"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("junk.json"), "not json").unwrap();
        let live = running_in(&dir);
        assert!(live.contains("live-one"));
        assert!(!live.contains("gone"));
        assert_eq!(live.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Made-up lines in the shapes Claude Code writes.
    const TRANSCRIPT: &str = r#"{"type":"user","cwd":"/tmp/p","uuid":"u1","message":{"role":"user","content":"Fix the footer"}}
{"type":"assistant","uuid":"a1","message":{"model":"m","content":[{"type":"text","text":"On it."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}
{"type":"user","uuid":"r1","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a.txt"}]}}
{"type":"user","uuid":"s1","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}
{"type":"user","uuid":"u2","message":{"content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}},{"type":"text","text":"Like this [Image #1]"}]}}
{"type":"assistant","uuid":"x1","isApiErrorMessage":true,"message":{"model":"<synthetic>","content":[{"type":"text","text":"You've hit your limit"}]}}
{"type":"system","subtype":"compact_boundary","uuid":"c1"}
{"type":"user","isCompactSummary":true,"uuid":"c2","message":{"content":"This session is being continued"}}
{"type":"user","isMeta":true,"uuid":"c3","message":{"content":"<local-command-caveat>x</local-command-caveat>"}}
{"type":"user","uuid":"c4","message":{"content":"<command-name>/compact</command-name>\n<command-args></command-args>"}}
{"type":"user","uuid":"c5","message":{"content":"<local-command-stdout>Compacted (ctrl+o to see full summary)</local-command-stdout>"}}
{"type":"user","uuid":"m1","message":{"content":"<command-name>/model</command-name>"}}
{"type":"user","uuid":"m2","message":{"content":"<local-command-stdout>Set model to \u001b[1mOpus\u001b[22m</local-command-stdout>"}}
{"type":"user","uuid":"n1","message":{"content":"<task-notification>done</task-notification>"}}
{"type":"assistant","uuid":"side","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent"}]}}
{"type":"ai-title","aiTitle":"Footer fixes"}
"#;

    #[test]
    fn transcript_times_read_as_unix_milliseconds() {
        assert_eq!(timestamp_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            timestamp_ms("2026-10-13T23:56:30.647Z"),
            Some(1_791_935_790_647)
        );
        assert_eq!(timestamp_ms("2000-02-29T12:00:00Z"), Some(951_825_600_000));
        assert_eq!(timestamp_ms("nope"), None);
    }

    #[test]
    fn a_transcript_imports_as_it_happened() {
        let dir = std::env::temp_dir().join(format!("dent-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        std::fs::write(&path, TRANSCRIPT).unwrap();
        let got = import(&path, &dir.join("media")).unwrap();
        assert_eq!(got.cwd, "/tmp/p");
        assert_eq!(got.title, "Footer fixes");
        let said: Vec<String> = got
            .bodies
            .iter()
            .map(|b| match b {
                Body::User {
                    text, attachments, ..
                } => format!("you: {text} +{}", attachments.len()),
                Body::Assistant { blocks, .. } => format!(
                    "agent: {}",
                    blocks
                        .iter()
                        .map(|b| match b {
                            Block::Text { text } => text.clone(),
                            Block::Tool { name, .. } => format!("[{name}]"),
                            _ => "?".into(),
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                Body::ToolResult { output, .. } => format!("result: {output}"),
                Body::TurnStarted => "start".into(),
                Body::TurnEnded { outcome } => format!("end: {outcome:?}"),
                Body::Notice { text } => format!("note: {text}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            said,
            [
                "you: Fix the footer +0",
                "start",
                "agent: On it. [Bash]",
                "result: a.txt",
                "end: Interrupted { reason: None }",
                "you: Like this +1",
                "start",
                "end: Error { message: \"You've hit your limit\" }",
                "you: /compact +0",
                "Compacted { auto: false, before: 0, after: None }",
                "you: /model +0",
                "start",
                "note: Set model to Opus",
                "end: Done",
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
