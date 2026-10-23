//! A watch on how much memory each agent uses. Under systemd, the agents'
//! slice already makes running out of memory kill the agent rather than
//! the daemon ([decision 0065]). Nothing on macOS does that, and an agent
//! that runs away can take a Mac's whole memory down with it. So every
//! [`every`], each agent's process group is measured: past [`Limits::warn`]
//! its session says so once; past [`Limits::stop`] the agent is stopped and
//! its turn ends saying why.
//!
//! [decision 0065]: ../../docs/decisions/0065-launcher-and-updates.md

use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;

const GIB: u64 = 1 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Said once in the session.
    pub warn: u64,
    /// The agent is stopped.
    pub stop: u64,
}

/// 4 GiB to say so; half the machine's memory (never under 6 GiB) to stop.
/// `DENT_MEMORY_WARN` and `DENT_MEMORY_STOP` (bytes) set them instead.
pub fn limits() -> Limits {
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse().ok());
    let stop = (physical() / 2).max(6 * GIB);
    Limits {
        warn: env("DENT_MEMORY_WARN").unwrap_or(4 * GIB),
        stop: env("DENT_MEMORY_STOP").unwrap_or(stop),
    }
}

/// How often agents are measured; `DENT_MEMORY_EVERY_MS` for tests.
pub fn every() -> Duration {
    let ms = std::env::var("DENT_MEMORY_EVERY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    Duration::from_millis(ms)
}

/// The machine's memory, in bytes.
fn physical() -> u64 {
    // SAFETY: sysconf has no memory preconditions.
    let (pages, size) = unsafe {
        (
            libc::sysconf(libc::_SC_PHYS_PAGES),
            libc::sysconf(libc::_SC_PAGESIZE),
        )
    };
    if pages <= 0 || size <= 0 {
        return 16 * GIB;
    }
    pages as u64 * size as u64
}

/// Resident memory of each process group, in bytes, by group id: an agent
/// leads its own group, with everything it started. One `ps` for all of
/// them.
pub fn by_group() -> HashMap<u32, u64> {
    let Ok(out) = Command::new("ps").args(["-A", "-o", "pgid=,rss="]).output() else {
        return HashMap::new();
    };
    parse(&String::from_utf8_lossy(&out.stdout))
}

fn parse(ps: &str) -> HashMap<u32, u64> {
    let mut all = HashMap::new();
    for line in ps.lines() {
        let mut f = line.split_whitespace();
        if let (Some(Ok(group)), Some(Ok(kib))) = (
            f.next().map(str::parse::<u32>),
            f.next().map(str::parse::<u64>),
        ) {
            *all.entry(group).or_insert(0) += kib * 1024;
        }
    }
    all
}

/// "9.1 GB", as the session says it.
pub fn words(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GIB as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_is_everything_in_it() {
        let g = parse("  501  1000\n  501  24\n  77  8\nbad line\n");
        assert_eq!(g[&501], 1024 * 1024);
        assert_eq!(g[&77], 8 * 1024);
    }

    #[test]
    fn stopping_is_never_set_below_six_gib() {
        let l = limits();
        assert!(l.stop >= 6 * GIB || std::env::var("DENT_MEMORY_STOP").is_ok());
        assert!(l.warn < l.stop);
    }
}
