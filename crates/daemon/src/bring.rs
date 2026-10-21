//! Bringing a project to this machine from another
//! ([remote-plan.md](../../../docs/remote-plan.md), stage 2): the machine
//! that has it says what a clone needs (its remotes, branch and the
//! ignored files a clone won't have) and reads the chosen ones; the
//! machine it goes to clones it, recreates the remotes, checks out the
//! branch and writes the files in. Daemons never talk to each other: the
//! client both trust carries the pack across.

use std::path::{Component, Path, PathBuf};

use dent_protocol::{Bring, CARRIED_MAX, Carried, GitRemote, Ignored, Pack, b64};

use crate::workspace::run;

/// More ignored files than this aren't listed: a checkout full of build
/// output is offered its first ones.
const MAX_IGNORED: usize = 500;

/// What another machine needs to bring the checkout at `dir` there; `None`
/// when it isn't in a Git repository.
pub fn pack(dir: &Path) -> Option<Pack> {
    let root = PathBuf::from(run(dir, &["rev-parse", "--show-toplevel"]).ok()?.trim());
    let mut remotes: Vec<GitRemote> = run(&root, &["remote"])
        .unwrap_or_default()
        .lines()
        .filter_map(|name| {
            let url = run(&root, &["remote", "get-url", name]).ok()?;
            let url = url.trim().to_string();
            cloneable(&url).then(|| GitRemote {
                name: name.to_string(),
                url,
            })
        })
        .collect();
    remotes.sort_by_key(|r| r.name != "origin");
    const IGNORED: [&str; 4] = ["ls-files", "-z", "--others", "--ignored"];
    // A folder of ignored files (`node_modules/`) comes as one entry
    // ending in a slash, and isn't offered; but an environment file inside
    // one (`web/.env` with nothing else in `web/`) still is.
    let top = run(
        &root,
        &[
            &IGNORED[..],
            &["--exclude-standard", "--directory", "--no-empty-directory"],
        ]
        .concat(),
    )
    .unwrap_or_default();
    let env = run(
        &root,
        &[
            &IGNORED[..],
            &[
                "--exclude-standard",
                "--",
                ":(glob)**/.env",
                ":(glob)**/.env.*",
            ],
        ]
        .concat(),
    )
    .unwrap_or_default();
    let mut paths: Vec<&str> = top
        .split('\0')
        .chain(env.split('\0'))
        .filter(|p| !p.is_empty() && !p.ends_with('/'))
        .collect();
    paths.sort_unstable();
    paths.dedup();
    let mut ignored: Vec<Ignored> = paths
        .into_iter()
        .filter_map(|p| {
            let meta = std::fs::symlink_metadata(root.join(p)).ok()?;
            (meta.is_file() && meta.len() as usize <= CARRIED_MAX).then(|| Ignored {
                path: p.to_string(),
                size: meta.len(),
                carry: is_env(p),
            })
        })
        .collect();
    ignored.sort_by(|a, b| b.carry.cmp(&a.carry).then_with(|| a.path.cmp(&b.path)));
    ignored.truncate(MAX_IGNORED);
    Some(Pack {
        name: root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        remotes,
        branch: crate::workspace::branch(&root),
        ignored,
    })
}

/// A URL another machine can clone from: not a path on this one.
fn cloneable(url: &str) -> bool {
    ["https://", "http://", "ssh://", "git://", "git@"]
        .iter()
        .any(|p| url.starts_with(p))
        // scp-like `host:path`, as SSH config aliases are written.
        || (url.contains(':') && !url.starts_with('/') && !url.contains("://"))
}

/// What git said, in one line for the chat.
fn first_line(e: &str) -> String {
    e.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("git failed")
        .trim_end_matches('.')
        .to_string()
}

/// The HTTPS address of a repository named by SSH: `git@github.com:a/b.git`
/// and `ssh://git@github.com/a/b.git` are `https://github.com/a/b.git`.
fn https_of(url: &str) -> Option<String> {
    let (host, path) = if let Some(rest) = url.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/')?;
        // A port is SSH's, not the web's.
        (host.split(':').next()?, path)
    } else if !url.contains("://") {
        let rest = url.split_once('@').map_or(url, |(_, r)| r);
        rest.split_once(':')?
    } else {
        return None;
    };
    // An SSH config alias (`studio:repos/b`) names no web host.
    (host.contains('.') && !path.is_empty()).then(|| format!("https://{host}/{path}"))
}

