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
    let tmp = tempdir(root, id)?;
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

/// A branch's name without what landing beside added to it
/// (`-moved`, `-moved-2`).
fn moved_base(branch: &str) -> &str {
    if let Some(b) = branch.strip_suffix("-moved") {
        return b;
    }
    match branch.rsplit_once("-moved-") {
        // As dent numbers them: 2 and on, never a year or a ticket.
        Some((b, n)) if !b.is_empty() && n.len() <= 3 && n.parse::<u32>().is_ok_and(|n| n >= 2) => {
            b
        }
        _ => branch,
    }
}

/// A folder of our own beside the repository's metadata, for move `id`
/// alone: sessions of one repository can be packed at once, and each
/// takes its folder away when done.
fn tempdir(root: &Path, id: &str) -> Result<PathBuf, String> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let git_dir = PathBuf::from(git(root, &["rev-parse", "--absolute-git-dir"], &[])?);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let t = git_dir.join(format!("dent-move-{}-{id}-{n}", std::process::id()));
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
    let mut on = None;
    if !m.branch.is_empty() {
        let head = format!("refs/heads/{}", m.branch);
        let cur = git(repo, &["rev-parse", "-q", "--verify", &head], &[]).unwrap_or_default();
        let list = git(repo, &["worktree", "list", "--porcelain"], &[]).unwrap_or_default();
        // Its branch here is the person's to keep as it is when it's
        // checked out (as often as not in the main checkout), or has
        // commits the session hasn't: the session lands on a branch of its
        // own at its commit instead, said so, and nothing of theirs moves.
        let mut path = "";
        let mut out_at = None;
        for l in list.lines() {
            if let Some(p) = l.strip_prefix("worktree ") {
                path = p;
            } else if l == format!("branch {head}") {
                out_at = Some(path.to_string());
            }
        }
        let behind = !cur.is_empty()
            && cur != m.tip
            && git(repo, &["merge-base", "--is-ancestor", &cur, &m.tip], &[]).is_err();
        let why = match out_at {
            Some(p) => {
                // As the chat says paths: home as ~.
                let home = crate::paths::home().display().to_string();
                let p = match p.strip_prefix(&home) {
                    Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
                    _ => p,
                };
                Some(format!("{} is checked out in {p}", m.branch))
            }
            None if behind => Some(format!("{} here has commits it hasn't", m.branch)),
            None => None,
        };
        if let Some(why) = why {
            // A try of this move cut short made one at its commit, out
            // nowhere now: that's the one it lands on again.
            let out_anywhere = |name: &str| {
                list.lines()
                    .any(|l| l == format!("branch refs/heads/{name}"))
            };
            // Moved beside before, it's numbered on, not named again:
            // `work-moved-2`, never `work-moved-moved`.
            let base = moved_base(&m.branch);
            let mut n = 1;
            let mut name = format!("{base}-moved");
            loop {
                let r = format!("refs/heads/{name}");
                match git(repo, &["rev-parse", "-q", "--verify", &r], &[]) {
                    Err(_) => {
                        git(repo, &["update-ref", &r, &m.tip, ""], &[])?;
                        undo.steps
                            .push(vec!["update-ref".into(), "-d".into(), r, m.tip.clone()]);
                        break;
                    }
                    Ok(at) if at == m.tip && !out_anywhere(&name) => break,
                    Ok(_) => {
                        n += 1;
                        name = format!("{base}-moved-{n}");
                    }
                }
            }
            on = Some((name.clone(), why));
            checkout = name;
        } else if !cur.is_empty() && cur != m.tip {
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
    crate::workspace::one_at_a_time(repo, || {
        if checkout.is_empty() {
            git(
                repo,
                &["worktree", "add", "-q", "--detach", &new_s, &m.tip],
                &[],
            )
        } else {
            git(repo, &["worktree", "add", "-q", &new_s, &checkout], &[])
        }
    })?;
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
    Ok(Applied(undo, on))
}

/// An apply that's done, undone if dropped before [`Applied::keep`]: the
/// steps after it (placing the conversation) can still fail.
pub struct Applied(Undo, Option<(String, String)>);

impl std::fmt::Debug for Applied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Applied")
    }
}

impl Applied {
    /// The branch it landed on instead of its own, and why, when it did.
    pub fn instead(&self) -> Option<&(String, String)> {
        self.1.as_ref()
    }

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
            crate::workspace::one_at_a_time(&self.repo, || {
                let _ = git(
                    &self.repo,
                    &["worktree", "remove", "--force", &new.display().to_string()],
                    &[],
                );
                let _ = std::fs::remove_dir_all(new);
                let _ = git(&self.repo, &["worktree", "prune"], &[]);
            });
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

