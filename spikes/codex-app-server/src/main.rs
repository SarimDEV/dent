//! Spike: drive `codex app-server` over stdio with generated types.
//!
//! Type a message to start a turn (or steer the running one). While an
//! approval is pending, the next line answers it: `y` accept, `s` accept for
//! session, `n` decline. Commands: `/interrupt`, `/limits`, `/quit`.
//!
//! Every inbound message is decoded through the generated types; decode
//! failures are counted and printed at exit, which is the point of the spike.
//! Raw traffic is recorded to `recordings/`.

#[allow(dead_code, clippy::all)]
mod dispatch;
#[allow(dead_code, clippy::all)]
mod generated;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use dispatch::{IncomingRequest, Notification, Request};
use generated::*;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};

struct Args {
    cwd: PathBuf,
    model: Option<String>,
}

fn args() -> Result<Args> {
    let mut a = Args {
        cwd: std::env::current_dir()?,
        model: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--cwd" => a.cwd = val()?.into(),
            "--model" => a.model = Some(val()?),
            other => bail!("unknown flag {other} (expected --cwd, --model)"),
        }
    }
    Ok(a)
}

#[derive(Default)]
struct Stats {
    messages: usize,
    unknown_methods: Vec<String>,
    decode_errors: Vec<String>,
}

struct Rpc {
    stdin: ChildStdin,
    rec: tokio::fs::File,
    start: Instant,
    next_id: i64,
    /// Request id -> method, so responses can be decoded with the right type.
    pending: HashMap<i64, &'static str>,
}

