//! The Claude Code driver: how to start `claude -p`, and how its stream
//! becomes dent events ([decision 0008]).
//!
//! [decision 0008]: ../../../docs/decisions/0008-claude-code-stream-json.md

pub mod commands;
pub mod sheets;
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

/// A one-off side question: the conversation `agent_session` so far, forked
/// so the original never sees it, answered without changing anything.
pub fn aside_argv(
    claude: &str,
    path: &str,
    model: Option<&str>,
    agent_session: &str,
    question: &str,
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
            "--output-format",
            "stream-json",
            "--verbose",
            "--resume",
            agent_session,
            "--fork-session",
            // Reading is fine; nothing that changes the project or runs.
            "--disallowedTools",
            "Bash,Write,Edit,MultiEdit,NotebookEdit",
            "--append-system-prompt",
            "This is a side question about the conversation so far. Answer it directly and briefly. Don't change any files.",
        ]
        .map(String::from),
    );
    if let Some(m) = model {
        v.extend(["--model".into(), m.into()]);
    }
    v.extend(["-p".into(), question.into()]);
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
                        resolved: m["resolvedModel"].as_str().unwrap_or_default().into(),
                        efforts: m["supportedEffortLevels"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|e| e.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default(),
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
                        args: Default::default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Models to offer before the agent has told us its list: only until the
/// first usage reading, which asks it.
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
        resolved: String::new(),
        efforts: Vec::new(),
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

/// Output styles from `initialize`, with what each does when known.
pub fn styles(init: &Value) -> (Vec<(String, String)>, Option<String>) {
    let list = init["available_output_styles"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| Some((s.as_str()?.to_string(), String::new())))
                .collect()
        })
        .unwrap_or_default();
    (list, init["output_style"].as_str().map(str::to_string))
}

/// The questions Claude Code may put to the person through the host
/// (`request_user_dialog`), which dent shows as it shows the agent's own
/// questions.
pub const DIALOGS: [&str; 3] = [
    "refusal_fallback_prompt",
    "fable_overage_consent_prompt",
    "auto_mode_server_fallback",
];

/// A dialog as an agent question: the question tool's input, and for each
/// option its label and the answer Claude Code wants back.
pub fn dialog(kind: &str, p: &Value) -> Option<(Value, Vec<(String, String)>)> {
    let name = |k: &str| {
        p[k].as_str()
            .map(model_name)
            .unwrap_or_else(|| "another model".into())
    };
    let (header, question, options): (&str, String, Vec<(String, &str, &str)>) = match kind {
        "refusal_fallback_prompt" => {
            let fallback = name("fallbackModel");
            let mut q = format!("{} declined to answer this.", name("originalModel"));
            if let Some(g) = p["guidanceText"].as_str().filter(|g| !g.is_empty()) {
                q = format!("{q} {g}");
            }
            (
                "Declined",
                q,
                vec![
                    (
                        format!("Try with {fallback}"),
                        "retry_fallback",
                        "Send the same message to the other model",
                    ),
                    (
                        "Edit my message".into(),
                        "edit_prompt",
                        "Change what you asked and send it again",
                    ),
                    ("Cancel".into(), "cancelled", ""),
                ],
            )
        }
        "fable_overage_consent_prompt" => {
            let model = p["modelName"].as_str().unwrap_or("This model");
            let q = if p["overagesEnabled"] == true {
                format!("{model} uses usage credits once your plan's limit is reached.")
            } else {
                format!("{model} needs usage credits, which aren't turned on for this account.")
            };
            (
                "Usage credits",
                q,
                vec![
                    (
                        format!("Use {model}"),
                        "consent",
                        "Draw on usage credits for it",
                    ),
                    (
                        "Use the default model".into(),
                        "switch_default",
                        "Carry on without credits",
                    ),
                    ("Cancel".into(), "cancelled", ""),
                ],
            )
        }
        "auto_mode_server_fallback" => {
            let mut q = p["title"]
                .as_str()
                .unwrap_or("Auto mode changed")
                .to_string();
            if let Some(paras) = p["paragraphs"].as_array() {
                for para in paras.iter().filter_map(Value::as_str) {
                    q = format!("{q}\n\n{para}");
                }
            }
            (
                "Auto mode",
                q,
                vec![
                    ("Continue".into(), "continue", ""),
                    ("Stop".into(), "interrupt", "End this turn"),
                ],
            )
        }
        _ => return None,
    };
    // The answers ride along, so a daemon that restarts mid-question
    // still knows them.
    let input = serde_json::json!({ "dent_dialog": options.iter().map(|(l, v, _)| [l.as_str(), *v]).collect::<Vec<_>>(), "questions": [{
        "question": question,
        "header": header,
        "multiSelect": false,
        "options": options.iter().map(|(label, _, about)| serde_json::json!({
            "label": label, "description": about,
        })).collect::<Vec<_>>(),
    }]});
    let values = options
        .into_iter()
        .map(|(label, value, _)| (label, value.to_string()))
        .collect();
    Some((input, values))
}

/// A dialog's labels and answers, from the question it became.
pub fn dialog_values(input: &Value) -> Option<Vec<(String, String)>> {
    input["dent_dialog"].as_array().map(|a| {
        a.iter()
            .filter_map(|p| Some((p[0].as_str()?.to_string(), p[1].as_str()?.to_string())))
            .collect()
    })
}

/// A model id as people say it: `claude-opus-5-5` is "Opus 5.5".
pub fn model_name(id: &str) -> String {
    let id = id.trim_start_matches("claude-");
    let mut parts = id.split('-');
    let Some(family) = parts.next() else {
        return id.to_string();
    };
    let version: Vec<&str> = parts
        .take_while(|p| p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit()))
        .collect();
    let mut name: String = family
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + &family[c.len_utf8()..])
        .unwrap_or_default();
    if !version.is_empty() {
        name = format!("{name} {}", version.join("."));
    }
    name
}

#[cfg(test)]
mod tests {
    #[test]
    fn model_ids_read_as_names() {
        assert_eq!(super::model_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(super::model_name("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(super::model_name("opus"), "Opus");
    }
}
