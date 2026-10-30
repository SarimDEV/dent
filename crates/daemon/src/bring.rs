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

/// More ignored files than this aren't listed (they're counted): a
/// checkout full of build output is offered its environment files and the
/// first of the rest.
const MAX_IGNORED: usize = 500;

/// A folder of only ignored files with more than this many in it is
/// output or data: only its environment files are offered.
const FOLDER_MAX: usize = 100;

/// An environment file larger than this is listed but not ticked: it's
/// data, not settings.
const ENV_TICKED_MAX: u64 = 1 << 20;

/// Folders of what's installed or built, never looked in, wherever they
/// sit: a package's own `.env` inside them isn't the project's.
const NOT_IN: &[&str] = &[
    "node_modules",
    "bower_components",
    "jspm_packages",
    ".pnpm-store",
    ".yarn",
    ".venv",
    "venv",
    ".tox",
    "site-packages",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    "vendor",
    "Pods",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".cache",
    "coverage",
    ".gradle",
    ".terraform",
    ".serverless",
    ".git",
];

/// Files no one wants on another machine: the system's and editors' own.
fn junk(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(name, ".DS_Store" | "Thumbs.db" | "desktop.ini")
        || [".swp", ".swo", ".pyc", ".log", "~"]
            .iter()
            .any(|e| name.ends_with(e))
}

/// Inside a folder of what's installed or built: one named so that Git
/// ignores (inside one of the `ignored` folders git reports whole). A
/// tracked `build/` of Dockerfiles is the project's own.
fn in_deps(path: &str, ignored: &[&str]) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let mut at = String::new();
    for p in &parts[..parts.len().saturating_sub(1)] {
        at.push_str(p);
        at.push('/');
        if NOT_IN.contains(p) && ignored.iter().any(|f| at.starts_with(f)) {
            return true;
        }
    }
    false
}

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
    // A folder of ignored files (`node_modules/`, `certs/`) comes as one
    // entry ending in a slash.
    let top = run(
        &root,
        &[
            &IGNORED[..],
            &["--exclude-standard", "--directory", "--no-empty-directory"],
        ]
        .concat(),
    )
    .unwrap_or_default();
    let folders: Vec<&str> = top.split('\0').filter(|p| p.ends_with('/')).collect();
    // Every ignored file, what's surely installed left out as git walks
    // (it still looks, but says less).
    let mut excludes: Vec<String> = ["node_modules", ".venv", "site-packages", ".pnpm-store"]
        .iter()
        .map(|d| format!(":(exclude,glob)**/{d}/**"))
        .collect();
    // An ignored folder of what's built or installed (`target/`, `dist/`)
    // isn't listed at all: nothing in it is offered (`in_deps`), and a
    // build folder can hold hundreds of thousands of files, which took
    // seconds to list and sift.
    excludes.extend(
        folders
            .iter()
            .map(|f| f.trim_end_matches('/'))
            .filter(|f| NOT_IN.contains(&f.rsplit('/').next().unwrap_or(f)))
            .map(|f| format!(":(exclude,literal){f}")),
    );
    let mut args: Vec<&str> = IGNORED.to_vec();
    args.extend(["--exclude-standard", "--", "."]);
    args.extend(excludes.iter().map(String::as_str));
    let every = run(&root, &args).unwrap_or_default();
    let all: Vec<&str> = every
        .split('\0')
        .filter(|p| !p.is_empty() && !in_deps(p, &folders) && !junk(p))
        .collect();
    // A folder of only ignored files that's small is the project's own
    // (`certs/`, `secrets/`): its files are offered. A big one is output
    // or data, offered only its environment files.
    let under = |p: &str| folders.iter().find(|f| p.starts_with(**f)).copied();
    let mut in_folder: std::collections::HashMap<&str, usize> = Default::default();
    for p in &all {
        if let Some(f) = under(p) {
            *in_folder.entry(f).or_default() += 1;
        }
    }
    let mut paths: Vec<&str> = all
        .into_iter()
        .filter(|p| {
            under(p).is_none_or(|f| is_env(p) || in_folder.get(f).is_some_and(|n| *n <= FOLDER_MAX))
        })
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
                carry: is_env(p) && meta.len() <= ENV_TICKED_MAX,
                env: is_env(p),
            })
        })
        .collect();
    // What goes first (environment files, nearest the top first), then
    // environment files too big to tick, then the rest.
    let rank = |i: &Ignored| (!i.carry, !is_env(&i.path), i.path.matches('/').count());
    ignored.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.path.cmp(&b.path)));
    let more = ignored.len().saturating_sub(MAX_IGNORED) as u32;
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
        more,
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

