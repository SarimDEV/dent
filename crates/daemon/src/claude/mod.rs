//! The Claude Code driver: how to start `claude -p`, and how its stream
//! becomes dent events ([decision 0008]).
//!
//! [decision 0008]: ../../../docs/decisions/0008-claude-code-stream-json.md

pub mod wire;

use dent_protocol::{Block, Body, Model, Permission, Task, Window};
use serde_json::Value;

use wire::AssistantBlock;

/// The agent's process, with dent's flags. `env` strips variables a parent
/// Claude Code session would leak into it, and sets the login shell's PATH
/// so its tools behave as in a terminal.
pub fn argv(
    claude: &str,
    path: &str,
    model: Option<&str>,
    permission: Permission,
    start: &Start,
    effort: Option<&str>,
) -> Vec<String> {
    let mut v: Vec<String> = [
        "/usr/bin/env",
        "-u",
        "CLAUDECODE",
        "-u",
        "CLAUDE_CODE_ENTRYPOINT",
        "-u",
        "CLAUDE_CODE_SSE_PORT",
    ]
    .map(String::from)
    .into();
    v.push(format!("PATH={path}"));
    v.push(claude.into());
    v.extend(
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            // Lets the person switch to full access mid-session; it does
            // nothing unless they do.
            "--allow-dangerously-skip-permissions",
        ]
        .map(String::from),
    );
    if let Some(mode) = mode(permission) {
        v.extend(["--permission-mode".into(), mode.into()]);
    }
    if let Some(m) = model {
        v.extend(["--model".into(), m.into()]);
    }
    if let Some(e) = effort {
        v.extend(["--effort".into(), e.into()]);
    }
    match start {
        Start::Fresh(id) => v.extend(["--session-id".into(), id.clone()]),
        Start::Resume(id) => v.extend(["--resume".into(), id.clone()]),
        // A rewind: the conversation as it was at that message, as a new
        // session so the old one stays whole.
        Start::ResumeAt { id, at } => v.extend([
            "--resume".into(),
            id.clone(),
            "--resume-session-at".into(),
            at.clone(),
            "--fork-session".into(),
        ]),
    }
    v
}

/// How an agent process picks up its conversation.
pub enum Start {
    /// A new conversation, under an id dent chose.
    Fresh(String),
    Resume(String),
    /// Resume up to the assistant message `at`, as a new session.
    ResumeAt {
        id: String,
        at: String,
    },
}

/// Claude Code's name for a permission mode. `None` is its default.
pub fn mode(p: Permission) -> Option<&'static str> {
    match p {
        Permission::Ask => None,
        Permission::Edits => Some("acceptEdits"),
        Permission::Plan => Some("plan"),
        Permission::Full => Some("bypassPermissions"),
    }
}

/// The mode name for `set_permission_mode`, which wants one for every mode.
pub fn mode_name(p: Permission) -> &'static str {
    mode(p).unwrap_or("default")
}

pub fn permission_from(mode: &str) -> Option<Permission> {
    Some(match mode {
        "default" | "manual" => Permission::Ask,
        "acceptEdits" => Permission::Edits,
        "plan" => Permission::Plan,
        "bypassPermissions" => Permission::Full,
        _ => return None,
    })
}

pub fn models(init: &Value) -> Vec<Model> {
    init["models"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    Some(Model {
                        id: m["value"].as_str()?.into(),
                        name: m["displayName"].as_str().unwrap_or_default().into(),
                        description: m["description"].as_str().unwrap_or_default().into(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn commands(init: &Value) -> Vec<dent_protocol::Command> {
    init["commands"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(dent_protocol::Command {
                        name: c["name"].as_str()?.into(),
                        description: c["description"].as_str().unwrap_or_default().into(),
                        hint: c["argumentHint"].as_str().unwrap_or_default().into(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Models to offer before any session has told us the real list.
pub fn default_models() -> Vec<Model> {
    [
        ("default", "Default", "The account's recommended model"),
        ("opus", "Opus", "For complex work"),
        ("sonnet", "Sonnet", "Efficient for routine tasks"),
        ("haiku", "Haiku", "Fastest for quick answers"),
    ]
    .map(|(id, name, description)| Model {
        id: id.into(),
        name: name.into(),
        description: description.into(),
    })
    .into()
}

pub fn blocks(blocks: Vec<AssistantBlock>) -> Vec<Block> {
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

pub fn limits(info: &wire::RateLimitInfo) -> Body {
    Body::Limits {
        windows: info
            .unified_windows
            .iter()
            .map(|(name, w)| Window {
                name: name.clone(),
                used: w.utilization,
                resets_at: w.resets_at,
            })
            .collect(),
    }
}

pub fn tasks(tasks: &[wire::BackgroundTask]) -> Vec<Task> {
    tasks
        .iter()
        .map(|t| Task {
            id: t.task_id.clone(),
            description: t.description.clone(),
        })
        .collect()
}

/// The reply to a question tool: the agent's own input, plus answers keyed
/// by each question's text.
pub fn answers(input: &Value, picked: &[String]) -> Value {
    let mut out = input.clone();
    let mut map = serde_json::Map::new();
    if let Some(qs) = input["questions"].as_array() {
        for (q, a) in qs.iter().zip(picked) {
            if let Some(text) = q["question"].as_str() {
                map.insert(text.into(), Value::String(a.clone()));
            }
        }
    }
    out["answers"] = Value::Object(map);
    out
}
