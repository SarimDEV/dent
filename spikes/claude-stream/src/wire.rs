//! The slice of Claude Code's stream-json protocol dent cares about.
//!
//! Every line is parsed into [`Inbound`]; anything unrecognised becomes
//! [`Inbound::Other`] so a new event type never breaks the driver.

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug)]
pub enum Inbound {
    /// `system/init`: emitted at the start of every turn, including turns the
    /// agent starts itself after background work finishes.
    Init {
        session_id: String,
        model: String,
        version: String,
        permission_mode: String,
    },
    /// `system/status`, e.g. `requesting`.
    Status(String),
    /// `system/background_tasks_changed`: the full current set of background tasks.
    BackgroundTasks(Vec<BackgroundTask>),
    /// `system/task_notification`: a background task finished.
    TaskFinished {
        task_id: String,
        summary: String,
    },
    /// `stream_event` text delta (needs `--include-partial-messages`).
    TextDelta(String),
    /// A complete assistant message (text and tool calls).
    Assistant {
        parent_tool_use_id: Option<String>,
        blocks: Vec<AssistantBlock>,
    },
    /// Tool results come back as `user` messages.
    ToolResults(Vec<ToolResult>),
    RateLimit(RateLimitInfo),
    /// `control_request/can_use_tool`: the agent is blocked on us.
    CanUseTool {
        request_id: String,
        tool_name: String,
        input: Value,
        description: Option<String>,
    },
    ControlResponse {
        request_id: String,
        ok: bool,
        body: Value,
    },
    /// End of a turn.
    Result {
        subtype: String,
        terminal_reason: Option<String>,
        is_error: bool,
        text: Option<String>,
    },
    Other {
        kind: String,
    },
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
    Thinking,
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
    pub preview: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitInfo {
    pub status: String,
    pub rate_limit_type: Option<String>,
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

pub fn parse(line: &str) -> serde_json::Result<Inbound> {
    let v: Value = serde_json::from_str(line)?;
    let kind = s(&v, "type");
    let sub = s(&v, "subtype");
    Ok(match (kind.as_str(), sub.as_str()) {
        ("system", "init") => Inbound::Init {
            session_id: s(&v, "session_id"),
            model: s(&v, "model"),
            version: s(&v, "claude_code_version"),
            permission_mode: s(&v, "permissionMode"),
        },
        ("system", "status") => Inbound::Status(s(&v, "status")),
        ("system", "background_tasks_changed") => {
            Inbound::BackgroundTasks(serde_json::from_value(v["tasks"].clone()).unwrap_or_default())
        }
        ("system", "task_notification") => Inbound::TaskFinished {
            task_id: s(&v, "task_id"),
            summary: s(&v, "summary"),
        },
        ("stream_event", _) => {
            let e = &v["event"];
            match e["delta"]["type"].as_str() {
                Some("text_delta") if e["type"] == "content_block_delta" => {
                    Inbound::TextDelta(s(&e["delta"], "text"))
                }
                _ => Inbound::Other {
                    kind: format!("stream_event/{}", s(e, "type")),
                },
            }
        }
        ("assistant", _) => {
            let blocks = v["message"]["content"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|b| match b["type"].as_str()? {
                            "text" => Some(AssistantBlock::Text(s(b, "text"))),
                            "thinking" | "redacted_thinking" => Some(AssistantBlock::Thinking),
                            "tool_use" => Some(AssistantBlock::ToolUse {
                                id: s(b, "id"),
                                name: s(b, "name"),
                                input: b["input"].clone(),
                            }),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let parent_tool_use_id = v["parent_tool_use_id"].as_str().map(str::to_owned);
            Inbound::Assistant {
                parent_tool_use_id,
                blocks,
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
                            preview: preview(&b["content"]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Inbound::ToolResults(results)
        }
        ("rate_limit_event", _) => match serde_json::from_value(v["rate_limit_info"].clone()) {
            Ok(info) => Inbound::RateLimit(info),
            Err(_) => Inbound::Other { kind },
        },
        ("control_request", _) if v["request"]["subtype"] == "can_use_tool" => {
            let r = &v["request"];
            Inbound::CanUseTool {
                request_id: s(&v, "request_id"),
                tool_name: s(r, "tool_name"),
                input: r["input"].clone(),
                description: r["description"].as_str().map(str::to_owned),
            }
        }
        ("control_response", _) => {
            let r = &v["response"];
            Inbound::ControlResponse {
                request_id: s(r, "request_id"),
                ok: r["subtype"] == "success",
                body: if r["subtype"] == "success" {
                    r["response"].clone()
                } else {
                    r["error"].clone()
                },
            }
        }
        ("result", _) => Inbound::Result {
            subtype: sub,
            terminal_reason: v["terminal_reason"].as_str().map(str::to_owned),
            is_error: v["is_error"].as_bool().unwrap_or(false),
            text: v["result"].as_str().map(str::to_owned),
        },
        _ => Inbound::Other {
            kind: if sub.is_empty() {
                kind
            } else {
                format!("{kind}/{sub}")
            },
        },
    })
}

fn preview(content: &Value) -> String {
    let text = match content {
        Value::String(t) => t.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    let first = text.lines().next().unwrap_or_default();
    let mut out: String = first.chars().take(100).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

// ---- outbound ----

pub fn user_message(text: &str) -> Value {
    json!({
        "type": "user",
        "message": { "role": "user", "content": text },
        "parent_tool_use_id": null,
        "session_id": "",
    })
}

pub fn control_request(request_id: &str, subtype: &str, mut extra: Value) -> Value {
    extra["subtype"] = json!(subtype);
    json!({ "type": "control_request", "request_id": request_id, "request": extra })
}

pub fn allow(request_id: &str, input: &Value) -> Value {
    control_success(
        request_id,
        json!({ "behavior": "allow", "updatedInput": input }),
    )
}

pub fn deny(request_id: &str, reason: &str) -> Value {
    control_success(request_id, json!({ "behavior": "deny", "message": reason }))
}

fn control_success(request_id: &str, response: Value) -> Value {
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": response },
    })
}
