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
        .map(|n| {
            if n == "Default" {
                "Default model".to_string()
            } else {
                n
            }
        })
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

/// What the bridge built for a session last time, so the next build only
/// redoes the entries that changed: usually just the reply streaming in.
#[derive(Default)]
pub struct Built {
    items: Rc<Vec<Item>>,
    /// The item index each entry's items start at.
    starts: Vec<usize>,
    /// Items that come from entries; the streaming reply and waiting
    /// messages follow them.
    body: usize,
    /// Dev servers found so far, with the entry each first appeared in.
    servers: Vec<(usize, String)>,
}

/// Turn a timeline into what the window draws. `from` is the first entry
/// that changed since `built` was made (`None`: no entry changed).
pub fn session(
    v: &SessionView,
    cx: Context<'_>,
    built: &mut Built,
    from: Option<usize>,
) -> Session {
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
    // Back off two entries: an answer's note slots in before the entry
    // ahead of it.
    let start = from
        .unwrap_or(v.entries.len())
        .min(built.starts.len())
        .saturating_sub(2);
    let items = Rc::make_mut(&mut built.items);
    let keep = built.starts.get(start).copied().unwrap_or(built.body);
    items.truncate(keep);
    built.starts.truncate(start);
    built.servers.retain(|(at, _)| *at < start);
    for (k, e) in v.entries[start..].iter().enumerate() {
        built.starts.push(items.len());
        // A dev server it started gets a card where it first shows up.
        for url in servers_in(e) {
            if !built.servers.iter().any(|(_, u)| *u == url) {
                built.servers.push((start + k, url.clone()));
                items.push(Item::Made {
                    art: art_id(&v.id, &url),
                    version: 1,
                });
            }
        }
        match e {
            Entry::User {
                id,
                text,
                queued: q,
                attachments,
                ..
            } => {
                if *q {
                    // Waiting messages go last, below everything.
                } else if attachments.is_empty() {
                    items.push(Item::User {
                        id: id.clone(),
                        text: text.clone(),
                    });
                } else {
                    let mut lines: Vec<String> = attachments
                        .iter()
                        .map(|p| format!("› Attached · {}", relative(cwd, p)))
                        .collect();
                    if !text.is_empty() {
                        lines.push(text.clone());
                    }
                    items.push(Item::User {
                        id: id.clone(),
                        text: lines.join("\n"),
                    });
                }
            }
            Entry::Text { text } => items.push(Item::Text(text.clone())),
            Entry::Thinking { .. } => {}
            Entry::Tools { calls } => {
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
                    // While a card asks to run a call, the card says what it is.
                    .filter(|c| {
                        c.result.is_some()
                            || !open_asks.contains(&(c.name.clone(), c.input.clone()))
                    })
                    .cloned()
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
                items.push(Item::Tools {
                    summary: summary(&calls, cwd),
                    lines: calls
                        .iter()
                        .map(|c| {
                            let l = line(c, cwd);
                            if !matches!(c.name.as_str(), "Task" | "Agent") {
                                return l;
                            }
                            // A subagent says how far it got.
                            let steps = calls
                                .iter()
                                .filter(|x| x.parent.as_deref() == Some(c.id.as_str()))
                                .count();
                            let state = match &c.result {
                                None => "working",
                                Some((true, _)) => "failed",
                                Some((false, _)) => "done",
                            };
                            match steps {
                                0 => format!("{l} · {state}"),
                                1 => format!("{l} · {state} · 1 step"),
                                n => format!("{l} · {state} · {n} steps"),
                            }
                        })
                        .collect(),
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
                            Some(Item::Tools { .. }) if items.len() > keep => {
                                items.insert(items.len() - 1, note)
                            }
                            _ => items.push(note),
                        }
                    }
                }
            }
            Entry::Made { path, version } => items.push(Item::Made {
                art: art_id(&v.id, path),
                version: *version,
            }),
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
    built.body = items.len();
    if !v.partial.is_empty() {
        items.push(Item::Text(v.partial.clone()));
    }
    for e in &v.entries {
        if let Entry::User {
            id,
            text,
            queued: true,
            ..
        } = e
        {
            items.push(Item::Queued {
                id: id.clone(),
                text: text.clone(),
            });
        }
    }
    let mut arts: Vec<&'static str> = Vec::new();
    for i in items.iter() {
        if let Item::Made { art, .. } = i
            && !arts.contains(art)
        {
            arts.push(art);
        }
    }
    let status = status(v.status);
    let working_since = v.working_since.map(|ms| {
        Instant::now()
            .checked_sub(Duration::from_millis(cx.now_ms.saturating_sub(ms)))
            .unwrap_or_else(Instant::now)
    });
    Session {
        id: v.id.clone(),
        title: v.title.clone(),
        cwd: v.cwd.clone(),
        project: cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| v.cwd.clone()),
        machine: cx.machine.to_string(),
        agent: format!("Claude · {}", model_name(cx.models, v.model.as_deref())),
        model: v.model.clone(),
        effort: v.effort.clone(),
        permission: v.permission,
        status,
        detail: detail(v),
        updated: v.updated,
        working_since,
        closed: v.closed,
        stopped: v.status == Wire::Interrupted,
        items: built.items.clone(),
        arts,
        diff: cx.diff.unwrap_or_default(),
        terminal: terminal(v),
        context: v.context,
        checkpoints: v.checkpoints.clone(),
        servers: built.servers.iter().map(|(_, u)| u.clone()).collect(),
        made_sig: v.made.iter().map(|m| m.versions.len()).sum::<usize>() + v.made.len() * 1000,
        todos: todos(v),
    }
}