/// An environment file: `.env`, `.env.local`, `apps/web/.env.production`.
fn is_env(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == ".env" || (name.starts_with(".env.") && !name.ends_with(".example"))
}

/// A path relative to a checkout that stays inside it.
fn inside(rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    let ok = !rel.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)));
    ok.then(|| p.to_path_buf())
}

/// The ignored files `paths` of the checkout at `dir`, for another
/// machine. Only files inside it, and no more than [`CARRIED_MAX`] in all.
pub fn read_files(dir: &Path, paths: &[String]) -> Result<Vec<Carried>, String> {
    let root = PathBuf::from(
        run(dir, &["rev-parse", "--show-toplevel"])
            .map_err(|_| format!("{} isn't in a Git repository", dir.display()))?
            .trim(),
    );
    let real = root.canonicalize().map_err(|e| e.to_string())?;
    let mut total = 0;
    let mut files = Vec::new();
    for rel in paths {
        let p = inside(rel).ok_or_else(|| format!("{rel} isn't inside the project"))?;
        let full = root
            .join(&p)
            .canonicalize()
            .map_err(|e| format!("{rel}: {e}"))?;
        if !full.starts_with(&real) {
            return Err(format!("{rel} isn't inside the project"));
        }
        let bytes = std::fs::read(&full).map_err(|e| format!("{rel}: {e}"))?;
        total += bytes.len();
        if total > CARRIED_MAX {
            return Err(format!(
                "the files come to more than {} MB",
                CARRIED_MAX >> 20
            ));
        }
        files.push(Carried {
            name: rel.clone(),
            bytes_b64: b64::encode(&bytes),
        });
    }
    Ok(files)
}

/// Where a brought project's files wait until its clone is done: under the
/// state folder, so a restart mid-clone still has them.
pub fn staging(state: &Path, session: &str) -> PathBuf {
    state.join("bring").join(session)
}

/// Keep what `bring` carries under `staging`, each at its path.
pub fn stage(staging: &Path, files: &[Carried]) -> Result<(), String> {
    for f in files {
        let rel = inside(&f.name).ok_or_else(|| format!("{} isn't inside the project", f.name))?;
        let bytes =
            b64::decode(&f.bytes_b64).ok_or_else(|| format!("{} didn't arrive whole", f.name))?;
        let to = staging.join(rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&to, bytes).map_err(|e| format!("{}: {e}", f.name))?;
    }
    Ok(())
}

