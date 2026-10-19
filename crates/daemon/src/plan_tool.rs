//! dent's own tools for the agent: a tiny MCP server, `dent-daemon
//! plan-tool`, that every Claude Code session gets. JSON-RPC over stdin and
//! stdout, a message a line.
//!
//! - `update_plan` takes the whole plan each time (`items`: `content` and
//!   `status`) and only says thanks; dent reads the plan from the call
//!   itself, as it streams by, and shows it above the composer.
//! - `show_visual` takes a page (`title`, `html`) and only says it showed;
//!   dent reads the page from the call and shows it beside the chat, so a
//!   chart or a mock-up needs no file in the project.
//! - `ask_secret` asks the person for a key or a password (`name`, `why`)
//!   through a private card. The daemon saves what they type where they
//!   choose and leaves an answer in `$DENT_ASKS/<name>.answer`; this server
//!   waits for it and tells the agent where the secret is, never what it is.

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

/// The tools' full names as the agent sees them, for allowing them up front.
pub const TOOL: &str = "mcp__dent__update_plan";
pub const VISUAL: &str = "mcp__dent__show_visual";
pub const SECRET: &str = "mcp__dent__ask_secret";

/// The `--mcp-config` that adds this server, run by `exe`; `asks` is the
/// folder its secret answers arrive in.
pub fn config(exe: &str, asks: &Path) -> String {
    json!({"mcpServers": {"dent": {
        "command": exe,
        "args": ["plan-tool"],
        "env": {"DENT_ASKS": asks.display().to_string()},
    }}})
    .to_string()
}

/// Serve until stdin closes. A secret's wait runs on its own thread, so
/// the agent's other calls (and pings) are answered meanwhile.
pub fn run() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let say = |out: &Mutex<std::io::Stdout>, reply: Value| {
        if let Ok(mut o) = out.lock() {
            let _ = writeln!(o, "{reply}");
            let _ = o.flush();
        }
    };
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if msg["method"] == "tools/call" && msg["params"]["name"] == "ask_secret" {
            let out = out.clone();
            std::thread::spawn(move || {
                let dir = std::env::var_os("DENT_ASKS").map(std::path::PathBuf::from);
                let text = ask_secret(dir.as_deref(), &msg["params"]["arguments"]);
                say(&out, called(msg["id"].clone(), &text));
            });
            continue;
        }
        if let Some(reply) = answer(&msg) {
            say(&out, reply);
        }
    }
    Ok(())
}

fn called(id: Value, text: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": text}]}})
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
        "tools/list" => json!({"tools": [
            {
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
            },
            {
                "name": "show_visual",
                "description": "Show the person a visual you made, as a web page beside the chat: a chart, a diagram, a mock-up, a comparison table. Give one whole HTML document (inline CSS and scripts; it may load libraries from a CDN). Nothing is written to the project. Use it when a picture explains better than words; call it again with the same title to show a new version.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "A few words naming it."},
                        "html": {"type": "string", "description": "The whole HTML document."},
                    },
                    "required": ["title", "html"],
                },
            },
            {
                "name": "ask_secret",
                "description": "Ask the person for a secret (an API key, a token, a password) through a private card, and wait for them. You never see the value: it is saved where they choose, either the project's .env file or a file of this session's, under the name you give, and you are told which file. Load it in commands from there (for example `set -a; . <file>; set +a`) and never print, echo or log it. Never ask for secrets in chat, and never invent one.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "The environment variable it's saved as, like STRIPE_SECRET_KEY."},
                        "why": {"type": "string", "description": "What you need it for, in a few words."},
                    },
                    "required": ["name", "why"],
                },
            },
        ]}),
        "tools/call" => {
            let text = match msg["params"]["name"].as_str() {
                Some("show_visual") => "Shown beside the chat.",
                // A secret is answered by `run`, on its own thread.
                Some("ask_secret") => "dent can't ask for secrets here.",
                _ => "Plan updated.",
            };
            return Some(called(id, text));
        }
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

/// A secret's name as a file name: letters, digits, `_` and `-`, at most 64.
pub fn secret_key(name: &str) -> String {
    let key: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .take(64)
        .collect();
    if key.is_empty() { "SECRET".into() } else { key }
}

/// Wait for the person's answer to `ask_secret` and say what came of it.
fn ask_secret(dir: Option<&Path>, args: &Value) -> String {
    let Some(dir) = dir else {
        return "dent can't ask for secrets here.".into();
    };
    let name = secret_key(args["name"].as_str().unwrap_or_default());
    let file = dir.join(format!("{name}.answer"));
    let _ = std::fs::remove_file(&file);
    loop {
        if let Ok(s) = std::fs::read_to_string(&file)
            && let Ok(v) = serde_json::from_str::<Value>(&s)
        {
            let _ = std::fs::remove_file(&file);
            return said(&name, &v);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// What the agent hears of an answer: where the secret is, or that none
/// came.
pub fn said(name: &str, answer: &Value) -> String {
    match answer["saved"].as_str() {
        Some(path) => {
            let mut s = format!(
                "The person saved {name} in {path}. Load it in commands from there (`set -a; . '{path}'; set +a`) or let the project's tools read it; never print, echo or log its value."
            );
            if answer["ignored"] == true {
                s.push_str(" dent added .env to .gitignore, so it's never committed.");
            }
            s
        }
        None => format!(
            "The person chose not to give {name}. Don't ask for it again this turn: carry on without it, or say what it blocks."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_lists_its_tools_and_answers_calls() {
        let init = answer(
            &json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
        );
        assert_eq!(
            init.as_ref().map(|r| &r["result"]["protocolVersion"]),
            Some(&json!("2025-06-18"))
        );
        // Notifications get no reply.
        assert!(answer(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        let list = answer(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(names, ["update_plan", "show_visual", "ask_secret"]);
        let call = answer(
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"update_plan","arguments":{"items":[]}}}),
        );
        assert_eq!(call.as_ref().map(|r| &r["id"]), Some(&json!(2)));
        let other = answer(&json!({"jsonrpc":"2.0","id":3,"method":"resources/list"}));
        assert!(other.is_some_and(|r| r.get("error").is_some()));
    }

    #[test]
    fn a_secret_waits_for_its_answer_and_never_says_it() {
        let dir = std::env::temp_dir().join(format!("dent-asks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.clone();
        let waiting = std::thread::spawn(move || {
            ask_secret(Some(&d), &json!({"name": "API KEY!", "why": "tests"}))
        });
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(
            dir.join("APIKEY.answer"),
            json!({"saved": "/p/.env", "ignored": true}).to_string(),
        )
        .unwrap();
        let said = waiting.join().unwrap();
        assert!(said.contains("saved APIKEY in /p/.env"), "{said}");
        assert!(said.contains(".gitignore"), "{said}");
        assert!(!dir.join("APIKEY.answer").exists());
        assert!(super::said("K", &json!({"declined": true})).contains("chose not to give K"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
