//! Worktrees and branches: the repository a folder is in, a new worktree
//! for a session (its `.env` files copied over, its setup script run), its
//! branch named from the task, and taking it away again. All through the
//! git CLI ([decision 0003]); what a project is and who makes worktrees is
//! [decision 0053].
//!
//! [decision 0003]: ../../docs/decisions/0003-git-through-cli.md
//! [decision 0053]: ../../docs/decisions/0053-projects-are-repos.md

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use dent_protocol::{Repo, Worktree};

/// Branches dent names before the task has a name: `dent/1a2b3c4d`.
const PREFIX: &str = "dent/";
/// The script a repository can keep to ready each new worktree.
pub const SETUP: &str = ".dent/setup";
const MAX_BRANCHES: usize = 200;

/// Runs `f`, which adds, removes or prunes `dir`'s repository's worktrees,
/// while no other such change to that repository runs in this daemon: two
/// `git worktree add`s at once can each read the other's half-made entry
/// and fail. Keyed by the git folder every worktree of a repository shares.
pub fn one_at_a_time<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock, Mutex};
    static LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
        LazyLock::new(Default::default);
    let key = run(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()
    .and_then(|d| std::fs::canonicalize(d.trim()).ok())
    .unwrap_or_else(|| dir.to_path_buf());
    let lock = LOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key)
        .or_default()
        .clone();
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

/// git, its output, or what it said went wrong.
pub fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        Err(if err.is_empty() {
            format!("git {} failed", args.first().copied().unwrap_or_default())
        } else {
            err.trim_start_matches("fatal: ")
                .trim_start_matches("error: ")
                .to_string()
        })
    }
}

/// Where dent keeps the worktrees it makes: `~/dent/worktrees`, beside
/// the projects and the scratch folder.
pub fn worktrees_root() -> PathBuf {
    crate::paths::home().join("dent/worktrees")
}

/// For a linked worktree's folder, its repository's main checkout. Read
/// from the `.git` file alone, with no git process, so the project list
/// can ask of every folder it has.
pub fn main_checkout(dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dir.join(".git")).ok()?;
    let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let gitdir = dir.join(gitdir);
    // <main>/.git/worktrees/<name>
    let common = gitdir.parent()?;
    if common.file_name()? != "worktrees" {
        return None;
    }
    let dotgit = common.parent()?;
    if dotgit.file_name()? != ".git" {
        return None;
    }
    let main = dotgit.parent()?;
    Some(std::fs::canonicalize(main).unwrap_or_else(|_| main.to_path_buf()))
}

/// The branch checked out in `dir`; `None` when detached or not a repo.
pub fn branch(dir: &Path) -> Option<String> {
    run(dir, &["symbolic-ref", "--short", "-q", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// What a new session can choose from in `dir`'s repository.
pub fn repo(dir: &Path) -> Option<Repo> {
    let top = PathBuf::from(run(dir, &["rev-parse", "--show-toplevel"]).ok()?.trim());
    let root = main_checkout(&top).unwrap_or(top);
    let branches = run(
        &root,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:short)",
            "refs/heads",
        ],
    )
    .unwrap_or_default()
    .lines()
    .filter(|l| !l.is_empty())
    .take(MAX_BRANCHES)
    .map(str::to_owned)
    .collect();
    Some(Repo {
        current: branch(dir),
        branches,
        worktrees: worktrees(&root),
        setup: root.join(SETUP).is_file(),
        root: root.display().to_string(),
    })
}

/// A repository's linked worktrees that are still there, newest first.
fn worktrees(root: &Path) -> Vec<Worktree> {
    let list = run(root, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    let mut out: Vec<(Worktree, std::time::SystemTime)> = Vec::new();
    // Blocks of lines, the main checkout first.
    for block in list.split("\n\n").skip(1) {
        let mut path = None;
        let mut branch = None;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(p.to_string());
            } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                branch = Some(b.to_string());
            }
        }
        let Some(path) = path else { continue };
        let Ok(made) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            continue;
        };
        out.push((Worktree { path, branch }, made));
    }
    out.sort_by_key(|w| std::cmp::Reverse(w.1));
    out.into_iter().map(|(w, _)| w).collect()
}

/// Eight random hex digits.
fn token() -> String {
    let id = crate::daemon::uuid().replace('-', "");
    id.chars().rev().take(8).collect()
}

