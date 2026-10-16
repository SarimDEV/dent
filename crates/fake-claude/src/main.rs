//! Stands in for `claude -p` stream-json in tests and demos, so nothing
//! touches a real account. It speaks the slice of the protocol dent uses
//! (initialize, user messages, approvals, questions, interrupts, background
//! work, rate limits, usage) with the shapes recorded in spike 01.
//!
//! What it does depends on words in the message:
//!
//! - `run` or `approve`: a Bash call that needs approval
//! - `present a plan`: a plan to approve, as plan mode ends (`long`: a plan
//!   of 30 steps)
//! - `ask`: a question with choices (`ask several`: more than one answer)
//! - `write <path>`: writes the file in its directory (asks first in Ask mode)
//! - `background`: starts background work, ends the turn, then follows up
//! - `subagent`: a Task call whose steps nest under it
//! - `dev server`: serves a page on a free port and prints its address
//! - `long`: a long reply, for stopping halfway (`slowly`: ten times slower;
//!   with `make a plan`, each step holds ten times longer)
//! - `colour output`: a Bash call whose output is coloured, with a
//!   progress line and one very long line
//! - `half-typed`: a reply that stops for a while each time bold, a link,
//!   a table and a list are half typed
//! - `fail`: the turn errors
//! - `hit the limit`: the 5-hour usage limit is used up and the turn fails
//! - `crash`: the process exits mid-turn
//! - `your args`: replies with its command line
//! - `retry`: the API fails twice and is retried before it answers
//! - `declined`: the model declines; Claude Code asks whether to retry
//! - `/compact`, `/context`, `/usage`, `/config`, `/autocompact`,
//!   `/output-style`, `/advisor`: answered as Claude Code answers them
//!   itself, with no model turn; `/theme` is terminal-only, and an unknown
//!   command gets a suggestion
//! - `/status`, `/mcp`, `/skills`, `/memory`, `/hooks`, `/permissions`:
//!   refused as dialogs, with their control requests answered; the MCP
//!   servers turn on and off, reconnect (except `figma`) and sign in
//! - anything else: a short Markdown reply
//!
//! `FAKE_CLAUDE_SPEED` scales every pause (0 makes it instant);
//! `FAKE_CLAUDE_LINGER_MS` keeps it running that long after its input
//! closes. `set_model` refuses models named `unavailable…`.
//! `FAKE_CLAUDE_DEAF` answers interrupts but carries on regardless.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

struct Agent {
    rx: Receiver<Value>,
    session: String,
    model: String,
    mode: String,
    speed: f64,
    queue: VecDeque<String>,
    stdin_open: bool,
    sent_limits: bool,
    next: u64,
    context: u64,
    /// A steered message arrived during the current turn.
    steer: bool,
    /// MCP servers: name, status, scope.
    mcp: Vec<(String, String, String)>,
    /// How long `say` waits between chunks, unscaled.
    chunk_ms: u64,
}

/// Why a turn stopped early.
enum Cut {
    Interrupted,
    /// A message sent with `priority: now` cut the request short.
    Steered,
    Closed,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    let mut a = Agent {
        rx,
        // A fork keeps the conversation under a new id.
        session: if args.iter().any(|a| a == "--fork-session") {
            new_id()
        } else {
            arg("--resume")
                .or_else(|| arg("--session-id"))
                .unwrap_or_else(new_id)
        },
        mcp: [
            ("docs", "connected", "user"),
            ("tickets", "needs-auth", "project"),
            ("figma", "failed", "user"),
            ("old-search", "disabled", "user"),
            ("claude.ai Calendar", "needs-auth", "claudeai"),
        ]
        .iter()
        .map(|(n, st, sc)| (n.to_string(), st.to_string(), sc.to_string()))
        .collect(),
        model: arg("--model").unwrap_or_else(|| "sonnet".into()),
        mode: arg("--permission-mode").unwrap_or_else(|| "default".into()),
        speed: std::env::var("FAKE_CLAUDE_SPEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1.0),
        queue: VecDeque::new(),
        stdin_open: true,
        sent_limits: false,
        next: 0,
        context: 18_000,
        steer: false,
        chunk_ms: 40,
    };
    // One-off: `-p "prompt"` without stream-json input answers and exits.
    if !args.iter().any(|x| x == "--input-format")
        && let Some(prompt) = arg("-p").filter(|p| !p.starts_with("--"))
    {
        let _ = a.turn(&prompt);
        return;
    }
    // Claude Code stuck before it starts (a lock held elsewhere, say).
    if std::env::var_os("FAKE_CLAUDE_HANG").is_some() {
        eprintln!("waiting for a lock");
        loop {
            std::thread::park();
        }
    }
    // Claude Code with no record of the conversation it was asked to resume.
    if std::env::var_os("FAKE_CLAUDE_FORGET").is_some() && arg("--resume").is_some() {
        while let Ok(v) = a.rx.recv() {
            if v["type"] == "user" {
                break;
            }
        }
        let id = arg("--resume").unwrap_or_default();
        out(
            json!({"type": "result", "subtype": "error_during_execution", "is_error": true,
            "num_turns": 0, "session_id": id,
            "errors": [format!("No conversation found with session ID: {id}")]}),
        );
        eprintln!("No conversation found with session ID: {id}");
        std::process::exit(1);
    }
    loop {
        if let Some(text) = a.queue.pop_front() {
            if a.turn(&text).is_err() && !a.stdin_open {
                return;
            }
            continue;
        }
        if !a.stdin_open {
            // Winding down after its input closed, as the real CLI takes a
            // moment to.
            if let Some(ms) = std::env::var("FAKE_CLAUDE_LINGER_MS")
                .ok()
                .and_then(|s| s.parse().ok())
            {
                std::thread::sleep(Duration::from_millis(ms));
            }
            return;
        }
        match a.rx.recv() {
            Ok(v) => {
                a.handle(v);
            }
            Err(_) => a.stdin_open = false,
        }
    }
}

/// Where Claude Code keeps a project's own rules, not committed.
fn local_settings() -> std::path::PathBuf {
    std::path::Path::new(".claude").join("settings.local.json")
}

/// The allow rules saved in the project's local settings.
fn local_rules() -> Vec<String> {
    std::fs::read(local_settings())
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["permissions"]["allow"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.as_str().map(String::from))
        .collect()
}

fn save_local_rules(rules: &[String]) {
    let _ = std::fs::create_dir_all(".claude");
    let _ = std::fs::write(
        local_settings(),
        serde_json::to_vec_pretty(&json!({"permissions": {"allow": rules}})).unwrap_or_default(),
    );
}

fn new_id() -> String {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{:08x}-fake-4000-8000-{:012x}",
        (n >> 32) as u32,
        (n as u64) & 0xffff_ffff_ffff
    )
}

