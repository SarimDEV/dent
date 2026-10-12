//! Spike: drive `claude -p` over stream-json from Rust.
//!
//! Type a message to send it. While an approval is pending, the next line
//! answers it: `y` allows, `n [reason]` denies. Commands: `/interrupt`,
//! `/mode <mode>`, `/model <model>`, `/context`, `/quit`.
//!
//! Every raw line in both directions is recorded to `recordings/` for the
//! future replay driver.

mod status;
// The wire types describe more of the protocol than this spike prints.
#[allow(dead_code)]
mod wire;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use status::{Notify, Tracker};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use wire::{AssistantBlock, Inbound};

struct Args {
    cwd: PathBuf,
    model: Option<String>,
    resume: Option<String>,
}

fn args() -> Result<Args> {
    let mut a = Args {
        cwd: std::env::current_dir()?,
        model: None,
        resume: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--cwd" => a.cwd = val()?.into(),
            "--model" => a.model = Some(val()?),
            "--resume" => a.resume = Some(val()?),
            other => bail!("unknown flag {other} (expected --cwd, --model, --resume)"),
        }
    }
    Ok(a)
}

struct Recorder {
    file: tokio::fs::File,
    start: Instant,
}

impl Recorder {
    async fn line(&mut self, dir: &str, raw: &str) -> Result<()> {
        let rec = json!({ "t": self.start.elapsed().as_secs_f64(), "dir": dir, "line": raw });
        self.file.write_all(format!("{rec}\n").as_bytes()).await?;
        Ok(())
    }
}

struct Driver {
    stdin: ChildStdin,
    rec: Recorder,
    next_id: u64,
}