/// Clone `b` into `dest` (an empty folder this machine made for it, or one
/// a clone cut short left: emptied first), recreate its other remotes,
/// check out its branch and write in the files waiting in `staging`. What
/// happened, in a line for the chat, or what went wrong.
pub fn clone_into(dest: &Path, b: &Bring, staging: &Path) -> Result<String, String> {
    let first = b
        .remotes
        .first()
        .ok_or_else(|| format!("{} has no remote this machine can clone from", b.name))?;
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| e.to_string())?;
    }
    let parent = dest.parent().ok_or("no folder to clone into")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let to = dest.display().to_string();
    let mut notes = Vec::new();
    let clone = |url: &str| {
        run(parent, &["clone", "-q", "-o", &first.name, "--", url, &to]).map_err(|e| {
            let _ = std::fs::remove_dir_all(dest);
            first_line(&e)
        })
    };
    if let Err(e) = clone(&first.url) {
        // This machine often has no SSH key for the host the other one
        // pushes to over SSH; a public repository clones over HTTPS all
        // the same, and stays that way here.
        let https =
            https_of(&first.url).ok_or_else(|| format!("couldn't clone {}: {e}", first.url))?;
        clone(&https).map_err(|_| format!("couldn't clone {}: {e}", first.url))?;
        notes.push(format!(
            "{} over HTTPS, as SSH to it didn't work here",
            first.name
        ));
    }
    for r in &b.remotes[1..] {
        if let Err(e) = run(dest, &["remote", "add", "--", &r.name, &r.url]) {
            notes.push(format!("couldn't add the remote {}: {e}", r.name));
        }
    }
    if b.remotes.len() > 1 {
        let _ = run(dest, &["fetch", "-q", "--all"]);
    }
    let on = crate::workspace::branch(dest);
    if let Some(want) = b.branch.as_deref().filter(|w| on.as_deref() != Some(*w)) {
        // A branch the remote has comes down tracking it; one only the
        // other machine has stays there.
        if run(dest, &["checkout", "-q", want]).is_err() {
            notes.push(format!(
                "{want} isn't on {}, so this is {}",
                first.name,
                on.as_deref().unwrap_or("the default branch")
            ));
        }
    }
    let mut brought = 0;
    let mut unignored = Vec::new();
    for (rel, from) in walk(staging) {
        let to = dest.join(&rel);
        // A file the clone has is the project's own, not the one carried.
        if to.exists() {
            notes.push(format!("kept the project's own {}", rel.display()));
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&from, &to).map_err(|e| format!("{}: {e}", rel.display()))?;
        brought += 1;
        // Ignored where it came from by that checkout's own rules
        // (`.git/info/exclude`, a global excludes file), which a clone
        // doesn't have: it stays ignored here, so it's never committed.
        let rel = rel.to_string_lossy();
        if run(dest, &["check-ignore", "-q", "--", &rel]).is_err() {
            unignored.push(format!("/{rel}"));
        }
    }
    if !unignored.is_empty() {
        exclude(dest, &unignored)
            .map_err(|e| format!("couldn't keep the files out of Git: {e}"))?;
    }
    let branch = crate::workspace::branch(dest).unwrap_or_else(|| "its default branch".into());
    let mut line = format!("Cloned {} from {} on {branch}", b.name, first.name);
    match brought {
        0 => {}
        1 => line.push_str(" and brought 1 ignored file"),
        n => line.push_str(&format!(" and brought {n} ignored files")),
    }
    for n in notes {
        line.push_str("; ");
        line.push_str(&n);
    }
    Ok(line)
}

/// Add `paths` to the checkout's own ignore list, `.git/info/exclude`,
/// which is never committed.
fn exclude(dest: &Path, paths: &[String]) -> Result<(), String> {
    let file = run(dest, &["rev-parse", "--git-path", "info/exclude"])?;
    let file = dest.join(file.trim());
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("# Brought by dent from another machine, where Git ignores them\n");
    for p in paths {
        // Glob characters in a name are matched as themselves.
        let mut line = String::new();
        for c in p.chars() {
            if matches!(c, '*' | '?' | '[' | '\\' | '!' | '#') {
                line.push('\\');
            }
            line.push(c);
        }
        text.push_str(&line);
        text.push('\n');
    }
    std::fs::write(&file, text).map_err(|e| e.to_string())
}

/// Every file under `dir`, with its path relative to it.
fn walk(dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() => {
                    if let Ok(rel) = p.strip_prefix(dir) {
                        out.push((rel.to_path_buf(), p.clone()));
                    }
                }
                _ => {}
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_files_are_ticked() {
        assert!(is_env(".env"));
        assert!(is_env("apps/web/.env.local"));
        assert!(!is_env(".env.example"));
        assert!(!is_env("notes.env"));
        assert!(!is_env(".envrc"));
    }

    #[test]
    fn ssh_addresses_have_an_https_twin() {
        assert_eq!(
            https_of("git@github.com:a/b.git").as_deref(),
            Some("https://github.com/a/b.git")
        );
        assert_eq!(
            https_of("ssh://git@gitlab.com:2222/a/b").as_deref(),
            Some("https://gitlab.com/a/b")
        );
        assert_eq!(https_of("studio:repos/b"), None);
        assert_eq!(https_of("https://github.com/a/b"), None);
    }

    #[test]
    fn only_paths_inside() {
        assert!(inside("a/.env").is_some());
        for bad in ["", "/etc/passwd", "../x", "a/../../x", "./a"] {
            assert!(inside(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn only_remotes_another_machine_can_reach() {
        assert!(cloneable("https://github.com/a/b.git"));
        assert!(cloneable("git@github.com:a/b.git"));
        assert!(cloneable("studio:repos/b"));
        assert!(!cloneable("/Users/me/b"));
        assert!(!cloneable("../b"));
        assert!(!cloneable("file:///Users/me/b"));
    }
}
