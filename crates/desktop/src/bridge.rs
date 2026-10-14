//! From the client core's timelines to what the window draws.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use dent_client_core::{Call, Entry, Note, SessionView};
use dent_protocol::{Answer, Model, Status as Wire};
use serde_json::Value;

use crate::data::{Artifact, FileDiff, Item, Kind, Question, Session, ToolLine, intern};
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

/// A model as people say it, with its version: "Opus 5.5", or for the
/// account's default, what it runs: "Default · Sonnet 5.5".
pub fn model_name(models: &[Model], id: Option<&str>) -> String {
    let id = id.unwrap_or("default");
    let Some(m) = models.iter().find(|m| m.id == id) else {
        return pretty_model(id);
    };
    if m.id == "default" {
        return match runs(models, m) {
            Some(name) => format!("Default · {name}"),
            None => "Default model".into(),
        };
    }
    m.name.trim_end_matches(" (recommended)").to_string()
}

/// The named model a choice like "default" runs, found by its exact model.
pub fn runs<'a>(models: &'a [Model], m: &Model) -> Option<&'a str> {
    models
        .iter()
        .find(|o| o.id != m.id && !m.resolved.is_empty() && o.resolved == m.resolved)
        .map(|o| o.name.as_str())
}

/// A model id the list doesn't know, made readable: `claude-opus-4-6` is
/// "Opus 4.6", `opus` is "Opus".
pub fn pretty_model(id: &str) -> String {
    let id = id.split('[').next().unwrap_or(id);
    let mut parts = id.trim_start_matches("claude-").split('-');
    let Some(family) = parts.next().filter(|f| !f.is_empty()) else {
        return id.to_string();
    };
    let mut name: String = family[..1].to_uppercase() + &family[1..];
    // Dated snapshots (`-20251001`) name the same version.
    let version: Vec<&str> = parts
        .filter(|p| p.len() < 8 && p.chars().all(|c| c.is_ascii_digit()))
        .collect();
    if !version.is_empty() {
        name.push(' ');
        name.push_str(&version.join("."));
    }
    name
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
/// A path as people read it: inside the session's folder, relative to it;
/// elsewhere in home, from `~`.
pub fn relative(cwd: &Path, path: &str) -> String {
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
    if let Ok(alias) = cwd.strip_prefix("/private")
        && let Ok(r) = p.strip_prefix(Path::new("/").join(alias))
    {
        return r.display().to_string();
    }
    // Outside the project: home reads as ~.
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&format!("{home}/")) => {
            format!("~{}", &path[home.len()..])
        }
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
            // The same port seen again, under another host name, is the same server.
            let port = url.rsplit(':').next().unwrap_or_default().to_string();
            if !built
                .servers
                .iter()
                .any(|(_, u)| u.ends_with(&format!(":{port}")))
            {
                built.servers.push((start + k, url.clone()));
                items.push(Item::Made {
                    art: art_id(&v.id, &url),
                    version: 1,
                    review: None,
                });
            }
        }
        match e {
            Entry::User {
                id,
                text,
                queued: q,
                attachments,
                at,
                ..
            } => {
                if *q {
                    // Waiting messages go last, below everything.
                } else if attachments.is_empty() {
                    items.push(Item::User {
                        id: id.clone(),
                        text: text.clone(),
                        at: *at,
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
                        at: *at,
                    });
                }
            }
            Entry::Text { text } => items.push(Item::Text(text.clone())),
            Entry::Thinking { text } => items.push(Item::Thinking(text.clone())),
            Entry::Tools { calls } => {
                // The plan belongs with the composer, and questions and plans
                // have their own cards.
                let calls: Vec<Call> = calls
                    .iter()
                    .filter(|c| {
                        !matches!(
                            c.name.as_str(),
                            // The plan belongs with the composer; questions and
                            // plans have their own cards; the rest is the
                            // agent's housekeeping.
                            "TodoWrite"
                                | "AskUserQuestion"
                                | "ExitPlanMode"
                                | "EnterPlanMode"
                                | "ToolSearch"
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
                let mut lines = tree(&calls, cwd);
                let mut summary = summary(&calls, cwd);
                // One call at the top: the folded line is that call, so its
                // line doesn't repeat it, and what's under it moves up.
                if lines.iter().filter(|l| l.depth == 0).count() == 1 && lines.len() > 1 {
                    summary = lines.remove(0).text;
                    for l in &mut lines {
                        l.depth -= 1;
                    }
                }
                items.push(Item::Tools { summary, lines });
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
                review: None,
            }),
            Entry::Changed { paths } => {
                // A file this turn wrote already has its card: Review goes
                // on the card. The rest get a line of their own.
                let turn = items
                    .iter()
                    .rposition(|i| matches!(i, Item::User { .. }))
                    .map_or(0, |u| u + 1);
                let mut rest = Vec::new();
                for p in paths {
                    let id = art_id(&v.id, p);
                    let card = items[turn..]
                        .iter_mut()
                        .rev()
                        .find_map(|i| match i {
                            Item::Made { art, review, .. } if *art == id => Some(review),
                            _ => None,
                        });
                    match card {
                        Some(review) => *review = Some(relative(cwd, p)),
                        None => rest.push(relative(cwd, p)),
                    }
                }
                if !rest.is_empty() {
                    items.push(Item::Changed { paths: rest });
                }
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
        context: v.context,
        asides: v.asides.clone(),
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
                    // One server per port, however it's written.
                    let url = format!("http://{shown}:{p}/");
                    let port = format!(":{p}/");
                    if !out.iter().any(|u: &String| u.ends_with(&port)) {
                        out.push(url);
                    }
                }
                rest = &tail[port.len()..];
            }
        }
    }
    out
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
            Some(Entry::Ask { tool, input, .. }) => match tool.as_str() {
                "AskUserQuestion" => "Asked you a question".into(),
                "ExitPlanMode" => "Plan ready for review".into(),
                // Worded as the approval card words it.
                t => {
                    let input = serde_json::from_str(input).unwrap_or(Value::Null);
                    what(t, &input, Path::new(&v.cwd)).0
                }
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

/// The group's calls as a tree: each call, then what any subagent it
/// started did, a level deeper.
fn tree(calls: &[Call], cwd: &Path) -> Vec<ToolLine> {
    let mut out = Vec::new();
    branch(calls, None, 0, cwd, &mut out);
    out
}

fn branch(calls: &[Call], parent: Option<&str>, depth: usize, cwd: &Path, out: &mut Vec<ToolLine>) {
    // A call whose parent isn't in this group roots here.
    let here = |c: &&Call| match parent {
        None => c
            .parent
            .as_deref()
            .is_none_or(|p| !calls.iter().any(|x| x.id == p)),
        Some(p) => c.parent.as_deref() == Some(p),
    };
    for c in calls.iter().filter(here) {
        let agent = matches!(c.name.as_str(), "Task" | "Agent");
        let mut text = line(c, cwd);
        if agent {
            // A subagent says how far it got.
            let steps = calls
                .iter()
                .filter(|x| x.parent.as_deref() == Some(c.id.as_str()))
                .count();
            // Finished is the usual case and says nothing; the others do.
            let state = match &c.result {
                None => " · working",
                Some((true, _)) => " · failed",
                Some((false, _)) => "",
            };
            text = match steps {
                0 => format!("{text}{state}"),
                1 => format!("{text}{state} · 1 step"),
                n => format!("{text}{state} · {n} steps"),
            };
        }
        out.push(ToolLine {
            text,
            depth,
            report: None,
            id: c.id.clone(),
            detail: (!agent).then(|| Rc::new(call_detail(c, cwd))),
        });
        if !agent || depth > 8 {
            continue;
        }
        branch(calls, Some(&c.id), depth + 1, cwd, out);
        match (&c.result, &c.said) {
            // While it works, what it last said.
            (None, Some(said)) => out.push(ToolLine {
                text: format!("“{}”", first_line(said)),
                depth: depth + 1,
                report: None,
                id: c.id.clone(),
                detail: None,
            }),
            (Some((false, report)), _) if !report.trim().is_empty() => out.push(ToolLine {
                text: String::new(),
                depth: depth + 1,
                report: Some(report.trim().to_string()),
                id: c.id.clone(),
                detail: None,
            }),
            _ => {}
        }
    }
}

/// What an edit does, as the lines it takes out ("- ") and puts in ("+ ");
/// for a new file, its contents.
pub fn edit_text(tool: &str, v: &Value) -> Option<String> {
    let edit = |old: &str, new: &str| {
        let mut out: Vec<String> = old.lines().map(|l| format!("- {l}")).collect();
        out.extend(new.lines().map(|l| format!("+ {l}")));
        out.join("\n")
    };
    let s = |e: &Value, k: &str| e[k].as_str().unwrap_or_default().to_string();
    match tool {
        "Edit" => Some(edit(&s(v, "old_string"), &s(v, "new_string"))),
        "MultiEdit" => v["edits"].as_array().map(|es| {
            es.iter()
                .map(|e| edit(&s(e, "old_string"), &s(e, "new_string")))
                .collect::<Vec<_>>()
                .join("\n…\n")
        }),
        "Write" => Some(s(v, "content")),
        _ => None,
    }
}

/// A call in full, for its line opened up.
fn call_detail(c: &Call, cwd: &Path) -> crate::data::CallDetail {
    let name = c.name.trim_start_matches("waiting:");
    let v = c.input();
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let input = match name {
        "Bash" => format!("$ {}", s("command")),
        "Edit" | "MultiEdit" | "Write" => edit_text(name, &v).unwrap_or_default(),
        "Read" => String::new(),
        _ => serde_json::to_string_pretty(&v).unwrap_or_default(),
    };
    let path = c.path().map(|p| {
        if p.starts_with('/') {
            p
        } else {
            cwd.join(p).display().to_string()
        }
    });
    crate::data::CallDetail {
        input,
        output: c.result.clone(),
        path,
        edits: matches!(name, "Write" | "Edit" | "MultiEdit" | "NotebookEdit"),
    }
}

/// One line per call.
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
    format!("{l}{mark}")
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
            let (mut title, mut command) = what(tool, input, cwd);
            // An edit to approve names its file in the title, and the box
            // holds what it changes.
            let change = edit_text(tool, input).filter(|e| !e.trim().is_empty());
            if let Some(e) = &change {
                if let Some(t) = title.strip_suffix(" a file") {
                    title = format!("{t} {command}");
                }
                command = e.clone();
            }
            Item::Approval {
                id: id.into(),
                title,
                command,
                why: description.unwrap_or_default().into(),
                diff: change.is_some() && tool != "Write",
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

#[cfg(test)]
mod model_tests {
    use super::*;

    fn m(id: &str, name: &str, resolved: &str) -> Model {
        Model {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            resolved: resolved.into(),
            efforts: Vec::new(),
        }
    }

    #[test]
    fn names_carry_versions() {
        let list = [
            m("default", "Default (recommended)", "claude-sonnet-5-5"),
            m("opus", "Opus 5.5", "claude-opus-5-5"),
            m("sonnet", "Sonnet 5.5", "claude-sonnet-5-5"),
        ];
        assert_eq!(model_name(&list, None), "Default · Sonnet 5.5");
        assert_eq!(model_name(&list, Some("opus")), "Opus 5.5");
        assert_eq!(model_name(&list, Some("claude-opus-4-6")), "Opus 4.6");
        assert_eq!(
            model_name(&[], Some("claude-haiku-4-5-20251001")),
            "Haiku 4.5"
        );
        assert_eq!(model_name(&[], Some("claude-opus-5-5[1m]")), "Opus 5.5");
        assert_eq!(model_name(&[], Some("fable")), "Fable");
        assert_eq!(
            model_name(&[m("default", "Default", "")], None),
            "Default model"
        );
    }
}