/// A new worktree of `dir`'s repository on a new branch from `base` (the
/// branch checked out in `dir` when `None`), in `under/<project>/`, with
/// the main checkout's `.env` files copied in. Returns its folder, its
/// branch and the main checkout.
pub fn make(
    dir: &Path,
    base: Option<&str>,
    under: &Path,
) -> Result<(PathBuf, String, PathBuf), String> {
    let top = PathBuf::from(
        run(dir, &["rev-parse", "--show-toplevel"])
            .map_err(|_| format!("{} isn't in a Git repository", dir.display()))?
            .trim(),
    );
    let root = main_checkout(&top).unwrap_or(top);
    let base = match base {
        Some(b) => b.to_string(),
        None => branch(dir).unwrap_or_else(|| "HEAD".into()),
    };
    if run(
        &root,
        &["rev-parse", "--verify", "-q", &format!("{base}^{{commit}}")],
    )
    .is_err()
    {
        return Err(if base == "HEAD" {
            "the repository has no commits yet, so there's nothing to start a worktree from".into()
        } else {
            format!("there's no branch called {base}")
        });
    }
    let project = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    let (path, branch) = loop {
        let t = token();
        let path = under.join(&project).join(&t);
        let branch = format!("{PREFIX}{t}");
        if !path.exists() && !has_branch(&root, &branch) {
            break (path, branch);
        }
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let p = path.display().to_string();
    one_at_a_time(&root, || {
        run(&root, &["worktree", "add", "-b", &branch, &p, &base])
    })?;
    copy_env(&root, &path);
    Ok((path, branch, root))
}

fn has_branch(root: &Path, branch: &str) -> bool {
    run(
        root,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

/// The ignored `.env` files of the main checkout (secrets, local settings)
/// copied into a new worktree, which git leaves without them. Folders git
/// ignores whole (`node_modules`) aren't looked in.
pub fn copy_env(root: &Path, to: &Path) {
    let Ok(list) = run(
        root,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
    ) else {
        return;
    };
    for rel in list
        .split('\0')
        .filter(|p| !p.is_empty() && !p.ends_with('/'))
    {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        if !name.starts_with(".env") {
            continue;
        }
        let (from, dest) = (root.join(rel), to.join(rel));
        if !from.is_file() || dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::copy(&from, &dest) {
            eprintln!("dent-daemon: couldn't copy {rel} into a new worktree: {e}");
        }
    }
}

/// The setup script for a new worktree: its own copy, as committed, or
/// the main checkout's when it isn't committed yet.
pub fn setup_script(worktree: &Path, root: &Path) -> Option<PathBuf> {
    [worktree, root]
        .into_iter()
        .map(|d| d.join(SETUP))
        .find(|p| p.is_file())
}

/// How long a setup script may run before it's stopped.
const SETUP_FOR: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Run a setup script in a new worktree with `sh`: Ok or Err with the end
/// of what it printed. One still running after ten minutes is stopped, so
/// a script waiting on something never holds the task back for good.
pub fn run_setup(script: &Path, worktree: &Path, root: &Path) -> Result<String, String> {
    run_setup_for(script, worktree, root, SETUP_FOR)
}

fn run_setup_for(
    script: &Path,
    worktree: &Path,
    root: &Path,
    limit: std::time::Duration,
) -> Result<String, String> {
    // Output goes to a file, not a pipe: a chatty script can't fill a pipe
    // nobody reads while this waits on it.
    let log = std::env::temp_dir().join(format!("dent-setup-{}.log", token()));
    let file = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    let err = file.try_clone().map_err(|e| e.to_string())?;
    let mut child = Command::new("sh")
        .arg(script)
        .current_dir(worktree)
        .env("DENT_ROOT", root)
        .env("DENT_WORKTREE", worktree)
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(err)
        .spawn()
        .map_err(|e| format!("couldn't run {SETUP}: {e}"))?;
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < limit => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(12)..].join("\n");
    match status {
        Some(s) if s.success() => Ok(tail),
        Some(_) => Err(tail),
        None => Err(format!(
            "stopped after {} minutes{}{tail}",
            limit.as_secs() / 60,
            if tail.is_empty() { "" } else { "\n" }
        )),
    }
}

/// A branch name from a model's answer: `dent/` and at most five words
/// joined by `-`; `None` if nothing usable is left.
pub fn branch_name(answer: &str) -> Option<String> {
    let line = answer.lines().find(|l| !l.trim().is_empty())?;
    let line = line.trim().trim_start_matches(PREFIX);
    let slug = crate::projects::slug(line);
    (!slug.is_empty()).then(|| format!("{PREFIX}{slug}"))
}

/// Whether dent named this branch before the task had a name.
#[cfg(test)]
pub fn is_placeholder(branch: &str) -> bool {
    branch
        .strip_prefix(PREFIX)
        .is_some_and(|t| t.len() == 8 && t.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Give a worktree's placeholder branch the name it should have: `wanted`,
/// or `wanted-2` and so on when that's taken. Returns the name it got.
pub fn rename(worktree: &Path, old: &str, wanted: &str) -> Result<String, String> {
    let root = main_checkout(worktree).unwrap_or_else(|| worktree.to_path_buf());
    let name = (1..20)
        .map(|n| {
            if n == 1 {
                wanted.to_string()
            } else {
                format!("{wanted}-{n}")
            }
        })
        .find(|b| !has_branch(&root, b))
        .ok_or("every name like it is taken")?;
    run(worktree, &["branch", "-m", old, &name])?;
    Ok(name)
}

/// Take a worktree away; its branch stays. Without `force`, one with
/// changes not committed is kept. One already gone is forgotten.
pub fn remove(worktree: &Path, root: &Path, force: bool) -> Result<(), String> {
    one_at_a_time(root, || remove_now(worktree, root, force))
}

fn remove_now(worktree: &Path, root: &Path, force: bool) -> Result<(), String> {
    if !worktree.exists() {
        return run(root, &["worktree", "prune"]).map(|_| ());
    }
    let p = worktree.display().to_string();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&p);
    run(root, &args).map(|_| ()).map_err(|e| {
        if e.contains("modified or untracked") {
            "it has changes that aren't committed".to_string()
        } else {
            e
        }
    })
}

/// Bring back a removed worktree on its branch, for a session reopened.
pub fn restore(worktree: &Path, root: &Path, branch: &str) -> Result<(), String> {
    if let Some(parent) = worktree.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let p = worktree.display().to_string();
    one_at_a_time(root, || {
        let _ = run(root, &["worktree", "prune"]);
        run(root, &["worktree", "add", &p, branch])
    })?;
    copy_env(root, worktree);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder holding a repository with one commit, and a folder
    /// for its worktrees.
    fn repo_with_commit(name: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("dent-ws-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let d = base.join("proj");
        std::fs::create_dir_all(&d).unwrap();
        let g = |args: &[&str]| {
            run(&d, args).unwrap();
        };
        g(&["init", "-q", "-b", "main"]);
        std::fs::write(d.join("README.md"), "hi\n").unwrap();
        std::fs::write(d.join(".gitignore"), ".env\n").unwrap();
        std::fs::write(d.join(".env"), "KEY=1\n").unwrap();
        g(&["add", "."]);
        g(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
        ]);
        (std::fs::canonicalize(&d).unwrap(), base.join("worktrees"))
    }

    #[test]
    fn a_worktree_is_made_named_and_taken_away() {
        let (d, under) = repo_with_commit("made");
        let (wt, branch, root) = make(&d, None, &under).unwrap();
        assert!(is_placeholder(&branch), "{branch}");
        assert!(wt.starts_with(under.join("proj")), "{}", wt.display());
        assert_eq!(root, d);
        assert_eq!(main_checkout(&wt).unwrap(), root);
        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "KEY=1\n");
        let r = repo(&wt).unwrap();
        assert_eq!(r.root, root.display().to_string());
        assert_eq!(r.current.as_deref(), Some(branch.as_str()));
        assert_eq!(r.worktrees.len(), 1);

        let named = branch_name("Fix the login redirect\n").unwrap();
        assert_eq!(named, "dent/fix-the-login-redirect");
        assert_eq!(rename(&wt, &branch, &named).unwrap(), named);
        assert_eq!(super::branch(&wt).as_deref(), Some(named.as_str()));

        std::fs::write(wt.join("new.txt"), "x").unwrap();
        assert!(remove(&wt, &root, false).unwrap_err().contains("committed"));
        remove(&wt, &root, true).unwrap();
        assert!(!wt.exists());
        // The branch stays, and the worktree comes back on it.
        restore(&wt, &root, &named).unwrap();
        assert_eq!(super::branch(&wt).as_deref(), Some(named.as_str()));
    }

    #[test]
    fn setup_reports_failure_and_stops_a_script_that_hangs() {
        let (d, _) = repo_with_commit("setup");
        let script = d.join("s.sh");
        std::fs::write(&script, "echo one\necho two >&2\nexit 3\n").unwrap();
        assert_eq!(run_setup(&script, &d, &d).unwrap_err(), "one\ntwo");
        std::fs::write(&script, "echo waiting\nsleep 30\n").unwrap();
        let err =
            run_setup_for(&script, &d, &d, std::time::Duration::from_millis(300)).unwrap_err();
        assert!(
            err.starts_with("stopped after") && err.ends_with("waiting"),
            "{err}"
        );
    }

    #[test]
    fn a_missing_base_says_so() {
        let (d, under) = repo_with_commit("base");
        assert!(make(&d, Some("nope"), &under).unwrap_err().contains("nope"));
    }
}
