//! Directories to offer for a new session: where sessions ran before, the
//! projects Claude Code already knows from the terminal, and the ones added
//! here; less any taken off the list. Also making them: a new project from
//! a name, a clone, and folder completion for a typed path.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use dent_protocol::Project;

/// `recent` is each session directory with when it was last used; `added`
/// the folders added here, with when; `hidden` the ones taken off.
pub fn list(
    recent: &[(String, u64)],
    added: &HashMap<String, u64>,
    hidden: &HashSet<String>,
) -> Vec<Project> {
    let mut by_path: HashMap<String, u64> = HashMap::new();
    // A project is a repository: a worktree counts as its main checkout
    // (decision 0053).
    let mut add = |p: String, t: u64| {
        let p = crate::workspace::main_checkout(Path::new(&p))
            .map(|m| m.display().to_string())
            .unwrap_or(p);
        let e = by_path.entry(p).or_default();
        *e = (*e).max(t);
    };
    for (p, t) in recent.iter().cloned() {
        add(p, t);
    }
    for (p, t) in added {
        add(p.clone(), *t);
    }
    // From each agent's terminal history, a folder in temporary space was
    // a one-off; one used or added here is kept wherever it is. Ordered by
    // when the terminal last used it, so ones never used here still sort
    // by recency, not by name.
    for d in crate::agent::DRIVERS {
        for (p, used) in d.projects() {
            if !temporary(&p) {
                add(p, used);
            }
        }
    }
    let mut out: Vec<Project> = by_path
        .into_iter()
        .filter(|(p, _)| !hidden.contains(p) && Path::new(p).is_dir())
        .map(|(path, last_used)| Project {
            name: Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone()),
            path,
            last_used,
        })
        .collect();
    out.sort_by(|a, b| b.last_used.cmp(&a.last_used).then(a.name.cmp(&b.name)));
    out
}

/// A folder in the system's temporary space.
fn temporary(path: &str) -> bool {
    let tmp = std::env::temp_dir();
    let tmp = tmp.to_string_lossy();
    let tmp = tmp.trim_end_matches('/');
    [
        tmp,
        "/tmp",
        "/private/tmp",
        "/private/var/folders",
        "/var/folders",
    ]
    .iter()
    .any(|t| path == *t || path.starts_with(&format!("{t}/")))
}

/// Where projects made here go: `~/dent/projects`, beside the scratch folder.
pub fn root() -> PathBuf {
    crate::paths::home().join("dent/projects")
}

/// A folder name from what was typed: lowercase letters and digits, words
/// joined by `-`, at most five words and 48 characters, so it stays one
/// path segment whatever was pasted.
pub fn slug(text: &str) -> String {
    let words: Vec<String> = text
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(5)
        .map(str::to_owned)
        .collect();
    let mut s = words.join("-");
    s.truncate(48);
    s.trim_end_matches('-').to_string()
}

/// `root/name`, or `name-2`, `name-3`… when taken.
fn free(root: &Path, name: &str) -> Result<PathBuf> {
    for n in 1..100 {
        let p = if n == 1 {
            root.join(name)
        } else {
            root.join(format!("{name}-{n}"))
        };
        if !p.exists() {
            return Ok(p);
        }
    }
    bail!("no free folder for {name} in {}", root.display())
}

fn run(dir: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        // Never wait on a password prompt nobody can see.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .unwrap_or("git failed")
            .trim_start_matches("fatal: ")
            .to_string())
    }
}

/// A new project from a name: its folder, a Git repository with a README,
/// and a first commit. A commit that fails (no Git identity) keeps the
/// project and says why.
pub fn create(name: &str) -> Result<(PathBuf, Option<String>)> {
    let slug = slug(name);
    if slug.is_empty() {
        bail!("Give the project a name with a letter or digit in it");
    }
    let root = root();
    std::fs::create_dir_all(&root)?;
    let dir = free(&root, &slug)?;
    std::fs::create_dir(&dir)?;
    std::fs::write(dir.join("README.md"), format!("# {}\n", name.trim()))?;
    if let Err(e) = run(&dir, &["init", "-q"]) {
        return Ok((dir, Some(format!("Not a Git repository: {e}"))));
    }
    let warning = run(&dir, &["add", "README.md"])
        .and_then(|()| run(&dir, &["commit", "-q", "-m", "First commit"]))
        .err()
        .map(|e| {
            if e.contains("email") || e.contains("identity") {
                "Made it, without a first commit: Git doesn't know your name and email yet \
                 (git config --global user.email)"
                    .to_string()
            } else {
                format!("Made it, without a first commit: {e}")
            }
        });
    Ok((dir, warning))
}

