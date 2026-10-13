//! `dent-session-holder --dir DIR [--idle-ms N]`. Started by the daemon.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use dent_session_holder::holder;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let mut o = holder::Options::new(PathBuf::from(get("--dir").context("--dir DIR")?));
    if let Some(ms) = get("--idle-ms").and_then(|s| s.parse().ok()) {
        o.idle = Duration::from_millis(ms);
    }
    holder::serve(o)
}
