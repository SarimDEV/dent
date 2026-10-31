//! What a session's directory has changed: `git diff` against HEAD, plus
//! untracked files shown as wholly added, or what one turn changed between
//! its checkpoints. Writes only to put back what you discard (a file or one
//! hunk), the checkpoints and a rewind's restore; all through the git CLI
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

/// What changed from snapshot `from` to snapshot `to`, or to the working
/// tree as it is now: one turn's work, between the checkpoints either side
/// of it.
pub fn changes_between(dir: &Path, from: &str, to: Option<&str>) -> Vec<FileChange> {
    let now;
    let to = match to {
        Some(t) => t,
        None => {
            let Some(s) = snapshot(dir) else {
                return Vec::new();
            };
            now = s;
            &now
        }
    };
    let diff = git(
        dir,
        &[
            "diff",
            from,
            to,
            "--no-color",
            "--no-ext-diff",
            "-M",
            "--relative",
        ],
    )
    .unwrap_or_default();
    split(&diff)
}

/// What `base` is when none is given: the last commit, or nothing yet.
fn head_or_empty(dir: &Path) -> String {
    if git(dir, &["rev-parse", "--verify", "-q", "HEAD"]).is_some() {
        "HEAD".into()
    } else {
        EMPTY_TREE.into()
    }
}

/// Put a file back as it was at `base` (the last commit when `None`): a
/// file that wasn't there then goes. With `hunk`, only that hunk (counted
/// from 0 in the file's diff) goes back, and only if the file still reads
/// as the diff showed it.
pub fn discard(
    dir: &Path,
    base: Option<&str>,
    path: &str,
    hunk: Option<usize>,
    seen: Option<&str>,
) -> Result<(), String> {
    git(dir, &["rev-parse", "--is-inside-work-tree"]).ok_or("not a git repository")?;
    if path.split('/').any(|p| p == "..") || path.starts_with('/') {
        return Err("that file isn't in this folder".into());
    }
    let base = base.map_or_else(|| head_or_empty(dir), str::to_string);
    let file = dir.join(path);
    let existed = git(dir, &["cat-file", "-e", &format!("{base}:./{path}")]).is_some();
    let Some(n) = hunk else {
        if existed {
            git(
                dir,
                &[
                    "restore",
                    &format!("--source={base}"),
                    "--worktree",
                    "--",
                    path,
                ],
            )
            .ok_or("couldn't put the file back")?;
        } else if file.exists() {
            std::fs::remove_file(&file).map_err(|e| e.to_string())?;
        }
        return Ok(());
    };
    // One hunk: what it reads now gives way to what it read before.
    let then = if existed {
        git(dir, &["show", &format!("{base}:./{path}")])
            .ok_or("couldn't read the file as it was")?
    } else {
        String::new()
    };
    let now = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let diff = unified(&then, &now, path);
    let hunks = hunks(&diff);
    // The hunk that reads as it was shown, wherever it is now: a hunk put
    // back moves the ones after it up, so its number alone can name
    // another. None reads so: it's gone, or changed since.
    let n = match seen {
        None => n,
        Some(seen) => {
            let prints = hunk_prints(&diff);
            if prints.get(n).map(String::as_str) == Some(seen) {
                n
            } else {
                prints
                    .iter()
                    .position(|p| p == seen)
                    .ok_or("that change isn't there any more; look again")?
            }
        }
    };
    let h = hunks.get(n).ok_or("that change has already gone")?;
    let mut lines: Vec<&str> = now.split_inclusive('\n').collect();
    let start = h.new_start.saturating_sub(1);
    let expect: Vec<&str> = h.new_lines.iter().map(String::as_str).collect();
    if lines.get(start..start + expect.len()) != Some(&expect[..]) {
        return Err("the file has changed since; look again".into());
    }
    lines.splice(
        start..start + expect.len(),
        h.old_lines.iter().map(String::as_str),
    );
    std::fs::write(&file, lines.concat()).map_err(|e| e.to_string())
}

/// A diff of two texts, as git writes it, through `git diff --no-index`.
fn unified(old: &str, new: &str, path: &str) -> String {
    // Its own folder each time: two discards at once (two sessions, files
    // of one name) never diff each other's texts.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "dent-discard-{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        path.replace('/', "_")
    ));
    let _ = std::fs::create_dir_all(&tmp);
    let (a, b) = (tmp.join("a"), tmp.join("b"));
    let _ = std::fs::write(&a, old);
    let _ = std::fs::write(&b, new);
    let out = Command::new("git")
        .args(["diff", "--no-index", "--no-color", "--no-ext-diff"])
        .arg(&a)
        .arg(&b)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

/// Each hunk of a diff as [`dent_protocol::hunk_print`] writes it.
fn hunk_prints(diff: &str) -> Vec<String> {
    let mut out: Vec<Vec<(char, &str)>> = Vec::new();
    for line in diff.split_inclusive('\n') {
        if line.starts_with("@@ ") {
            out.push(Vec::new());
            continue;
        }
        let Some(h) = out.last_mut() else { continue };
        if let Some(kind @ (' ' | '+' | '-')) = line.chars().next() {
            h.push((kind, &line[1..]));
        }
    }
    out.into_iter().map(dent_protocol::hunk_print).collect()
}

/// A hunk: where it starts in the new text, and its lines old and new.
struct Hunk {
    new_start: usize,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
}

