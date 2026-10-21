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
    // What a clone of the remote wouldn't have: commits on no remote, and
    // tracked files changed and not committed.
    let count = |args: &[&str]| -> u32 {
        run(&root, args).map_or(0, |out| {
            if args[0] == "rev-list" {
                out.trim().parse().unwrap_or(0)
            } else {
                out.lines().filter(|l| !l.trim().is_empty()).count() as u32
            }
        })
    };
    let unpushed = count(&["rev-list", "--count", "HEAD", "--not", "--remotes"]);
    let changed = count(&["status", "--porcelain", "--untracked-files=no"]);
    Some(Pack {
        name: root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        remotes,
        branch: crate::workspace::branch(&root),
        ignored,
        unpushed,
        changed,
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

/// A path relative to a checkout that stays inside it, and out of its
/// repository (`.git`, however it's cased: the Mac's disk reads `.GIT` as
/// `.git`).
fn inside(rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    let ok = !rel.is_empty()
        && p.components().all(|c| {
            matches!(c, Component::Normal(n)
                if !n.to_string_lossy().trim_end_matches(['.', ' ']).eq_ignore_ascii_case(".git"))
        });
    ok.then(|| p.to_path_buf())
}

/// A remote's name that git reads as a name: no option, no path.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '.'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `rel` under `dest`, if writing it there stays there: no folder on the
/// way is a link (a clone can hold `web -> ~/.ssh`). `None` also when
/// something's already at the end, link or not: the project's own.
fn writable(dest: &Path, rel: &Path) -> Option<PathBuf> {
    // However a name reaches it (another case, another spelling the disk
    // reads as the same), nothing goes into the repository.
    let git = dest.join(".git").canonicalize().ok();
    let mut at = dest.to_path_buf();
    let parts: Vec<_> = rel.components().collect();
    for (i, c) in parts.iter().enumerate() {
        at.push(c);
        match std::fs::symlink_metadata(&at) {
            Err(_) => return Some(dest.join(rel)),
            Ok(m) if i + 1 == parts.len() || m.file_type().is_symlink() || !m.is_dir() => {
                return None;
            }
            Ok(_)
                if git
                    .as_ref()
                    .is_some_and(|g| at.canonicalize().is_ok_and(|r| r.starts_with(g))) =>
            {
                return None;
            }
            Ok(_) => {}
        }
    }
    None
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
        // Only what Git ignores, as `pack` offered: the rest comes with
        // the clone.
        if run(&root, &["check-ignore", "-q", "--", rel]).is_err() {
            return Err(format!("{rel} isn't a file Git ignores"));
        }
        let meta = std::fs::metadata(&full).map_err(|e| format!("{rel}: {e}"))?;
        total += meta.len() as usize;
        if !meta.is_file() || total > CARRIED_MAX {
            return Err(format!(
                "the files come to more than {} MB",
                CARRIED_MAX >> 20
            ));
        }
        let bytes = std::fs::read(&full).map_err(|e| format!("{rel}: {e}"))?;
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
/// a clone cut short left), recreate its other remotes, check out its
/// branch and write in the files waiting in `staging`. What happened, as a
/// line for the chat and notes to follow it, or what went wrong.
pub fn clone_into(dest: &Path, b: &Bring, staging: &Path) -> Result<(String, Vec<String>), String> {
    let first = b
        .remotes
        .first()
        .ok_or_else(|| format!("{} has no remote this machine can clone from", b.name))?;
    let parent = dest.parent().ok_or("no folder to clone into")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    // Each try clones into a folder of its own beside `dest`, moved into
    // place once whole: a clone a killed daemon left running writes to its
    // own folder, never this one's. Those left by earlier tries go.
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tag = format!(".{name}.cloning-");
    if let Ok(rd) = std::fs::read_dir(parent) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&tag) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let temp = parent.join(format!("{tag}{nanos}"));
    let to = temp.display().to_string();
    let mut notes = Vec::new();
    if !plain_name(&first.name) {
        return Err(format!("{} isn't a remote's name", first.name));
    }
    let clone = |url: &str| {
        quiet_git(parent, &["clone", "-q", "-o", &first.name, "--", url, &to]).map_err(|e| {
            let _ = std::fs::remove_dir_all(&temp);
            first_line(&e)
        })
    };
    if let Err(e) = clone(&first.url) {
        // This machine often has no SSH key for the host the other one
        // pushes to over SSH; a public repository clones over HTTPS all
        // the same, and stays that way here.
        let https =
            https_of(&first.url).ok_or_else(|| format!("couldn't clone {}: {e}", first.url))?;
        clone(&https)
            .map_err(|e2| format!("couldn't clone {}: {e}; nor over HTTPS: {e2}", first.url))?;
        notes.push(format!(
            "{} over HTTPS, as SSH to it didn't work here",
            first.name
        ));
    }
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&temp, dest).map_err(|e| e.to_string())?;
    // The branch first, while the clone's remote is the only one that
    // could have it.
    let on = crate::workspace::branch(dest);
    if let Some(want) = b.branch.as_deref().filter(|w| on.as_deref() != Some(*w)) {
        let named =
            !want.starts_with('-') && run(dest, &["check-ref-format", "--branch", want]).is_ok();
        // A branch the remote has comes down tracking it; one only the
        // other machine has stays there.
        let tracked = format!("{}/{want}", first.name);
        if !named
            || run(
                dest,
                &["checkout", "-q", "--track", "-b", want, &tracked, "--"],
            )
            .is_err()
        {
            notes.push(format!(
                "{want} isn't on {}, so this is {}",
                first.name,
                on.as_deref().unwrap_or("the default branch")
            ));
        }
    }
    for r in &b.remotes[1..] {
        if !plain_name(&r.name) {
            notes.push(format!("left out the remote {}", r.name));
            continue;
        }
        if let Err(e) = run(dest, &["remote", "add", "--", &r.name, &r.url]) {
            notes.push(format!(
                "couldn't add the remote {}: {}",
                r.name,
                first_line(&e)
            ));
        }
    }
    if b.remotes.len() > 1 {
        let _ = quiet_git(dest, &["fetch", "-q", "--all"]);
    }
    let mut brought = 0;
    let mut unignored = Vec::new();
    for (rel, from) in walk(staging) {
        // A file the clone has is the project's own, not the one carried;
        // and nothing's written through a link out of the checkout.
        // A name with a line break in it can't be kept out of Git (an
        // exclude rule is one line): it isn't brought, rather than left
        // where it could be committed.
        if rel.to_string_lossy().contains(['\n', '\r']) {
            notes.push(format!(
                "left out {:?}: Git can't keep a name with a line break out",
                rel.to_string_lossy()
            ));
            continue;
        }
        let Some(to) = writable(dest, &rel) else {
            notes.push(format!("kept the project's own {}", rel.display()));
            continue;
        };
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
    Ok((line, notes))
}

/// The longest a clone or fetch may take.
const GIT_FOR: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// git for a clone or fetch, which talks to another host: never waiting on
/// a prompt (SSH's included), given up when the link stalls for a minute,
/// and stopped after [`GIT_FOR`] in all.
fn quiet_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=20 -o ServerAliveInterval=15 -o ServerAliveCountMax=4",
        )
        .env("GIT_HTTP_LOW_SPEED_LIMIT", "1000")
        .env("GIT_HTTP_LOW_SPEED_TIME", "60")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    // Read as it goes, so a full pipe never stalls it.
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let reading = std::thread::spawn(move || {
        use std::io::Read;
        let (mut o, mut e) = (String::new(), String::new());
        if let Some(s) = err.as_mut() {
            let _ = s.read_to_string(&mut e);
        }
        if let Some(s) = out.as_mut() {
            let _ = s.read_to_string(&mut o);
        }
        (o, e)
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > GIT_FOR => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "git took more than {} minutes",
                    GIT_FOR.as_secs() / 60
                ));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let (o, e) = reading.join().unwrap_or_default();
    if status.success() {
        Ok(o)
    } else {
        Err(e.trim().trim_start_matches("fatal: ").to_string())
    }
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
        // Git drops trailing spaces unless each is escaped.
        let kept = line.trim_end_matches(' ').len();
        let spaces = line.len() - kept;
        line.truncate(kept);
        line.push_str(&"\\ ".repeat(spaces));
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
    fn nothing_is_written_through_a_link() {
        let d = std::env::temp_dir().join(format!("dent-writable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("web")).unwrap();
        std::os::unix::fs::symlink("/tmp", d.join("out")).unwrap();
        std::os::unix::fs::symlink("/nowhere/x", d.join(".env")).unwrap();
        std::fs::write(d.join("kept"), "").unwrap();
        assert!(writable(&d, Path::new("web/.env")).is_some());
        assert!(writable(&d, Path::new("new/dir/.env")).is_some());
        assert!(writable(&d, Path::new("out/.env")).is_none());
        assert!(writable(&d, Path::new(".env")).is_none(), "a dangling link");
        assert!(writable(&d, Path::new("kept")).is_none());
        assert!(writable(&d, Path::new("kept/x")).is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn remote_names_are_names() {
        assert!(plain_name("origin") && plain_name("up-stream_2"));
        for bad in ["", "-x", "--upload-pack=x", "a/b", ".x", "a b"] {
            assert!(!plain_name(bad), "{bad}");
        }
    }

    #[test]
    fn only_paths_inside() {
        assert!(inside("a/.env").is_some());
        // The repository however it's cased, as the Mac's disk reads it.
        for git in [".GIT/hooks/pre-commit", "a/.Git/config", ".git./x"] {
            assert!(inside(git).is_none(), "{git}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../../x",
            "./a",
            ".git/config",
            "a/.git/x",
        ] {
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
