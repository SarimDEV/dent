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
    /// A complete assistant message.
    Assistant {
        parent: Option<String>,
        uuid: Option<String>,
        blocks: Vec<AssistantBlock>,
        /// Input plus cache tokens: how full the context is.
        context: Option<u64>,
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
    /// The agent no longer needs an answer (after an interrupt, say).
    CancelRequest {
        request_id: String,
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
        let names: Vec<String> = attachments.iter().map(|p| format!("Attached: {p}")).collect();
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

/// Allow a tool call. With `always`, the agent's own suggestions (or a rule
/// for the tool) are sent back so the same kind of call is allowed for the
/// rest of the session.
pub fn allow(
    request_id: &str,
    tool: &str,
    input: &Value,
    always: bool,
    suggestions: &Value,
) -> Value {
    let mut r = json!({
        "behavior": "allow",
        "updatedInput": input,
        "decisionClassification": if always { "user_permanent" } else { "user_temporary" },
    });
    if always {
        let rules: Vec<Value> = suggestions
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|s| s["type"] == "addRules")
                    .map(|s| {
                        let mut s = s.clone();
                        s["destination"] = json!("session");
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
        assert_eq!(user_message("plain", "u", true, &[])["message"]["content"], "plain");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_output_is_cut_on_a_char_boundary() {
        let long = "é".repeat(OUTPUT_CAP);
        let out = output(&Value::String(long));
        assert!(out.len() <= OUTPUT_CAP + 4);
        assert!(out.ends_with('…'));
    }
}
