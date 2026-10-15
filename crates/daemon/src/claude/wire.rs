//! The slice of Claude Code's stream-json protocol dent uses ([spike 01]).
//!
//! Every line parses into [`Inbound`]; anything unrecognised becomes
//! [`Inbound::Other`], so a new event type never breaks the driver.
//!
//! [spike 01]: ../../../docs/spikes/01-claude-code-stream-json.md

use serde::Deserialize;
use serde_json::{Value, json};

/// Tool output kept in the log; the rest stays in the agent's transcript.
const OUTPUT_CAP: usize = 4 * 1024;

#[derive(Debug)]
pub enum Inbound {
    /// `system/init`: the start of every turn, including turns the agent
    /// starts itself after background work finishes.
    Init {
        session_id: String,
        version: String,
        permission_mode: String,
    },
    /// `system/background_tasks_changed`: the full current set.
    BackgroundTasks(Vec<BackgroundTask>),
    /// A new assistant message is starting to stream.
    MessageStart {
        parent: Option<String>,
    },
    /// Streamed text (needs `--include-partial-messages`).
    TextDelta {
        parent: Option<String>,
        text: String,
    },
    /// A tool call starts streaming its input.
    ToolStart {
        parent: Option<String>,
        name: String,
    },
    /// More of that call's input, as raw JSON text.
    InputDelta {
        parent: Option<String>,
        json: String,
    },
    /// A complete assistant message.
    Assistant {
        parent: Option<String>,
        uuid: Option<String>,
        blocks: Vec<AssistantBlock>,
        /// Input plus cache tokens: how full the context is.
        context: Option<u64>,
        /// Written by Claude Code itself, not the model: a usage limit or an
        /// API error, which the turn's `result` also reports.
        synthetic: bool,
    },
    /// Tool results come back as `user` messages.
    ToolResults {
        parent: Option<String>,
        results: Vec<ToolResult>,
    },
    RateLimit(RateLimitInfo),
    /// `control_request/can_use_tool`: the agent is blocked on us.
    CanUseTool {
        request_id: String,
        tool_name: String,
        input: Value,
        description: Option<String>,
        suggestions: Value,
    },
    ControlResponse {
        request_id: String,
        ok: bool,
        body: Value,
    },
    /// Any other `control_request`. The agent waits on every one of these,
    /// so each gets an answer, even if only "no".
    Request {
        request_id: String,
        subtype: String,
        server: Option<String>,
    },
    /// The agent no longer needs an answer (after an interrupt, say).
    CancelRequest {
        request_id: String,
    },
    /// Claude Code summarised the conversation so far to make room.
    Compacted {
        auto: bool,
        before: u64,
        after: Option<u64>,
    },
    /// `system/status`: compacting started (`Some(true)`), or stopped.
    Compacting {
        on: bool,
        /// Why a compaction failed, when it did.
        failed: Option<String>,
    },
    /// `system/api_retry`: a request failed and will be tried again.
    Retrying {
        attempt: u32,
        max: u32,
        delay_ms: u64,
        error: String,
    },
    /// `system/commands_changed`: the whole new list.
    CommandsChanged(Value),
    /// A command Claude Code ran itself: its answer, as the synthetic
    /// assistant message carries it.
    LocalCommand {
        name: Option<String>,
        args: String,
        text: String,
        outcome: dent_protocol::CommandOutcome,
        view: Option<dent_protocol::View>,
    },
    /// Something the agent wants said in the chat: a notification, a
    /// warning, a model falling back.
    Notice(String),
    /// `control_request/request_user_dialog`: a question for the person.
    Dialog {
        request_id: String,
        kind: String,
        payload: Value,
    },
    /// The end of a turn.
    Result {
        terminal_reason: Option<String>,
        is_error: bool,
        /// The reply, or on error what went wrong.
        text: Option<String>,
    },
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackgroundTask {
    pub task_id: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug)]
