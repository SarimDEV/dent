//! From the client core's timelines to what the window draws.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use dent_client_core::{Call, Entry, Note, SessionView};
use dent_protocol::{Answer, Model, Status as Wire};
use serde_json::Value;

use crate::data::{Artifact, FileDiff, Item, Kind, Question, Session, Tone, intern};
use crate::theme::Status;

/// Everything outside the timeline a session row needs.
pub struct Context<'a> {
    pub machine: &'a str,
    pub models: &'a [Model],
    pub diff: Option<Rc<Vec<FileDiff>>>,
    /// The previous build, to keep what the person opened or folded.
    pub prev: Option<&'a Session>,
    pub now_ms: u64,
}

pub fn status(s: Wire) -> Status {
    match s {
        Wire::New | Wire::Working => Status::Working,
        Wire::Background => Status::Waiting,
        Wire::NeedsYou | Wire::Error => Status::Needs,
        Wire::Done => Status::Done,
        Wire::Interrupted => Status::Idle,
    }
}

pub fn model_name(models: &[Model], id: Option<&str>) -> String {
    let id = id.unwrap_or("default");
    models
        .iter()
        .find(|m| m.id == id)
        .map(|m| m.name.trim_end_matches(" (recommended)").to_string())
        .unwrap_or_else(|| id.to_string())
}

/// A session's artifacts: every viewable file it wrote, each version.
pub fn artifacts(v: &SessionView) -> Vec<Artifact> {
    let cwd = Path::new(&v.cwd);
    v.made
        .iter()
        .filter_map(|m| {
            let kind = Kind::of(&m.path)?;
            let rel = relative(cwd, &m.path);
            let abs = cwd.join(&rel);
            let latest = m.versions.len();
            let versions = m
                .versions
                .iter()
                .enumerate()
                .map(|(i, ver)| match &ver.content {
                    Some(c) => Rc::from(c.as_bytes()),
                    // An edit to a file we never saw whole: the latest is on
                    // disk on this machine.
                    None if i + 1 == latest => std::fs::read(&abs)
                        .map(Rc::from)
                        .unwrap_or_else(|_| Rc::from(&b""[..])),
                    None => Rc::from(&b""[..]),
                })
                .collect();
            let title = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            Some(Artifact {
                id: art_id(&v.id, &m.path),
                kind,
                title: intern(&title),
                path: intern(&rel),
                dir: Some(cwd.to_path_buf()),
                versions,
            })
        })
        .collect()
}

pub fn art_id(session: &str, path: &str) -> &'static str {
    intern(&format!("{session}:{path}"))
}

/// A path as the person would say it: relative to the session's folder
/// when it's inside it.
fn relative(cwd: &Path, path: &str) -> String {
    let p = PathBuf::from(path);
    if let Ok(r) = p.strip_prefix(cwd) {
        return r.display().to_string();
    }
    // The agent may name the real path where the session's folder is a
    // link to it (on macOS, /var is /private/var).
    if let Ok(real) = cwd.canonicalize()
        && let Ok(r) = p.strip_prefix(&real)
    {
        return r.display().to_string();
    }
    match cwd.strip_prefix("/private") {
        Ok(alias) if alias != cwd => match p.strip_prefix(Path::new("/").join(alias)) {
            Ok(r) => r.display().to_string(),
            Err(_) => path.to_string(),
        },
        _ => path.to_string(),
    }
}

