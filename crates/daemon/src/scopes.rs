//! Under systemd (the service on Linux), what the daemon starts runs in
//! scopes of its own, outside the service's cgroup: the holder, so stopping
//! the service never reaches it, and each agent, under `dentagents.slice`,
//! so running out of memory kills only that agent, and the slice's
//! `oom_kill` count tells "out of memory" from a plain kill
//! ([spike 13](../../../docs/spikes/13-service-and-updates.md)).

/// The slice every agent's scope goes in.
const SLICE: &str = "dentagents.slice";

/// Started by a systemd unit (systemd sets `INVOCATION_ID` for what a
/// unit runs, and it's inherited), with `systemd-run` there to use.
pub fn under_systemd() -> bool {
    static YES: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        cfg!(target_os = "linux")
            && std::env::var_os("INVOCATION_ID").is_some()
            && std::process::Command::new("systemd-run")
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
    });
    *YES
}

/// `argv` run in a scope of its own (in `slice`, when given), when under
/// systemd; as it is otherwise. The scope keeps the same process (systemd
/// registers it, then runs the command in place), so its pid and pipes
/// are the command's.
fn scoped(mut argv: Vec<String>, slice: Option<&str>) -> Vec<String> {
    if !under_systemd() {
        return argv;
    }
    let mut pre: Vec<String> = ["systemd-run", "--user", "--scope", "--quiet", "--collect"]
        .map(String::from)
        .into();
    if let Some(s) = slice {
        pre.push(format!("--slice={s}"));
    }
    pre.push("--".into());
    argv.splice(0..0, pre);
    argv
}

/// The holder, outside the service's cgroup.
pub fn holder(argv: Vec<String>) -> Vec<String> {
    scoped(argv, None)
}

/// An agent, in a scope of its own under the agents' slice.
pub fn agent(argv: Vec<String>) -> Vec<String> {
    scoped(argv, Some(SLICE))
}

/// How many times the kernel has killed something in the agents' slice for
/// memory: read as an agent starts and again when it's killed, a rise
/// means it ran out of memory. `None` when there's no slice to read.
pub fn oom_kills() -> Option<u64> {
    if !under_systemd() {
        return None;
    }
    let events = std::fs::read_to_string(slice_dir()?.join("memory.events")).ok()?;
    parse_oom_kills(&events)
}

/// The slice's cgroup folder: beside the service's own, under the user's
/// manager (`…/user@1000.service/dentagents.slice`).
fn slice_dir() -> Option<std::path::PathBuf> {
    let me = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = me.lines().find_map(|l| l.strip_prefix("0::"))?;
    let at = path.find(".service/")?;
    let manager = &path[..at + ".service".len()];
    Some(
        std::path::Path::new("/sys/fs/cgroup")
            .join(manager.trim_start_matches('/'))
            .join(SLICE),
    )
}

fn parse_oom_kills(events: &str) -> Option<u64> {
    events
        .lines()
        .find_map(|l| l.strip_prefix("oom_kill "))
        .and_then(|n| n.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_kill_count_is_read_from_memory_events() {
        let e = "low 0\nhigh 0\nmax 3\noom 2\noom_kill 2\noom_group_kill 0\n";
        assert_eq!(super::parse_oom_kills(e), Some(2));
        assert_eq!(super::parse_oom_kills("low 0\n"), None);
    }
}
