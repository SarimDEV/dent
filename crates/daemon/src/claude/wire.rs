//! The slice of Claude Code's stream-json protocol dent uses ([spike 01]).
//!
//! Every line parses into [`Inbound`]; anything unrecognised becomes
//! [`Inbound::Other`], so a new event type never breaks the driver.
//!
//! [spike 01]: ../../../docs/spikes/01-claude-code-stream-json.md

use serde::Deserialize;
use serde_json::{Value, json};

/// Tool output kept in the log: its start and its end, where a build or a
/// test run says how it went. The middle stays in the agent's transcript.
const HEAD: usize = 8 * 1024;
const TAIL: usize = 24 * 1024;
/// A subagent's report is its whole answer, in Markdown: kept far longer.
const REPORT_HEAD: usize = 56 * 1024;
const REPORT_TAIL: usize = 8 * 1024;

#[derive(Debug)]
pub enum Inbound {
    /// `system/init`: the start of every turn, including turns the agent
    /// starts itself after background work finishes.
    Init {
        session_id: String,
        version: String,
        permission_mode: String,
        /// Each MCP server and how it stands: connected, failed,
        /// needs-auth, pending, disabled.
        mcp: Vec<(String, String)>,
    },
    /// `system/background_tasks_changed`: the full current set.
    BackgroundTasks(Vec<BackgroundTask>),
    /// `system/task_started`: a task and the tool call that started it.
    TaskStarted {
        task_id: String,
        tool_use_id: Option<String>,
    },
    /// A new assistant message is starting to stream.
    MessageStart {
        parent: Option<String>,
    },
    /// Streamed text (needs `--include-partial-messages`).
    TextDelta {
        parent: Option<String>,
        text: String,
    },
    /// Streamed thinking.
    ThinkingDelta {
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
        /// Results of server tools (web search, web fetch, code execution),
        /// which come back in the same message as their calls.
        results: Vec<ToolResult>,
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
    #[serde(default)]
    pub task_type: String,
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
    /// Pictures in the result, as their `source` objects.
    pub images: Vec<Value>,
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
            mcp: v["mcp_servers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|m| (s(m, "name"), s(m, "status")))
                .collect(),
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
        // A hook turned your message away before it reached the agent: say
        // why in its own words, not with the hook's command and your prompt.
        ("system", "informational") if v["prompt_submit_outcome"]["kind"] == "blocked_by_hook" => {
            let reason = v["prompt_submit_outcome"]["reason"]
                .as_str()
                .map(str::trim)
                .filter(|r| !r.is_empty());
            Inbound::Notice(match reason {
                Some(r) => format!("A hook stopped this message before it reached Claude: {r}"),
                None => "A hook stopped this message before it reached Claude.".into(),
            })
        }
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
        ("system", "task_started") => Inbound::TaskStarted {
            task_id: s(&v, "task_id"),
            tool_use_id: v["tool_use_id"].as_str().map(str::to_owned),
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
                (Some("content_block_delta"), Some("thinking_delta")) => Inbound::ThinkingDelta {
                    parent: parent(&v),
                    text: s(&e["delta"], "thinking"),
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
            let results = m["content"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|b| {
                            b["type"]
                                .as_str()
                                .is_some_and(|t| t.ends_with("_tool_result"))
                                && b["tool_use_id"].is_string()
                        })
                        .map(|b| {
                            let (output, is_error) = server_output(&b["content"]);
                            ToolResult {
                                tool_use_id: s(b, "tool_use_id"),
                                is_error: is_error || b["is_error"] == true,
                                output,
                                images: Vec::new(),
                            }
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
                results,
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
                            images: images(&b["content"]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Inbound::ToolResults {
                parent: parent(&v),
                results,
            }
        }
        // One with no windows says nothing about them: logging it would
        // wipe the last reading.
        ("rate_limit_event", _) => {
            match serde_json::from_value::<RateLimitInfo>(v["rate_limit_info"].clone()) {
                Ok(info) if !info.unified_windows.is_empty() => Inbound::RateLimit(info),
                _ => Inbound::Other,
            }
        }
        ("control_request", _) if v["request"]["subtype"] == "can_use_tool" => {
            let r = &v["request"];
            Inbound::CanUseTool {
                request_id: s(&v, "request_id"),
                tool_name: s(r, "tool_name"),
                input: r["input"].clone(),
                description: r["description"].as_str().map(str::to_owned),
                // None offered is known, unlike a question rebuilt from an
                // older log (`null`).
                suggestions: match &r["permission_suggestions"] {
                    Value::Null => json!([]),
                    s => s.clone(),
                },
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
                .or_else(|| (!errors.is_empty()).then(|| errors.join("\n")))
                .or_else(|| match sub.as_str() {
                    "error_max_turns" => {
                        Some("Claude Code stopped: it reached its limit of turns".into())
                    }
                    "error_max_budget_usd" => {
                        Some("Claude Code stopped: it reached its spending limit".into())
                    }
                    "error_during_execution" => {
                        Some("Claude Code hit an error while working".into())
                    }
                    s if s.starts_with("error") => {
                        Some(format!("Claude Code stopped ({})", s.replace('_', " ")))
                    }
                    _ => None,
                });
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

/// A tool result's text, whole; [`keep`] cuts it for the log.
/// An image in it is named, so a result that was only a picture doesn't
/// read as empty.
fn output(content: &Value) -> String {
    match content {
        Value::String(t) => t.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// A result's pictures: their `source` objects, kept apart from the text.
fn images(content: &Value) -> Vec<Value> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["type"] == "image")
        .map(|p| p["source"].clone())
        .collect()
}

/// What a server tool's result says: the search hits, the page fetched,
/// the code's output, or the error. True when it failed.
fn server_output(content: &Value) -> (String, bool) {
    let one = |c: &Value| -> Option<String> {
        if let Some(t) = c["title"].as_str() {
            let url = c["url"].as_str().unwrap_or_default();
            return Some(format!("{t} — {url}").trim_end_matches(" — ").to_string());
        }
        for k in ["url", "text", "stdout", "content"] {
            if let Some(t) = c[k].as_str().filter(|t| !t.is_empty()) {
                return Some(t.to_string());
            }
        }
        None
    };
    match content {
        Value::Array(items) => (
            items.iter().filter_map(one).collect::<Vec<_>>().join("\n"),
            false,
        ),
        Value::Object(_)
            if content["type"]
                .as_str()
                .is_some_and(|t| t.ends_with("_error")) =>
        {
            let code = content["error_code"]
                .as_str()
                .or(content["type"].as_str())
                .unwrap_or("error");
            (code.to_string(), true)
        }
        Value::Object(_) => {
            let failed = content["return_code"].as_i64().is_some_and(|c| c != 0);
            let mut text = one(content).unwrap_or_default();
            if let Some(e) = content["stderr"].as_str().filter(|e| !e.is_empty()) {
                text = if text.is_empty() {
                    e.to_string()
                } else {
                    format!("{text}\n{e}")
                };
            }
            (text, failed)
        }
        other => (output(other), false),
    }
}

/// What of a tool's output goes in the log. Long output keeps its start
/// and its end, cut at line breaks where it can (so never inside a colour
/// code), with a line between them saying how much was left out. A
/// subagent's report (`report`) keeps far more.
pub fn keep(text: String, report: bool) -> String {
    let (head, tail) = if report {
        (REPORT_HEAD, REPORT_TAIL)
    } else {
        (HEAD, TAIL)
    };
    if text.len() <= head + tail {
        return text;
    }
    let head_end = cut_back(&text, head);
    let tail_start = cut_forward(&text, text.len() - tail);
    let gone = &text[head_end..tail_start];
    let lines = gone.matches('\n').count();
    let what = if lines > 0 {
        format!("{} line{}", group(lines), if lines == 1 { "" } else { "s" })
    } else {
        format!("{} characters", group(gone.chars().count()))
    };
    // A colour left on at the cut doesn't run into the line.
    let reset = if text.contains('\x1b') { "\x1b[0m" } else { "" };
    let mut out = String::with_capacity(head + tail + 64);
    out.push_str(text[..head_end].trim_end_matches('\n'));
    out.push_str(&format!("{reset}\n… {what} not kept …\n"));
    out.push_str(text[tail_start..].trim_start_matches('\n'));
    out
}

/// Where the kept start ends: after the last line break before `at` if
/// one is near, else at a character boundary outside any escape sequence
/// (before the one it falls in).
fn cut_back(text: &str, at: usize) -> usize {
    let mut end = at;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if let Some(nl) = text[..end].rfind('\n')
        && end - nl < 2048
    {
        return nl + 1;
    }
    crate::escapes::around(text, end).map_or(end, |r| r.start)
}

/// Where the kept end starts: after the first line break from `at` if one
/// is near, else at the next character boundary outside any escape
/// sequence (after the one it falls in), so the tail never starts inside
/// a colour code.
fn cut_forward(text: &str, at: usize) -> usize {
    let mut start = at;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    match text[start..].find('\n') {
        Some(nl) if nl < 2048 => start + nl + 1,
        _ => crate::escapes::around(text, start).map_or(start, |r| r.end),
    }
}

/// 18402 as "18,402".
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
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

/// Images the model reads directly, up to 5 MB. The kind is read from the
/// file's first bytes, not its name: a TIFF called `.png` sent as PNG would
/// be refused, and fail every turn after it too.
fn image_block(path: &str) -> Option<Value> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 5 * 1024 * 1024 {
        return None;
    }
    let media = picture_kind(&bytes)?;
    Some(json!({
        "type": "image",
        "source": { "type": "base64", "media_type": media, "data": dent_protocol::b64::encode(&bytes) },
    }))
}

/// A picture's media type from its first bytes, for the kinds the model reads.
pub(crate) fn picture_kind(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Whether the agent offered a rule it can save for this kind of call.
pub fn keeps(suggestions: &Value) -> bool {
    suggestions
        .as_array()
        .is_some_and(|a| a.iter().any(|s| s["type"] == "addRules"))
}

/// Allow a tool call. With `always`, the agent's own suggestions (or, when
/// it offered none, a rule for the tool) are sent back so the same kind of
/// call is allowed for the rest of the session. With `project` too, its
/// suggestions go to the project's local settings, where Claude Code
/// keeps, lists and enforces them from now on. When what it offered isn't
/// known (`null`: a question from before a restart, in an older log), the
/// call is allowed once: never more than the agent offered.
pub fn allow(
    request_id: &str,
    tool: &str,
    input: &Value,
    always: bool,
    project: bool,
    suggestions: &Value,
) -> Value {
    let always = (always || project) && !suggestions.is_null();
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
    fn a_picture_goes_as_what_its_bytes_say() {
        assert_eq!(picture_kind(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(picture_kind(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        // A TIFF, whatever it's called, isn't sent as a picture.
        assert_eq!(picture_kind(b"MM\0*\0\0\0\x08"), None);
        let dir = std::env::temp_dir().join(format!("dent-wire-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("shot.png");
        std::fs::write(&fake, b"II*\0\x08\0\0\0").unwrap();
        assert!(image_block(fake.to_str().unwrap()).is_none());
        let real = dir.join("real.jpg");
        std::fs::write(&real, b"\x89PNG\r\n\x1a\n").unwrap();
        let b = image_block(real.to_str().unwrap()).unwrap();
        assert_eq!(b["source"]["media_type"], "image/png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_message_a_hook_blocked_says_why() {
        // As Claude Code 2.1.301 sends it.
        let line = br#"{"type":"system","subtype":"informational","content":"UserPromptSubmit operation blocked by hook:\n[echo 'No secrets' >&2; exit 2]: No secrets\n\n\nOriginal prompt: hello","level":"warning","prevent_continuation":true,"prompt_submit_outcome":{"kind":"blocked_by_hook","prompt_kept":false,"reason":"No secrets"},"uuid":"u","session_id":"s"}"#;
        match parse(line).unwrap() {
            Inbound::Notice(t) => assert_eq!(
                t,
                "A hook stopped this message before it reached Claude: No secrets"
            ),
            _ => panic!("not a notice"),
        }
    }

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
        // A PNG's signature: what it is is read from its bytes.
        std::fs::write(&png, [137, 80, 78, 71, 13, 10, 26, 10]).unwrap();
        let txt = dir.join("notes.txt").display().to_string();
        let png = png.display().to_string();
        let v = user_message("look", "u", false, &[png.clone(), txt.clone()]);
        let blocks = v["message"]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[0]["source"]["data"], "iVBORw0KGgo=");
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
    fn server_tool_results_images_and_errors_say_something() {
        let a = br#"{"type":"assistant","message":{"content":[{"type":"server_tool_use","id":"srv1","name":"web_search","input":{"query":"dent"}},{"type":"web_search_tool_result","tool_use_id":"srv1","content":[{"type":"web_search_result","title":"Dent","url":"https://dent.dev"}]},{"type":"web_fetch_tool_result","tool_use_id":"srv2","content":{"type":"web_fetch_tool_error","error_code":"url_not_accessible"}},{"type":"text","text":"Found it."}]},"parent_tool_use_id":null}"#;
        match parse(a).unwrap() {
            Inbound::Assistant {
                blocks, results, ..
            } => {
                assert_eq!(blocks.len(), 2);
                assert_eq!(results.len(), 2);
                assert_eq!(results[0].tool_use_id, "srv1");
                assert_eq!(results[0].output, "Dent — https://dent.dev");
                assert!(!results[0].is_error);
                assert_eq!(results[1].output, "url_not_accessible");
                assert!(results[1].is_error);
            }
            other => panic!("{other:?}"),
        }
        let r = br#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"x"}}]}]}}"#;
        match parse(r).unwrap() {
            Inbound::ToolResults { results, .. } => {
                assert_eq!(results[0].output, "");
                assert_eq!(results[0].images.len(), 1);
            }
            other => panic!("{other:?}"),
        }
        let m = br#"{"type":"result","subtype":"error_max_turns","is_error":true}"#;
        match parse(m).unwrap() {
            Inbound::Result { text, is_error, .. } => {
                assert!(is_error);
                assert!(text.unwrap().contains("limit of turns"));
            }
            other => panic!("{other:?}"),
        }
        let e = br#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#;
        assert!(matches!(parse(e).unwrap(), Inbound::Other));
    }

    #[test]
    fn always_never_allows_more_than_was_offered() {
        let offered = json!([{"type": "addRules", "behavior": "allow", "destination": "session",
            "rules": [{"toolName": "Bash", "ruleContent": "npm test:*"}]}]);
        let r = allow("r", "Bash", &json!({}), true, true, &offered);
        let p = &r["response"]["response"]["updatedPermissions"];
        assert_eq!(p[0]["destination"], "localSettings");
        assert_eq!(p[0]["rules"][0]["ruleContent"], "npm test:*");
        // Nothing offered: the tool for this session.
        let r = allow("r", "Write", &json!({}), true, false, &json!([]));
        let p = &r["response"]["response"]["updatedPermissions"];
        assert_eq!(p[0]["rules"][0]["toolName"], "Write");
        // Not known what was offered: once.
        let r = allow("r", "Bash", &json!({}), true, true, &Value::Null);
        let resp = &r["response"]["response"];
        assert!(resp.get("updatedPermissions").is_none());
        assert_eq!(resp["decisionClassification"], "user_temporary");
    }

    #[test]
    fn long_output_keeps_its_start_and_end() {
        // Short output is kept whole.
        assert_eq!(keep("ok\n".into(), false), "ok\n");
        // Numbered lines: the first and the last are kept, the middle is
        // counted.
        let text: String = (1..=20000).map(|i| format!("{i}\n")).collect();
        let out = keep(text.clone(), false);
        assert!(out.len() <= HEAD + TAIL + 64);
        assert!(out.starts_with("1\n2\n"));
        assert!(out.ends_with("19999\n20000\n"));
        let marker = out.lines().find(|l| l.contains("not kept")).unwrap();
        let kept = out.lines().filter(|l| !l.contains("not kept")).count();
        let n: usize = marker
            .trim_matches(['…', ' '])
            .split(' ')
            .next()
            .unwrap()
            .replace(',', "")
            .parse()
            .unwrap();
        assert_eq!(kept + n, 20000, "{marker}");
        // Every kept line is whole.
        for l in out.lines().filter(|l| !l.contains("not kept")) {
            assert!(l.parse::<u32>().is_ok(), "{l:?}");
        }
        // A report keeps much more.
        let report = "word ".repeat(10_000);
        assert_eq!(keep(report.clone(), true), report);
    }

    #[test]
    fn a_cut_never_splits_a_character_or_an_escape() {
        // One long line: no line break to cut at.
        let long = "é".repeat(HEAD + TAIL);
        let out = keep(long, false);
        assert!(out.contains("characters not kept"));
        // Colour codes all the way: the cut lands before an escape, never
        // inside one, and colour is reset before the marker line.
        let coloured = "\x1b[31mred\x1b[0m ".repeat(HEAD);
        let out = keep(coloured, false);
        let (head, _) = out.split_once("\x1b[0m\n…").unwrap();
        assert!(
            head.ends_with(' ') || head.ends_with('m'),
            "{:?}",
            &head[head.len() - 8..]
        );
        let parsed_ok = |s: &str| !s.contains("\x1b[3\n") && !s.contains("\x1b\n");
        assert!(parsed_ok(&out));
        // The tail side too: it starts at an escape or between them, never
        // inside one, whatever the offset the cut first lands on.
        for pad in 0..12 {
            let coloured = format!(
                "{}{}",
                "x".repeat(pad),
                "\x1b[38;2;255;0;0mred ".repeat(HEAD)
            );
            let out = keep(coloured, false);
            let (_, tail) = out.split_once("not kept …\n").unwrap();
            assert!(
                tail.starts_with('\x1b') || tail.starts_with("red") || tail.starts_with(' '),
                "{pad}: {:?}",
                &tail[..12]
            );
        }
    }

    #[test]
    fn group_puts_in_thousands_separators() {
        assert_eq!(group(5), "5");
        assert_eq!(group(1234), "1,234");
        assert_eq!(group(18402), "18,402");
        assert_eq!(group(1_000_000), "1,000,000");
    }
}