/// The folder a clone of `url` goes in: the repository's name.
fn repo_name(url: &str) -> String {
    let last = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or_default();
    slug(last.strip_suffix(".git").unwrap_or(last))
}

/// A folder no one has yet in the projects folder, named `name` (`-2`,
/// `-3` when taken).
pub fn free_in_root(name: &str) -> Result<PathBuf> {
    let name = if name.is_empty() { "project" } else { name };
    let root = root();
    std::fs::create_dir_all(&root)?;
    free(&root, name)
}

/// Clone a Git URL into the projects folder. Only remote URLs, and never
/// one that git would read as an option.
pub fn clone(url: &str) -> Result<PathBuf> {
    let url = url.trim();
    let remote = ["https://", "http://", "ssh://", "git://", "git@"]
        .iter()
        .any(|p| url.starts_with(p));
    if !remote || url.contains(char::is_whitespace) {
        bail!("That isn't a Git URL: it should start with https:// or git@");
    }
    let name = repo_name(url);
    if name.is_empty() {
        bail!("No repository name in {url}");
    }
    let root = root();
    std::fs::create_dir_all(&root)?;
    let dir = free(&root, &name)?;
    let dest = dir.display().to_string();
    if let Err(e) = run(&root, &["clone", "-q", "--", url, &dest]) {
        let _ = std::fs::remove_dir_all(&dir);
        bail!("Couldn't clone: {e}");
    }
    Ok(dir)
}

/// Folders matching a typed path: the folder it names (or the one its last
/// part is in) and the subfolders starting with that part, sorted, at most
/// 200. Paths come back as typed, `~` kept.
pub fn folders(partial: &str) -> Result<(String, Vec<String>)> {
    let partial = if partial.is_empty() { "~/" } else { partial };
    let home = crate::paths::home();
    let expand = |p: &str| -> PathBuf {
        match p.strip_prefix('~') {
            Some(rest) => home.join(rest.trim_start_matches('/')),
            None => PathBuf::from(p),
        }
    };
    let ends = partial.ends_with('/') || partial == "~";
    let (parent_typed, prefix) = if ends {
        (partial.trim_end_matches('/').to_string(), String::new())
    } else {
        match partial.rsplit_once('/') {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => bail!("Type a path starting with / or ~"),
        }
    };
    let parent_typed = if parent_typed.is_empty() {
        "/".to_string()
    } else {
        parent_typed
    };
    let parent = expand(&parent_typed);
    if !parent.is_absolute() {
        bail!("Type a path starting with / or ~");
    }
    let shown = |name: &str| {
        if parent_typed == "/" {
            format!("/{name}")
        } else {
            format!("{parent_typed}/{name}")
        }
    };
    let lower = prefix.to_lowercase();
    let mut out: Vec<String> = std::fs::read_dir(&parent)
        .with_context(|| format!("{} can't be read", parent.display()))?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) || e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.to_lowercase().starts_with(&lower))
        .filter(|n| !n.starts_with('.') || prefix.starts_with('.'))
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    out.truncate(200);
    let folders = out.iter().map(|n| shown(n)).collect();
    Ok((parent_typed, folders))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_folders_are_not_projects() {
        assert!(temporary("/tmp/x") && temporary("/private/tmp/a/b"));
        assert!(temporary("/private/var/folders/7h/T/dent-1"));
        assert!(!temporary("/Users/me/code") && !temporary("/tmpfoo"));
    }

    #[test]
    fn slugs_and_repo_names() {
        assert_eq!(slug("My Cool  App!"), "my-cool-app");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("???"), "");
        assert_eq!(repo_name("https://github.com/acme/Widget.git"), "widget");
        assert_eq!(repo_name("git@github.com:acme/tool"), "tool");
    }

    #[test]
    fn clones_only_remote_urls() {
        assert!(clone("--upload-pack=evil").is_err());
        assert!(clone("/tmp/repo").is_err());
        assert!(clone("https://x y").is_err());
    }

    #[test]
    fn lists_matching_folders() {
        let dir = std::env::temp_dir().join(format!("dent-folders-{}", std::process::id()));
        for d in ["alpha", "Apple", "beta", ".hidden"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("afile"), "").unwrap();
        let base = dir.display().to_string();
        let (parent, got) = folders(&format!("{base}/a")).unwrap();
        assert_eq!(parent, base);
        assert_eq!(got, [format!("{base}/alpha"), format!("{base}/Apple")]);
        let (_, all) = folders(&format!("{base}/")).unwrap();
        assert_eq!(all.len(), 3, "{all:?}");
        let (_, hidden) = folders(&format!("{base}/.h")).unwrap();
        assert_eq!(hidden, [format!("{base}/.hidden")]);
        assert!(folders("relative/path").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
