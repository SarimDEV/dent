//! Account usage, read through the agent's own CLI ([decision 0031]): a
//! short-lived `claude` answers `get_usage` from the provider, so the
//! reading counts every way the account is used. No model is called. The
//! same short-lived CLI also says which models the account has, with their
//! versions, so the model menu is right before any session starts.
//!
//! [decision 0031]: ../../docs/decisions/0031-usage-limits-through-agent-clis.md

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dent_protocol::Window;
use serde_json::{Value, json};

use crate::agent::{Catalog, Probe};
use crate::env::AgentEnv;

/// Ask the CLI for the account's usage windows and its models.
pub fn probe(env: &AgentEnv, claude: &str) -> Probe {
    let Some(Answers {
        init,
        usage,
        config,
        settings,
    }) = ask(env, claude)
    else {
        return Probe::default();
    };
    let mut catalog = Catalog::default();
    if let Some(text) = config {
        catalog.settings = crate::claude::commands::settings_from_usage(&text);
    }
    if let Some(s) = settings {
        catalog.effective = s["effective"].clone();
        catalog.applied = s["applied"].clone();
    }
    if let Some(init) = &init {
        (catalog.styles, catalog.style) = crate::claude::styles(init);
    }
    Probe {
        catalog,
        windows: usage.as_ref().and_then(windows),
        models: init.as_ref().map(crate::claude::models).unwrap_or_default(),
        commands: init
            .as_ref()
            .map(crate::claude::commands)
            .unwrap_or_default(),
    }
}

/// What the short-lived CLI answered; each `None` if it didn't.
struct Answers {
    init: Option<Value>,
    usage: Option<Value>,
    /// `/config`'s usage text: every setting it takes.
    config: Option<String>,
    settings: Option<Value>,
}

/// Ask the CLI: `initialize`, `get_usage`, `get_settings`, and `/config`
/// with nothing after it, which the CLI answers itself with no model call.
/// Nothing is saved (`--no-session-persistence`) and the person's hooks
/// don't run for it.
fn ask(env: &AgentEnv, claude: &str) -> Option<Answers> {
    let mut child = Command::new("/usr/bin/env")
        .args(["-u", "CLAUDECODE", "-u", "CLAUDE_CODE_ENTRYPOINT"])
        .arg(format!("PATH={}", env.path))
        .arg(claude)
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--strict-mcp-config",
            "--settings",
            r#"{"disableAllHooks":true}"#,
        ])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    for (id, sub, extra) in [
        ("u-init", "initialize", json!({})),
        ("u-usage", "get_usage", json!({ "skip_behaviors": true })),
        ("u-settings", "get_settings", json!({})),
    ] {
        let mut req = json!({ "type": "control_request", "request_id": id, "request": extra });
        req["request"]["subtype"] = json!(sub);
        writeln!(stdin, "{req}").ok()?;
    }
    let config = json!({
        "type": "user",
        "message": { "role": "user", "content": "/config" },
        "parent_tool_use_id": null,
        "session_id": "",
    });
    writeln!(stdin, "{config}").ok()?;
    let out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v["type"] == "control_response" {
                let id = v["response"]["request_id"].as_str().unwrap_or_default();
                let _ = tx.send((id.to_string(), v["response"]["response"].clone()));
            } else if v["type"] == "result" {
                let _ = tx.send(("config".into(), v["result"].clone()));
                return;
            }
        }
    });
    let until = Instant::now() + Duration::from_secs(20);
    let mut a = Answers {
        init: None,
        usage: None,
        config: None,
        settings: None,
    };
    while a.usage.is_none() || a.config.is_none() || a.settings.is_none() {
        let Ok((id, v)) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) else {
            break;
        };
        match id.as_str() {
            "u-init" => a.init = Some(v),
            "u-usage" => a.usage = Some(v),
            "u-settings" => a.settings = Some(v),
            _ => a.config = Some(v.as_str().unwrap_or_default().to_string()),
        }
    }
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    Some(a)
}

fn windows(usage: &Value) -> Option<Vec<Window>> {
    let rl = usage["rate_limits"].as_object()?;
    let w: Vec<Window> = ["five_hour", "seven_day"]
        .iter()
        .filter_map(|name| {
            let w = rl.get(*name)?;
            Some(Window {
                name: (*name).into(),
                // get_usage says percent; the stream's events say 0 to 1.
                used: w["utilization"].as_f64()? / 100.,
                resets_at: w["resets_at"].as_str().and_then(unix).unwrap_or(0),
            })
        })
        .collect();
    (!w.is_empty()).then_some(w)
}

/// Unix seconds from an RFC 3339 time like `2026-10-12T06:10:00.43+00:00`.
pub fn unix(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let time: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ':')
        .collect();
    let mut t = time.split(':').map(|x| x.parse::<i64>());
    let (hh, mm, ss) = (
        t.next()?.ok()?,
        t.next()?.ok()?,
        t.next()?.ok().unwrap_or(0),
    );
    let tz = &rest[time.len()..];
    let tz = tz.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match tz.as_bytes().first() {
        Some(b'+') | Some(b'-') => {
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            let mut p = tz[1..].split(':').map(|x| x.parse::<i64>());
            sign * (p.next()?.ok()? * 3600 + p.next().and_then(|x| x.ok()).unwrap_or(0) * 60)
        }
        _ => 0,
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss - offset)
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_rfc3339() {
        assert_eq!(super::unix("1970-01-01T00:00:00+00:00"), Some(0));
        assert_eq!(
            super::unix("2026-10-12T06:10:00.433002+00:00"),
            Some(1_791_785_400)
        );
        assert_eq!(
            super::unix("2026-10-12T08:10:00+02:00"),
            Some(1_791_785_400)
        );
    }

    #[test]
    fn windows_from_percent() {
        let v = serde_json::json!({"rate_limits": {"five_hour": {"utilization": 25, "resets_at": "1970-01-01T01:00:00Z"}, "seven_day": null}});
        let w = super::windows(&v).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].used, 0.25);
        assert_eq!(w[0].resets_at, 3600);
    }
}
