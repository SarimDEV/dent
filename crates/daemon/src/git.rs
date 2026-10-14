//! What a session's directory has changed: `git diff` against HEAD, plus
//! untracked files shown as wholly added. Reads only, through the git CLI
//! ([decision 0003]).
//!
//! [decision 0003]: ../../docs/decisions/0003-git-through-cli.md

use std::path::Path;
use std::process::Command;

use dent_protocol::FileChange;

/// Untracked files bigger than this are listed without their text.
const UNTRACKED_CAP: u64 = 256 * 1024;
const MAX_UNTRACKED: usize = 200;
/// git's empty tree, for repositories with no commits yet.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn changes(dir: &Path) -> Vec<FileChange> {
    if git(dir, &["rev-parse", "--is-inside-work-tree"]).is_none() {
        return Vec::new();
    }
    let base = if git(dir, &["rev-parse", "--verify", "-q", "HEAD"]).is_some() {
        "HEAD"
    } else {
        EMPTY_TREE
    };
    let diff = git(
        dir,
        &[
            "diff",
            base,
            "--no-color",
            "--no-ext-diff",
            "-M",
            "--relative",
        ],
    )
    .unwrap_or_default();
    let mut files = split(&diff);
    let untracked =
        git(dir, &["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default();
    for path in untracked
        .split('\0')
        .filter(|p| !p.is_empty())
        .take(MAX_UNTRACKED)
    {
        files.push(added(dir, path));
    }
    files
}

/// Files in a folder for @ mentions: what git tracks plus untracked files
/// it doesn't ignore; outside git, a shallow walk.
pub fn files(dir: &Path, cap: usize) -> Vec<String> {
    if let Some(out) = git(
        dir,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    ) {
        return out
            .split('\0')
            .filter(|p| !p.is_empty())
            .take(cap)
            .map(str::to_owned)
            .collect();
    }
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            if p.is_dir() {
                if depth < 4 {
                    stack.push((p, depth + 1));
                }
            } else if let Ok(rel) = p.strip_prefix(dir) {
                out.push(rel.display().to_string());
                if out.len() >= cap {
                    return out;
                }
            }
        }
    }
    out
}

/// One [`FileChange`] per `diff --git` section.
fn split(diff: &str) -> Vec<FileChange> {
    let mut out: Vec<FileChange> = Vec::new();
    for line in diff.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let path = rest
                .trim_end()
                .rsplit_once(" b/")
                .map(|(_, b)| b.to_string())
                .unwrap_or_else(|| rest.trim_end().to_string());
            out.push(FileChange {
                path,
                added: 0,
                removed: 0,
                diff: String::new(),
            });
        }
        let Some(f) = out.last_mut() else { continue };
        if line.starts_with('+') && !line.starts_with("+++") {
            f.added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            f.removed += 1;
        }
        f.diff.push_str(line);
    }
    out
}

