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

use dent_protocol::{Model, Window};
use serde_json::{Value, json};

use crate::env::AgentEnv;

/// What the CLI said: usage windows (`None` when it can't say: signed out,
/// an API key, or the CLI failed) and the models it offers (empty if it
/// didn't answer).
pub struct Probe {
    pub windows: Option<Vec<Window>>,
    pub models: Vec<Model>,
    /// The slash commands any session starts with: built-ins, the user's
    /// own commands and skills (not a project's, which need its folder).
    pub commands: Vec<dent_protocol::Command>,
}

/// Ask the CLI for the account's usage windows and its models.
pub fn probe(env: &AgentEnv) -> Probe {
    let none = || Probe {
        windows: None,
        models: Vec::new(),
        commands: Vec::new(),
    };
    let Some((init, usage)) = ask(env) else {
        return none();
    };
    Probe {
        windows: usage.as_ref().and_then(windows),
        models: init.as_ref().map(crate::claude::models).unwrap_or_default(),
        commands: init
            .as_ref()
            .map(crate::claude::commands)
            .unwrap_or_default(),
    }
}

/// The `initialize` and `get_usage` answers, each `None` if it didn't come.
fn ask(env: &AgentEnv) -> Option<(Option<Value>, Option<Value>)> {
    let mut child = Command::new("/usr/bin/env")
        .args(["-u", "CLAUDECODE", "-u", "CLAUDE_CODE_ENTRYPOINT"])
        .arg(format!("PATH={}", env.path))
        .arg(&env.claude)
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--strict-mcp-config",
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
    ] {
        let mut req = json!({ "type": "control_request", "request_id": id, "request": extra });
        req["request"]["subtype"] = json!(sub);
        writeln!(stdin, "{req}").ok()?;
    }
    let out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && v["type"] == "control_response"
            {
                let id = v["response"]["request_id"].as_str().unwrap_or_default();
                let done = id == "u-usage";
                let _ = tx.send((id.to_string(), v["response"]["response"].clone()));
                if done {
                    return;
                }
            }
        }
    });
    let until = Instant::now() + Duration::from_secs(20);
    let (mut init, mut usage) = (None, None);
    while usage.is_none() {
        let Ok((id, v)) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) else {
            break;
        };
        match id.as_str() {
            "u-init" => init = Some(v),
            _ => usage = Some(v),
        }
    }
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    Some((init, usage))
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
fn unix(s: &str) -> Option<i64> {
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