impl Rpc {
    async fn write(&mut self, v: Value) -> Result<()> {
        let s = v.to_string();
        self.record("out", &s).await?;
        self.stdin.write_all(format!("{s}\n").as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn record(&mut self, dir: &str, raw: &str) -> Result<()> {
        let rec = json!({ "t": self.start.elapsed().as_secs_f64(), "dir": dir, "line": raw });
        self.rec.write_all(format!("{rec}\n").as_bytes()).await?;
        Ok(())
    }

    async fn call<R: Request>(&mut self, params: &R) -> Result<()> {
        self.call_raw(R::METHOD, serde_json::to_value(params)?)
            .await
    }

    async fn call_raw(&mut self, method: &'static str, params: Value) -> Result<()> {
        self.next_id += 1;
        self.pending.insert(self.next_id, method);
        let mut msg = json!({ "id": self.next_id, "method": method });
        if !params.is_null() {
            msg["params"] = params;
        }
        self.write(msg).await
    }
}

struct Session {
    thread_id: Option<String>,
    turn_id: Option<String>,
    approvals: VecDeque<(RequestId, &'static str)>,
    mid_text: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let a = args()?;
    let mut child = Command::new("codex")
        .arg("app-server")
        .current_dir(&a.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .context("spawning `codex app-server` (is codex on PATH?)")?;
    let stdin = child.stdin.take().context("child stdin")?;
    let mut out = BufReader::new(child.stdout.take().context("child stdout")?).lines();

    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("recordings");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!(
        "{}.ndjson",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
    ));
    let rec = tokio::fs::File::create(&path).await?;
    eprintln!("recording to {}", path.display());

    let mut rpc = Rpc {
        stdin,
        rec,
        start: Instant::now(),
        next_id: 0,
        pending: HashMap::new(),
    };
    rpc.call(&InitializeParams {
        capabilities: None,
        client_info: ClientInfo {
            name: "dent-spike".into(),
            title: None,
            version: "0.0.0".into(),
        },
    })
    .await?;
    rpc.write(json!({ "method": "initialized" })).await?;
    rpc.call(&GetAccountParams {
        refresh_token: Some(false),
    })
    .await?;
    rpc.call_raw("account/rateLimits/read", Value::Null).await?;
    rpc.call(&ThreadStartParams {
        cwd: Some(a.cwd.display().to_string()),
        model: a.model.clone(),
        approval_policy: Some(AskForApproval::Untrusted),
        sandbox: Some(SandboxMode::WorkspaceWrite),
        ..Default::default()
    })
    .await?;

    let mut stats = Stats::default();
    let mut s = Session {
        thread_id: None,
        turn_id: None,
        approvals: VecDeque::new(),
        mid_text: false,
    };
    let mut user_in = BufReader::new(tokio::io::stdin()).lines();

    loop {
        tokio::select! {
            line = out.next_line() => {
                let Some(line) = line? else { eprintln!("\n[codex exited]"); break };
                rpc.record("in", &line).await?;
                stats.messages += 1;
                if let Err(e) = handle(&line, &mut rpc, &mut s, &mut stats) {
                    stats.decode_errors.push(format!("{e:#}"));
                    println!("  ✗ decode error: {e:#}");
                }
            }
            line = user_in.next_line() => {
                let Some(line) = line? else { break };
                let line = line.trim();
                if line.is_empty() { continue }
                if let Some((id, method)) = s.approvals.pop_front() {
                    let decision = match line { "y" => "accept", "s" => "acceptForSession", _ => "decline" };
                    let _ = method;
                    rpc.write(json!({ "id": id, "result": { "decision": decision } })).await?;
                } else if line == "/quit" {
                    break;
                } else if line == "/limits" {
                    rpc.call_raw("account/rateLimits/read", Value::Null).await?;
                } else if line == "/interrupt" {
                    if let (Some(thread_id), Some(turn_id)) = (s.thread_id.clone(), s.turn_id.clone()) {
                        rpc.call(&TurnInterruptParams { thread_id, turn_id }).await?;
                    }
                } else if let Some(thread_id) = s.thread_id.clone() {
                    let input = vec![UserInput::TextUserInput {
                        text: line.into(),
                        text_elements: vec![],
                        type_: TextUserInputType::Text,
                    }];
                    match s.turn_id.clone() {
                        // A turn is running: steer it.
                        Some(turn_id) => {
                            rpc.call_raw("turn/steer", json!({ "threadId": thread_id, "expectedTurnId": turn_id, "input": input })).await?;
                        }
                        None => {
                            // Required fields mean no Default; go through JSON for the optional rest.
                            let p: TurnStartParams = serde_json::from_value(json!({ "threadId": thread_id, "input": input }))?;
                            rpc.call(&p).await?
                        }
                    }
                } else {
                    println!("  (no thread yet)");
                }
            }
        }
    }

    println!(
        "\n── {} messages · {} decode errors · unknown methods: {:?}",
        stats.messages,
        stats.decode_errors.len(),
        stats.unknown_methods
    );
    for e in &stats.decode_errors {
        println!("   ✗ {e}");
    }
    drop(rpc);
    let _ = child.kill().await;
    eprintln!("recorded {}", path.display());
    Ok(())
}

fn handle(line: &str, rpc: &mut Rpc, s: &mut Session, stats: &mut Stats) -> Result<()> {
    let v: Value = serde_json::from_str(line)?;
    let method = v["method"].as_str();
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    match (method, v.get("id")) {
        (Some(m), Some(id)) => {
            let req = IncomingRequest::parse(m, params).with_context(|| format!("request {m}"))?;
            let id: RequestId = serde_json::from_value(id.clone())?;
            on_request(req, id, s, stats);
        }
        (Some(m), None) => {
            let n = Notification::parse(m, params).with_context(|| format!("notification {m}"))?;
            on_notification(n, s, stats);
        }
        (None, Some(id)) => {
            let id = id.as_i64().context("non-integer response id")?;
            let method = rpc.pending.remove(&id).unwrap_or("?");
            if let Some(err) = v.get("error") {
                println!("  ✗ {method}: {err}");
                return Ok(());
            }
            on_response(method, v["result"].clone(), s)
                .with_context(|| format!("response to {method}"))?;
        }
        (None, None) => bail!("neither method nor id"),
    }
    Ok(())
}

fn end_text(s: &mut Session) {
    if s.mid_text {
        println!();
        s.mid_text = false;
    }
}

fn on_response(method: &str, result: Value, s: &mut Session) -> Result<()> {
    end_text(s);
    match method {
        "initialize" => println!("  ⚙ initialized: {}", result["userAgent"]),
        "account/read" => {
            let r: GetAccountResponse = serde_json::from_value(result)?;
            // Don't print the email; plan type is enough.
            let plan = serde_json::to_value(&r.account)?["planType"].clone();
            println!("  ⚙ account: plan {plan}");
        }
        "account/rateLimits/read" => {
            let r: GetAccountRateLimitsResponse = serde_json::from_value(result)?;
            print_limits(&r.rate_limits);
        }
        "thread/start" => {
            let r: ThreadStartResponse = serde_json::from_value(result)?;
            println!(
                "── thread {} · {} · approvals {:?}",
                r.thread.id, r.model, r.approval_policy
            );
            s.thread_id = Some(r.thread.id);
        }
        "turn/start" => {
            let r: TurnStartResponse = serde_json::from_value(result)?;
            s.turn_id = Some(r.turn.id);
        }
        other => println!("  ⚙ {other}: ok"),
    }
    Ok(())
}

fn print_limits(r: &RateLimitSnapshot) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let fmt = |w: &RateLimitWindow| {
        let label = match w.window_duration_mins {
            Some(300) => "5h".to_string(),
            Some(10080) => "week".to_string(),
            Some(m) => format!("{}d", m / 1440),
            None => "?".to_string(),
        };
        let resets = w
            .resets_at
            .map(|t| format!(" (resets in {}h)", (t - now) / 3600))
            .unwrap_or_default();
        format!("{label} {}%{resets}", w.used_percent)
    };
    let windows: Vec<String> = [&r.primary, &r.secondary]
        .into_iter()
        .flatten()
        .map(fmt)
        .collect();
    println!("  ▤ limits: {}", windows.join(" · "));
}

fn on_request(req: IncomingRequest, id: RequestId, s: &mut Session, stats: &mut Stats) {
    end_text(s);
    match req {
        IncomingRequest::ItemCommandExecutionRequestApproval(p) => {
            println!(
                "  ■ needs you: run `{}` — {}",
                p.command.unwrap_or_default(),
                p.reason.unwrap_or_default()
            );
            println!("    approve? [y / s (session) / n]");
            s.approvals.push_back((id, "command"));
            println!("  🔔 notify: blocked on approval (time-sensitive)");
        }
        IncomingRequest::ItemFileChangeRequestApproval(p) => {
            println!(
                "  ■ needs you: file change — {}",
                p.reason.unwrap_or_default()
            );
            println!("    approve? [y / s (session) / n]");
            s.approvals.push_back((id, "fileChange"));
            println!("  🔔 notify: blocked on approval (time-sensitive)");
        }
        IncomingRequest::Unknown { method, .. } => stats.unknown_methods.push(method),
        other => println!("  ? unhandled request {}", other.method()),
    }
}

fn on_notification(n: Notification, s: &mut Session, stats: &mut Stats) {
    if !matches!(n, Notification::ItemAgentMessageDelta(_)) {
        end_text(s);
    }
    match n {
        Notification::ItemAgentMessageDelta(d) => {
            print!("{}", d.delta);
            s.mid_text = true;
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
        Notification::TurnStarted(t) => s.turn_id = Some(t.turn.id),
        Notification::TurnCompleted(t) => {
            s.turn_id = None;
            println!("── end of turn: {}", t.turn.status);
            match t.turn.status {
                TurnStatus::Completed => println!("  🔔 notify: finished, waiting for you"),
                TurnStatus::Failed => println!("  🔔 notify: error"),
                _ => {}
            }
        }
        Notification::ThreadStatusChanged(c) => {
            let status = match c.status {
                ThreadStatus::Active(flags)
                    if flags
                        .iter()
                        .any(|f| matches!(f, ThreadActiveFlag::WaitingOnApproval)) =>
                {
                    "Blocked"
                }
                ThreadStatus::Active(_) => "Working",
                ThreadStatus::Idle => "Idle",
                ThreadStatus::SystemError => "Errored",
                ThreadStatus::NotLoaded => "NotLoaded",
            };
            println!("  ── status: {status}");
        }
        Notification::ItemStarted(i) => {
            let item = serde_json::to_value(&i.item).unwrap_or_default();
            match item["type"].as_str() {
                Some("commandExecution") => {
                    println!("├─ $ {}", item["command"].as_str().unwrap_or_default())
                }
                Some("fileChange") => println!("├─ edit {}", item["changes"]),
                Some("reasoning") => println!("· thinking"),
                _ => {}
            }
        }
        Notification::ItemCompleted(i) => {
            let item = serde_json::to_value(&i.item).unwrap_or_default();
            if item["type"] == "commandExecution" {
                let out = item["aggregatedOutput"].as_str().unwrap_or_default();
                println!(
                    "│  └─ exit {} {}",
                    item["exitCode"],
                    out.lines().next().unwrap_or_default()
                );
            }
        }
        Notification::AccountRateLimitsUpdated(r) => print_limits(&r.rate_limits),
        Notification::ServerRequestResolved(_) => println!("  ⚙ request resolved"),
        Notification::Error(e) => println!(
            "  ✗ error: {}",
            serde_json::to_value(&e).unwrap_or_default()
        ),
        Notification::Unknown { method, .. } => stats.unknown_methods.push(method),
        _ => {}
    }
}
