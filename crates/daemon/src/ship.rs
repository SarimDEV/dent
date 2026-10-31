//! Getting a session's work out: where its branch stands, and committing,
//! pushing, opening a pull request (with the GitHub CLI) and pulling. Git
//! through its CLI ([decision 0003]); messages a model writes come in from
//! the daemon.
//!
//! [decision 0003]: ../../docs/decisions/0003-git-through-cli.md

use std::path::Path;
use std::process::{Command, Stdio};

use dent_protocol::{Checks, GitStatus, PrState, PullRequest, Review};

use crate::workspace::{branch, run};

/// A diff longer than this is cut before a model reads it.
const DIFF_CAP: usize = 12_000;

/// The GitHub CLI on `path`, if it's installed; `DENT_GH` names another
/// (the walkthroughs' stand-in).
pub fn gh(path: &str) -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("DENT_GH") {
        return Some(p.into());
    }
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
    if remote {
        fetch_now_and_then(dir);
    }
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
        (Some(gh), Some(_), true) => pull_request(dir, gh).ok().flatten(),
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

/// How long a fetch stands before status fetches again.
const FETCHED_FOR: std::time::Duration = std::time::Duration::from_secs(120);

/// Fetch from origin unless the repository fetched in the last couple of
/// minutes, so a branch that's behind its remote says so. Never asks for a
/// password or passphrase: one that's needed means no fetch.
fn fetch_now_and_then(dir: &Path) {
    let Ok(common) = run(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) else {
        return;
    };
    let fresh = std::fs::metadata(Path::new(common.trim()).join("FETCH_HEAD"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < FETCHED_FOR);
    if fresh {
        return;
    }
    let mut git = crate::git::cmd();
    git.arg("-C")
        .arg(dir)
        .args(["fetch", "-q", "--no-tags", "origin"]);
    crate::git::never_hangs(&mut git)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = git.status();
}

/// The latest pull request for the branch checked out: `Ok(None)` when
/// GitHub has none, `Err` when it couldn't be asked (offline, signed out,
/// a limit), which must not read as the pull request having gone.
pub fn pull_request(dir: &Path, gh: &Path) -> Result<Option<PullRequest>, String> {
    match gh_in(dir, gh, &["pr", "view", "--json", PR_FIELDS]) {
        Ok(out) => parse_pr(&out)
            .map(Some)
            .ok_or_else(|| "gh answered something unreadable".to_string()),
        Err(e) if e.contains("no pull requests found") => Ok(None),
        Err(e) => Err(e),
    }
}

/// A check's verdict, from a check run's conclusion or a commit status's
/// state; empty while it runs.
fn verdict(c: &serde_json::Value) -> &str {
    c["conclusion"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| c["state"].as_str())
        .unwrap_or_default()
}

/// What a verdict counts as: one place, so the badge and the brief agree.
fn failed(verdict: &str) -> Option<bool> {
    match verdict {
        "SUCCESS" | "NEUTRAL" | "SKIPPED" => Some(false),
        "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => {
            Some(true)
        }
        // Running, queued, or a state this doesn't know yet.
        _ => None,
    }
}

const PR_FIELDS: &str = "number,url,state,reviewDecision,statusCheckRollup";

/// A pull request from `gh pr view --json` with [`PR_FIELDS`].
fn parse_pr(json: &str) -> Option<PullRequest> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let number = v["number"].as_u64()?;
    let state = match v["state"].as_str()? {
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => PrState::Open,
    };
    let mut checks = Checks::default();
    for c in v["statusCheckRollup"].as_array().into_iter().flatten() {
        match failed(verdict(c)) {
            Some(false) => checks.passed += 1,
            Some(true) => checks.failed += 1,
            None => checks.pending += 1,
        }
    }
    let review = match v["reviewDecision"].as_str() {
        Some("APPROVED") => Some(Review::Approved),
        Some("CHANGES_REQUESTED") => Some(Review::ChangesRequested),
        _ => None,
    };
    Some(PullRequest {
        number,
        url: v["url"].as_str().unwrap_or_default().to_string(),
        state,
        checks,
        review,
    })
}

/// Run the GitHub CLI in `dir`, never prompting: its output or why not.
fn gh_in(dir: &Path, gh: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(gh)
        .args(args)
        .current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run gh: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The lines of a log worth an agent's reading: the last `n`, and no more
/// than `LOG_CAP` bytes of them (minified output makes long lines).
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let kept = lines[lines.len().saturating_sub(n)..].join("\n");
    if kept.len() <= LOG_CAP {
        return kept;
    }
    let mut start = kept.len() - LOG_CAP;
    while !kept.is_char_boundary(start) {
        start += 1;
    }
    format!("… (cut short)\n{}", &kept[start..])
}