fn added(dir: &Path, path: &str) -> FileChange {
    let full = dir.join(path);
    let size = std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
    let text = if size <= UNTRACKED_CAP {
        std::fs::read(&full)
            .ok()
            .filter(|b| !b.contains(&0))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    } else {
        None
    };
    let Some(text) = text else {
        return FileChange {
            path: path.into(),
            added: 0,
            removed: 0,
            diff: String::new(),
        };
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut diff = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for l in &lines {
        diff.push('+');
        diff.push_str(l);
        diff.push('\n');
    }
    FileChange {
        path: path.into(),
        added: lines.len() as u32,
        removed: 0,
        diff,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn splits_and_counts() {
        let d = "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1,2 @@\n-a\n+b\n+c\ndiff --git a/y b/y\n--- a/y\n+++ b/y\n@@ -1 +0,0 @@\n-z\n";
        let f = super::split(d);
        assert_eq!(f.len(), 2);
        assert_eq!(
            (f[0].path.as_str(), f[0].added, f[0].removed),
            ("x.rs", 2, 1)
        );
        assert_eq!((f[1].path.as_str(), f[1].added, f[1].removed), ("y", 0, 1));
    }
}

/// Snapshot the working tree as a commit under `refs/dent/`, without
/// touching the person's index: a copy of it takes the changes, so files
/// git already knows aren't hashed again. Untracked files that aren't
/// ignored are included. Returns the commit.
pub fn checkpoint(dir: &Path, name: &str) -> Option<String> {
    let commit = snapshot(dir)?;
    git(dir, &["update-ref", &format!("refs/dent/{name}"), &commit])?;
    Some(commit)
}

/// The working tree as a commit, kept by nothing.
fn snapshot(dir: &Path) -> Option<String> {
    git(dir, &["rev-parse", "--is-inside-work-tree"])?;
    let index = temp_index(dir)?;
    let real = git(dir, &["rev-parse", "--git-path", "index"])?;
    let real = dir.join(real.trim());
    if real.exists() {
        std::fs::copy(&real, &index).ok()?;
    }
    let with = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let tree = with(&["add", "-A", "--", "."]).and_then(|_| with(&["write-tree"]));
    let _ = std::fs::remove_file(&index);
    let tree = tree?;
    let head = git(dir, &["rev-parse", "--verify", "-q", "HEAD"]).map(|h| h.trim().to_string());
    let mut args = vec![
        "-c",
        "user.name=dent",
        "-c",
        "user.email=dent@localhost",
        "commit-tree",
        &tree,
        "-m",
        "dent checkpoint",
    ];
    if let Some(h) = &head {
        args.extend(["-p", h]);
    }
    git(dir, &args).map(|c| c.trim().to_string())
}

fn temp_index(dir: &Path) -> Option<std::path::PathBuf> {
    let p = git(
        dir,
        &[
            "rev-parse",
            "--git-path",
            &format!("dent-index-{}", std::process::id()),
        ],
    )?;
    Some(dir.join(p.trim()))
}

/// Put the working tree back as it was at `commit`: files changed since
/// are restored and files created since are removed. Ignored files and the
/// person's index are left alone.
pub fn restore(dir: &Path, commit: &str) -> Result<(), String> {
    let now = snapshot(dir).ok_or("couldn't read the working tree")?;
    let added = git(
        dir,
        &[
            "diff-tree",
            "-r",
            "--name-only",
            "--diff-filter=A",
            "--no-renames",
            commit,
            &now,
        ],
    )
    .ok_or("couldn't compare with the checkpoint")?;
    for f in added.lines().filter(|l| !l.is_empty()) {
        let _ = std::fs::remove_file(dir.join(f));
    }
    let index = temp_index(dir).ok_or("not a git repository")?;
    let run = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    let ok = run(&["read-tree", commit]) && run(&["checkout-index", "-a", "-f"]);
    let _ = std::fs::remove_file(&index);
    if ok {
        Ok(())
    } else {
        Err("couldn't write the checkpoint's files".into())
    }
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    #[test]
    fn restore_brings_back_changed_and_removes_new_files() {
        let dir = std::env::temp_dir().join(format!("dent-cp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let g = |a: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(a)
                .output()
                .unwrap();
        };
        g(&["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
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
        std::fs::write(dir.join("wip.txt"), "draft\n").unwrap();
        let before_index = std::fs::read(dir.join(".git/index")).unwrap();
        let c = checkpoint(&dir, "s/1").expect("checkpoint");
        assert_eq!(
            std::fs::read(dir.join(".git/index")).unwrap(),
            before_index,
            "index untouched"
        );
        std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
        std::fs::write(dir.join("new.txt"), "agent made this\n").unwrap();
        std::fs::remove_file(dir.join("wip.txt")).unwrap();
        restore(&dir, &c).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\n");
        assert_eq!(
            std::fs::read_to_string(dir.join("wip.txt")).unwrap(),
            "draft\n"
        );
        assert!(!dir.join("new.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
