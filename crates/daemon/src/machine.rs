//! How this machine is set up for what dent does besides agents, for
//! onboarding and Settings: Git (worktrees, checkpoints, commits), who
//! commits are by, the GitHub CLI (pull requests), and starting at login.
//! Every question is bounded, so a hung tool never holds up the answer.

use dent_protocol::{Gh, Machine};

use crate::agent::run_briefly;

/// Ask everything. Blocking: a few seconds at most.
pub fn check(path: &str) -> Machine {
    let env_run = |bin: &str, args: &[&str]| {
        let mut argv = vec![
            "/usr/bin/env".to_string(),
            format!("PATH={path}"),
            bin.to_string(),
        ];
        argv.extend(args.iter().map(|a| a.to_string()));
        run_briefly(&argv, 10).map(|s| s.trim().to_string())
    };
    let git = env_run("git", &["--version"]).filter(|v| !v.is_empty());
    let git_user = git.as_ref().and_then(|_| {
        // As a commit would take it: the environment first, then config.
        let pick = |var: &str, key: &str| {
            std::env::var(var)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| env_run("git", &["config", key]).filter(|v| !v.is_empty()))
        };
        identity(
            pick("GIT_AUTHOR_NAME", "user.name"),
            pick("GIT_AUTHOR_EMAIL", "user.email"),
        )
    });
    let gh = match crate::ship::gh(path) {
        None => Gh::Missing,
        Some(gh) => {
            let gh = gh.display().to_string();
            // `gh auth status` says who on stdout or stderr, by version;
            // its exit code says whether at all.
            let argv = [
                "/bin/sh".to_string(),
                "-c".into(),
                format!(
                    "{} auth status --hostname github.com 2>&1",
                    crate::agent::shell_word(&gh)
                ),
            ];
            match run_briefly(&argv, 10) {
                Some(out) => Gh::SignedIn {
                    user: gh_user(&out),
                },
                None => Gh::SignedOut,
            }
        }
    };
    Machine {
        git,
        git_user,
        gh,
        login_item: crate::login::is_on(),
        login_item_blocked: crate::login::blocked(),
    }
}

/// "Ada <ada@example.com>", from what Git would use; `None` when either is
/// missing, since a commit then fails.
fn identity(name: Option<String>, email: Option<String>) -> Option<String> {
    Some(format!("{} <{}>", name?.trim(), email?.trim()))
}

/// Who `gh auth status` says is signed in: "Logged in to github.com
/// account ada (keyring)", or in older versions "as ada".
fn gh_user(out: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let l = l.trim();
        let rest = l
            .split_once(" account ")
            .or_else(|| l.split_once("Logged in to github.com as "))?
            .1;
        let user = rest.split_whitespace().next()?;
        Some(
            user.trim_matches(|c: char| !c.is_alphanumeric() && c != '-')
                .to_string(),
        )
        .filter(|u| !u.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_commits_and_who_is_signed_in_to_github() {
        assert_eq!(
            identity(Some("Ada".into()), Some("ada@example.com".into())).as_deref(),
            Some("Ada <ada@example.com>")
        );
        assert_eq!(identity(Some("Ada".into()), None), None);
        let new = "github.com\n  ✓ Logged in to github.com account SarimDEV (keyring)\n  - Active account: true";
        assert_eq!(gh_user(new).as_deref(), Some("SarimDEV"));
        let old = "github.com\n  ✓ Logged in to github.com as ada (oauth_token)";
        assert_eq!(gh_user(old).as_deref(), Some("ada"));
        assert_eq!(gh_user("You are not logged into any GitHub hosts."), None);
    }
}
