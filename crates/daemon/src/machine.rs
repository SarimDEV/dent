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
        // As a commit would take it: the environment first, even set to
        // nothing (which Git refuses to commit with), then config.
        let pick = |var: &str, key: &str| match std::env::var(var) {
            Ok(v) => Some(v).filter(|v| !v.trim().is_empty()),
            Err(_) => env_run("git", &["config", key]).filter(|v| !v.is_empty()),
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
        git_user_settable: true,
        gh,
        login_item: crate::login::is_on(),
        login_item_blocked: crate::login::blocked(),
        // The daemon says these.
        lan: false,
        defaults: None,
        defaults_set: false,
        project_defaults: Default::default(),
    }
}

/// Make `name <email>` who commits here are by, in Git's global settings.
/// Refused when dent's environment names someone, which commits would take
/// instead.
pub fn set_git_user(path: &str, name: &str, email: &str) -> anyhow::Result<()> {
    let (name, email) = (name.trim(), email.trim());
    check_git_user(name, email)?;
    for var in ["GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL"] {
        if std::env::var_os(var).is_some() {
            anyhow::bail!("dent runs with {var} set there, which commits take instead");
        }
    }
    // One at a time: two devices saving at once would leave one's name
    // with the other's address.
    static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    // Another program writing Git's settings holds its lock a moment:
    // tried again, briefly.
    let git = |args: &[&str]| {
        let mut tries = 0;
        loop {
            let out = std::process::Command::new("/usr/bin/env")
                .arg(format!("PATH={path}"))
                .arg("git")
                .args(args)
                .stdin(std::process::Stdio::null())
                .output()?;
            tries += 1;
            let locked = String::from_utf8_lossy(&out.stderr).contains("could not lock");
            if !locked || tries == 10 {
                return std::io::Result::Ok(out);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    // Both or neither: a name saved with the old address is someone else.
    let was = git(&["config", "--global", "user.name"])?;
    let was = was
        .status
        .success()
        .then(|| String::from_utf8_lossy(&was.stdout).trim().to_string());
    let failed = |out: &std::process::Output| {
        anyhow::anyhow!(
            "git couldn't save it: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
    };
    let out = git(&["config", "--global", "user.name", name])?;
    if !out.status.success() {
        return Err(failed(&out));
    }
    let out = git(&["config", "--global", "user.email", email])?;
    if !out.status.success() {
        let _ = match &was {
            Some(old) => git(&["config", "--global", "user.name", old]),
            None => git(&["config", "--global", "--unset", "user.name"]),
        };
        return Err(failed(&out));
    }
    Ok(())
}

/// The longest name, and address (as mail allows), Git is given.
const NAME_MAX: usize = 200;
const EMAIL_MAX: usize = 254;

/// Whether `name <email>` is fit for commits, and why not.
fn check_git_user(name: &str, email: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        anyhow::bail!("a name is needed");
    }
    if name.chars().count() > NAME_MAX {
        anyhow::bail!("a name can be at most {NAME_MAX} characters");
    }
    if email.chars().count() > EMAIL_MAX {
        anyhow::bail!("an email can be at most {EMAIL_MAX} characters");
    }
    // Git takes < and > as the bounds of the address; control characters
    // (line breaks, tabs, terminal colours) would show up in every log.
    if [name, email]
        .iter()
        .any(|v| v.contains(['<', '>']) || v.chars().any(char::is_control))
    {
        anyhow::bail!("a name or email can't have < >, line breaks or other control characters");
    }
    if !looks_like_email(email) {
        anyhow::bail!("“{email}” isn't an email address");
    }
    Ok(())
}

fn looks_like_email(s: &str) -> bool {
    s.split_once('@').is_some_and(|(a, b)| {
        !a.is_empty() && !b.is_empty() && !b.contains('@') && !s.contains(char::is_whitespace)
    })
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

#[cfg(test)]
mod git_user_tests {
    use super::check_git_user;

    #[test]
    fn only_a_name_and_address_fit_for_commits_go_in() {
        for (name, email) in [
            ("Ada Lovelace", "ada@example.com"),
            ("Zoë Ångström 李小龙", "zoe@exämple.com"),
            ("🦀 Crab", "a+tag@sub.example.co.uk"),
            ("O'Brien \"The\" \\", "ob@example.com"),
            (
                &"N".repeat(200),
                &format!("{}@example.com", "a".repeat(242)),
            ),
        ] {
            assert!(check_git_user(name, email).is_ok(), "{name:?} {email:?}");
        }
        for (name, email, why) in [
            ("", "a@example.com", "name is needed"),
            (&"N".repeat(201), "a@example.com", "at most 200"),
            (
                "A",
                &format!("{}@example.com", "a".repeat(243)),
                "at most 254",
            ),
            ("A\tB", "a@example.com", "control"),
            ("A\u{1b}[31mB", "a@example.com", "control"),
            ("A\0B", "a@example.com", "control"),
            ("A\nB", "a@example.com", "control"),
            ("A <b>", "a@example.com", "< >"),
            ("A", "a@b@example.com", "isn't an email"),
            ("A", "a b@example.com", "isn't an email"),
            ("A", "@example.com", "isn't an email"),
            ("A", "a@", "isn't an email"),
        ] {
            let e = check_git_user(name, email).unwrap_err().to_string();
            assert!(e.contains(why), "{name:?} {email:?}: {e}");
        }
    }
}