    /// Its branch here checked out (in the person's own checkout, say):
    /// it lands on a branch of its own at its commit, and their checkout
    /// and branch are as they were.
    #[test]
    fn a_branch_checked_out_here_is_left_and_it_lands_beside_it() {
        let (base, src, dst) = rig("out");
        let bundle = base.join("m4.bundle");
        let m = pack(&src, "m4", &tips(&dst), &[], &bundle).unwrap();
        sh(&dst, "git checkout -q work");
        let before = out(&dst, &["rev-parse", "refs/heads/work"]);
        let status = out(&dst, &["status", "--porcelain"]);
        let new = base.join("dst-moved");
        let a = apply(&dst, &m, "m4", &bundle, &new).unwrap();
        let (name, why) = a.instead().cloned().unwrap();
        assert_eq!(name, "work-moved");
        assert!(why.starts_with("work is checked out in "), "{why}");
        a.keep();
        assert_eq!(view(&new), view(&src));
        assert_eq!(
            out(&new, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "work-moved"
        );
        assert_eq!(out(&dst, &["rev-parse", "--abbrev-ref", "HEAD"]), "work");
        assert_eq!(out(&dst, &["rev-parse", "refs/heads/work"]), before);
        assert_eq!(out(&dst, &["status", "--porcelain"]), status);
        // Cut short there and tried again (its worktree taken away first,
        // as a retry does): it lands on the same branch, not a second.
        out(
            &dst,
            &["worktree", "remove", "--force", &new.display().to_string()],
        );
        let a = apply(&dst, &m, "m4", &bundle, &new).unwrap();
        assert_eq!(a.instead().unwrap().0, "work-moved");
        a.keep();
        assert!(
            git(
                &dst,
                &["rev-parse", "-q", "--verify", "refs/heads/work-moved-2"],
                &[]
            )
            .is_err()
        );
        // Another move of it while that one's out: a branch of its own.
        let other = base.join("dst-moved-again");
        let b6 = base.join("m6.bundle");
        let m6 = pack(&src, "m6", &tips(&dst), &[], &b6).unwrap();
        let a = apply(&dst, &m6, "m6", &b6, &other).unwrap();
        assert_eq!(a.instead().unwrap().0, "work-moved-2");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_moved_branch_is_numbered_on() {
        assert_eq!(super::moved_base("work"), "work");
        assert_eq!(super::moved_base("work-moved"), "work");
        assert_eq!(super::moved_base("work-moved-3"), "work");
        assert_eq!(super::moved_base("feat/x-moved-12"), "feat/x");
        assert_eq!(super::moved_base("-moved"), "");
        assert_eq!(super::moved_base("unmoved-moved-x"), "unmoved-moved-x");
        assert_eq!(super::moved_base("moved-moved-2"), "moved");
        assert_eq!(
            super::moved_base("release-moved-2024"),
            "release-moved-2024"
        );
        assert_eq!(super::moved_base("fix-moved-0"), "fix-moved-0");
    }

    #[test]
    fn nothing_of_theirs_moves_and_what_fails_leaves_nothing_behind() {
        let (base, src, dst) = rig("refuse");
        let bundle = base.join("m2.bundle");
        let m = pack(&src, "m2", &tips(&dst), &[], &bundle).unwrap();
        // The branch moved on here: theirs is untouched, and it lands on a
        // branch of its own; not kept, that goes again with the worktree.
        sh(
            &dst,
            "git checkout -q work && printf 'mine\\n' > mine.txt && git add mine.txt && git commit -qm mine && git checkout -q --detach",
        );
        let before = out(&dst, &["rev-parse", "refs/heads/work"]);
        let new = base.join("dst-moved");
        let a = apply(&dst, &m, "m2", &bundle, &new).unwrap();
        let (name, why) = a.instead().cloned().unwrap();
        assert_eq!(name, "work-moved");
        assert!(why.contains("has commits it hasn't"), "{why}");
        assert_eq!(view(&new), view(&src));
        assert_eq!(out(&dst, &["rev-parse", "refs/heads/work"]), before);
        drop(a);
        assert!(!new.exists());
        assert!(
            git(
                &dst,
                &["rev-parse", "-q", "--verify", "refs/heads/work-moved"],
                &[]
            )
            .is_err()
        );
        assert!(out(&dst, &["for-each-ref", "refs/dent"]).is_empty());
        // A bundle cut short, where its commits aren't yet: refused, with
        // nothing left behind. (Where they are already, from a try before,
        // what's there is what the manifest names, and that's checked.)
        let (base3, src3, dst3) = rig("short");
        let b3 = base3.join("m5.bundle");
        let m3 = pack(&src3, "m5", &tips(&dst3), &[], &b3).unwrap();
        let short = base3.join("short.bundle");
        let bytes = std::fs::read(&b3).unwrap();
        std::fs::write(&short, &bytes[..bytes.len() / 2]).unwrap();
        let new3 = base3.join("dst-moved");
        assert!(apply(&dst3, &m3, "m5", &short, &new3).is_err());
        assert!(!new3.exists());
        assert!(out(&dst3, &["for-each-ref", "refs/dent"]).is_empty());
        let _ = std::fs::remove_dir_all(&base3);
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
