//! Getting a session's work out: where its branch stands, and committing,
//! pushing, opening a pull request (with the GitHub CLI) and pulling. Git
//! through its CLI ([decision 0003]); messages a model writes come in from
//! the daemon.
//!
//! [decision 0003]: ../../docs/decisions/0003-git-through-cli.md

use std::path::Path;
use std::process::{Command, Stdio};

use dent_protocol::{GitStatus, PullRequest};

use crate::workspace::{branch, run};

/// A diff longer than this is cut before a model reads it.
const DIFF_CAP: usize = 12_000;

/// The GitHub CLI on `path`, if it's installed.
pub fn gh(path: &str) -> Option<std::path::PathBuf> {
    std::env::split_paths(path)
        .map(|d| d.join("gh"))
        .find(|p| p.is_file())
}

/// Where the branch in `dir` stands. `None` outside a Git repository.
/// `gh` is the GitHub CLI, when there is one, to find an open pull request.
pub fn status(dir: &Path, gh: Option<&Path>) -> Option<GitStatus> {
    run(dir, &["rev-parse", "--is-inside-work-tree"]).ok()?;
    let branch = branch(dir);
    let changes = run(dir, &["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let remote = run(dir, &["remote"])
        .map(|s| s.lines().any(|l| l == "origin"))
        .unwrap_or(false);
    let default_branch = run(
        dir,
        &["symbolic-ref", "--short", "-q", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .map(|s| s.trim().trim_start_matches("origin/").to_string())
    .filter(|s| !s.is_empty());
    let upstream = run(dir, &["rev-parse", "--abbrev-ref", "-q", "@{u}"]).is_ok();
    let count = |range: &str| -> (u32, u32) {
        run(dir, &["rev-list", "--left-right", "--count", range])
            .ok()
            .and_then(|s| {
                let mut it = s.split_whitespace().map(|n| n.parse::<u32>().ok());
                Some((it.next()??, it.next()??))
            })
            .unwrap_or((0, 0))
    };
    let (behind, ahead) = if upstream {
        count("@{u}...HEAD")
    } else {
        // Never pushed: its commits that no remote branch has.
        let n = run(dir, &["rev-list", "--count", "HEAD", "--not", "--remotes"])
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        (0, n)
    };
    let pr = match (gh, &branch, remote) {
        (Some(gh), Some(_), true) => open_pr(dir, gh),
        _ => None,
    };
    Some(GitStatus {
        branch,
        default_branch,
        changes,
        remote,
        upstream,
        ahead,
        behind,
        gh: gh.is_some(),
        pr,
    })
}

/// The open pull request for the branch checked out, if GitHub has one.
fn open_pr(dir: &Path, gh: &Path) -> Option<PullRequest> {
    let out = Command::new(gh)
        .args(["pr", "view", "--json", "number,url,state"])
        .current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    (v["state"] == "OPEN").then(|| PullRequest {
        number: v["number"].as_u64().unwrap_or(0),
        url: v["url"].as_str().unwrap_or_default().to_string(),
    })
}

/// What a commit would hold, for a model to write its message: the files
/// and the diff, cut short when long.
pub fn pending(dir: &Path) -> String {
    let _ = run(dir, &["add", "-A", "--intent-to-add"]);
    let stat = run(dir, &["diff", "HEAD", "--stat"]).unwrap_or_default();
    let diff = run(dir, &["diff", "HEAD", "--no-color", "--no-ext-diff"]).unwrap_or_default();
    format!("{stat}\n{}", clip(&diff, DIFF_CAP))
}

/// What the branch has that `base` hasn't, for a pull request's text.
pub fn since(dir: &Path, base: &str) -> String {
    let range = format!("{base}..HEAD");
    let log = run(dir, &["log", "--format=- %s%n%b", &range]).unwrap_or_default();
    let stat = run(dir, &["diff", "--stat", &format!("{base}...HEAD")]).unwrap_or_default();
    format!("Commits:\n{}\n\nFiles:\n{stat}", clip(&log, DIFF_CAP))
}

fn clip(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (cut short)", &s[..end])
}

/// Commit everything, untracked files too, with `message`.
pub fn commit(dir: &Path, message: &str) -> Result<(), String> {
    run(dir, &["add", "-A"])?;
    run(dir, &["commit", "-q", "-m", message]).map(|_| ())
}

/// Push the branch, setting where it tracks the first time.
pub fn push(dir: &Path, upstream: bool) -> Result<(), String> {
    if upstream {
        run(dir, &["push", "-q"])
    } else {
        run(dir, &["push", "-q", "-u", "origin", "HEAD"])
    }
    .map(|_| ())
}

/// Bring in the remote's commits, only when that needs no merge.
pub fn pull(dir: &Path) -> Result<(), String> {
    run(dir, &["pull", "-q", "--ff-only"]).map(|_| ())
}

/// Open a pull request for the pushed branch; its address.
pub fn open(dir: &Path, gh: &Path, title: &str, body: &str) -> Result<String, String> {
    let out = Command::new(gh)
        .args(["pr", "create", "--title", title, "--body", body])
        .current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run gh: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if out.status.success() {
        Ok(text
            .lines()
            .rev()
            .find(|l| l.starts_with("http"))
            .unwrap_or_default()
            .to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// A commit message from a model's answer: its first line as the subject,
/// any body after a blank line; `None` when it's empty.
pub fn message(answer: &str) -> Option<String> {
    let text = answer.trim().trim_matches('`').trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// A pull request's title and body from a model's answer: the first line,
/// then the rest.
pub fn title_and_body(answer: &str) -> (String, String) {
    let text = answer.trim();
    let (title, body) = text.split_once('\n').unwrap_or((text, ""));
    let title = title.trim().trim_start_matches('#').trim().to_string();
    (title, body.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_counts_work_not_yet_pushed() {
        let base = std::env::temp_dir().join(format!("dent-ship-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (origin, d) = (base.join("origin.git"), base.join("work"));
        std::fs::create_dir_all(&d).unwrap();
        run(&base, &["init", "-q", "--bare", "-b", "main", "origin.git"]).unwrap();
        let g = |args: &[&str]| run(&d, args).unwrap();
        g(&["init", "-q", "-b", "main"]);
        g(&["config", "user.name", "t"]);
        g(&["config", "user.email", "t@t"]);
        g(&["remote", "add", "origin", &origin.display().to_string()]);
        std::fs::write(d.join("a.txt"), "a\n").unwrap();
        commit(&d, "First").unwrap();
        push(&d, false).unwrap();
        let s = status(&d, None).unwrap();
        assert_eq!((s.upstream, s.ahead, s.changes), (true, 0, false));

        std::fs::write(d.join("b.txt"), "b\n").unwrap();
        let s = status(&d, None).unwrap();
        assert!(s.changes);
        assert!(pending(&d).contains("b.txt"));
        commit(&d, "Second").unwrap();
        let s = status(&d, None).unwrap();
        assert_eq!((s.ahead, s.changes), (1, false));
        push(&d, true).unwrap();
        assert_eq!(status(&d, None).unwrap().ahead, 0);
    }

    #[test]
    fn answers_become_messages_and_titles() {
        assert_eq!(message("```\nFix it\n```").as_deref(), Some("Fix it"));
        assert_eq!(message("  "), None);
        assert_eq!(
            title_and_body("# Fix login\n\nIt redirects now."),
            ("Fix login".into(), "It redirects now.".into())
        );
    }
}