pub enum AssistantBlock {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub is_error: bool,
    pub output: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitInfo {
    #[serde(default)]
    pub unified_windows: std::collections::BTreeMap<String, Window>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub utilization: f64,
    pub resets_at: i64,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn parent(v: &Value) -> Option<String> {
    v["parent_tool_use_id"].as_str().map(str::to_owned)
}

pub fn parse(line: &[u8]) -> serde_json::Result<Inbound> {
    let v: Value = serde_json::from_slice(line)?;
    let kind = s(&v, "type");
    let sub = s(&v, "subtype");
    Ok(match (kind.as_str(), sub.as_str()) {
        ("system", "init") => Inbound::Init {
            session_id: s(&v, "session_id"),
            version: s(&v, "claude_code_version"),
            permission_mode: s(&v, "permissionMode"),
        },
        ("system", "compact_boundary") => {
            let m = &v["compact_metadata"];
            Inbound::Compacted {
                auto: m["trigger"] == "auto",
                before: m["pre_tokens"].as_u64().unwrap_or(0),
                after: m["post_tokens"].as_u64(),
            }
        }
        ("system", "status") => match (v["status"].as_str(), v["compact_result"].as_str()) {
            (Some("compacting"), _) => Inbound::Compacting {
                on: true,
                failed: None,
            },
            (_, Some(r)) => Inbound::Compacting {
                on: false,
                failed: (r == "failed").then(|| {
                    v["compact_error"]
                        .as_str()
                        .unwrap_or("Claude Code couldn't summarise the conversation")
                        .to_string()
                }),
            },
            _ => Inbound::Other,
        },
        ("system", "api_retry") => Inbound::Retrying {
            attempt: v["attempt"].as_u64().unwrap_or(1) as u32,
            max: v["max_retries"].as_u64().unwrap_or(0) as u32,
            delay_ms: v["retry_delay_ms"].as_u64().unwrap_or(0),
            error: retry_error(&v),
        },
        ("system", "commands_changed") => Inbound::CommandsChanged(v),
        ("system", "notification") => match v["text"].as_str() {
            Some(t) if !t.trim().is_empty() => Inbound::Notice(t.to_string()),
            _ => Inbound::Other,
        },
        // Suggestions and plain info are the terminal's chatter.
        ("system", "informational")
            if matches!(v["level"].as_str(), Some("warning" | "notice")) =>
        {
            match v["content"].as_str() {
                Some(t) if !t.trim().is_empty() => Inbound::Notice(t.to_string()),
                _ => Inbound::Other,
            }
        }
        (
            "system",
            "model_refusal_fallback" | "model_refusal_no_fallback" | "model_consent_fallback",
        ) => match v["content"].as_str() {
            Some(t) if !t.trim().is_empty() => Inbound::Notice(t.to_string()),
            _ => Inbound::Other,
        },
        ("assistant", _) if v.get("local_command_source").is_some() => local_command(&v),
        ("system", "background_tasks_changed") => {
            Inbound::BackgroundTasks(serde_json::from_value(v["tasks"].clone()).unwrap_or_default())
        }
        ("stream_event", _) => {
            let e = &v["event"];
            match (e["type"].as_str(), e["delta"]["type"].as_str()) {
                (Some("message_start"), _) => Inbound::MessageStart { parent: parent(&v) },
                (Some("content_block_delta"), Some("text_delta")) => Inbound::TextDelta {
                    parent: parent(&v),
                    text: s(&e["delta"], "text"),
                },
                (Some("content_block_start"), _) if e["content_block"]["type"] == "tool_use" => {
                    Inbound::ToolStart {
                        parent: parent(&v),
                        name: s(&e["content_block"], "name"),
                    }
                }
                (Some("content_block_delta"), Some("input_json_delta")) => Inbound::InputDelta {
                    parent: parent(&v),
                    json: s(&e["delta"], "partial_json"),
                },
                _ => Inbound::Other,
            }
        }
        ("assistant", _) => {
            let m = &v["message"];
            let blocks = m["content"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|b| match b["type"].as_str()? {
                            "text" => Some(AssistantBlock::Text(s(b, "text"))),
                            "thinking" => Some(AssistantBlock::Thinking(s(b, "thinking"))),
                            "tool_use" | "server_tool_use" => Some(AssistantBlock::ToolUse {
                                id: s(b, "id"),
                                name: s(b, "name"),
                                input: b["input"].clone(),
                            }),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let u = &m["usage"];
            let context = u["input_tokens"].as_u64().map(|i| {
                i + u["cache_creation_input_tokens"].as_u64().unwrap_or(0)
                    + u["cache_read_input_tokens"].as_u64().unwrap_or(0)
            });
            Inbound::Assistant {
                parent: parent(&v),
                uuid: v["uuid"].as_str().map(str::to_owned),
                blocks,
                context,
                synthetic: m["model"] == "<synthetic>" || v["isApiErrorMessage"] == true,
            }
        }
        ("user", _) => {
            let results = v["message"]["content"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|b| b["type"] == "tool_result")
                        .map(|b| ToolResult {
                            tool_use_id: s(b, "tool_use_id"),
                            is_error: b["is_error"].as_bool().unwrap_or(false),
                            output: output(&b["content"]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Inbound::ToolResults {
                parent: parent(&v),
                results,
            }
        }
        ("rate_limit_event", _) => match serde_json::from_value(v["rate_limit_info"].clone()) {
            Ok(info) => Inbound::RateLimit(info),
            Err(_) => Inbound::Other,
        },
        ("control_request", _) if v["request"]["subtype"] == "can_use_tool" => {
            let r = &v["request"];
            Inbound::CanUseTool {
                request_id: s(&v, "request_id"),
                tool_name: s(r, "tool_name"),
                input: r["input"].clone(),
                description: r["description"].as_str().map(str::to_owned),
                suggestions: r["permission_suggestions"].clone(),
            }
        }
        ("control_request", _) if v["request"]["subtype"] == "request_user_dialog" => {
            Inbound::Dialog {
                request_id: s(&v, "request_id"),
                kind: s(&v["request"], "dialog_kind"),
                payload: v["request"]["payload"].clone(),
            }
        }
        ("control_request", _) => Inbound::Request {
            request_id: s(&v, "request_id"),
            subtype: s(&v["request"], "subtype"),
            server: v["request"]["mcp_server_name"].as_str().map(str::to_owned),
        },
        ("control_response", _) => {
            let r = &v["response"];
            let ok = r["subtype"] == "success";
            Inbound::ControlResponse {
                request_id: s(r, "request_id"),
                ok,
                body: if ok {
                    r["response"].clone()
                } else {
                    r["error"].clone()
                },
            }
        }
        ("control_cancel_request", _) => Inbound::CancelRequest {
            request_id: s(&v, "request_id"),
        },
        ("result", _) => {
            // API failures come as `success` with `is_error`; the errors
            // list carries internal diagnostics nobody should see.
            let errors: Vec<&str> = v["errors"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .filter(|e| !e.starts_with("[ede_diagnostic]"))
                        .collect()
                })
                .unwrap_or_default();
            let text = v["result"]
                .as_str()
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
                .or_else(|| (!errors.is_empty()).then(|| errors.join("\n")));
            Inbound::Result {
                terminal_reason: v["terminal_reason"].as_str().map(str::to_owned),
                is_error: v["is_error"].as_bool().unwrap_or(false) || sub.starts_with("error"),
                text,
            }
        }
        _ => Inbound::Other,
    })
}

/// What a retry is about, short: the status and the agent's words.
fn retry_error(v: &Value) -> String {
    let words = match &v["error"] {
        Value::String(s) => s.clone(),
        e => e["message"].as_str().unwrap_or_default().to_string(),
    };
    let words = if v["no_response"].is_object() {
        "No response from the API".to_string()
    } else {
        words
    };
    match v["error_status"].as_u64() {
        Some(529) if words.is_empty() || words.contains("verloaded") => {
            "Claude is overloaded".into()
        }
        Some(code) if words.is_empty() => format!("The API answered {code}"),
        _ if words.is_empty() => "The request failed".into(),
        _ => words,
    }
}

/// A command Claude Code ran itself, from its synthetic assistant message.
fn local_command(v: &Value) -> Inbound {
    use dent_protocol::{CommandOutcome, View};
    let text = v["message"]["content"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let run = &v["local_command_run"];
    let o = &v["local_command_outcome"];
    let outcome = match o["kind"].as_str() {
        Some("unavailable_headless") => CommandOutcome::TerminalOnly,
        Some("unknown") => CommandOutcome::Unknown {
            suggestion: o["suggestion"]
                .as_str()
                .map(|s| s.trim_start_matches('/').to_string()),
        },
        Some("failed") => CommandOutcome::Failed,
        _ => CommandOutcome::Done,
    };
    let cu = &v["context_usage"];
    let ur = &v["usage_report"];
    let view = if let Some(used) = cu["total_tokens"].as_u64() {
        Some(View::Meter {
            used,
            max: cu["raw_max_tokens"].as_u64().unwrap_or(0),
            compacts_at: None,
            parts: parts(&cu["categories"]),
        })
    } else if ur.is_object() {
        usage_bars(ur).map(|bars| View::Limits { bars })
    } else {
        None
    }
    .filter(|v| !matches!(v, View::Meter { max: 0, .. }));
    Inbound::LocalCommand {
        name: run["command"].as_str().map(str::to_string),
        args: run["args"].as_str().unwrap_or_default().to_string(),
        text,
        outcome,
        view,
    }
}

/// What fills the context, by kind: used and deferred, never the free
/// space or the buffer, which are what's left.
pub fn parts(categories: &Value) -> Vec<dent_protocol::Part> {
    categories
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|c| {
                    !matches!(c["kind"].as_str(), Some("free" | "buffer"))
                        && !matches!(
                            c["name"].as_str(),
                            Some("Free space" | "Autocompact buffer")
                        )
                })
                .filter_map(|c| {
                    Some(dent_protocol::Part {
                        // Listed under "loaded only when needed", the
                        // name needn't say it.
                        name: c["name"]
                            .as_str()?
                            .trim_end_matches(" (deferred)")
                            .to_string(),
                        tokens: c["tokens"].as_u64()?,
                        deferred: c["kind"] == "deferred" || c["isDeferred"] == true,
                    })
                })
                .filter(|p| p.tokens > 0)
                .collect()
        })
        .unwrap_or_default()
}

/// `/usage`'s limits, from its report, labelled as the terminal labels
/// them.
fn usage_bars(report: &Value) -> Option<Vec<dent_protocol::Bar>> {
    let bars: Vec<dent_protocol::Bar> = report["rate_limits"]["limits"]
        .as_array()?
        .iter()
        .filter_map(|l| {
            let scope = l["scope"]["model"]["display_name"].as_str();
            let label = match (l["kind"].as_str()?, scope) {
                ("session", _) => "Current session".to_string(),
                (_, Some(m)) => format!("This week ({m})"),
                ("weekly_all", None) => "This week (all models)".to_string(),
                (k, None) => k.replace('_', " "),
            };
            Some(dent_protocol::Bar {
                label,
                used: l["percent"].as_f64()? / 100.,
                resets_at: l["resets_at"]
                    .as_str()
                    .and_then(crate::usage::unix)
                    .unwrap_or(0),
            })
        })
        .collect();
    (!bars.is_empty()).then_some(bars)
}

/// A tool result's text, cut to [`OUTPUT_CAP`] on a character boundary.
fn output(content: &Value) -> String {
    let text = match content {
        Value::String(t) => t.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    if text.len() <= OUTPUT_CAP {
        return text;
    }
    let mut end = OUTPUT_CAP;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

// ---- outbound ----

/// A message from the person. `uuid` is fresh per message (the CLI ignores
/// one it has seen). `steer` puts it into the running turn instead of after.
pub fn user_message(text: &str, uuid: &str, steer: bool, attachments: &[String]) -> Value {
    let content = if attachments.is_empty() {
        json!(text)
    } else {
        // Images go in as images, so the agent sees them straight away; every
        // file is also named, so its tools can open it.
        let mut blocks: Vec<Value> = attachments.iter().filter_map(|p| image_block(p)).collect();
        let names: Vec<String> = attachments
            .iter()
            .map(|p| format!("Attached: {p}"))
            .collect();
        let body = if text.is_empty() {
            names.join("\n")
        } else {
            format!("{text}\n\n{}", names.join("\n"))
        };
        blocks.push(json!({ "type": "text", "text": body }));
        Value::Array(blocks)
    };
    let mut v = json!({
        "type": "user",
        "message": { "role": "user", "content": content },
        "parent_tool_use_id": null,
        "session_id": "",
        "uuid": uuid,
    });
    if steer {
        v["priority"] = json!("now");
    }
    v
}

pub fn control_request(request_id: &str, subtype: &str, mut extra: Value) -> Value {
    extra["subtype"] = json!(subtype);
    json!({ "type": "control_request", "request_id": request_id, "request": extra })
}

/// Images the model reads directly, up to 5 MB.
fn image_block(path: &str) -> Option<Value> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    let media = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 5 * 1024 * 1024 {
        return None;
    }
    Some(json!({
        "type": "image",
        "source": { "type": "base64", "media_type": media, "data": dent_protocol::b64::encode(&bytes) },
    }))
}

/// Whether the agent offered a rule it can save for this kind of call.
pub fn keeps(suggestions: &Value) -> bool {
    suggestions
        .as_array()
        .is_some_and(|a| a.iter().any(|s| s["type"] == "addRules"))
}

/// Allow a tool call. With `always`, the agent's own suggestions (or a rule
/// for the tool) are sent back so the same kind of call is allowed for the
/// rest of the session. With `project` too, its suggestions go to the
/// project's local settings, where Claude Code keeps, lists and enforces
/// them from now on.
pub fn allow(
    request_id: &str,
    tool: &str,
    input: &Value,
    always: bool,
    project: bool,
    suggestions: &Value,
) -> Value {
    let always = always || project;
    let mut r = json!({
        "behavior": "allow",
        "updatedInput": input,
        "decisionClassification": if always { "user_permanent" } else { "user_temporary" },
    });
    if always {
        let destination = if project { "localSettings" } else { "session" };
        let rules: Vec<Value> = suggestions
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|s| s["type"] == "addRules")
                    .map(|s| {
                        let mut s = s.clone();
                        s["destination"] = json!(destination);
                        s
                    })
                    .collect()
            })
            .unwrap_or_default();
        r["updatedPermissions"] = if rules.is_empty() {
            json!([{ "type": "addRules", "rules": [{ "toolName": tool }], "behavior": "allow", "destination": "session" }])
        } else {
            Value::Array(rules)
        };
    }
    control_success(request_id, r)
}

pub fn deny(request_id: &str, reason: &str) -> Value {
    let message = if reason.trim().is_empty() {
        "User declined tool execution."
    } else {
        reason
    };
    control_success(
        request_id,
        json!({ "behavior": "deny", "message": message, "decisionClassification": "user_reject" }),
    )
}

/// Turn down a request dent doesn't handle, so the agent stops waiting.
pub fn refuse(request_id: &str, subtype: &str) -> Value {
    if subtype == "elicitation" {
        // An MCP server asking the person for input: a plain decline.
        return control_success(request_id, json!({ "action": "decline" }));
    }
    json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": request_id,
            "error": format!("dent doesn't handle {subtype} requests"),
        },
    })
}