/// Local dev-server addresses in an entry: command output and replies.
fn servers_in(e: &Entry) -> Vec<String> {
    let mut texts: Vec<&str> = Vec::new();
    match e {
        Entry::Text { text } => texts.push(text),
        Entry::Tools { calls } => {
            for c in calls {
                if let Some((false, out)) = &c.result
                    && c.name == "Bash"
                {
                    texts.push(out);
                }
            }
        }
        _ => {}
    }
    let mut out = Vec::new();
    for t in texts {
        for (host, shown) in [
            ("http://localhost:", "localhost"),
            ("http://127.0.0.1:", "127.0.0.1"),
            // Listening everywhere: this machine reaches it as localhost.
            ("http://0.0.0.0:", "localhost"),
        ] {
            let mut rest = t;
            while let Some(at) = rest.find(host) {
                let tail = &rest[at + host.len()..];
                let port: String = tail.chars().take_while(char::is_ascii_digit).collect();
                if let Ok(p) = port.parse::<u16>()
                    && p > 0
                {
                    // As printed: localhost may be IPv6 only, 127.0.0.1 IPv4 only.
                    let url = format!("http://{shown}:{p}/");
                    if !out.contains(&url) {
                        out.push(url);
                    }
                }
                rest = &tail[port.len()..];
            }
        }
    }
    out
}

/// The last commands it ran and what they printed, newest last, capped.
fn terminal(v: &SessionView) -> Vec<(Tone, String)> {
    let mut blocks: Vec<Vec<(Tone, String)>> = Vec::new();
    let mut lines = 0;
    'scan: for e in v.entries.iter().rev() {
        let Entry::Tools { calls } = e else { continue };
        for c in calls.iter().rev() {
            let (c, Some((err, out))) = (c, &c.result) else {
                continue;
            };
            if c.name != "Bash" {
                continue;
            }
            let cmd = c.input()["command"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let tone = if *err { Tone::Quiet } else { Tone::Good };
            let mut b = vec![(Tone::Command, format!("$ {cmd}"))];
            b.extend(out.lines().take(40).map(|l| (tone, l.to_string())));
            lines += b.len();
            blocks.push(b);
            if lines >= 400 {
                break 'scan;
            }
        }
    }
    blocks.into_iter().rev().flatten().collect()
}