pub fn session(v: &SessionView, cx: Context<'_>) -> Session {
    let cwd = Path::new(&v.cwd);
    let open_asks: Vec<(String, String)> = v
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::Ask {
                tool,
                input,
                answer: None,
                ..
            } => Some((tool.clone(), input.clone())),
            _ => None,
        })
        .collect();
    let mut items: Vec<Item> = Vec::with_capacity(v.entries.len() + 2);
    let mut queued: Vec<Item> = Vec::new();
    let mut arts: Vec<&'static str> = Vec::new();
    let mut terminal: Vec<(Tone, String)> = Vec::new();
    for e in &v.entries {
        match e {
            Entry::User {
                id,
                text,
                queued: q,
                attachments,
                ..
            } => {
                if *q {
                    queued.push(Item::Queued {
                        id: id.clone(),
                        text: text.clone(),
                    });
                } else if attachments.is_empty() {
                    items.push(Item::User(text.clone()));
                } else {
                    let mut lines: Vec<String> = attachments
                        .iter()
                        .map(|p| format!("› Attached · {}", relative(cwd, p)))
                        .collect();
                    if !text.is_empty() {
                        lines.push(text.clone());
                    }
                    items.push(Item::User(lines.join("\n")));
                }
            }
            Entry::Text { text } => items.push(Item::Text(text.clone())),
            Entry::Thinking { .. } => {}
            Entry::Tools { calls } => {
                for c in calls
                    .iter()
                    .filter(|c| c.name == "Bash" && c.result.is_some())
                {
                    let cmd = c.input()["command"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    terminal.push((Tone::Command, format!("$ {cmd}")));
                    if let Some((err, out)) = &c.result {
                        let tone = if *err { Tone::Quiet } else { Tone::Good };
                        for l in out.lines().take(40) {
                            terminal.push((tone, l.to_string()));
                        }
                    }
                }
                // The plan belongs with the composer, and questions and plans
                // have their own cards.
                let calls: Vec<Call> = calls
                    .iter()
                    .filter(|c| {
                        !matches!(
                            c.name.as_str(),
                            "TodoWrite" | "AskUserQuestion" | "ExitPlanMode"
                        )
                    })
                    .map(|c| {
                        let mut c = c.clone();
                        if c.result.is_none()
                            && open_asks.contains(&(c.name.clone(), c.input.clone()))
                        {
                            // Not running yet: it's waiting for you.
                            c.name = format!("waiting:{}", c.name);
                        }
                        c
                    })
                    // While the card asks, the card says what it is.
                    .filter(|c| !c.name.starts_with("waiting:"))
                    .collect();
                if calls.is_empty() {
                    continue;
                }
                // Writes that became a card need no line of their own.
                let carded = calls.iter().all(|c| {
                    matches!(c.name.as_str(), "Write" | "Edit" | "MultiEdit")
                        && matches!(c.result, Some((false, _)))
                        && c.path().is_some_and(|p| Kind::of(&p).is_some())
                });
                if carded {
                    continue;
                }
                let calls = &calls;
                let open = match cx.prev.and_then(|p| p.items.get(items.len())) {
                    Some(Item::Tools { open, .. }) => *open,
                    _ => false,
                };
                items.push(Item::Tools {
                    summary: summary(calls, cwd),
                    lines: calls.iter().map(|c| line(c, cwd)).collect(),
                    open,
                });
            }
            Entry::Ask {
                id,
                tool,
                input,
                description,
                answer,
            } => {
                let input: Value = serde_json::from_str(input).unwrap_or(Value::Null);
                match answer {
                    None => items.push(ask(id, tool, &input, description.as_deref(), cwd)),
                    // The answer reads before what it let happen.
                    Some(a) => {
                        let note = answered(tool, &input, a, cwd);
                        match items.last() {
                            Some(Item::Tools { .. }) => items.insert(items.len() - 1, note),
                            _ => items.push(note),
                        }
                    }
                }
            }
            Entry::Made { path, version } => {
                let art = art_id(&v.id, path);
                if !arts.contains(&art) {
                    arts.push(art);
                }
                items.push(Item::Made {
                    art,
                    version: *version,
                });
            }
            Entry::Note { note, text } => items.push(Item::Note {
                icon: match note {
                    Note::Stopped => "square",
                    Note::Error => "circle-alert",
                    Note::Info => "info",
                },
                text: text.clone(),
            }),
        }
    }
    if !v.partial.is_empty() {
        items.push(Item::Text(v.partial.clone()));
    }
    items.extend(queued);
    let tail = terminal.len().saturating_sub(400);
    terminal.drain(..tail);
    let status = status(v.status);
    let working_since = v.working_since.map(|ms| {
        Instant::now()
            .checked_sub(Duration::from_millis(cx.now_ms.saturating_sub(ms)))
            .unwrap_or_else(Instant::now)
    });
    Session {
        id: v.id.clone(),
        title: v.title.clone(),
        project: cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| v.cwd.clone()),
        machine: cx.machine.to_string(),
        agent: format!("Claude · {}", model_name(cx.models, v.model.as_deref())),
        model: v.model.clone(),
        permission: v.permission,
        status,
        detail: detail(v),
        updated: v.updated,
        working_since,
        closed: v.closed,
        stopped: v.status == Wire::Interrupted,
        items,
        arts,
        diff: cx.diff.unwrap_or_default(),
        terminal,
        context: v.context,
    }
}