/// The person's answer to a dialog Claude Code asked through dent; `None`
/// when they dismissed it.
pub fn dialog_answer(request_id: &str, value: Option<&str>) -> Value {
    control_success(
        request_id,
        match value {
            Some(v) if v != "cancelled" => json!({ "behavior": "completed", "result": v }),
            _ => json!({ "behavior": "cancelled" }),
        },
    )
}

fn control_success(request_id: &str, response: Value) -> Value {
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": response },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_recorded_shapes() {
        let a = br#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}],"usage":{"input_tokens":2,"cache_creation_input_tokens":10,"cache_read_input_tokens":100,"output_tokens":5}},"parent_tool_use_id":null}"#;
        match parse(a).unwrap() {
            Inbound::Assistant {
                blocks, context, ..
            } => {
                assert_eq!(context, Some(112));
                assert!(
                    matches!(&blocks[0], AssistantBlock::ToolUse { name, .. } if name == "Bash")
                );
            }
            other => panic!("{other:?}"),
        }
        let r = br#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}],"is_error":true}]},"parent_tool_use_id":"p"}"#;
        match parse(r).unwrap() {
            Inbound::ToolResults { parent, results } => {
                assert_eq!(parent.as_deref(), Some("p"));
                assert_eq!(results[0].output, "a\nb");
                assert!(results[0].is_error);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse(br#"{"type":"brand_new"}"#).unwrap(),
            Inbound::Other
        ));
    }

    #[test]
    fn attachments_become_image_blocks_and_names() {
        let dir = std::env::temp_dir().join(format!("dent-att-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("shot.png");
        std::fs::write(&png, [137, 80, 78, 71]).unwrap();
        let txt = dir.join("notes.txt").display().to_string();
        let png = png.display().to_string();
        let v = user_message("look", "u", false, &[png.clone(), txt.clone()]);
        let blocks = v["message"]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[0]["source"]["data"], "iVBORw==");
        let text = blocks[1]["text"].as_str().unwrap();
        assert!(text.starts_with("look") && text.contains(&png) && text.contains(&txt));
        assert_eq!(
            user_message("plain", "u", true, &[])["message"]["content"],
            "plain"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A recording of the real CLI, when one is given
    /// (`DENT_WIRE_SAMPLE=path`): every line parses, and what dent types
    /// by kind comes out typed.
    #[test]
    fn a_real_recording_parses_by_kind() {
        let Ok(path) = std::env::var("DENT_WIRE_SAMPLE") else {
            return;
        };
        let text = std::fs::read_to_string(path).unwrap();
        let mut kinds = std::collections::BTreeMap::<String, usize>::new();
        for line in text.lines().filter(|l| l.starts_with('{')) {
            let k = match parse(line.as_bytes()).unwrap() {
                Inbound::LocalCommand {
                    name,
                    outcome,
                    view,
                    ..
                } => {
                    format!("command {name:?} {outcome:?} view={}", view.is_some())
                }
                Inbound::Compacted {
                    auto,
                    before,
                    after,
                } => {
                    format!("compacted auto={auto} {before}->{after:?}")
                }
                Inbound::Compacting { on, .. } => format!("compacting {on}"),
                Inbound::Retrying { .. } => "retrying".into(),
                Inbound::CommandsChanged(_) => "commands changed".into(),
                Inbound::Notice(t) => format!("notice {t}"),
                other => format!("{other:?}")
                    .split([' ', '(', '{'])
                    .next()
                    .unwrap()
                    .into(),
            };
            *kinds.entry(k).or_default() += 1;
        }
        for (k, n) in &kinds {
            eprintln!("{n:4}  {k}");
        }
    }

    #[test]
    fn long_output_is_cut_on_a_char_boundary() {
        let long = "é".repeat(OUTPUT_CAP);
        let out = output(&Value::String(long));
        assert!(out.len() <= OUTPUT_CAP + 4);
        assert!(out.ends_with('…'));
    }
}
