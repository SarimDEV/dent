//! A worktree's git state, moved exactly ([spike 11]): two commits on top
//! of `HEAD` (the working tree, built in a copy of the index, and the index
//! itself, so even the staged/unstaged split travels), in a bundle of only
//! the commits the other machine lacks. Applying it moves the branch fast
//! forward only, adds a worktree, lays the working tree down (deletions
//! included) and puts the index back. Every early exit, a refusal
//! included, undoes what it made.
//!
//! [spike 11]: ../../../../docs/spikes/11-moving-git.md

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use dent_protocol::MoveRepo;

/// The most a move's work may weigh: past it, push the branch first.
pub const MAX_BUNDLE: u64 = 200 * 1024 * 1024;

/// `git` in `dir`, prompts off, with `env` added and nothing of a caller's
/// git environment let in: its output, or what it said went wrong.
fn git(dir: &Path, args: &[&str], env: &[(&str, &Path)]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    for v in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_CEILING_DIRECTORIES",
    ] {
        cmd.env_remove(v);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .stdin(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().map_err(|e| format!("couldn't run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err
            .trim()
            .trim_start_matches("fatal: ")
            .trim_start_matches("error: ")
            .to_string())
    }
}

/// The repository `dir` is in: its root, and the folder inside it.
pub fn repo_of(dir: &Path) -> Option<(PathBuf, String)> {
    let root = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"], &[]).ok()?);
    let inside = git(dir, &["rev-parse", "--show-prefix"], &[]).unwrap_or_default();
    Some((root, inside.trim_end_matches('/').to_string()))
}

/// Pack the worktree at `root` for move `id` into `bundle`, leaving out
/// commits in `have` (tips the other machine has), and with the session's
/// `checkpoints` (the snapshots its turns were taken at), so going back to
/// one still works there. Returns what the other side needs to apply it.
pub fn pack(
    root: &Path,
    id: &str,
    have: &[String],
    checkpoints: &[String],
    bundle: &Path,
) -> Result<MoveRepo, String> {
    let tip = git(root, &["rev-parse", "--verify", "HEAD"], &[])
        .map_err(|_| "the repository has no commits yet; commit once, then move".to_string())?;
    let branch = git(root, &["symbolic-ref", "-q", "HEAD"], &[])
        .unwrap_or_default()
        .trim_start_matches("refs/heads/")
        .to_string();
    // What a snapshot can't carry.
    let status = git(
        root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
        &[],
    )?;
    if status
        .lines()
        .any(|l| l.starts_with("?? ") && l.ends_with('/'))
    {
        return Err("a separate git checkout is inside this folder; move it out first".into());
    }
    let raw = git(root, &["diff", "--raw", "HEAD"], &[])?;
    if raw
        .lines()
        .any(|l| l.contains(" 160000 ") || l.starts_with(":160000"))
    {
        return Err("a submodule has changes; commit or discard them first".into());
    }
    let snap_ref = format!("refs/dent/move/{id}/snapshot");
    let index_ref = format!("refs/dent/move/{id}/index");
    // Nothing left behind in the person's repository, however this ends.
    let mut cleanup = Cleanup {
        root: root.to_path_buf(),
        refs: vec![snap_ref.clone(), index_ref.clone()],
        dir: None,
    };
    let tmp = tempdir(root)?;
    cleanup.dir = Some(tmp.clone());
    let idx = tmp.join("index");
    let real = PathBuf::from(git(root, &["rev-parse", "--git-path", "index"], &[])?);
    let real = if real.is_absolute() {
        real
    } else {
        root.join(real)
    };
    if real.is_file() {
        std::fs::copy(&real, &idx).map_err(|e| e.to_string())?;
        // An old time marks every entry racily clean, so a same-size edit
        // made in the same second as the last index write is read again,
        // not trusted.
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let _ = std::fs::File::options()
            .write(true)
            .open(&idx)
            .and_then(|f| f.set_modified(old));
    }
    let cfg = [
        "-c",
        "core.splitIndex=false",
        "-c",
        "splitIndex.sharedIndexExpire=never",
        "-c",
        "core.fsmonitor=false",
    ];
    fn with<'a>(cfg: &[&'a str], args: &[&'a str]) -> Vec<&'a str> {
        [cfg, args].concat()
    }
    git(
        root,
        &with(&cfg, &["add", "--all", "--", ":/"]),
        &[("GIT_INDEX_FILE", &idx)],
    )?;
    let snap_tree = git(root, &["write-tree"], &[("GIT_INDEX_FILE", &idx)])?;
    let idx_tree = git(root, &with(&cfg, &["write-tree"]), &[])?;
    let _ = std::fs::remove_dir_all(&tmp);
    let who = ["-c", "user.name=dent", "-c", "user.email=dent@localhost"];
    let commit = |tree: &str, what: &str| {
        git(
            root,
            &[
                &who[..],
                &[
                    "commit-tree",
                    tree,
                    "-p",
                    &tip,
                    "-m",
                    &format!("dent move {id}: {what}"),
                ],
            ]
            .concat(),
            &[],
        )
    };
    let snapshot = commit(&snap_tree, "working tree")?;
    let index = commit(&idx_tree, "index")?;
    git(root, &["update-ref", &snap_ref, &snapshot], &[])?;
    git(root, &["update-ref", &index_ref, &index], &[])?;
    // Each checkpoint this repository still has, under a ref the bundle
    // can name.
    let mut extra = Vec::new();
    for (n, c) in checkpoints.iter().enumerate() {
        if git(root, &["cat-file", "-e", &format!("{c}^{{commit}}")], &[]).is_ok() {
            let r = format!("refs/dent/move/{id}/cp/{n}");
            git(root, &["update-ref", &r, c], &[])?;
            cleanup.refs.push(r.clone());
            extra.push(r);
        }
    }
    // Only what the other machine lacks.
    let mut args: Vec<String> = vec![
        "bundle".into(),
        "create".into(),
        "-q".into(),
        bundle.display().to_string(),
        snap_ref.clone(),
        index_ref.clone(),
    ];
    if !branch.is_empty() {
        args.push(format!("refs/heads/{branch}"));
    }
    args.extend(extra);
    for h in have {
        if git(root, &["cat-file", "-e", &format!("{h}^{{commit}}")], &[]).is_ok() {
            args.push(format!("^{h}"));
        }
    }
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    git(root, &argv, &[])?;
    let size = std::fs::metadata(bundle).map(|m| m.len()).unwrap_or(0);
    if size > MAX_BUNDLE {
        let _ = std::fs::remove_file(bundle);
        return Err("its work is over 200 MiB; push the branch, then move".into());
    }
    drop(cleanup);
    Ok(MoveRepo {
        tip,
        snapshot,
        index,
        branch,
        remotes: Vec::new(),
        subdir: String::new(),
    })
}

/// Refs and a folder made for a move, gone when it's done with them.
struct Cleanup {
    root: PathBuf,
    refs: Vec<String>,
    dir: Option<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for r in &self.refs {
            let _ = git(&self.root, &["update-ref", "-d", r], &[]);
        }
        if let Some(d) = &self.dir {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// A folder of our own beside the repository's metadata.
fn tempdir(root: &Path) -> Result<PathBuf, String> {
    let git_dir = PathBuf::from(git(root, &["rev-parse", "--absolute-git-dir"], &[])?);
    let t = git_dir.join(format!("dent-move-{}", std::process::id()));
    std::fs::create_dir_all(&t).map_err(|e| e.to_string())?;
    Ok(t)
}

/// Every remote of the repository at `root`, `origin` first: what names the
/// repository, so the other machine finds its own copy of it (whether any
/// can be cloned from is another matter).
pub fn remotes(root: &Path) -> Vec<dent_protocol::GitRemote> {
    let mut out: Vec<dent_protocol::GitRemote> = git(root, &["remote"], &[])
        .unwrap_or_default()
        .lines()
        .filter_map(|name| {
            let url = git(root, &["remote", "get-url", name], &[]).ok()?;
            Some(dent_protocol::GitRemote {
                name: name.to_string(),
                url,
            })
        })
        .collect();
    out.sort_by_key(|r| r.name != "origin");
    out
}

/// The commits `root` already has, for the other side to leave out: every
/// branch, remote branch and tag tip.
pub fn tips(root: &Path) -> Vec<String> {
    git(
        root,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
        &[],
    )
    .unwrap_or_default()
    .lines()
    .map(str::to_string)
    .collect()
}

/// Apply move `id`'s bundle into a new worktree at `new` of the repository
/// at `repo`. Refused before anything changes for a branch that has moved on
/// here, one checked out elsewhere at another commit, or a bundle that isn't
/// what the manifest says; anything made is undone on any failure after
/// that. The bundle's bytes are checked against the manifest's SHA-256
/// first (git's own check passes a bundle cut short in its header).
pub fn apply(
    repo: &Path,
    m: &MoveRepo,
    id: &str,
    bundle: &Path,
    new: &Path,
) -> Result<Applied, String> {
    let snap_ref = format!("refs/dent/move/{id}/snapshot");
    let index_ref = format!("refs/dent/move/{id}/index");
    let heads = git(
        repo,
        &["bundle", "list-heads", &bundle.display().to_string()],
        &[],
    )
    .map_err(|_| "the bundle is damaged".to_string())?;
    if !heads.lines().any(|l| l.ends_with(&snap_ref))
        || !heads.lines().any(|l| l.ends_with(&index_ref))
    {
        return Err("the bundle is damaged".into());
    }
    let verify = |repo: &Path| {
        git(
            repo,
            &["bundle", "verify", "-q", &bundle.display().to_string()],
            &[],
        )
        .is_ok()
    };
    if !verify(repo) {
        let _ = git(repo, &["fetch", "-q", "origin"], &[]);
        if !verify(repo) {
            return Err("commits it builds on aren't on this machine; push the branch from where it was, then move".into());
        }
    }
    if !m.branch.is_empty()
        && git(
            repo,
            &["check-ref-format", &format!("refs/heads/{}", m.branch)],
            &[],
        )
        .is_err()
    {
        return Err(format!("{:?} isn't a branch name git takes", m.branch));
    }
    let mut undo = Undo {
        repo: repo.to_path_buf(),
        steps: Vec::new(),
        made: None,
        refs: vec![snap_ref.clone(), index_ref.clone()],
        done: false,
    };
    git(
        repo,
        &[
            "fetch",
            "-q",
            &bundle.display().to_string(),
            &format!("+{snap_ref}:{snap_ref}"),
            &format!("+{index_ref}:{index_ref}"),
            // The session's checkpoints, kept here under the move.
            &format!("+refs/dent/move/{id}/cp/*:refs/dent/moved/{id}/cp/*"),
        ],
        &[],
    )?;
    let at = |r: &str| git(repo, &["rev-parse", r], &[]).unwrap_or_default();
    if at(&snap_ref) != m.snapshot
        || at(&index_ref) != m.index
        || at(&format!("{}^", m.snapshot)) != m.tip
    {
        return Err("the bundle isn't the one the manifest describes".into());
    }
    let mut checkout = m.branch.clone();
    if !m.branch.is_empty() {
        let head = format!("refs/heads/{}", m.branch);
        let cur = git(repo, &["rev-parse", "-q", "--verify", &head], &[]).unwrap_or_default();
        let elsewhere = git(repo, &["worktree", "list", "--porcelain"], &[])
            .unwrap_or_default()
            .lines()
            .any(|l| l == format!("branch {head}"));
        if elsewhere {
            if cur == m.tip {
                // Out elsewhere at the same commit: a detached worktree.
                checkout.clear();
            } else {
                return Err(format!(
                    "{} is checked out in another worktree here",
                    m.branch
                ));
            }
        }
        if !cur.is_empty() && cur != m.tip {
            if git(repo, &["merge-base", "--is-ancestor", &cur, &m.tip], &[]).is_err() {
                return Err(format!(
                    "{} has commits here the move doesn't; bring them together, then move again",
                    m.branch
                ));
            }
            git(repo, &["update-ref", &head, &m.tip, &cur], &[])?;
            undo.steps.push(vec![
                "update-ref".into(),
                head.clone(),
                cur.clone(),
                m.tip.clone(),
            ]);
        } else if cur.is_empty() {
            git(repo, &["update-ref", &head, &m.tip, ""], &[])?;
            undo.steps.push(vec![
                "update-ref".into(),
                "-d".into(),
                head.clone(),
                m.tip.clone(),
            ]);
        }
    }
    if new.exists() {
        return Err(format!("{} is there already", new.display()));
    }
    undo.made = Some(new.to_path_buf());
    let new_s = new.display().to_string();
    if checkout.is_empty() {
        git(
            repo,
            &["worktree", "add", "-q", "--detach", &new_s, &m.tip],
            &[],
        )?;
    } else {
        git(repo, &["worktree", "add", "-q", &new_s, &checkout], &[])?;
    }
    git(
        new,
        &["checkout", "-q", "--no-overlay", &m.snapshot, "--", "."],
        &[],
    )?;
    git(
        new,
        &["read-tree", "-m", &format!("{}^{{tree}}", m.index)],
        &[],
    )?;
    Ok(Applied(undo))
}

/// An apply that's done, undone if dropped before [`Applied::keep`]: the
/// steps after it (placing the conversation) can still fail.
pub struct Applied(Undo);

impl std::fmt::Debug for Applied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Applied")
    }
}

impl Applied {
    pub fn keep(mut self) {
        self.0.done = true;
    }
}

/// What an apply made, undone unless it finished: the worktree first (a
/// branch checked out there can't be put back), then each ref change,
/// newest first, each a compare-and-swap, so a branch someone moved
/// meanwhile is left alone.
struct Undo {
    repo: PathBuf,
    steps: Vec<Vec<String>>,
    made: Option<PathBuf>,
    refs: Vec<String>,
    done: bool,
}

impl Drop for Undo {
    fn drop(&mut self) {
        for r in &self.refs {
            let _ = git(&self.repo, &["update-ref", "-d", r], &[]);
        }
        if self.done {
            return;
        }
        if let Some(new) = &self.made {
            let _ = git(
                &self.repo,
                &["worktree", "remove", "--force", &new.display().to_string()],
                &[],
            );
            let _ = std::fs::remove_dir_all(new);
            let _ = git(&self.repo, &["worktree", "prune"], &[]);
        }
        for step in self.steps.iter().rev() {
            let args: Vec<&str> = step.iter().map(String::as_str).collect();
            let _ = git(&self.repo, &args, &[]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, script: &str) {
        let ok = Command::new("sh")
            .arg("-c")
            .arg(script)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap()
            .success();
        assert!(ok, "{script}");
    }

    fn out(dir: &Path, args: &[&str]) -> String {
        git(dir, args, &[]).unwrap()
    }

    /// The source worktree of spike 11: every kind of change at once.
    fn rig(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("dent-move-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (origin, src, dst) = (base.join("origin.git"), base.join("src"), base.join("dst"));
        sh(
            &base,
            "git init -q --bare origin.git && git clone -q origin.git src",
        );
        sh(
            &src,
            "git checkout -q -b work && printf 'a\\n' > a.txt && printf 'b\\n' > b.txt && printf 'c\\n' > c.txt \
             && printf 'd\\n' > d.txt && printf 'node_modules/\\n.env\\n' > .gitignore && printf 'x' > run.sh \
             && git add -A && git commit -qm base && git push -q origin work",
        );
        // A clone on the "other machine", with the pushed history only.
        sh(&base, "git clone -q origin.git dst");
        sh(
            &src,
            "printf 'two\\n' >> a.txt && git commit -qam 'not pushed' \
             && printf '\\x00\\x01bin' > blob.bin && git add blob.bin && git commit -qm 'binary, not pushed' \
             && printf 'staged\\n' >> b.txt && git add b.txt && printf 'then edited\\n' >> b.txt \
             && printf 'unstaged\\n' >> c.txt \
             && git mv d.txt d2.txt \
             && chmod +x run.sh \
             && git rm -q a.txt && git checkout -q HEAD -- a.txt && rm a.txt \
             && mkdir -p new/dir && printf 'n\\n' > new/dir/n.txt \
             && printf 'SECRET=1\\n' > .env && git add -f .env && printf 'SECRET=2\\n' > .env \
             && printf '\\x00\\xffraw' > untracked.bin \
             && mkdir -p node_modules/x && printf 'junk' > node_modules/x/i.js",
        );
        let _ = origin;
        (base, src, dst)
    }

    fn view(dir: &Path) -> (String, String, String, String) {
        let files = out(dir, &["ls-files", "-s"]);
        let mut hashes: Vec<String> = Vec::new();
        for f in walk(dir) {
            let rel = f.strip_prefix(dir).unwrap().display().to_string();
            if rel.starts_with(".git") || rel.starts_with("node_modules") {
                continue;
            }
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o111;
            hashes.push(format!("{rel} {:?} {mode:o}", std::fs::read(&f).unwrap()));
        }
        hashes.sort();
        (
            out(dir, &["status", "--porcelain=v1", "--untracked-files=all"]),
            out(dir, &["diff"]),
            out(dir, &["diff", "--cached"]) + &files,
            hashes.join("\n"),
        )
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn a_worktree_arrives_exactly_as_it_was() {
        let (base, src, dst) = rig("exact");
        let bundle = base.join("m1.bundle");
        let m = pack(&src, "m1", &tips(&dst), &[], &bundle).unwrap();
        assert_eq!(m.branch, "work");
        // Only what the other side lacks: tiny next to the repository.
        assert!(std::fs::metadata(&bundle).unwrap().len() < 20_000);
        // The person's own index and refs are as they were.
        assert!(out(&src, &["for-each-ref", "refs/dent"]).is_empty());
        let new = base.join("dst-moved");
        apply(&dst, &m, "m1", &bundle, &new).unwrap().keep();
        assert_eq!(view(&new), view(&src));
        // Ignored folders stay behind; the moved branch is where it was.
        assert!(!new.join("node_modules").exists());
        assert_eq!(out(&dst, &["rev-parse", "refs/heads/work"]), m.tip);
        assert!(out(&dst, &["for-each-ref", "refs/dent"]).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn what_would_lose_work_is_refused_and_leaves_nothing_behind() {
        let (base, src, dst) = rig("refuse");
        let bundle = base.join("m2.bundle");
        let m = pack(&src, "m2", &tips(&dst), &[], &bundle).unwrap();
        // The branch moved on here: refused, branch untouched, no worktree.
        sh(
            &dst,
            "git checkout -q work && printf 'mine\\n' > mine.txt && git add mine.txt && git commit -qm mine && git checkout -q --detach",
        );
        let before = out(&dst, &["rev-parse", "refs/heads/work"]);
        let new = base.join("dst-moved");
        let e = apply(&dst, &m, "m2", &bundle, &new).unwrap_err();
        assert!(e.contains("has commits here"), "{e}");
        assert_eq!(out(&dst, &["rev-parse", "refs/heads/work"]), before);
        assert!(!new.exists());
        assert!(out(&dst, &["for-each-ref", "refs/dent"]).is_empty());
        // A bundle cut short: refused, with nothing left behind.
        let short = base.join("short.bundle");
        let bytes = std::fs::read(&bundle).unwrap();
        std::fs::write(&short, &bytes[..bytes.len() / 2]).unwrap();
        assert!(apply(&dst, &m, "m2", &short, &new).is_err());
        assert!(!new.exists());
        assert!(out(&dst, &["for-each-ref", "refs/dent"]).is_empty());
        // The new folder there already: refused, and the branch it made is
        // taken out again, so a clean retry works.
        let (base2, src2, dst2) = rig("exists");
        let b2 = base2.join("m3.bundle");
        let m2 = pack(&src2, "m3", &tips(&dst2), &[], &b2).unwrap();
        sh(&dst2, "git branch -q -D work 2>/dev/null; true");
        let taken = base2.join("taken");
        std::fs::create_dir_all(&taken).unwrap();
        assert!(apply(&dst2, &m2, "m3", &b2, &taken).is_err());
        assert!(
            git(
                &dst2,
                &["rev-parse", "-q", "--verify", "refs/heads/work"],
                &[]
            )
            .is_err()
        );
        let fine = base2.join("fine");
        apply(&dst2, &m2, "m3", &b2, &fine).unwrap().keep();
        assert_eq!(view(&fine), view(&src2));
        // A separate checkout inside: refused on the source, before packing.
        sh(
            &src,
            "mkdir -p inner && cd inner && git init -q && printf 'z' > z && git add z && git -c user.name=t -c user.email=t@t commit -qm z",
        );
        assert!(
            pack(&src, "m4", &[], &[], &base.join("m4.bundle"))
                .unwrap_err()
                .contains("separate git checkout")
        );
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base2);
    }
}
