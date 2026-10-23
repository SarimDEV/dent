//! The environment agents run in. A daemon started by an app doesn't get the
//! person's shell PATH, so ask their login shell once, the way a terminal
//! would, and give agents that.

use std::process::{Command, Stdio};
use std::time::Duration;

/// What every agent runs with; which command each runs is its driver's
/// ([`crate::agent::Driver::command`]).
pub struct AgentEnv {
    pub path: String,
    /// Everything else agents get from the login shell (see [`passed`]),
    /// given to them through the holder, never on a command line, where
    /// `ps` would show it.
    pub vars: Vec<(String, String)>,
    /// Where `path` came from, for `dent-daemon doctor`.
    pub source: String,
}

/// What the login shell gives agents beside PATH: locale, certificates and
/// proxies, as T3 Code passes them, always. A service (no keychain, nothing
/// signed in but what the shell sets) also passes each agent's own
/// credentials (`Driver::env_names`), as running it over SSH would; the
/// app doesn't, so an API key in your shell profile never quietly moves its
/// sessions off your subscription.
fn passed(service: bool) -> Vec<&'static str> {
    let mut names = vec![
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "SSH_AUTH_SOCK",
        "HOMEBREW_PREFIX",
        "HOMEBREW_CELLAR",
        "HOMEBREW_REPOSITORY",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NODE_EXTRA_CA_CERTS",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "no_proxy",
    ];
    // Only where there's no keychain to sign in with: a Mac's LaunchAgent
    // runs in your login session, keychain and all, as the app does.
    if service && cfg!(target_os = "linux") {
        for d in crate::agent::DRIVERS {
            names.extend(d.env_names());
        }
    }
    names
}

impl AgentEnv {
    /// What agents of this daemon get: as a service when the launcher runs it.
    pub fn detect() -> Self {
        Self::detect_as(crate::launcher::get().is_some())
    }

    /// What agents would get run as a service, or not (for `doctor`, run
    /// from a terminal about the service).
    pub fn detect_as(service: bool) -> Self {
        let shell = login_shell();
        match login_env(&shell) {
            Some(env) => {
                let path = env
                    .iter()
                    .find(|(k, _)| k == "PATH")
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(fallback_path);
                Self {
                    path: dedupe(&path),
                    vars: pick(&env, &passed(service)),
                    source: format!("your login shell ({shell})"),
                }
            }
            None => Self {
                path: dedupe(&fallback_path()),
                vars: Vec::new(),
                source: format!("the usual folders ({shell} didn't say in 5 s)"),
            },
        }
    }
}

/// The named variables, in the order named, empty ones left out.
fn pick(env: &[(String, String)], names: &[&str]) -> Vec<(String, String)> {
    names
        .iter()
        .filter_map(|n| env.iter().find(|(k, v)| k == n && !v.is_empty()).cloned())
        .collect()
}

/// A PATH with each folder once, first place kept.
fn dedupe(path: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    path.split(':')
        .filter(|d| !d.is_empty() && seen.insert(*d))
        .collect::<Vec<_>>()
        .join(":")
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

/// The environment as a terminal would have it: an interactive login shell
/// (what `.bashrc` or `.zshrc` sets counts, as it does when you type
/// `claude`), read from `env` between markers, so whatever a profile prints
/// around it, and a shell like fish that keeps PATH as a list, don't get in.
fn login_env(shell: &str) -> Option<Vec<(String, String)>> {
    let mut cmd = Command::new(shell);
    cmd.args([
        "-i",
        "-l",
        "-c",
        &format!("echo {BEGIN}; /usr/bin/env; echo {END}"),
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    // A group of its own, so what a profile starts (an agent, a prompt
    // daemon) goes with it, and can't hold the pipe open.
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().ok()?;
    let group = child.id() as i32;
    // A slow or broken shell profile mustn't hold up the daemon.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut out = child.stdout.take()?;
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut out, &mut s);
        let _ = tx.send(s);
    });
    let got = rx.recv_timeout(Duration::from_secs(5)).ok();
    // SAFETY: signalling the group we just made.
    unsafe { libc::kill(-group, libc::SIGKILL) };
    let _ = child.wait();
    env_from(&got?)
}

/// `env` output between the markers, as pairs; `None` without a PATH.
/// (A value with a newline in it is cut there: none of the names passed
/// has one.)
fn env_from(out: &str) -> Option<Vec<(String, String)>> {
    let inside = out.split_once(BEGIN)?.1.split_once(END)?.0;
    let env: Vec<(String, String)> = inside
        .lines()
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    env.iter()
        .any(|(k, v)| k == "PATH" && v.contains('/'))
        .then_some(env)
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
    fn the_environment_is_read_between_the_markers_whatever_a_profile_prints() {
        let out = "Welcome to Ubuntu\nPATH=/wrong\n__DENT_ENV_BEGIN__\nHOME=/home/a\nPATH=/home/a/.local/bin:/usr/bin\nCLAUDE_CODE_OAUTH_TOKEN=sk-x=y\nLANG=\n__DENT_ENV_END__\nlogout\n";
        let env = super::env_from(out).unwrap();
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("PATH"), Some("/home/a/.local/bin:/usr/bin"));
        // A value may hold `=`.
        assert_eq!(get("CLAUDE_CODE_OAUTH_TOKEN"), Some("sk-x=y"));
        assert_eq!(super::env_from("PATH=/usr/bin\n"), None);
        // Only what's named, and nothing empty.
        let picked = super::pick(&env, &["LANG", "CLAUDE_CODE_OAUTH_TOKEN", "HOME"]);
        assert_eq!(picked.len(), 2);
        assert_eq!(picked[0].0, "CLAUDE_CODE_OAUTH_TOKEN");
    }

    #[test]
    fn a_path_keeps_each_folder_once_in_its_first_place() {
        assert_eq!(super::dedupe("/a:/b:/a::/c:/b"), "/a:/b:/c");
    }

    #[test]
    fn a_real_login_shell_gives_a_path() {
        let env = super::AgentEnv::detect();
        assert!(env.path.contains("/usr/bin"), "{}", env.path);
    }
}
