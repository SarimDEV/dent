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