/// An environment file: `.env`, `.env.local`, `apps/web/.env.production`,
/// direnv's `.envrc`, Wrangler's `.dev.vars`; not a template of one
/// (`.env.example`, `.env.sample`, `.env.template`, `.env.dist`).
fn is_env(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let template = [".example", ".sample", ".template", ".dist"]
        .iter()
        .any(|t| name.ends_with(t));
    name == ".env"
        || name == ".envrc"
        || name == ".dev.vars"
        || (name.starts_with(".env.") && !template)
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
pub(crate) fn writable(dest: &Path, rel: &Path) -> Option<PathBuf> {
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
    let ignored = ignored_of(&root, paths)?;
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
        if !ignored.contains(rel.as_str()) {
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

/// Which of `paths` Git ignores at `root`, asked once for all of them (a
/// monorepo's hundreds of `.env` files were a process each).
fn ignored_of(root: &Path, paths: &[String]) -> Result<std::collections::HashSet<String>, String> {
    use std::io::Write;
    let mut child = std::process::Command::new("git")
        .args(["check-ignore", "-z", "--stdin"])
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("git: {e}"))?;
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(p.as_bytes());
        input.push(0);
    }
    let mut stdin = child.stdin.take().ok_or("git: no input")?;
    // Written from its own thread, so a long answer can't stall the
    // question.
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let out = child.wait_with_output().map_err(|e| format!("git: {e}"))?;
    let _ = writer.join();
    // 0: some are ignored; 1: none are. Anything else went wrong, and
    // says so, rather than every file reading as not ignored.
    if !matches!(out.status.code(), Some(0 | 1)) {
        return Err(format!(
            "git couldn't say which files it ignores: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect())
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

    /// A monorepo as people have them: what's listed, what's ticked, in
    /// what order, and what never is.
    #[test]
    fn a_monorepo_lists_its_own_env_files_first_and_nothing_installed() {
        let root = std::env::temp_dir().join(format!("dent-pack-mono-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let w = |rel: &str, body: &[u8]| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        w(
            ".gitignore",
            b"node_modules/\n.venv/\ndist/\n.env\n.env.*\n*.log\n.DS_Store\nlocal.db\ncerts/\n",
        );
        w("README.md", b"x");
        w(".env", b"A=1");
        w("apps/web/.env.local", b"B=1");
        w("apps/my app/conf/.env.production.local", b"C=1");
        w("packages/a/node_modules/dep/.env", b"no");
        w("node_modules/x/.env", b"no");
        w(".venv/lib/site-packages/m/.env.local", b"no");
        w("packages/a/dist/.env", b"no");
        w(".env.sample", b"S=1");
        w(".env.big", &vec![b'x'; (ENV_TICKED_MAX + 1) as usize]);
        w("debug.log", b"l");
        w("apps/web/.DS_Store", b"j");
        w("local.db", b"d");
        // A tracked build/ is the project's own, its ignored .env too.
        w("build/Dockerfile", b"FROM x");
        w("build/docker/.env", b"D=1");
        // A small folder of only ignored files: offered, file by file.
        w("certs/dev.pem", b"k");
        let git = |args: &[&str]| run(&root, args).unwrap();
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "i",
        ]);
        let p = pack(&root).unwrap();
        let paths: Vec<&str> = p.ignored.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                ".env",
                "apps/web/.env.local",
                "build/docker/.env",
                "apps/my app/conf/.env.production.local",
                ".env.big",
                ".env.sample",
                "local.db",
                "certs/dev.pem",
            ]
        );
        let ticked: Vec<&str> = p
            .ignored
            .iter()
            .filter(|i| i.carry)
            .map(|i| i.path.as_str())
            .collect();
        assert_eq!(ticked, &paths[..4]);
        assert!(
            p.ignored
                .iter()
                .find(|i| i.path == ".env.big")
                .is_some_and(|i| i.env && !i.carry)
        );
        assert!(
            p.ignored
                .iter()
                .find(|i| i.path == ".env.sample")
                .is_some_and(|i| !i.env)
        );
        assert_eq!(p.more, 0);
        // Read back in one go, a path with a space among them.
        let got = read_files(
            &root,
            &ticked.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(got.len(), 4);
        // A tracked file isn't one to carry.
        assert!(read_files(&root, &["README.md".into()]).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn env_files_are_ticked() {
        assert!(is_env(".env"));
        assert!(is_env("apps/web/.env.local"));
        assert!(!is_env(".env.example"));
        assert!(!is_env("notes.env"));
        assert!(is_env(".envrc"));
        assert!(is_env(".dev.vars"));
        assert!(!is_env(".env.sample"));
        assert!(!is_env("config/.env.template"));
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
