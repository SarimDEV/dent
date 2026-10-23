//! Claude Code's side of moving a session ([decision 0064], [spike 09]):
//! its transcript (`~/.claude/projects/<folder>/<id>.jsonl`) and the side
//! files beside it (`<id>/`: subagents, saved tool results), carried and
//! put in place for the new folder under a new id, which resumes with full
//! context.
//!
//! [decision 0064]: ../../../../docs/decisions/0064-moving-sessions.md
//! [spike 09]: ../../../../docs/spikes/09-moving-sessions.md

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Where Claude Code keeps conversations for work in `cwd`: its real path
/// (Claude Code resolves links: `/tmp` is `/private/tmp` on a Mac) with
/// every character but a letter or digit made a dash.
pub fn folder_for(cwd: &str) -> PathBuf {
    let real = std::fs::canonicalize(cwd)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| cwd.to_string());
    let name: String = real
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    super::terminal::store().join(name)
}

/// The conversation's files and their names in the move.
pub fn conversation(agent_session: &str) -> Option<Vec<(PathBuf, String)>> {
    let transcript = super::terminal::find(agent_session)?;
    let mut out = vec![(transcript.clone(), "transcript.jsonl".to_string())];
    let side = transcript.with_extension("");
    for f in files_under(&side) {
        let rel = f.strip_prefix(&side).ok()?.display().to_string();
        out.push((f, format!("agent/{rel}")));
    }
    Some(out)
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => out.extend(files_under(&p)),
            Ok(t) if t.is_file() => out.push(p),
            _ => {}
        }
    }
    out
}

/// Put a moved conversation in place for `cwd` as `to`: the transcript and
/// its side files, with the old id made the new one throughout (the CLI
/// resumes a re-keyed transcript with its whole context).
pub fn place(dir: &Path, from: &str, to: &str, cwd: &str) -> Result<()> {
    let dest = folder_for(cwd);
    std::fs::create_dir_all(&dest).context("make Claude Code's folder for it")?;
    let rekey = |bytes: Vec<u8>| match String::from_utf8(bytes) {
        Ok(text) if !from.is_empty() => text.replace(from, to).into_bytes(),
        Ok(text) => text.into_bytes(),
        Err(e) => e.into_bytes(),
    };
    let transcript = std::fs::read(dir.join("transcript.jsonl")).context("the transcript")?;
    write_whole(&dest.join(format!("{to}.jsonl")), &rekey(transcript))?;
    let side = dir.join("agent");
    for f in files_under(&side) {
        let rel = f.strip_prefix(&side)?;
        // A side file's path can't leave the conversation's own folder.
        if rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            continue;
        }
        let at = dest.join(to).join(rel);
        if let Some(p) = at.parent() {
            std::fs::create_dir_all(p)?;
        }
        write_whole(&at, &rekey(std::fs::read(&f)?))?;
    }
    Ok(())
}

/// Written whole, and readable by you alone, as Claude Code keeps its own:
/// a conversation can hold anything you pasted.
fn write_whole(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("dent-part");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn claude_codes_folder_is_the_real_path_with_dashes() {
        let f = super::folder_for("/home/a.b/dent/worktrees/x_y");
        assert!(
            f.ends_with("-home-a-b-dent-worktrees-x-y"),
            "{}",
            f.display()
        );
        // Through a link, as Claude Code sees it: where it really is.
        let real = std::env::temp_dir().join(format!("dent-real-{}", std::process::id()));
        let link = std::env::temp_dir().join(format!("dent-link-{}", std::process::id()));
        std::fs::create_dir_all(&real).unwrap();
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            super::folder_for(&link.display().to_string()),
            super::folder_for(&std::fs::canonicalize(&real).unwrap().display().to_string())
        );
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&real);
    }
}