/// A CI log as a person would read it. `gh run view --log-failed` puts the
/// job, the step and a timestamp before every line, keeps the terminal's
/// colour codes, and marks folds with `##[group]`: all of that goes.
fn clean_log(text: &str) -> String {
    let mut out = Vec::new();
    for line in text.lines() {
        // job <tab> step <tab> rest
        let rest = line.splitn(3, '\t').nth(2).unwrap_or(line);
        let rest = rest.trim_start_matches('\u{feff}');
        // An ISO timestamp, then a space.
        let rest = match rest.split_once(' ') {
            Some((stamp, after))
                if stamp.len() >= 20 && stamp.ends_with('Z') && stamp.contains('T') =>
            {
                after
            }
            _ => rest,
        };
        let rest = strip_ansi(rest);
        if rest.starts_with("##[group]") || rest.starts_with("##[endgroup]") {
            continue;
        }
        let rest = match rest.strip_prefix("##[error]") {
            Some(e) => format!("error: {e}"),
            None => rest,
        };
        out.push(rest);
    }
    out.join("\n")
}

/// Text without terminal escape sequences (colours, cursor moves), whether
/// as the escape character or as GitHub's logs spell it, `^[`.
fn strip_ansi(s: &str) -> String {
    let s = s.replace("^[[", "\u{1b}[");
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ … final letter, or ESC and one more character.
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The most of a failing check's log a brief carries.
const LOG_CAP: usize = 8 * 1024;

/// A message for the agent about pull request `pr`'s failing checks: which
/// failed, and the end of the first failure's log.
pub fn checks_brief(dir: &Path, gh: &Path, pr: u64) -> Result<String, String> {
    let n = pr.to_string();
    let rollup = gh_in(dir, gh, &["pr", "view", &n, "--json", "statusCheckRollup"])?;
    let v: serde_json::Value = serde_json::from_str(&rollup).map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    let mut run = None;
    for c in v["statusCheckRollup"].as_array().into_iter().flatten() {
        if failed(verdict(c)) == Some(true) {
            let name = c["name"]
                .as_str()
                .or_else(|| c["context"].as_str())
                .unwrap_or("a check");
            names.push(format!("- {name}"));
            let url = c["detailsUrl"]
                .as_str()
                .or_else(|| c["targetUrl"].as_str())
                .unwrap_or_default();
            if run.is_none() {
                run = url
                    .split("/runs/")
                    .nth(1)
                    .and_then(|r| r.split('/').next())
                    .filter(|r| r.chars().all(|c| c.is_ascii_digit()) && !r.is_empty())
                    .map(str::to_owned);
            }
        }
    }
    if names.is_empty() {
        return Err(format!("no checks are failing on pull request #{pr}"));
    }
    let mut text = format!(
        "The CI checks on pull request #{pr} failed:\n{}\n\nFind out why, fix it, and push.",
        names.join("\n")
    );
    if let Some(run) = run
        && let Ok(log) = gh_in(dir, gh, &["run", "view", &run, "--log-failed"])
        && !log.trim().is_empty()
    {
        // Indented, not fenced: a message shown folded never stops on a
        // bare fence.
        let indented: Vec<String> = tail(&clean_log(&log), 80)
            .lines()
            .map(|l| format!("    {l}"))
            .collect();
        text.push_str(&format!(
            "\n\nThe end of the first failure's log:\n\n{}",
            indented.join("\n")
        ));
    }
    Ok(text)
}

/// A message for the agent about pull request `pr`'s review: the changes
/// asked for and the comments on lines.
pub fn review_brief(dir: &Path, gh: &Path, pr: u64) -> Result<String, String> {
    let n = pr.to_string();
    let reviews = gh_in(dir, gh, &["pr", "view", &n, "--json", "reviews"])?;
    let v: serde_json::Value = serde_json::from_str(&reviews).map_err(|e| e.to_string())?;
    let mut said = Vec::new();
    for r in v["reviews"].as_array().into_iter().flatten() {
        let body = r["body"].as_str().unwrap_or_default().trim();
        // An approval's words aren't something to change.
        if !body.is_empty() && r["state"] != "APPROVED" && r["state"] != "DISMISSED" {
            let who = r["author"]["login"].as_str().unwrap_or("A reviewer");
            said.push(format!("{who}: {body}"));
        }
    }
    let lines = gh_in(
        dir,
        gh,
        &[
            "api",
            "--paginate",
            &format!("repos/{{owner}}/{{repo}}/pulls/{pr}/comments"),
            "--jq",
            // Comments on lines the branch has since changed are outdated
            // (no position): already dealt with, or no longer apply.
            r#".[] | select(.position != null) | "\(.path):\(.line // .original_line) \(.user.login): \(.body)""#,
        ],
    )
    .unwrap_or_default();
    said.extend(
        lines
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned),
    );
    if said.is_empty() {
        return Err(format!(
            "pull request #{pr} has no review comments to answer"
        ));
    }
    Ok(format!(
        "Pull request #{pr} has review comments:\n\n{}\n\nMake the changes asked for, reply where you disagree, and push.",
        said.join("\n\n")
    ))
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

        // Another clone pushes: status fetches, and says it's behind.
        let other = base.join("other");
        run(
            &base,
            &["clone", "-q", &origin.display().to_string(), "other"],
        )
        .unwrap();
        std::fs::write(other.join("c.txt"), "c\n").unwrap();
        run(&other, &["add", "."]).unwrap();
        run(
            &other,
            &[
                "-c",
                "user.name=o",
                "-c",
                "user.email=o@o",
                "commit",
                "-qm",
                "Third",
            ],
        )
        .unwrap();
        run(&other, &["push", "-q"]).unwrap();
        let _ = std::fs::remove_file(d.join(".git/FETCH_HEAD"));
        assert_eq!(status(&d, None).unwrap().behind, 1);
        pull(&d).unwrap();
        assert_eq!(status(&d, None).unwrap().behind, 0);
    }

    #[test]
    fn a_pull_request_reads_its_state_checks_and_review() {
        let json = r#"{"number":12,"url":"https://github.com/o/r/pull/12","state":"OPEN",
            "reviewDecision":"CHANGES_REQUESTED","statusCheckRollup":[
            {"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"FAILURE"},
            {"__typename":"CheckRun","name":"lint","status":"COMPLETED","conclusion":"SUCCESS"},
            {"__typename":"CheckRun","name":"build","status":"IN_PROGRESS","conclusion":""},
            {"__typename":"StatusContext","context":"deploy","state":"SUCCESS"}]}"#;
        let pr = parse_pr(json).unwrap();
        assert_eq!(pr.number, 12);
        assert_eq!(pr.state, PrState::Open);
        assert_eq!(
            (pr.checks.passed, pr.checks.failed, pr.checks.pending),
            (2, 1, 1)
        );
        assert_eq!(pr.review, Some(Review::ChangesRequested));
        let merged =
            parse_pr(r#"{"number":3,"url":"u","state":"MERGED","statusCheckRollup":[]}"#).unwrap();
        assert_eq!((merged.state, merged.review), (PrState::Merged, None));
        assert!(parse_pr("no pull requests found").is_none());
        // Cancelled and action-required count as failing, for the badge
        // and the brief alike.
        let odd = parse_pr(
            r#"{"number":4,"url":"u","state":"OPEN","statusCheckRollup":[
            {"name":"a","conclusion":"CANCELLED"},{"name":"b","conclusion":"ACTION_REQUIRED"},
            {"context":"c","state":"PENDING"}]}"#,
        )
        .unwrap();
        assert_eq!((odd.checks.failed, odd.checks.pending), (2, 1));
    }

    #[test]
    fn a_ci_log_reads_as_its_lines() {
        // As `gh run view --log-failed` gave it for a real failing check.
        let log = "notes\tThe notes say they're fixed\t\u{feff}2026-10-20T07:27:41.6339530Z ##[group]Run if ! grep -q \"fixed\" ship.md; then\n\
notes\tThe notes say they're fixed\t2026-10-20T07:27:41.6342820Z ^[[36;1m  echo \"error: ship.md must say the footer is fixed\"^[[0m\n\
notes\tThe notes say they're fixed\t2026-10-20T07:27:41.6589344Z ##[endgroup]\n\
notes\tThe notes say they're fixed\t2026-10-20T07:27:41.6772582Z error: ship.md must say the footer is fixed\n\
notes\tThe notes say they're fixed\t2026-10-20T07:27:41.6791725Z ##[error]Process completed with exit code 1.";
        assert_eq!(
            clean_log(log),
            "  echo \"error: ship.md must say the footer is fixed\"\n\
error: ship.md must say the footer is fixed\n\
error: Process completed with exit code 1."
        );
    }

    #[test]
    fn a_long_log_is_cut_to_its_end() {
        let log = "x".repeat(LOG_CAP * 3);
        let t = tail(&log, 80);
        assert!(t.len() <= LOG_CAP + 20 && t.starts_with("… (cut short)"));
        assert_eq!(tail("a\nb\nc", 2), "b\nc");
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
