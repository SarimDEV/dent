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
            // dent shows the plan and each command's purpose rather than
            // the commands themselves, so ask for both.
            "--append-system-prompt",
            PLAN_NUDGE,
        ]
        .map(String::from),
    );
    // The plan tool, from this same binary, allowed up front: showing the
    // plan never needs asking.
    if let Some(exe) = std::env::current_exe()
        .ok()
        .and_then(|e| e.to_str().map(str::to_owned))
    {
        v.extend([
            "--mcp-config".into(),
            crate::plan_tool::config(&exe),
            "--allowedTools".into(),
            crate::plan_tool::TOOL.into(),
        ]);
    }
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

/// Added to every session's system prompt.
const PLAN_NUDGE: &str = "Before you start any task with three or more steps, call the update_plan tool (from the dent server) with the whole plan, one short line per step. Call it again whenever a step starts or finishes, marking it in_progress or completed, so the plan always shows where you are, and mark every step completed before your final answer. Give every Bash command a short description of what it is for, starting with a verb.";

/// A one-off side question: the conversation `agent_session` so far, forked
/// so the original never sees it, answered without changing anything.
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

/// Claude Code asked for a short title for a conversation: a small model,
/// one turn, no tools, nothing of the person's settings that could act.
pub fn title_argv(claude: &str, path: &str, conversation: &str) -> Vec<String> {
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
            "--model",
            "haiku",
            "--max-turns",
            "1",
            "--disallowedTools",
            "Bash,Write,Edit,MultiEdit,NotebookEdit,Read,Glob,Grep,WebFetch,WebSearch,Task,Agent",
        ]
        .map(String::from),
    );
    v.extend([
        "-p".into(),
        format!(
            "Title this conversation between a person and a coding agent. The title is at most \
             5 words, like a short task name (\"Fix the footer on mobile\"), sentence case, with no \
             colon, quotes or full stop. Reply with the title only.\n\n{conversation}"
        ),
    ]);
    v
}

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

pub use dent_protocol::runs_subagent;

/// Calls that run a subagent and whose report hasn't come yet: a report,
/// being the subagent's whole answer, keeps more of itself (`wire::keep`).
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
    results: Vec<wire::ToolResult>,
    reports: &mut Reports,
) -> Vec<Body> {
    let blocks = self::blocks(blocks);
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
    results: Vec<wire::ToolResult>,
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
                output: wire::keep(output, report),
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
    let ext = match wire::picture_kind(&bytes)? {
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

/// What a call being written out works on, once its input has streamed
/// that far: the file, or a subagent's task.
pub fn target(tool: &str, head: &str) -> Option<String> {
    let key = match tool {
        "Write" | "Edit" | "MultiEdit" | "Read" => "file_path",
        "NotebookEdit" => "notebook_path",
        t if runs_subagent(t) => "description",
        _ => return None,
    };
    // `"key": "value"`, the value complete.
    let at = head.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = head[at..].trim_start().strip_prefix(':')?.trim_start();
    let body = rest.strip_prefix('"')?;
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return serde_json::from_str(&rest[..i + 2]).ok(),
            _ => {}
        }
    }
    None
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

/// The agent's background tasks as dent's, each with the call that started
/// it when `calls` knows it.
pub fn tasks(
    tasks: &[wire::BackgroundTask],
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

    #[test]
    fn a_call_names_its_target_once_streamed() {
        use super::target;
        assert_eq!(target("Write", r#"{"file_pa"#), None);
        assert_eq!(target("Write", r#"{"file_path": "/x/no"#), None);
        assert_eq!(
            target("Write", r#"{"file_path": "/x/a \"b\".md", "content": "hi"#).as_deref(),
            Some(r#"/x/a "b".md"#)
        );
        assert_eq!(
            target("Agent", r#"{"description":"Survey the paywall","prompt":"#).as_deref(),
            Some("Survey the paywall")
        );
        assert_eq!(target("Bash", r#"{"command":"ls"}"#), None);
    }
}