fn hunks(diff: &str) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::new();
    // The kind of the last line read, for a "no newline" note after it.
    let mut last: Option<char> = None;
    for line in diff.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("@@ ") {
            // @@ -a,b +c,d @@
            let new_start = rest
                .split_whitespace()
                .find_map(|p| p.strip_prefix('+'))
                .and_then(|p| p.split(',').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            out.push(Hunk {
                new_start: new_start.max(1),
                old_lines: Vec::new(),
                new_lines: Vec::new(),
            });
            continue;
        }
        let Some(h) = out.last_mut() else { continue };
        if line.starts_with('\\') {
            // "\ No newline at end of file": the line before it has none,
            // on whichever side it was.
            let strip = |v: &mut Vec<String>| {
                if let Some(l) = v.last_mut() {
                    l.truncate(l.trim_end_matches('\n').len());
                }
            };
            match last {
                Some('-') => strip(&mut h.old_lines),
                Some('+') => strip(&mut h.new_lines),
                _ => {
                    strip(&mut h.old_lines);
                    strip(&mut h.new_lines);
                }
            }
            continue;
        }
        match line.split_at_checked(1) {
            Some((" ", rest)) => {
                h.old_lines.push(rest.into());
                h.new_lines.push(rest.into());
                last = Some(' ');
            }
            Some(("-", rest)) => {
                h.old_lines.push(rest.into());
                last = Some('-');
            }
            Some(("+", rest)) => {
                h.new_lines.push(rest.into());
                last = Some('+');
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod discard_tests {
    use super::*;

    fn repo(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dent-discard-{name}-{}", std::process::id()));
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
        std::fs::write(dir.join("a.txt"), "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n").unwrap();
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
        dir
    }

    #[test]
    fn a_file_goes_back_and_a_new_one_goes() {
        let dir = repo("file");
        std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
        std::fs::write(dir.join("new.txt"), "new\n").unwrap();
        discard(&dir, None, "a.txt", None, None).unwrap();
        discard(&dir, None, "new.txt", None, None).unwrap();
        assert!(
            std::fs::read_to_string(dir.join("a.txt"))
                .unwrap()
                .starts_with("1\n2\n")
        );
        assert!(!dir.join("new.txt").exists());
        assert!(discard(&dir, None, "../x", None, None).is_err());
    }

    #[test]
    fn one_hunk_goes_back_and_the_other_stays() {
        let dir = repo("hunk");
        std::fs::write(
            dir.join("a.txt"),
            "1\nTWO\n3\n4\n5\n6\n7\n8\n9\n10\nELEVEN\n12\n",
        )
        .unwrap();
        assert_eq!(changes(&dir).len(), 1);
        discard(&dir, None, "a.txt", Some(1), None).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "1\nTWO\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n"
        );
        discard(&dir, None, "a.txt", Some(0), None).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n"
        );
        assert!(
            discard(&dir, None, "a.txt", Some(0), None).is_err(),
            "nothing left to discard"
        );
    }

    /// A discard sent twice (its answer lost on a reconnect), or from a
    /// view that's out of date, puts back only the hunk that was shown:
    /// never the one that moved up into its place.
    #[test]
    fn a_hunk_named_by_what_it_read_goes_back_once() {
        let dir = repo("seen");
        std::fs::write(
            dir.join("a.txt"),
            "1\nTWO\n3\n4\n5\n6\n7\n8\n9\n10\nELEVEN\n12\n",
        )
        .unwrap();
        let diff = changes(&dir)[0].diff.clone();
        let shown = hunk_prints(&diff);
        assert_eq!(shown.len(), 2);
        assert!(shown[0].contains("+TWO") && shown[1].contains("+ELEVEN"));
        discard(&dir, None, "a.txt", Some(0), Some(&shown[0])).unwrap();
        // Again, as a resend: hunk 0 is now ELEVEN, and stays.
        let again = discard(&dir, None, "a.txt", Some(0), Some(&shown[0]));
        let why = again.unwrap_err();
        assert!(why.contains("isn't there any more"), "{why}");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\nELEVEN\n12\n"
        );
        // Shown as hunk 1 before the first went: found where it is now.
        discard(&dir, None, "a.txt", Some(1), Some(&shown[1])).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n"
        );
    }

    #[test]
    fn a_last_line_without_a_newline_goes_back_too() {
        let dir = repo("eol");
        std::fs::write(dir.join("b.txt"), "x\ny").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["add", "."])
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "b",
            ])
            .output()
            .unwrap();
        std::fs::write(dir.join("b.txt"), "x\nY").unwrap();
        discard(&dir, None, "b.txt", Some(0), None).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "x\ny");
    }

    #[test]
    fn a_turn_is_what_changed_after_its_checkpoint() {
        let dir = repo("turn");
        std::fs::write(dir.join("before.txt"), "mine\n").unwrap();
        let c = checkpoint(&dir, "s/1").unwrap();
        std::fs::write(dir.join("a.txt"), "agent\n").unwrap();
        let turn = changes_between(&dir, &c, None);
        let paths: Vec<&str> = turn.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["a.txt"],
            "only what the turn did, not what was there before it"
        );
        discard(&dir, Some(&c), "a.txt", None, None).unwrap();
        assert!(changes_between(&dir, &c, None).is_empty());
        assert!(
            dir.join("before.txt").exists(),
            "what was there before the turn stays"
        );
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
