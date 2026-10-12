//! Spike 06: the session holder surviving a daemon upgrade.
//!
//! `cargo run -p spike-holder -- test` runs every scenario with fake agents.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use spike_holder::{agent, daemon, harness, holder};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let num = |flag: &str, d: u64| get(flag).and_then(|s| s.parse().ok()).unwrap_or(d);
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let all = |flag: &str| -> Vec<String> {
        args.windows(2)
            .filter(|w| w[0] == flag)
            .map(|w| w[1].clone())
            .collect()
    };
    match args.first().map(String::as_str) {
        Some("holder") => {
            let mut o = holder::Options::new(PathBuf::from(get("--dir").context("--dir")?));
            o.idle = Duration::from_millis(num("--idle-ms", 60_000));
            o.proto = num("--proto", o.proto as u64) as u32;
            if let Some(v) = get("--version") {
                o.version = v;
            }
            holder::serve(o)
        }
        Some("agent") => std::process::exit(agent::run(agent::Opts {
            id: get("--id").unwrap_or_default(),
            lines: num("--lines", 0) as usize,
            every_us: num("--every-us", 0),
            size: num("--size", 64) as usize,
            binary: num("--binary", 0) as usize,
            stamp: has("--stamp"),
            echo: has("--echo"),
            ignore_term: has("--ignore-term"),
            child: has("--child"),
            linger: has("--linger"),
            exit: num("--exit", 0) as i32,
        })),
        Some("daemon") => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(daemon::run(daemon::Opts {
                dir: PathBuf::from(get("--dir").context("--dir")?),
                copy: PathBuf::from(get("--copy").context("--copy")?),
                agents: num("--agents", 1) as usize,
                agent_args: get("--agent-args")
                    .map(|s| s.split_whitespace().map(str::to_string).collect())
                    .unwrap_or_default(),
                send: all("--send"),
                close: has("--close"),
                until_done: has("--until-done"),
                want: num("--want", 0) as u32,
                holder_args: get("--holder-args")
                    .map(|s| s.split_whitespace().map(str::to_string).collect())
                    .unwrap_or_default(),
                direct: has("--direct"),
                stop_after_ms: get("--stop-after-ms").and_then(|s| s.parse().ok()),
                grace_ms: num("--grace-ms", 2000),
            }))
        }
        Some("test") => harness::run(args.get(1).map(String::as_str)),
        _ => {
            eprintln!(
                "usage: spike-holder test [name|launchd] | holder --dir D | daemon --dir D --copy C ... | agent ..."
            );
            Ok(())
        }
    }
}