/// What it's doing or wants, for the inbox row and the working line.
fn detail(v: &SessionView) -> String {
    match v.status {
        Wire::NeedsYou => match v.open_ask() {
            Some(Entry::Ask { tool, .. }) => match tool.as_str() {
                "AskUserQuestion" => "Asked you a question".into(),
                "ExitPlanMode" => "Plan ready for review".into(),
                "Bash" => "Wants to run a command".into(),
                "Write" | "Edit" | "MultiEdit" => "Wants to edit a file".into(),
                t => format!("Wants to use {t}"),
            },
            _ => "Needs you".into(),
        },
        Wire::Working | Wire::New => {
            if !v.partial.is_empty() {
                return "Writing".into();
            }
            match v.entries.last() {
                Some(Entry::Tools { calls }) => calls
                    .iter()
                    .rev()
                    .find(|c| c.parent.is_none())
                    .map(|c| c.line())
                    .unwrap_or_else(|| "Working".into()),
                _ => "Thinking".into(),
            }
        }
        Wire::Background => format!(
            "Waiting on {}",
            v.background
                .first()
                .map(|t| t.description.clone())
                .unwrap_or_else(|| "background work".into())
        ),
        Wire::Interrupted => "Stopped".into(),
        Wire::Error => v
            .entries
            .iter()
            .rev()
            .find_map(|e| match e {
                Entry::Note {
                    note: Note::Error,
                    text,
                } => Some(first_line(text)),
                _ => None,
            })
            .unwrap_or_else(|| "Something went wrong".into()),
        Wire::Done => v
            .entries
            .iter()
            .rev()
            .find_map(|e| match e {
                Entry::Text { text } => Some(first_line(text)),
                _ => None,
            })
            .unwrap_or_else(|| "Done".into()),
    }
}

/// The first line of Markdown as plain words, for one-line previews.
fn first_line(s: &str) -> String {
    let l = s
        .lines()
        .map(|l| l.trim_start_matches(['#', ' ', '*', '-', '>']).trim())
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .replace("**", "")
        .replace('`', "");
    let l = l.as_str();
    let mut out: String = l.chars().take(90).collect();
    if l.chars().count() > 90 {
        out.push('…');
    }
    out
}

/// One line per call; a subagent's calls sit under the call that started it.
fn line(c: &Call, cwd: &Path) -> String {
    if let Some(name) = c.name.strip_prefix("waiting:") {
        let real = Call {
            name: name.to_string(),
            ..c.clone()
        };
        let what = line(&real, cwd);
        let rest = what.split_once(' ').map(|x| x.1).unwrap_or(&what);
        return format!("Waiting for you: {rest}");
    }
    let mut l = c.line();
    if let Some(p) = c.path() {
        let rel = relative(cwd, &p);
        let short = p.rsplit('/').next().unwrap_or(&p).to_string();
        l = l.replacen(&short, &rel, 1);
    }
    let mark = match &c.result {
        Some((true, out)) if c.name != "Bash" => format!(" · failed: {}", first_line(out)),
        Some((true, out)) => format!(": {}", first_line(out)),
        _ => String::new(),
    };
    let indent = if c.parent.is_some() { "  " } else { "" };
    format!("{indent}{l}{mark}")
}

/// A group's folded line, like "Read 3 files, changed 2 files, and ran a
/// command": the two biggest kinds of work, the rest counted together.
fn summary(calls: &[Call], cwd: &Path) -> String {
    let top: Vec<&Call> = calls.iter().filter(|c| c.parent.is_none()).collect();
    if top.len() == 1 {
        return line(top[0], cwd).trim_start().to_string();
    }
    // (priority, label, count): edits and commands first.
    let mut kinds: Vec<(u8, &str, usize)> = Vec::new();
    let mut changed: Vec<String> = Vec::new();
    for c in &top {
        let kind = match c.name.trim_start_matches("waiting:") {
            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
                let p = c.path().unwrap_or_default();
                if changed.contains(&p) {
                    continue;
                }
                changed.push(p);
                (0, "changed")
            }
            "Bash" => (0, "ran"),
            "Read" => (1, "read"),
            "Grep" | "Glob" | "LS" => (1, "searched code"),
            "WebFetch" | "WebSearch" => (1, "searched the web"),
            "Task" | "Agent" => (1, "subagents"),
            _ => (2, "used"),
        };
        match kinds.iter_mut().find(|k| k.1 == kind.1) {
            Some(k) => k.2 += 1,
            None => kinds.push((kind.0, kind.1, 1)),
        }
    }
    kinds.sort_by_key(|k| k.0);
    let phrase = |(_, label, n): &(u8, &str, usize)| -> String {
        let n = *n;
        let s = |w: &str| {
            if n == 1 {
                w.to_string()
            } else {
                format!("{w}s")
            }
        };
        match *label {
            "changed" => format!("changed {n} {}", s("file")),
            "ran" => format!("ran {n} {}", s("command")),
            "read" => format!("read {n} {}", s("file")),
            "subagents" => format!("ran {n} {}", s("subagent")),
            "used" => format!("used {n} {}", s("tool")),
            l if n == 1 => l.to_string(),
            l => format!("{l} {n} times"),
        }
    };
    let mut parts: Vec<String> = kinds.iter().take(2).map(phrase).collect();
    let rest: usize = kinds.iter().skip(2).map(|k| k.2).sum();
    if rest > 0 {
        parts.push(format!(
            "performed {rest} other {}",
            if rest == 1 { "action" } else { "actions" }
        ));
    }
    let mut out = match parts.len() {
        0 => "Worked".to_string(),
        1 => parts.remove(0),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => {
            let last = parts.pop().unwrap_or_default();
            format!("{}, and {last}", parts.join(", "))
        }
    };
    if let Some(f) = out.get(..1) {
        out = f.to_uppercase() + &out[1..];
    }
    let failed = top
        .iter()
        .filter(|c| matches!(c.result, Some((true, _))))
        .count();
    if failed > 0 {
        out.push_str(&format!(" · {failed} failed"));
    }
    out
}