impl Driver {
    async fn send(&mut self, v: Value) -> Result<()> {
        let s = v.to_string();
        self.rec.line("out", &s).await?;
        self.stdin.write_all(s.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn control(&mut self, subtype: &str, extra: Value) -> Result<String> {
        self.next_id += 1;
        let id = format!("dent-{}", self.next_id);
        self.send(wire::control_request(&id, subtype, extra))
            .await?;
        Ok(id)
    }
}

struct Approval {
    request_id: String,
    input: Value,
}

#[tokio::main]
async fn main() -> Result<()> {
    let a = args()?;
    let mut cmd = Command::new("claude");
    cmd.args([
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
    ])
    .args([
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
    ])
    .current_dir(&a.cwd)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::inherit())
    .kill_on_drop(true);
    if let Some(m) = &a.model {
        cmd.args(["--model", m]);
    }
    if let Some(r) = &a.resume {
        cmd.args(["--resume", r]);
    }
    let mut child = cmd.spawn().context("spawning `claude` (is it on PATH?)")?;
    let stdin = child.stdin.take().context("child stdin")?;
    let mut out = BufReader::new(child.stdout.take().context("child stdout")?).lines();

    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("recordings");
    tokio::fs::create_dir_all(&dir).await?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let path = dir.join(format!("{stamp}.ndjson"));
    let rec = Recorder {
        file: tokio::fs::File::create(&path).await?,
        start: Instant::now(),
    };
    eprintln!("recording to {}", path.display());

    let mut d = Driver {
        stdin,
        rec,
        next_id: 0,
    };
    let init_id = d.control("initialize", json!({})).await?;
    let mut pending_controls: HashMap<String, String> =
        HashMap::from([(init_id, "initialize".into())]);

    let mut tracker = Tracker::default();
    let mut approvals: VecDeque<Approval> = VecDeque::new();
    let mut last_status = tracker.status();
    let mut mid_text = false;
    let mut user_in = BufReader::new(tokio::io::stdin()).lines();

    loop {
        tokio::select! {
            line = out.next_line() => {
                let Some(line) = line? else { eprintln!("\n[claude exited]"); break };
                d.rec.line("in", &line).await?;
                let ev = match wire::parse(&line) {
                    Ok(ev) => ev,
                    Err(e) => { eprintln!("[unparseable line: {e}]"); continue }
                };
                if !matches!(ev, Inbound::TextDelta(_)) && mid_text {
                    println!();
                    mid_text = false;
                }
                let notify = tracker.on_event(&ev);
                render(&ev, &mut mid_text, &mut pending_controls);
                if let Inbound::CanUseTool { request_id, input, .. } = ev {
                    approvals.push_back(Approval { request_id, input });
                    println!("    approve? [y / n reason]");
                }
                if let Some(n) = notify {
                    println!("  🔔 notify: {}", match n {
                        Notify::Blocked => "blocked on approval (time-sensitive)",
                        Notify::Finished => "finished, waiting for you",
                        Notify::Error => "error",
                    });
                }
            }
            line = user_in.next_line() => {
                let Some(line) = line? else { break };
                let line = line.trim();
                if line.is_empty() { continue }
                if let Some(ap) = approvals.pop_front() {
                    let msg = match line.split_once(' ').map_or((line, ""), |(a, b)| (a, b)) {
                        ("y" | "yes", _) => wire::allow(&ap.request_id, &ap.input),
                        (_, reason) => wire::deny(&ap.request_id, if reason.is_empty() { "Denied by user" } else { reason }),
                    };
                    d.send(msg).await?;
                    tracker.on_approval_answered();
                } else if line == "/quit" {
                    break;
                } else if line == "/interrupt" {
                    let id = d.control("interrupt", json!({})).await?;
                    pending_controls.insert(id, "interrupt".into());
                } else if line == "/context" {
                    let id = d.control("get_context_usage", json!({})).await?;
                    pending_controls.insert(id, "get_context_usage".into());
                } else if let Some(mode) = line.strip_prefix("/mode ") {
                    let id = d.control("set_permission_mode", json!({ "mode": mode })).await?;
                    pending_controls.insert(id, "set_permission_mode".into());
                } else if let Some(model) = line.strip_prefix("/model ") {
                    let id = d.control("set_model", json!({ "model": model })).await?;
                    pending_controls.insert(id, "set_model".into());
                } else {
                    d.send(wire::user_message(line)).await?;
                    tracker.on_user_sent();
                }
            }
        }
        let s = tracker.status();
        if s != last_status {
            println!("  ── status: {s:?}");
            last_status = s;
        }
    }

    drop(d);
    let _ = child.wait().await;
    eprintln!("recorded {}", path.display());
    Ok(())
}

fn render(ev: &Inbound, mid_text: &mut bool, pending: &mut HashMap<String, String>) {
    match ev {
        Inbound::Init {
            session_id,
            model,
            version,
            permission_mode,
        } => {
            println!(
                "── turn · session {session_id} · {model} · claude {version} · mode {permission_mode}"
            );
        }
        Inbound::TextDelta(t) => {
            print!("{t}");
            *mid_text = true;
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
        Inbound::Assistant {
            parent_tool_use_id,
            blocks,
        } => {
            let indent = if parent_tool_use_id.is_some() {
                "│  "
            } else {
                ""
            };
            for b in blocks {
                match b {
                    // Text was already streamed as deltas.
                    AssistantBlock::Text(_) => {}
                    AssistantBlock::Thinking => println!("{indent}· thinking"),
                    AssistantBlock::ToolUse { name, input, .. } => {
                        let detail = input["command"]
                            .as_str()
                            .or(input["file_path"].as_str())
                            .or(input["description"].as_str())
                            .unwrap_or_default();
                        println!("{indent}├─ {name} {detail}");
                    }
                }
            }
        }
        Inbound::ToolResults(rs) => {
            for r in rs {
                let mark = if r.is_error { "✗" } else { "✓" };
                println!("│  └─ {mark} {}", r.preview);
            }
        }
        Inbound::RateLimit(info) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let windows: Vec<String> = info
                .unified_windows
                .iter()
                .map(|(k, w)| {
                    format!(
                        "{k} {:.0}% (resets in {}m)",
                        w.utilization * 100.0,
                        (w.resets_at - now) / 60
                    )
                })
                .collect();
            println!("  ▤ limits: {} · {}", info.status, windows.join(" · "));
        }
        Inbound::CanUseTool {
            tool_name,
            input,
            description,
            ..
        } => {
            println!(
                "  ■ needs you: {tool_name} — {}",
                description.clone().unwrap_or_else(|| input.to_string())
            );
        }
        Inbound::BackgroundTasks(tasks) => {
            println!(
                "  ◌ background tasks: {}",
                tasks
                    .iter()
                    .map(|t| t.description.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Inbound::TaskFinished { task_id, summary } => println!("  ◌ {task_id}: {summary}"),
        Inbound::ControlResponse {
            request_id,
            ok,
            body,
        } => {
            let what = pending
                .remove(request_id)
                .unwrap_or_else(|| request_id.clone());
            match (what.as_str(), ok) {
                ("initialize", true) => println!("  ⚙ initialized (claude pid {})", body["pid"]),
                ("get_context_usage", true) => println!(
                    "  ⚙ context: {} / {} tokens ({}%)",
                    body["totalTokens"], body["maxTokens"], body["percentage"]
                ),
                (_, true) => println!("  ⚙ {what}: ok {body}"),
                (_, false) => println!("  ⚙ {what}: error {body}"),
            }
        }
        Inbound::Result {
            subtype,
            terminal_reason,
            ..
        } => {
            println!(
                "── end of turn: {subtype} ({})",
                terminal_reason.as_deref().unwrap_or("-")
            );
        }
        Inbound::Status(_) | Inbound::Other { .. } => {}
    }
}
