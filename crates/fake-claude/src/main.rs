//! Stands in for `claude -p` stream-json in tests and demos, so nothing
//! touches a real account. It speaks the slice of the protocol dent uses
//! (initialize, user messages, approvals, questions, interrupts, background
//! work, rate limits, usage) with the shapes recorded in spike 01.
//!
//! What it does depends on words in the message:
//!
//! - `run` or `approve`: a Bash call that needs approval
//! - `ask`: a question with choices
//! - `write <path>`: writes the file in its directory (asks first in Ask mode)
//! - `background`: starts background work, ends the turn, then follows up
//! - `subagent`: a Task call whose steps nest under it
//! - `long`: a long reply, for stopping halfway
//! - `fail`: the turn errors
//! - `crash`: the process exits mid-turn
//! - anything else: a short Markdown reply
//!
//! `FAKE_CLAUDE_SPEED` scales every pause (0 makes it instant).

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
        session: arg("--resume")
            .or_else(|| arg("--session-id"))
            .unwrap_or_else(new_id),
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
    };
    loop {
        if let Some(text) = a.queue.pop_front() {
            if a.turn(&text).is_err() && !a.stdin_open {
                return;
            }
            continue;
        }
        if !a.stdin_open {
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
                        "commands": [],
                        "agents": [],
                        "models": [
                            {"value": "default", "displayName": "Default (recommended)", "description": "Sonnet · Efficient for routine tasks"},
                            {"value": "opus", "displayName": "Opus", "description": "For complex work"},
                            {"value": "sonnet", "displayName": "Sonnet", "description": "Most efficient for simpler tasks"},
                            {"value": "haiku", "displayName": "Haiku", "description": "Fastest for quick answers"},
                        ],
                        "pid": std::process::id(),
                        "claude_code_version": "0.0.0-fake",
                        "current_permission_mode": self.mode,
                    }),
                    "set_model" => {
                        self.model = v["request"]["model"].as_str().unwrap_or("sonnet").into();
                        json!({})
                    }
                    "set_permission_mode" => {
                        self.mode = v["request"]["mode"].as_str().unwrap_or("default").into();
                        json!({})
                    }
                    "interrupt" => json!({ "still_queued": [] }),
                    "get_context_usage" => {
                        json!({ "totalTokens": self.context, "maxTokens": 200_000, "percentage": self.context / 2000 })
                    }
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
                    if interrupt {
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

    /// Ask for permission and wait for the answer.
    fn permission(&mut self, tool: &str, tool_id: &str, input: &Value) -> Result<Value, Cut> {
        self.next += 1;
        let id = format!("fake-req-{}", self.next);
        out(
            json!({"type": "control_request", "request_id": id, "request": {
            "subtype": "can_use_tool", "tool_name": tool, "display_name": tool, "input": input,
            "description": input["description"], "permission_suggestions": [], "tool_use_id": tool_id}}),
        );
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
            self.pause(40)?;
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

    fn tool(&mut self, name: &str, input: Value, parent: Option<&str>) -> String {
        self.next += 1;
        let id = format!("toolu_fake_{}", self.next);
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
        if needs {
            let resp = self.permission(name, &id, &input)?;
            if resp["behavior"] != "allow" {
                let why = resp["message"].as_str().unwrap_or("Denied").to_string();
                self.result(&id, &why, true, None);
                return Ok(Some(why));
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

    fn script(&mut self, text: &str) -> Result<(), Cut> {
        let lower = text.to_lowercase();
        let ask = self.mode == "default";
        self.pause(250)?;
        if lower.contains("crash") {
            self.say("Starting on that now, ", None)?;
            std::process::exit(3);
        }
        if lower.contains("fail") {
            self.say("Looking into it.", None)?;
            self.end("Something went wrong on the fake agent's side.", true);
            return Ok(());
        }
        if let Some(path) = lower
            .split_whitespace()
            .skip_while(|w| *w != "write")
            .nth(1)
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
        if lower.contains("ask") {
            let input = json!({"questions": [{"question": "Which database should the cache use?", "header": "Cache",
                "multiSelect": false, "options": [
                    {"label": "SQLite", "description": "One file, no server"},
                    {"label": "Redis", "description": "Shared between machines"}]}]});
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
            self.say("The db tests are the slow ones.", Some(&task))?;
            self.result(&task, "The db tests are the slow ones.", false, None);
            self.say(
                "The subagent found that the database tests are the slow ones.",
                None,
            )?;
            self.end("done", false);
            return Ok(());
        }
        if lower.contains("background") {
            let id = self.tool(
                "Bash",
                json!({"command": "sleep 2", "run_in_background": true}),
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
        let reply = if lower.contains("long") {
            LONG.to_string()
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
        self.say(&reply, None)?;
        self.end(&reply, false);
        Ok(())
    }
}

fn file_body(path: &str, prompt: &str) -> String {
    if path.ends_with(".html") {
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