fn out(v: Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

impl Agent {
    /// Handle one line from stdin outside a turn's own waits. Returns the
    /// request id when it was a control response (an answer).
    fn handle(&mut self, v: Value) -> Option<(String, Value)> {
        match v["type"].as_str() {
            Some("user") => {
                let text = match &v["message"]["content"] {
                    Value::String(s) => s.clone(),
                    Value::Array(parts) => parts
                        .iter()
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                if v["priority"] == "now" {
                    self.queue.push_front(text);
                    self.steer = true;
                } else {
                    self.queue.push_back(text);
                }
                None
            }
            Some("control_request") => {
                let id = v["request_id"].as_str().unwrap_or_default().to_string();
                let sub = v["request"]["subtype"].as_str().unwrap_or_default();
                let body = match sub {
                    "initialize" => json!({
                        "commands": [
                            {"name": "compact", "description": "Clear history but keep a summary in context", "argumentHint": "<instructions>"},
                            {"name": "context", "description": "Show current context usage", "argumentHint": ""},
                            {"name": "model", "description": "Set the AI model for Claude Code", "argumentHint": "<model>"},
                            {"name": "color", "description": "Set the prompt bar color for this session", "argumentHint": ""},
                            {"name": "review", "description": "Review a pull request", "argumentHint": ""},
                            {"name": "deep-research", "description": "Research a question across many sources", "argumentHint": "<question>"},
                            {"name": "autocompact", "description": "Configure the auto-compact window size", "argumentHint": "[auto|<tokens>]"},
                            {"name": "config", "description": "Set a setting by key", "argumentHint": "key=value"},
                            {"name": "output-style", "description": "List output styles or switch to one", "argumentHint": "[style]"},
                            {"name": "advisor", "description": "Let Claude consult a stronger model at key moments", "argumentHint": "[fable|opus|sonnet|off]"},
                            {"name": "usage", "description": "Show session cost, plan usage, and what's contributing to your limits", "argumentHint": ""},
                            {"name": "theme", "description": "Change the theme", "argumentHint": ""},
                            {"name": "status", "description": "Show Claude Code status", "argumentHint": ""},
                            {"name": "mcp", "description": "Manage MCP servers", "argumentHint": "[reconnect|enable|disable [<server>|all]]"},
                            {"name": "skills", "description": "List available skills", "argumentHint": ""},
                            {"name": "memory", "description": "Edit Claude memory files", "argumentHint": ""},
                            {"name": "hooks", "description": "View hook configurations", "argumentHint": ""},
                            {"name": "permissions", "description": "Manage allow & deny tool permission rules", "argumentHint": ""},
                        ],
                        "output_style": "default",
                        "available_output_styles": ["default", "Concise", "Explanatory", "Learning"],
                        "agents": [],
                        // The shape Claude Code 2.1 sends: aliases first, then
                        // pinned older versions, each with its exact model.
                        "models": [
                            {"value": "default", "resolvedModel": "claude-sonnet-5-5", "displayName": "Default (recommended)", "description": "Sonnet 5.5 · Efficient for routine tasks", "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]},
                            {"value": "opus", "resolvedModel": "claude-opus-5-5", "displayName": "Opus 5.5", "description": "For complex work and everyday tasks", "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]},
                            {"value": "fable", "resolvedModel": "claude-fable-5-1", "displayName": "Fable 5.1", "description": "For your toughest challenges", "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]},
                            {"value": "sonnet", "resolvedModel": "claude-sonnet-5-5", "displayName": "Sonnet 5.5", "description": "Most efficient for simpler tasks", "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]},
                            {"value": "haiku", "resolvedModel": "claude-haiku-5-5", "displayName": "Haiku 5.5", "description": "Fastest for quick answers", "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]},
                            {"value": "claude-haiku-4-5-20251001", "resolvedModel": "claude-haiku-4-5-20251001", "displayName": "Haiku 4.5", "description": "Fastest for quick answers"},
                            {"value": "claude-opus-4-6", "resolvedModel": "claude-opus-4-6", "displayName": "Opus 4.6", "description": "Best for everyday, complex tasks", "supportedEffortLevels": ["low", "medium", "high", "max"]},
                            {"value": "claude-sonnet-4-6", "resolvedModel": "claude-sonnet-4-6", "displayName": "Sonnet 4.6", "description": "Efficient for routine tasks", "supportedEffortLevels": ["low", "medium", "high", "max"]},
                        ],
                        "pid": std::process::id(),
                        "claude_code_version": "0.0.0-fake",
                        "current_permission_mode": self.mode,
                    }),
                    "set_model" => {
                        let model = v["request"]["model"].as_str().unwrap_or("sonnet");
                        // A model this account can't use is refused.
                        if model.starts_with("unavailable") {
                            out(json!({"type": "control_response", "response": {
                                "subtype": "error", "request_id": id,
                                "error": format!("Model {model} is not available")}}));
                            return None;
                        }
                        self.model = model.into();
                        json!({})
                    }
                    "set_permission_mode" => {
                        self.mode = v["request"]["mode"].as_str().unwrap_or("default").into();
                        json!({})
                    }
                    "interrupt" => json!({ "still_queued": [] }),
                    "get_usage" => json!({"rate_limits": {
                        "five_hour": {"utilization": 18, "resets_at": "2030-01-01T00:00:00+00:00"},
                        "seven_day": {"utilization": 41, "resets_at": "2030-01-05T00:00:00+00:00"}}}),
                    "get_context_usage" => {
                        let messages = self.context.saturating_sub(15_400);
                        json!({ "totalTokens": self.context, "maxTokens": 200_000, "percentage": self.context / 2000,
                            "autoCompactThreshold": 167_000, "isAutoCompactEnabled": true,
                            "categories": [
                                {"name": "System prompt", "tokens": 3_200, "kind": "used"},
                                {"name": "System tools", "tokens": 6_300, "kind": "used"},
                                {"name": "Skills", "tokens": 5_900, "kind": "used"},
                                {"name": "Messages", "tokens": messages, "kind": "used"},
                                {"name": "MCP tools (deferred)", "tokens": 3_800, "kind": "deferred", "isDeferred": true},
                                {"name": "Free space", "tokens": 200_000u64.saturating_sub(self.context), "kind": "free"},
                                {"name": "Autocompact buffer", "tokens": 33_000, "kind": "buffer"}]})
                    }
                    "get_status" => json!({"sections": [
                        {"title": "Session", "rows": [
                            {"label": "Version", "value": "0.0.0-fake"},
                            {"label": "Session ID", "value": self.session},
                            {"label": "cwd", "value": std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()},
                            {"label": "Login method", "value": "Claude Max account"}]},
                        {"title": "Environment", "rows": [
                            {"label": "Model", "value": self.model},
                            {"label": "MCP servers", "value": "1 connected · 2 need sign-in · /mcp"},
                            {"label": "Setting sources", "value": "User settings, Project settings"}]}]}),
                    "mcp_status" => json!({"mcpServers": self.mcp.iter().map(|(n, st, sc)| {
                        let mut v = json!({"name": n, "status": st, "scope": sc, "source": sc,
                            "config": {"type": "stdio", "command": "npx", "args": ["-y", n, "--api-key", "fake-secret"]}});
                        if st == "connected" {
                            v["tools"] = json!([{"name": "search"}, {"name": "fetch"}, {"name": "list"}]);
                            v["serverInfo"] = json!({"name": n, "description": "Up-to-date documentation for libraries. Fetched live."});
                        }
                        if st == "failed" {
                            v["error"] = json!("Connection closed: the server exited (code 1)");
                        }
                        v
                    }).collect::<Vec<_>>()}),
                    "mcp_toggle" | "mcp_reconnect" | "mcp_authenticate" => {
                        let name = v["request"]["serverName"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        let Some(server) = self.mcp.iter_mut().find(|m| m.0 == name) else {
                            out(json!({"type": "control_response", "response": {
                                "subtype": "error", "request_id": id, "error": format!("Server not found: {name}")}}));
                            return None;
                        };
                        std::thread::sleep(Duration::from_millis((600.0 * self.speed) as u64));
                        match sub {
                            "mcp_toggle" => {
                                server.1 = if v["request"]["enabled"] == true {
                                    "connected"
                                } else {
                                    "disabled"
                                }
                                .into();
                                json!({})
                            }
                            "mcp_reconnect" if name == "figma" => {
                                out(json!({"type": "control_response", "response": {
                                    "subtype": "error", "request_id": id,
                                    "error": "figma: Connection closed: the server exited (code 1)"}}));
                                return None;
                            }
                            "mcp_reconnect" => {
                                server.1 = "connected".into();
                                json!({})
                            }
                            // Signing in happens elsewhere; the fake finishes
                            // it at once rather than opening a browser.
                            _ => {
                                server.1 = "connected".into();
                                json!({"requiresUserAction": false, "callbackExpected": false})
                            }
                        }
                    }
                    "get_skills_dialog" => json!({"skills": [
                        {"name": "dataviz", "display_name": "dataviz", "description": "Use this skill whenever you are about to create any chart. It covers palettes and axes.", "source": "plugin", "tokens": 227, "state": "on", "locked_by": "plugin", "advertised": true},
                        {"name": "deep-research", "display_name": "deep-research", "description": "Research a topic across many sources and write a cited report.", "source": "claude.ai sync", "tokens": 200, "state": "on", "advertised": true},
                        {"name": "release-notes", "display_name": "release-notes", "description": "Draft release notes from the commits since the last tag.", "source": "project", "tokens": 140, "state": "off", "advertised": false}]}),
                    "get_memory_dialog" => {
                        let cwd = std::env::current_dir()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        json!({"files": [
                            {"kind": "user", "path": "/nonexistent/.claude/CLAUDE.md", "label": "User instructions", "description": "Saved in ~/.claude/CLAUDE.md", "exists": false},
                            {"kind": "project", "path": format!("{cwd}/README.md"), "label": "Project instructions", "description": "Saved in ./CLAUDE.md", "exists": true}],
                            "folders": [{"kind": "auto", "path": format!("{cwd}/"), "label": "Open auto-memory folder", "description": ""}],
                            "memories": [], "auto_memory": {"enabled": true, "toggleable": true, "status": "on"},
                            "auto_dream": {"shown": false}})
                    }
                    "get_hooks_listing" => json!({
                        "events": [
                            {"name": "PreToolUse", "summary": "Before tool execution", "supportsMatcher": true, "hookCount": 1},
                            {"name": "PostToolUse", "summary": "After tool execution", "supportsMatcher": true, "hookCount": 0},
                            {"name": "Stop", "summary": "Right before Claude concludes its response", "supportsMatcher": false, "hookCount": 1}],
                        "hooks": [
                            {"event": "PreToolUse", "matcher": "Bash", "source": "projectSettings", "sourceLabel": "Project settings (.claude/settings.json)", "type": "command", "displayText": "./scripts/check-command.sh"},
                            {"event": "Stop", "matcher": "", "source": "userSettings", "sourceLabel": "User settings (~/.claude/settings.json)", "type": "command", "displayText": "say done"}],
                        "eventCatalog": [], "policy": {"allDisabled": false, "disabledByPolicy": false}}),
                    "list_permission_rules" => {
                        json!({"state": {"rules": local_rules().into_iter().map(|r| json!(
                        {"behavior": "allow", "rule": r, "source": "localSettings", "editability": "editable"})).chain([
                        json!({"behavior": "allow", "rule": "Bash(cargo test:*)", "source": "projectSettings", "editability": "editable"}),
                        json!({"behavior": "allow", "rule": "WebFetch(domain:docs.rs)", "source": "userSettings", "editability": "editable"}),
                        json!({"behavior": "deny", "rule": "Bash(rm -rf:*)", "source": "userSettings", "editability": "editable"})]).collect::<Vec<_>>(),
                        "workspaceDirectories": [], "managedOnly": false}})
                    }
                    "get_settings" => json!({
                        "effective": {"autoCompact": true, "theme": "dark", "outputStyle": "default"},
                        "applied": {"model": self.model, "effort": null, "advisor": null}}),
                    _ => {
                        out(json!({"type": "control_response", "response": {
                            "subtype": "error", "request_id": id, "error": format!("Unsupported control request subtype: {sub}")}}));
                        return None;
                    }
                };
                out(json!({"type": "control_response", "response": {
                    "subtype": "success", "request_id": id, "response": body}}));
                None
            }
            Some("control_response") => {
                let r = &v["response"];
                Some((
                    r["request_id"].as_str().unwrap_or_default().to_string(),
                    r["response"].clone(),
                ))
            }
            _ => None,
        }
    }

    /// Pause for `ms` (scaled), still answering stdin. An interrupt cuts the
    /// turn; a closed stdin doesn't.
    fn pause(&mut self, ms: u64) -> Result<(), Cut> {
        let until = Instant::now() + Duration::from_millis((ms as f64 * self.speed) as u64);
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() && self.speed > 0.0 {
                return Ok(());
            }
            if !self.stdin_open {
                // Nothing more can arrive; finish the turn as the real CLI does.
                std::thread::sleep(left);
                return Ok(());
            }
            match self.rx.recv_timeout(left) {
                Ok(v) => {
                    let interrupt =
                        v["type"] == "control_request" && v["request"]["subtype"] == "interrupt";
                    self.handle(v);
                    if interrupt && std::env::var_os("FAKE_CLAUDE_DEAF").is_none() {
                        return Err(Cut::Interrupted);
                    }
                    if std::mem::take(&mut self.steer) {
                        return Err(Cut::Steered);
                    }
                }
                Err(RecvTimeoutError::Timeout) => return Ok(()),
                Err(RecvTimeoutError::Disconnected) => self.stdin_open = false,
            }
            if self.speed == 0.0 {
                return Ok(());
            }
        }
    }

    /// Ask for permission and wait for the answer. A command comes with a
    /// rule it could save, as Claude Code suggests one; other tools don't.
    fn permission(&mut self, tool: &str, tool_id: &str, input: &Value) -> Result<Value, Cut> {
        let suggestions = match input["command"].as_str() {
            Some(cmd) if tool == "Bash" => json!([{"type": "addRules", "behavior": "allow",
                "destination": "session", "rules": [{"toolName": "Bash", "ruleContent": cmd}]}]),
            _ => json!([]),
        };
        self.request(json!({
            "subtype": "can_use_tool", "tool_name": tool, "display_name": tool, "input": input,
            "description": input["description"], "permission_suggestions": suggestions, "tool_use_id": tool_id}))
    }

    /// Send a control request to the host and wait for its answer.
    fn request(&mut self, request: Value) -> Result<Value, Cut> {
        self.next += 1;
        let id = format!("fake-req-{}", new_id());
        out(json!({"type": "control_request", "request_id": id, "request": request}));
        loop {
            match self.rx.recv() {
                Ok(v) => {
                    let interrupt =
                        v["type"] == "control_request" && v["request"]["subtype"] == "interrupt";
                    if let Some((rid, resp)) = self.handle(v)
                        && rid == id
                    {
                        return Ok(resp);
                    }
                    if interrupt {
                        return Err(Cut::Interrupted);
                    }
                }
                Err(_) => {
                    self.stdin_open = false;
                    return Err(Cut::Closed);
                }
            }
        }
    }

    fn envelope(&self, mut v: Value, parent: Option<&str>) -> Value {
        v["session_id"] = json!(self.session);
        v["parent_tool_use_id"] = json!(parent);
        v["uuid"] = json!(new_id());
        v
    }

    fn usage(&self) -> Value {
        json!({"input_tokens": 2, "cache_creation_input_tokens": 400, "cache_read_input_tokens": self.context, "output_tokens": 60})
    }

    /// Stream text as deltas, then the whole message.
    fn say(&mut self, text: &str, parent: Option<&str>) -> Result<(), Cut> {
        self.context += text.len() as u64 / 3 + 200;
        let msg_id = format!("msg_fake_{}", new_id());
        out(self.envelope(
            json!({"type": "stream_event", "event": {"type": "message_start", "message": {
                "id": msg_id, "role": "assistant", "content": [], "usage": self.usage()}}}),
            parent,
        ));
        out(self.envelope(
            json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""}}}),
            parent,
        ));
        let words: Vec<&str> = text.split_inclusive(' ').collect();
        for chunk in words.chunks(3) {
            out(self.envelope(
                json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": chunk.concat()}}}),
                parent,
            ));
            self.pause(self.chunk_ms)?;
        }
        out(self.envelope(
            json!({"type": "assistant", "message": {"id": msg_id, "role": "assistant", "model": self.model,
                "content": [{"type": "text", "text": text}], "usage": self.usage()}}),
            parent,
        ));
        out(self.envelope(
            json!({"type": "stream_event", "event": {"type": "content_block_stop", "index": 0}}),
            parent,
        ));
        out(self.envelope(
            json!({"type": "stream_event", "event": {"type": "message_stop"}}),
            parent,
        ));
        Ok(())
    }

    /// One message as the real agent streams it: words, then a call whose
    /// input streams slowly, as writing a long file does.
    fn say_and_call(&mut self, text: &str, name: &str, input: Value) -> Result<String, Cut> {
        self.context += text.len() as u64 / 3 + 200;
        self.next += 1;
        let msg_id = format!("msg_fake_n{}", new_id());
        let id = format!("toolu_fake_{}", new_id());
        let ev = |e: Value| json!({"type": "stream_event", "event": e});
        out(self.envelope(
            ev(json!({"type": "message_start", "message": {
                "id": msg_id, "role": "assistant", "content": [], "usage": self.usage()}})),
            None,
        ));
        out(self.envelope(
            ev(json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "text", "text": ""}})),
            None,
        ));
        for chunk in text.split_inclusive(' ').collect::<Vec<_>>().chunks(3) {
            out(self.envelope(
                ev(json!({"type": "content_block_delta", "index": 0,
                    "delta": {"type": "text_delta", "text": chunk.concat()}})),
                None,
            ));
            self.pause(40)?;
        }
        out(self.envelope(
            json!({"type": "assistant", "message": {"id": msg_id, "role": "assistant", "model": self.model,
                "content": [{"type": "text", "text": text}], "usage": self.usage()}}),
            None,
        ));
        out(self.envelope(ev(json!({"type": "content_block_stop", "index": 0})), None));
        out(self.envelope(
            ev(json!({"type": "content_block_start", "index": 1,
                "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}})),
            None,
        ));
        let json = input.to_string();
        let chars: Vec<char> = json.chars().collect();
        for chunk in chars.chunks(12) {
            out(self.envelope(
                ev(json!({"type": "content_block_delta", "index": 1,
                    "delta": {"type": "input_json_delta", "partial_json": chunk.iter().collect::<String>()}})),
                None,
            ));
            self.pause(60)?;
        }
        out(self.envelope(
            json!({"type": "assistant", "message": {"id": msg_id, "role": "assistant", "model": self.model,
                "content": [{"type": "tool_use", "id": id, "name": name, "input": input}],
                "usage": self.usage()}}),
            None,
        ));
        out(self.envelope(ev(json!({"type": "content_block_stop", "index": 1})), None));
        out(self.envelope(ev(json!({"type": "message_stop"})), None));
        Ok(id)
    }

    /// A thinking block, whole, as the agent sends one before acting.
    fn think(&mut self, text: &str) {
        out(self.envelope(
            json!({"type": "assistant", "message": {"id": format!("msg_fake_k{}", new_id()), "role": "assistant",
                "model": self.model, "content": [{"type": "thinking", "thinking": text, "signature": "fake"}],
                "usage": self.usage()}}),
            None,
        ));
    }

    /// Stream `parts` a few characters at a time, stopping for a while
    /// after each but the last, then the whole message.
    /// A reply streamed in the pieces given, one delta each, `ms` apart.
    fn say_parts(&mut self, parts: &[&str], ms: u64) -> Result<String, Cut> {
        let text = parts.concat();
        self.context += text.len() as u64 / 3 + 200;
        let msg_id = format!("msg_fake_p{}", new_id());
        let ev = |e: Value| json!({"type": "stream_event", "event": e});
        out(self.envelope(
            ev(json!({"type": "message_start", "message": {
                "id": msg_id, "role": "assistant", "content": [], "usage": self.usage()}})),
            None,
        ));
        out(self.envelope(
            ev(json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "text", "text": ""}})),
            None,
        ));
        for part in parts {
            out(self.envelope(
                ev(json!({"type": "content_block_delta", "index": 0,
                    "delta": {"type": "text_delta", "text": part}})),
                None,
            ));
            self.pause(ms)?;
        }
        out(self.envelope(
            json!({"type": "assistant", "message": {"id": msg_id, "role": "assistant", "model": self.model,
                "content": [{"type": "text", "text": text}], "usage": self.usage()}}),
            None,
        ));
        out(self.envelope(ev(json!({"type": "content_block_stop", "index": 0})), None));
        out(self.envelope(ev(json!({"type": "message_stop"})), None));
        Ok(text)
    }

    fn say_held(&mut self, parts: &[&str]) -> Result<String, Cut> {
        let text = parts.concat();
        let msg_id = format!("msg_fake_h{}", new_id());
        let ev = |e: Value| json!({"type": "stream_event", "event": e});
        out(self.envelope(
            ev(json!({"type": "message_start", "message": {
                "id": msg_id, "role": "assistant", "content": [], "usage": self.usage()}})),
            None,
        ));
        out(self.envelope(
            ev(json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "text", "text": ""}})),
            None,
        ));
        for (n, part) in parts.iter().enumerate() {
            let chars: Vec<char> = part.chars().collect();
            for chunk in chars.chunks(4) {
                out(self.envelope(
                    ev(json!({"type": "content_block_delta", "index": 0,
                        "delta": {"type": "text_delta", "text": chunk.iter().collect::<String>()}})),
                    None,
                ));
                self.pause(60)?;
            }
            if n + 1 < parts.len() {
                self.pause(12_000)?;
            }
        }
        out(self.envelope(
            json!({"type": "assistant", "message": {"id": msg_id, "role": "assistant", "model": self.model,
                "content": [{"type": "text", "text": text}], "usage": self.usage()}}),
            None,
        ));
        out(self.envelope(ev(json!({"type": "content_block_stop", "index": 0})), None));
        out(self.envelope(ev(json!({"type": "message_stop"})), None));
        Ok(text)
    }

    fn tool(&mut self, name: &str, input: Value, parent: Option<&str>) -> String {
        self.next += 1;
        // Unique across processes, as real ids are: a resumed session's new
        // process never repeats an id the old one used.
        let id = format!("toolu_fake_{}", new_id());
        out(self.envelope(
            json!({"type": "assistant", "message": {"id": format!("msg_fake_t{}", self.next), "role": "assistant",
                "model": self.model, "content": [{"type": "tool_use", "id": id, "name": name, "input": input}],
                "usage": self.usage()}}),
            parent,
        ));
        id
    }

    fn result(&self, id: &str, content: &str, error: bool, parent: Option<&str>) {
        out(self.envelope(
            json!({"type": "user", "message": {"role": "user", "content": [
                {"tool_use_id": id, "type": "tool_result", "content": content, "is_error": error}]}}),
            parent,
        ));
    }

    /// Runs a tool that may need permission. Returns the denial, if any.
    fn guarded(&mut self, name: &str, input: Value, needs: bool) -> Result<Option<String>, Cut> {
        let id = self.tool(name, input.clone(), None);
        // A rule saved in the project's settings allows it without asking.
        let ruled = input["command"]
            .as_str()
            .is_some_and(|c| local_rules().contains(&format!("{name}({c})")));
        if needs && !ruled {
            let resp = self.permission(name, &id, &input)?;
            if resp["behavior"] != "allow" {
                let why = resp["message"].as_str().unwrap_or("Denied").to_string();
                self.result(&id, &why, true, None);
                return Ok(Some(why));
            }
            // Rules meant to last go to the project's local settings.
            for u in resp["updatedPermissions"].as_array().into_iter().flatten() {
                if u["destination"] == "localSettings" {
                    let mut rules = local_rules();
                    for r in u["rules"].as_array().into_iter().flatten() {
                        let rule = format!(
                            "{}({})",
                            r["toolName"].as_str().unwrap_or_default(),
                            r["ruleContent"].as_str().unwrap_or_default()
                        );
                        if !rules.contains(&rule) {
                            rules.push(rule);
                        }
                    }
                    save_local_rules(&rules);
                }
            }
        }
        self.pause(300)?;
        self.result(&id, "ok", false, None);
        Ok(None)
    }

    fn init(&self) {
        out(self.envelope(
            json!({"type": "system", "subtype": "init", "cwd": std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
                "tools": ["Bash", "Read", "Write", "Edit", "Task", "AskUserQuestion"], "model": self.model,
                "permissionMode": self.mode, "claude_code_version": "0.0.0-fake"}),
            None,
        ));
    }

    fn end(&self, text: &str, error: bool) {
        out(self.envelope(
            json!({"type": "result", "subtype": if error { "error_during_execution" } else { "success" },
                "is_error": error, "terminal_reason": if error { "error" } else { "completed" },
                "result": text, "duration_ms": 1200, "total_cost_usd": 0.0, "usage": self.usage()}),
            None,
        ));
    }

    fn turn(&mut self, text: &str) -> Result<(), ()> {
        self.init();
        if !self.sent_limits {
            self.sent_limits = true;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out(
                json!({"type": "rate_limit_event", "session_id": self.session, "rate_limit_info": {
                "status": "allowed", "unifiedWindows": {
                    "five_hour": {"utilization": 0.18, "resetsAt": now + 3 * 3600},
                    "seven_day": {"utilization": 0.41, "resetsAt": now + 4 * 86400}}}}),
            );
        }
        match self.script(text) {
            Ok(()) => Ok(()),
            Err(Cut::Interrupted) => {
                out(self.envelope(
                    json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user]"}]}}),
                    None,
                ));
                out(self.envelope(
                    json!({"type": "result", "subtype": "error_during_execution", "is_error": true,
                        "terminal_reason": "aborted_streaming", "usage": self.usage()}),
                    None,
                ));
                Ok(())
            }
            Err(Cut::Steered) => {
                out(self.envelope(
                    json!({"type": "result", "subtype": "error_during_execution", "is_error": true,
                        "terminal_reason": "aborted_streaming",
                        "errors": ["[ede_diagnostic] fake", "Error: Request was aborted."], "usage": self.usage()}),
                    None,
                ));
                Ok(())
            }
            Err(Cut::Closed) => Err(()),
        }
    }

    /// A command Claude Code runs itself: its answer as a synthetic
    /// message, then a result with no turns.
    fn local(&self, name: &str, args: &str, text: &str, extra: Value) {
        let mut m = json!({"type": "assistant",
            "message": {"model": "<synthetic>", "role": "assistant", "content": [{"type": "text", "text": text}],
                "usage": {"input_tokens": 0, "output_tokens": 0}},
            "local_command_source": format!("<local-command-stdout>{text}</local-command-stdout>")});
        if name.is_empty() {
            m["local_command_outcome"] = extra;
        } else {
            m["local_command_run"] = json!({"command": name, "args": args});
            if let Value::Object(more) = extra {
                for (k, v) in more {
                    m[k] = v;
                }
            }
        }
        out(self.envelope(m, None));
        out(self.envelope(
            json!({"type": "result", "subtype": "success", "is_error": false, "num_turns": 0,
                "result": text, "local_command": name.replace('-', "_"), "usage": self.usage()}),
            None,
        ));
    }

    fn command(&mut self, text: &str) -> Result<(), Cut> {
        let rest = &text[1..];
        let (name, args) = rest.split_once(' ').unwrap_or((rest, ""));
        let args = args.trim();
        match name {
            "compact" => {
                out(self.envelope(json!({"type": "system", "subtype": "status", "status": "compacting"}), None));
                self.pause(2500)?;
                let before = self.context;
                self.context = 4_200;
                out(self.envelope(json!({"type": "system", "subtype": "status", "status": null, "compact_result": "success"}), None));
                out(self.envelope(json!({"type": "system", "subtype": "compact_boundary",
                    "compact_metadata": {"trigger": "manual", "pre_tokens": before, "post_tokens": 2_100}}), None));
                // As the real CLI: the output only as a replayed message, and
                // an empty result.
                out(self.envelope(json!({"type": "user", "isReplay": true,
                    "message": {"role": "user", "content": "<local-command-stdout>Compacted </local-command-stdout>"}}), None));
                out(self.envelope(json!({"type": "result", "subtype": "success", "is_error": false,
                    "num_turns": 0, "result": "", "local_command": "compact", "usage": self.usage()}), None));
            }
            "context" => self.local("context", "",
                "## Context Usage\n\n**Model:** claude-sonnet-5-5  \n**Tokens:** 18k / 200k (9%)\n\n| Category | Tokens |\n|---|---|\n| System prompt | 3.2k |\n| Messages | 2.6k |",
                json!({"context_usage": {"model": "claude-sonnet-5-5", "total_tokens": self.context, "raw_max_tokens": 200_000,
                    "percentage": 9, "categories": [
                        {"name": "System prompt", "tokens": 3_200, "kind": "used"},
                        {"name": "System tools", "tokens": 6_300, "kind": "used"},
                        {"name": "Skills", "tokens": 5_900, "kind": "used"},
                        {"name": "Messages", "tokens": self.context.saturating_sub(15_400), "kind": "used"},
                        {"name": "MCP tools (deferred)", "tokens": 3_800, "kind": "deferred"},
                        {"name": "Free space", "tokens": 150_000, "kind": "free"}]}})),
            "usage" => self.local("usage", "",
                "You are currently using your subscription to power your Claude Code usage\n\nCurrent session: 18% used\nCurrent week (all models): 41% used",
                json!({"usage_report": {"rate_limits": {"limits": [
                    {"kind": "session", "percent": 18, "resets_at": "2030-01-01T00:00:00+00:00", "scope": null},
                    {"kind": "weekly_all", "percent": 41, "resets_at": "2030-01-05T00:00:00+00:00", "scope": null},
                    {"kind": "weekly_scoped", "percent": 11, "resets_at": "2030-01-05T00:00:00+00:00", "scope": {"model": {"display_name": "Fable"}}}]}}})),
            "config" if args.is_empty() => self.local("config", "",
                "Usage: /config key=value [key=value ...]\n  autoCompact=true|false\n  theme=auto|dark|light\n  language=<value>", json!({})),
            "config" => self.local("config", args, &format!("Set {}", args.replace('=', " to ")), json!({})),
            "autocompact" if args.is_empty() => self.local("autocompact", "",
                "Auto-compact window for Sonnet 5.5: 200k tokens (default for this model)\nAuto-compact summarizes the conversation when context usage approaches this limit.", json!({})),
            "autocompact" => self.local("autocompact", args, &format!("Auto-compact window set to {args}"), json!({})),
            "output-style" if args.is_empty() => self.local("output-style", "",
                "Output style: default\n\nAvailable styles:\n- default (current)\n- Concise: Claude responds tersely\n\nUsage: /output-style <style>", json!({})),
            "output-style" => self.local("output-style", args, &format!("Output style set to {args}"), json!({})),
            "advisor" if args.is_empty() => self.local("advisor", "", "Advisor: off\nUsage: /advisor <fable|opus|sonnet|off>", json!({})),
            "advisor" => self.local("advisor", args, &format!("Advisor set to {args}"), json!({})),
            "theme" => self.local("", "", "/theme isn't available in this environment.", json!({"kind": "unavailable_headless"})),
            "mcp" if args.is_empty() => self.local("mcp", "",
                "MCP servers:\n  docs: connected\n  tickets: needs authentication\n  figma: failed\n  old-search: disabled", json!({})),
            "mcp" => self.local("mcp", args, &format!("MCP: {args}"), json!({})),
            "status" | "skills" | "memory" | "hooks" | "permissions" => self.local("", "",
                &format!("/{name} isn't available in this environment."), json!({"kind": "unavailable_headless"})),
            _ => self.local("", "",
                &format!("Unknown command: /{name}. Did you mean /compact?"),
                json!({"kind": "unknown", "suggestion": "/compact"})),
        }
        Ok(())
    }

    fn script(&mut self, text: &str) -> Result<(), Cut> {
        let lower = text.to_lowercase();
        let ask = self.mode == "default";
        self.pause(250)?;
        if lower.starts_with('/') {
            return self.command(text);
        }
        // Edges, checked before anything else so the odd words in them
        // can't set off another script.
        if lower.contains("odd content") {
            let reply = odd_content();
            let parts: Vec<&str> = reply.split_inclusive("\n\n").collect();
            let text = self.say_parts(&parts, 80)?;
            self.end(&text, false);
            return Ok(());
        }
        if lower.contains("ten thousand lines") {
            let mut reply = String::from("All of it:\n\n```text\n");
            for n in 1..=10_000 {
                reply.push_str(&format!(
                    "line {n:05}: the quick brown fox jumps over the lazy dog\n"
                ));
            }
            reply.push_str("```\n\nThat's ten thousand lines.");
            let parts: Vec<&str> = reply.split_inclusive("dog\n").collect();
            let chunks: Vec<String> = parts.chunks(500).map(|c| c.concat()).collect();
            let chunks: Vec<&str> = chunks.iter().map(String::as_str).collect();
            let text = self.say_parts(&chunks, 80)?;
            self.end(&text, false);
            return Ok(());
        }
        if lower.contains("retry") {
            for (attempt, delay) in [(1, 1500), (2, 3000)] {
                out(self.envelope(
                    json!({"type": "system", "subtype": "api_retry", "attempt": attempt, "max_retries": 10,
                        "retry_delay_ms": delay, "error_status": 529, "error": "Overloaded"}),
                    None,
                ));
                self.pause(delay)?;
            }
            self.say("Back now; the API was busy for a moment.", None)?;
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("declined") {
            let resp = self.request(json!({"subtype": "request_user_dialog", "dialog_kind": "refusal_fallback_prompt",
                "payload": {"originalModel": "claude-fable-5-1", "fallbackModel": "claude-opus-5-5"}}))?;
            let said = match resp["result"].as_str() {
                Some("retry_fallback") => "Answered with Opus 5.5 instead.",
                Some("edit_prompt") => "Edit your message and send it again.",
                _ => "Left it there.",
            };
            self.say(said, None)?;
            self.end(said, false);
            return Ok(());
        }
        if lower.contains("colour output") {
            let id = self.tool(
                "Bash",
                json!({"command": "cargo test --color=always", "description": "Run the tests"}),
                None,
            );
            self.pause(300)?;
            self.result(&id, &coloured_output(), false, None);
            self.say("Tests ran: five passed and one failed.", None)?;
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("half-typed") {
            let text = self.say_held(&[
                "Streaming check: this word is **bo",
                "ld text** and here is [the docs](https://exa",
                "mple.com) for more.\n\n| Name | Size |\n| --",
                "- | ---: |\n| alpha | 12 KB |\n| beta | 3 KB |\n\n1. Parse the `tail` again\n2",
                ". Draw it\n3. Commit the block\n\nThat's the lot.",
            ])?;
            self.end(&text, false);
            return Ok(());
        }
        if lower.contains("crash") {
            self.say("Starting on that now, ", None)?;
            std::process::exit(3);
        }
        if lower.contains("dev server") {
            // A real little server, so a preview has something to show.
            let listener = std::net::TcpListener::bind("localhost:0").map_err(|_| Cut::Closed)?;
            let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut stream = stream;
                    let mut buf = [0u8; 1024];
                    let _ = std::io::Read::read(&mut stream, &mut buf);
                    let body = "<!doctype html><title>Dev</title><body style=\"font:16px system-ui;margin:40px\"><h1>Admin dashboard</h1><p>Served by the dev server.</p></body>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }
            });
            let id = self.tool(
                "Bash",
                json!({"command": "npm run dev", "description": "Start the dev server", "run_in_background": true}),
                None,
            );
            self.pause(300)?;
            self.result(
                &id,
                &format!(
                    "> vite\n\n  VITE ready in 312 ms\n\n  ➜  Local:   http://localhost:{port}/\n"
                ),
                false,
                None,
            );
            self.say(
                &format!("The dev server is up at http://localhost:{port}/."),
                None,
            )?;
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("make a plan") {
            let todo = |done: usize| {
                let steps = ["Read the handler", "Add the rate limit", "Run the tests"];
                let todos: Vec<Value> = steps
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let status = if i < done { "completed" } else if i == done { "in_progress" } else { "pending" };
                        json!({"content": s, "status": status, "activeForm": format!("Working on: {}", s.to_lowercase())})
                    })
                    .collect();
                json!({"todos": todos})
            };
            // `slowly` holds each step long enough to look at.
            let hold = if lower.contains("slowly") {
                12_000
            } else {
                1200
            };
            for done in 0..2 {
                let id = self.tool("TodoWrite", todo(done), None);
                self.result(&id, "Todos updated", false, None);
                self.pause(hold)?;
            }
            self.say("One of three done; adding the rate limit now.", None)?;
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("your args") {
            let args: Vec<String> = std::env::args().skip(1).collect();
            let text = args.join(" ");
            self.say(&text, None)?;
            self.end(&text, false);
            return Ok(());
        }
        if lower.contains("hit the limit") {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out(
                json!({"type": "rate_limit_event", "session_id": self.session, "rate_limit_info": {
                "status": "rejected", "unifiedWindows": {
                    "five_hour": {"utilization": 1.0, "resetsAt": now + 2 * 3600 + 600},
                    "seven_day": {"utilization": 0.62, "resetsAt": now + 4 * 86400}}}}),
            );
            self.end("You've hit your limit · resets later", true);
            return Ok(());
        }
        if lower.contains("fail") {
            self.say("Looking into it.", None)?;
            self.end("Something went wrong on the fake agent's side.", true);
            return Ok(());
        }
        // The path as typed: folders can have capitals.
        if let Some(path) = lower
            .split_whitespace()
            .position(|w| w == "write")
            .and_then(|at| text.split_whitespace().nth(at + 1))
        {
            let path = path.trim_matches(|c: char| c == '`' || c == '.' || c == ',');
            let body = file_body(path, text);
            let input = json!({"file_path": path, "content": body});
            let needs = ask && self.mode != "acceptEdits";
            if let Some(why) = self.guarded("Write", input, needs)? {
                self.say(&format!("Understood, I won't write it. ({why})"), None)?;
            } else {
                if let Some(dir) = std::path::Path::new(path).parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(path, &body);
                self.say(&format!("Wrote `{path}`."), None)?;
            }
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("elicit") {
            // An MCP server asking for input, the way the real CLI relays it.
            let resp = self.request(
                json!({"subtype": "elicitation", "mcp_server_name": "tickets",
                "message": "Which project?", "requested_schema": {"type": "object"}}),
            )?;
            let action = resp["action"].as_str().unwrap_or("no answer").to_string();
            self.say(&format!("The tickets server got: {action}."), None)?;
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("ask") {
            // `ask several`: a question that takes more than one answer.
            let input = if lower.contains("several") {
                json!({"questions": [{"question": "Which platforms should the build cover?", "header": "Platforms",
                    "multiSelect": true, "options": [
                        {"label": "macOS", "description": "Apple silicon and Intel"},
                        {"label": "Linux", "description": "x86_64 and arm64"},
                        {"label": "Windows", "description": "x86_64"}]}]})
            } else {
                json!({"questions": [{"question": "Which database should the cache use?", "header": "Cache",
                    "multiSelect": false, "options": [
                        {"label": "SQLite", "description": "One file, no server"},
                        {"label": "Redis", "description": "Shared between machines"}]}]})
            };
            let id = self.tool("AskUserQuestion", input.clone(), None);
            let resp = self.permission("AskUserQuestion", &id, &input)?;
            let picked = resp["updatedInput"]["answers"]
                .as_object()
                .and_then(|m| m.values().next())
                .and_then(Value::as_str)
                .unwrap_or("nothing")
                .to_string();
            self.result(&id, &format!("User answered: {picked}"), false, None);
            self.say(&format!("Going with **{picked}**."), None)?;
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("present a plan") || lower.contains("present a long plan") {
            let plan = if lower.contains("long") {
                (1..=30)
                    .map(|n| format!("{n}. Step {n}: change part {n} of the handler and test it."))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                "1. Read the handler.\n2. Add the rate limit.\n3. Run the tests.".to_string()
            };
            let input = json!({ "plan": plan });
            match self.guarded("ExitPlanMode", input, true)? {
                Some(why) => self.say(&format!("Okay, I'll keep planning: {why}"), None)?,
                None => self.say("Building it now.", None)?,
            }
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("run") || lower.contains("approve") {
            let input =
                json!({"command": "echo hello > hello.txt", "description": "Write hello.txt"});
            match self.guarded("Bash", input, ask)? {
                Some(why) => self.say(&format!("Okay, I didn't run it: {why}"), None)?,
                None => self.say("Ran it; `hello.txt` now says hello.", None)?,
            }
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("subagent") {
            let task = self.tool(
                "Task",
                json!({"description": "Survey the tests", "prompt": "Find slow tests"}),
                None,
            );
            for f in ["tests/api.rs", "tests/db.rs"] {
                let id = self.tool("Read", json!({"file_path": f}), Some(&task));
                self.pause(200)?;
                self.result(&id, "fn main() {}", false, Some(&task));
            }
            self.say("Timing the database tests next.", Some(&task))?;
            // A subagent can start its own.
            let inner = self.tool(
                "Task",
                json!({"description": "Time the db tests", "prompt": "Run them with timing"}),
                Some(&task),
            );
            let run = self.tool(
                "Bash",
                json!({"command": "cargo test db -- --report-time", "description": "Run the db tests with timings"}),
                Some(&inner),
            );
            self.pause(200)?;
            self.result(&run, "test db::migrate ... ok <4.1s>", false, Some(&inner));
            self.result(&inner, "`db::migrate` takes 4.1 s.", false, Some(&task));
            self.say("The db tests are the slow ones.", Some(&task))?;
            self.result(
                &task,
                "## Slow tests\n\nThe db tests are the slow ones.\n\n| Test | Time |\n| --- | --- |\n| `db::migrate` | 4.1 s |\n| `db::seed` | 1.2 s |\n| `api::health` | 0.1 s |\n\n- Migrations rebuild the schema each run.\n- Seeding writes 10,000 rows.\n- Sharing one database across tests would save about 5 s.\n- The api tests are already fast.",
                false,
                None,
            );
            self.say(
                "The subagent found that the database tests are the slow ones.",
                None,
            )?;
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("narrate") {
            let id = self.say_and_call(
                "First I'll read the handler to see how requests arrive.",
                "Read",
                json!({"file_path": "src/handler.rs"}),
            )?;
            // Long enough that the tool's output is clipped.
            let body: String = (1..=100).map(|n| format!("// line {n}\n")).collect();
            self.result(&id, &body, false, None);
            let notes = "Notes on the handler.\n".repeat(40);
            let id = self.say_and_call(
                "Now I'll write up what I found.",
                "Write",
                json!({"file_path": "notes/handler-notes.md", "content": notes}),
            )?;
            self.pause(400)?;
            self.result(&id, "File created", false, None);
            self.say("All done: the notes are in `handler-notes.md`.", None)?;
            self.end("done", false);
            return Ok(());
        }
        // A subagent sent to the background, as the real agent sends one:
        // a launch receipt now, its work and its end later.
        if lower.contains("in parallel") {
            let task = self.tool(
                "Agent",
                json!({"description": "Survey competitors", "prompt": "Find out how they onboard", "run_in_background": true}),
                None,
            );
            out(self.envelope(
                json!({"type": "system", "subtype": "background_tasks_changed",
                "tasks": [{"task_id": "a1", "description": "Survey competitors"}]}),
                None,
            ));
            self.result(
                &task,
                "Async agent launched successfully. (This tool result is internal metadata.)\nagentId: a1 (internal ID)\nThe agent is working in the background.",
                false,
                None,
            );
            self.say(
                "It's researching in the background; I'll write it up when it's done.",
                None,
            )?;
            self.end("done", false);
            self.pause(600)?;
            let id = self.tool(
                "WebSearch",
                json!({"query": "Monarch free trial"}),
                Some(&task),
            );
            self.pause(2500)?;
            self.result(&id, "Monarch: 7-day trial", false, Some(&task));
            self.say(
                "Monarch starts everyone on a trial before linking.",
                Some(&task),
            )?;
            self.pause(600)?;
            out(self.envelope(
                json!({"type": "system", "subtype": "background_tasks_changed", "tasks": []}),
                None,
            ));
            self.init();
            self.say("The research is in.", None)?;
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("background") {
            let id = self.tool(
                "Bash",
                json!({"command": "sleep 2", "description": "Wait two seconds", "run_in_background": true}),
                None,
            );
            self.result(&id, "Started in the background (task b1)", false, None);
            out(self.envelope(
                json!({"type": "system", "subtype": "background_tasks_changed",
                "tasks": [{"task_id": "b1", "description": "sleep 2"}]}),
                None,
            ));
            self.say(
                "Started it in the background; I'll check when it's done.",
                None,
            )?;
            self.end("done", false);
            self.pause(1500)?;
            out(self.envelope(
                json!({"type": "system", "subtype": "task_notification", "task_id": "b1",
                "status": "completed", "summary": "sleep 2 finished"}),
                None,
            ));
            out(self.envelope(
                json!({"type": "system", "subtype": "background_tasks_changed", "tasks": []}),
                None,
            ));
            self.init();
            self.say("The background job finished cleanly.", None)?;
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("think") {
            self.think("The user wants this considered first. The parser keeps a stack of open containers, so a list item can hold code. I should check the tests before answering.");
        }
        if lower.contains("slowly") {
            self.chunk_ms = 400;
        }
        let reply = if lower.contains("long") {
            LONG.to_string()
        } else if lower.contains("math") || lower.contains("diagram") || lower.contains("big table")
        {
            rich()
        } else {
            format!(
                "Here's what I'd do about **{}**:\n\n1. Read the code that handles it.\n2. Make the smallest change that works.\n3. Run the tests.\n\n```rust\nfn main() {{\n    println!(\"hello\");\n}}\n```\n\nWant me to start?",
                text.lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(60)
                    .collect::<String>()
            )
        };
        let said = self.say(&reply, None);
        self.chunk_ms = 40;
        said?;
        self.end(&reply, false);
        Ok(())
    }
}

/// What `cargo test --color=always` might print: colours, bold, dim, a
/// progress line redrawn with carriage returns, and one very long line.
fn coloured_output() -> String {
    let mut s = String::from("\x1b[1m\x1b[32m   Compiling\x1b[0m dent v0.1.0\n");
    s.push_str("Building [=====>              ] 30%\rBuilding [===========>        ] 60%\rBuilding [====================] 100%\n");
    s.push_str("\x1b[1mrunning 6 tests\x1b[0m\n");
    for t in [
        "parse::empty",
        "parse::nested",
        "log::append",
        "log::replay",
        "sync::resume",
    ] {
        s.push_str(&format!("test {t} ... \x1b[32mok\x1b[0m\n"));
    }
    s.push_str("test sync::conflict ... \x1b[31mFAILED\x1b[0m\n\n");
    s.push_str("\x1b[33mwarning\x1b[0m: unused variable: `seq` \x1b[2m(src/sync.rs:41)\x1b[0m\n");
    s.push_str(
        "\x1b[32m✓ passed\x1b[0m 5   \x1b[31m✗ failed\x1b[0m 1   \x1b[90m0 ignored\x1b[0m\n",
    );
    s.push_str("\x1b[36mcyan\x1b[0m \x1b[35mmagenta\x1b[0m \x1b[34mblue\x1b[0m \x1b[38;5;208m256-orange\x1b[0m \x1b[38;2;120;200;80mtrue colour\x1b[0m \x1b[3mitalic\x1b[0m\n");
    s.push_str("thread 'sync::conflict' panicked: ");
    for n in 0..80 {
        s.push_str(&format!("left=seq{n} "));
    }
    s.push('\n');
    s.push_str("\x1b[1m\x1b[31merror\x1b[0m: test failed, to rerun pass `--lib`\n");
    s
}

/// Long words and links, scripts of every direction, nesting, a wide table.
fn odd_content() -> String {
    let mut s = String::from("Odd content, for the edges.\n\n");
    s.push_str("A link with no spaces: https://example.com/a/very/deep/path/that/keeps/going/and/going/without/a/single/space/anywhere/in/it/so/the/line/has/to/break/somewhere?query=");
    s.push_str(&"a".repeat(90));
    s.push_str("&page=2\n\n");
    s.push_str("One word longer than the column: ");
    s.push_str(&"Pneumonoultramicroscopicsilicovolcanoconiosis".repeat(5));
    s.push_str(".\n\nAnd in code: `");
    s.push_str(&"very_long_identifier_".repeat(12));
    s.push_str("`.\n\n");
    s.push_str("Families 👩‍👩‍👧‍👦 flags 🇯🇵🇧🇷 and skin tones 👋🏽 ✅; 日本語の文章には単語の間に空白がありません、だから折り返しはどこでも起こります。中文也没有空格。한국어 문장도 있습니다.\n\n");
    s.push_str("مرحبا بالعالم، هذا نص من اليمين إلى اليسار مع كلمة English في الوسط.\n\nשלום עולם, עברית גם כן.\n\n");
    s.push_str("Combining marks: e\u{301}te\u{301}, n\u{303}, Z\u{337}\u{322}\u{353}a\u{338}l\u{335}g\u{334}o\u{336}.\n\n");
    for depth in 1..=6 {
        s.push_str(&"> ".repeat(depth));
        s.push_str(&format!("Quoted at depth {depth}.\n"));
    }
    s.push('\n');
    for depth in 0..8 {
        s.push_str(&"  ".repeat(depth));
        s.push_str(&format!(
            "- Listed at depth {}, with enough words after it to wrap at a narrow width\n",
            depth + 1
        ));
    }
    s.push_str("\n|");
    for c in 1..=16 {
        s.push_str(&format!(" Column {c} |"));
    }
    s.push_str("\n|");
    for _ in 1..=16 {
        s.push_str(" --- |");
    }
    s.push('\n');
    for r in 1..=40 {
        s.push('|');
        for c in 1..=16 {
            s.push_str(&format!(" r{r}c{c} |"));
        }
        s.push('\n');
    }
    s.push_str("\nThat's the lot.");
    s
}

/// Maths, inline and display, a diagram and a table too tall for the chat.
fn rich() -> String {
    let mut s = String::from(
        r#"Here's the frame budget, worked through.

With $n$ events and a viewport of $k$ rows, layout costs $O(k)$ rather than $O(n)$. A plan at $10/mo ($30/mo yearly) is still just a price.

$$
\mathrm{softmax}\!\left(\frac{QK^\top}{\sqrt{d_k}}\right)V, \qquad \sum_{i=1}^{n} \alpha_i = 1
$$

The cost grows with every term:

$$
f(x) = a_0 + a_1 x + a_2 x^2 + a_3 x^3 + a_4 x^4 + a_5 x^5 + a_6 x^6 + a_7 x^7 + a_8 x^8 + a_9 x^9 + a_{10} x^{10} + a_{11} x^{11} + a_{12} x^{12}
$$

```mermaid
flowchart LR
  A[Chunk arrives] --> B[Re-parse tail]
  B --> C{Block closed?}
  C -- yes --> D[Commit]
  C -- no --> E[Draw]
```

Every event, timed:

| # | Event | Duration |
| ---: | --- | ---: |
"#,
    );
    for r in 1..=60 {
        s.push_str(&format!(
            "| {r} | event number {r} | {}.{} ms |\n",
            r % 9,
            r % 7
        ));
    }
    s.push_str("\nThat's all sixty.");
    s
}

fn file_body(path: &str, prompt: &str) -> String {
    if path.ends_with(".html") && prompt.contains("with assets") {
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Page</title>\n<link rel=\"stylesheet\" href=\"style.css\"></head>\n<body><h1>Styled</h1><img src=\"/dot.gif\" alt=\"\"></body></html>\n".to_string()
    } else if path.ends_with(".html") {
        format!(
            "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Page</title>\n<style>body{{font:16px system-ui;margin:40px}}h1{{font-size:28px}}</style></head>\n<body><h1>Hello</h1><p>{}</p></body></html>\n",
            prompt.replace('<', "&lt;")
        )
    } else if path.ends_with(".md") {
        format!("# Notes\n\n{prompt}\n\n- one\n- two\n")
    } else {
        format!("{prompt}\n")
    }
}

const LONG: &str = "Let me walk through the whole design before changing anything, because the pieces depend on each other more than it first looks.\n\n## The event log\n\nEvery session is a list of events, and each one gets a number. A client that drops off asks for everything after the last number it saw, so catching up and staying live are the same thing. Nothing is ever rewritten, which keeps the log simple to reason about and cheap to copy.\n\n## The agents\n\nThe agent runs as a child of a tiny holder process rather than of the daemon itself. That way the daemon can restart, crash or upgrade without the agent noticing: the holder keeps the pipes open and writes everything to disk, and the new daemon picks up from the last byte it recorded.\n\n## The clients\n\nThe desktop and the phone are views. They keep a cache so they open instantly, and they send commands to the daemon that owns the session. They never decide anything on their own, so two of them can't disagree about what happened.\n\n## What I'd change\n\n1. Batch events while catching up, so a phone isn't held back by the cost of each message.\n2. Store only complete messages, and stream the rest.\n3. Keep the last usage reading per account, since the agent only reports it when it changes.\n\nThat's the plan. Shall I start with the first one?";