fn ask(id: &str, tool: &str, input: &Value, description: Option<&str>, cwd: &Path) -> Item {
    match tool {
        "AskUserQuestion" => Item::Question {
            id: id.into(),
            questions: input["questions"]
                .as_array()
                .map(|qs| {
                    qs.iter()
                        .map(|q| Question {
                            text: q["question"].as_str().unwrap_or_default().into(),
                            options: q["options"]
                                .as_array()
                                .map(|o| {
                                    o.iter()
                                        .map(|o| {
                                            (
                                                o["label"].as_str().unwrap_or_default().into(),
                                                o["description"]
                                                    .as_str()
                                                    .unwrap_or_default()
                                                    .into(),
                                            )
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        "ExitPlanMode" => Item::Plan {
            id: id.into(),
            plan: input["plan"].as_str().unwrap_or_default().into(),
        },
        _ => {
            let (title, command) = what(tool, input, cwd);
            Item::Approval {
                id: id.into(),
                title,
                command,
                why: description.unwrap_or_default().into(),
            }
        }
    }
}

/// How an approval reads: a title, and the command or file in question.
fn what(tool: &str, input: &Value, cwd: &Path) -> (String, String) {
    let path = || {
        input["file_path"]
            .as_str()
            .map(|p| relative(cwd, p))
            .unwrap_or_default()
    };
    match tool {
        "Bash" => (
            "Wants to run a command".into(),
            format!("$ {}", input["command"].as_str().unwrap_or_default()),
        ),
        "Write" => ("Wants to create a file".into(), path()),
        "Edit" | "MultiEdit" | "NotebookEdit" => ("Wants to edit a file".into(), path()),
        "WebFetch" => (
            "Wants to open a page".into(),
            input["url"].as_str().unwrap_or_default().into(),
        ),
        "WebSearch" => (
            "Wants to search the web".into(),
            input["query"].as_str().unwrap_or_default().into(),
        ),
        t => {
            let name = t
                .strip_prefix("mcp__")
                .map(|n| n.replace("__", " › "))
                .unwrap_or_else(|| t.to_string());
            (format!("Wants to use {name}"), compact(input))
        }
    }
}

fn compact(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 300 {
        format!("{}…", s.chars().take(300).collect::<String>())
    } else {
        s
    }
}

fn answered(tool: &str, input: &Value, a: &Answer, cwd: &Path) -> Item {
    let short = match tool {
        "AskUserQuestion" => String::new(),
        "ExitPlanMode" => "the plan".into(),
        _ => {
            let (_, c) = what(tool, input, cwd);
            c.trim_start_matches("$ ")
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect()
        }
    };
    let (icon, text) = match a {
        Answer::Allow { always: false } if tool == "ExitPlanMode" => {
            ("check", "Approved the plan".to_string())
        }
        Answer::Allow { always: false } => ("check", format!("Allowed {short}")),
        Answer::Allow { always: true } => ("check", format!("Allowed {short} for this session")),
        Answer::Deny { message } if tool == "ExitPlanMode" => {
            ("corner-down-right", format!("Kept planning: {message}"))
        }
        Answer::Deny { message } if message.is_empty() => ("x", format!("Denied {short}")),
        Answer::Deny { message } => ("x", format!("Denied {short}: {message}")),
        Answer::Choose { answers } => (
            "corner-down-right",
            format!("You chose {}", answers.join(", ")),
        ),
        Answer::Expired => ("clock", "No longer needed".to_string()),
    };
    Item::Note { icon, text }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_read_relative_through_links() {
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let dir = tmp.join(format!("dent-rel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("site/a.html").display().to_string();
        assert_eq!(relative(&dir, &file), "site/a.html");
        // Named through /private when the folder was given without it.
        if let Ok(short) = dir.strip_prefix("/private") {
            let short = Path::new("/").join(short);
            assert_eq!(relative(&short, &file), "site/a.html");
        }
        assert_eq!(relative(&dir, "/elsewhere/x"), "/elsewhere/x");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
