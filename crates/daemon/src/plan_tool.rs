//! The agent's plan tool: a tiny MCP server, `dent-daemon plan-tool`, that
//! every Claude Code session gets. Its one tool, `update_plan`, takes the
//! whole plan each time (`items`: `content` and `status`) and only says
//! thanks; dent reads the plan from the call itself, as it streams by, and
//! shows it above the composer. JSON-RPC over stdin and stdout, a message a
//! line.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// The tool's full name as the agent sees it, for allowing it up front.
pub const TOOL: &str = "mcp__dent__update_plan";

/// The `--mcp-config` that adds this server, run by `exe`.
pub fn config(exe: &str) -> String {
    json!({"mcpServers": {"dent": {"command": exe, "args": ["plan-tool"]}}}).to_string()
}

/// Serve until stdin closes.
pub fn run() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(reply) = answer(&msg) {
            writeln!(out, "{reply}")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// The reply to one message; notifications get none.
fn answer(msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let result = match msg["method"].as_str().unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "dent", "version": env!("CARGO_PKG_VERSION")},
        }),
        "tools/list" => json!({"tools": [{
            "name": "update_plan",
            "description": "Show your plan for the current task to the person you're working with, as a checklist. Send the whole plan every time: one short line per step, each pending, in_progress or completed. Call it before you start a task with three or more steps, and again whenever a step starts or finishes.",
            "inputSchema": {
                "type": "object",
                "properties": {"items": {"type": "array", "items": {
                    "type": "object",
                    "properties": {
                        "content": {"type": "string", "description": "The step, in a few words."},
                        "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                    },
                    "required": ["content", "status"],
                }}},
                "required": ["items"],
            },
        }]}),
        "tools/call" => json!({"content": [{"type": "text", "text": "Plan updated."}]}),
        "ping" => json!({}),
        _ => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "method not found"},
            }));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_lists_one_tool_and_answers_calls() {
        let init = answer(
            &json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
        );
        assert_eq!(
            init.as_ref().map(|r| &r["result"]["protocolVersion"]),
            Some(&json!("2025-06-18"))
        );
        // Notifications get no reply.
        assert!(answer(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        let list = answer(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
        assert_eq!(
            list.as_ref().map(|r| &r["result"]["tools"][0]["name"]),
            Some(&json!("update_plan"))
        );
        let call = answer(
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"update_plan","arguments":{"items":[]}}}),
        );
        assert_eq!(call.as_ref().map(|r| &r["id"]), Some(&json!(2)));
        let other = answer(&json!({"jsonrpc":"2.0","id":3,"method":"resources/list"}));
        assert!(other.is_some_and(|r| r.get("error").is_some()));
    }
}