/// The agent's latest plan, from its last todo list.
fn todos(v: &SessionView) -> Vec<(String, String, String)> {
    let last = v.entries.iter().rev().find_map(|e| match e {
        Entry::Tools { calls } => calls.iter().rev().find(|c| c.name == "TodoWrite"),
        _ => None,
    });
    let Some(c) = last else { return Vec::new() };
    c.input()["todos"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| {
                    (
                        t["status"].as_str().unwrap_or("pending").to_string(),
                        t["content"].as_str().unwrap_or_default().to_string(),
                        t["activeForm"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
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
            // The plan's current step says it best; then the latest call.
            let step = todos(v)
                .into_iter()
                .find(|(st, ..)| st == "in_progress")
                .map(|(_, what, doing)| if doing.is_empty() { what } else { doing });
            match v.entries.last() {
                Some(Entry::Tools { calls }) => calls
                    .iter()
                    .rev()
                    .find(|c| c.parent.is_none() && c.name != "TodoWrite")
                    .map(|c| c.line())
                    .or(step)
                    .unwrap_or_else(|| "Working".into()),
                _ => step.unwrap_or_else(|| "Thinking".into()),
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

#[cfg(test)]
mod bench {
    use dent_protocol::{Block, Body, Event, Out};

    /// A long real-looking session: 600 turns of reads, edits and replies.
    fn big() -> dent_client_core::Sessions {
        let mut s = dent_client_core::Sessions::default();
        let mut events = Vec::new();
        let mut seq = 0;
        let mut push = |events: &mut Vec<Event>, body: Body| {
            seq += 1;
            events.push(Event {
                seq,
                session: "s".into(),
                at: seq,
                body,
            });
        };
        for t in 0..600 {
            push(
                &mut events,
                Body::User {
                    id: format!("u{t}"),
                    text: "Please fix the failing test in the parser and explain why".into(),
                    queued: false,
                    steer: false,
                    attachments: vec![],
                },
            );
            for c in 0..4 {
                let id = format!("t{t}-{c}");
                push(
                    &mut events,
                    Body::Assistant {
                        parent: None,
                        uuid: None,
                        blocks: vec![Block::Tool {
                            id: id.clone(),
                            name: "Read".into(),
                            input: format!(r#"{{"file_path":"/w/src/mod{c}.rs"}}"#),
                        }],
                    },
                );
                push(
                    &mut events,
                    Body::ToolResult {
                        tool: id,
                        error: false,
                        output: "x".repeat(4000),
                        parent: None,
                    },
                );
            }
            push(&mut events, Body::Assistant { parent: None, uuid: None, blocks: vec![Block::Text { text: "The parser dropped the last token when the input ended without a newline. ".repeat(6) }] });
        }
        s.apply(&Out::Events { events });
        s
    }

    #[test]
    #[ignore]
    fn rebuild_cost() {
        let s = big();
        let v = &s.list["s"];
        let t = std::time::Instant::now();
        let n = 50;
        for _ in 0..n {
            let c = v.clone();
            std::hint::black_box(&c);
        }
        let clone = t.elapsed() / n;
        let t = std::time::Instant::now();
        for _ in 0..n {
            let mut b = super::Built::default();
            let out = super::session(
                v,
                super::Context {
                    machine: "m",
                    models: &[],
                    diff: None,
                    now_ms: 0,
                },
                &mut b,
                Some(0),
            );
            std::hint::black_box(&out);
        }
        let build = t.elapsed() / n;
        // Streaming: only the reply changes.
        let mut b = super::Built::default();
        let _ = super::session(
            v,
            super::Context {
                machine: "m",
                models: &[],
                diff: None,
                now_ms: 0,
            },
            &mut b,
            Some(0),
        );
        let t = std::time::Instant::now();
        for _ in 0..n {
            let out = super::session(
                v,
                super::Context {
                    machine: "m",
                    models: &[],
                    diff: None,
                    now_ms: 0,
                },
                &mut b,
                None,
            );
            std::hint::black_box(&out);
        }
        let stream = t.elapsed() / n;
        eprintln!("streaming rebuild {stream:?}");
        eprintln!(
            "entries {} · clone {clone:?} · build {build:?}",
            v.entries.len()
        );
    }
}
