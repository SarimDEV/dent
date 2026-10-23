//! What every daemon test harness shares: finding, and stopping, what a
//! test daemon left running.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The holder's folders for a state folder, where the daemon looks
/// (`paths.rs`): the user's runtime folder on Linux, and the temporary one.
pub fn holder_dirs(state: &Path) -> Vec<PathBuf> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in state.as_os_str().as_encoded_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let name = format!("dent-{h:016x}");
    let tmp = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    // SAFETY: getuid can't fail.
    let runtime = PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }));
    let mut all = vec![tmp.join(&name)];
    if runtime.is_dir() {
        all.insert(0, runtime.join(&name));
    }
    all
}

/// The holder's pid, from its lock file.
pub fn holder_pid(state: &Path) -> Option<i32> {
    holder_dirs(state).iter().find_map(|d| {
        std::fs::read_to_string(d.join("holder.lock"))
            .ok()?
            .trim()
            .parse()
            .ok()
    })
}

/// The pids of the agents the holder started, from its folder.
pub fn agent_pids(state: &Path) -> Vec<i32> {
    holder_dirs(state)
        .iter()
        .flat_map(|d| {
            std::fs::read_dir(d.join("procs"))
                .into_iter()
                .flatten()
                .flatten()
        })
        .filter_map(|e| std::fs::read_to_string(e.path().join("pid")).ok())
        .filter_map(|s| s.split_whitespace().next()?.parse().ok())
        .collect()
}

/// Stops the holder and every agent it started. Agents lead groups of
/// their own and outlive the holder, so a test that only stops its daemon
/// leaves them running. Only fake agents are stopped: a pid used again by
/// something else is left alone.
pub fn stop_all(state: &Path) {
    if let Some(pid) = holder_pid(state) {
        // SAFETY: kill has no memory preconditions.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    for pid in agent_pids(state) {
        let comm = std::process::Command::new("ps")
            .args(["-o", "comm=", "-p", &pid.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        if comm.contains("dent-fake-claude") {
            // SAFETY: as above; a negative pid is the agent's group.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
    }
}
