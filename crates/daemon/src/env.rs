//! The environment agents run in. A daemon started by an app doesn't get the
//! person's shell PATH, so ask their login shell once, the way a terminal
//! would, and give agents that.

use std::process::{Command, Stdio};
use std::time::Duration;

/// What every agent runs with; which command each runs is its driver's
/// ([`crate::agent::Driver::command`]).
pub struct AgentEnv {
    pub path: String,
    /// Where `path` came from, for `dent-daemon doctor`.
    pub source: String,
}

impl AgentEnv {
    pub fn detect() -> Self {
        let shell = login_shell();
        match login_path(&shell) {
            Some(path) => Self {
                path,
                source: format!("your login shell ({shell})"),
            },
            None => Self {
                path: fallback_path(),
                source: format!("the usual folders ({shell} didn't say in 5 s)"),
            },
        }
    }
}

/// The account's login shell, from the user database: a service starts
/// with no `SHELL` at all.
pub fn login_shell() -> String {
    // SAFETY: getpwuid returns a pointer into static storage or null; it's
    // read at once, on this thread.
    let from_db = unsafe {
        let pw = libc::getpwuid(libc::getuid());
        (!pw.is_null() && !(*pw).pw_shell.is_null()).then(|| {
            std::ffi::CStr::from_ptr((*pw).pw_shell)
                .to_string_lossy()
                .into_owned()
        })
    };
    from_db
        .filter(|s| std::path::Path::new(s).is_file())
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/sh".into())
}

const BEGIN: &str = "__DENT_ENV_BEGIN__";
const END: &str = "__DENT_ENV_END__";

/// PATH as a terminal would have it: an interactive login shell (PATH set
/// in `.bashrc` or `.zshrc` counts, as it does when you type `claude`), read
/// from `env` between markers, so whatever a profile prints around it, and
/// a shell like fish that keeps PATH as a list, don't get in.
fn login_path(shell: &str) -> Option<String> {
    let mut child = Command::new(shell)
        .args([
            "-i",
            "-l",
            "-c",
            &format!("echo {BEGIN}; /usr/bin/env; echo {END}"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // A slow or broken shell profile mustn't hold up the daemon.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut out = child.stdout.take()?;
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut out, &mut s);
        let _ = tx.send(s);
    });
    let got = rx.recv_timeout(Duration::from_secs(5)).ok();
    let _ = child.kill();
    let _ = child.wait();
    path_from(&got?)
}

/// PATH from `env` output between the markers.
fn path_from(out: &str) -> Option<String> {
    let inside = out.split_once(BEGIN)?.1.split_once(END)?.0;
    inside
        .lines()
        .find_map(|l| l.strip_prefix("PATH="))
        .map(str::to_string)
        .filter(|p| p.contains('/'))
}

fn fallback_path() -> String {
    let home = crate::paths::home();
    let mut dirs: Vec<String> = [".local/bin", ".claude/local", ".bun/bin", ".cargo/bin"]
        .iter()
        .map(|d| home.join(d).display().to_string())
        .collect();
    dirs.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ]
        .map(String::from),
    );
    if let Ok(p) = std::env::var("PATH") {
        dirs.push(p);
    }
    dirs.join(":")
}

#[cfg(test)]
mod tests {
    #[test]
    fn path_is_read_between_the_markers_whatever_a_profile_prints() {
        let out = "Welcome to Ubuntu\nPATH=/wrong\n__DENT_ENV_BEGIN__\nHOME=/home/a\nPATH=/home/a/.local/bin:/usr/bin\n__DENT_ENV_END__\nlogout\n";
        assert_eq!(
            super::path_from(out).as_deref(),
            Some("/home/a/.local/bin:/usr/bin")
        );
        assert_eq!(super::path_from("PATH=/usr/bin\n"), None);
    }

    #[test]
    fn a_real_login_shell_gives_a_path() {
        let env = super::AgentEnv::detect();
        assert!(env.path.contains("/usr/bin"), "{}", env.path);
    }
}
