//! Packing a session to move on the machine it leaves ([decision 0064]):
//! everything it is, as files in `moves/<id>/` with a manifest of their
//! sizes and SHA-256s, written once. A retry with the same move id gets the
//! same package, so a carrier can resume from any point.
//!
//! [decision 0064]: ../../../../docs/decisions/0064-moving-sessions.md

use std::io::Write;
use std::path::{Path, PathBuf};

use dent_protocol::{Event, MoveManifest, MovePart};
use sha2::{Digest, Sha256};

use crate::agent::Driver;
use crate::store::Row;

/// Where move `id`'s package is kept on the machine it leaves.
pub fn dir(state: &Path, id: &str) -> PathBuf {
    state.join("moves").join(id)
}

/// A move's id names folders: letters, digits and dashes only.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// What's needed to pack a session.
pub struct Session<'a> {
    pub state: &'a Path,
    pub machine: &'a str,
    pub row: &'a Row,
    pub driver: &'static dyn Driver,
    pub events: &'a [Event],
    pub scratch: &'a Path,
}

/// The package for move `id`, made if it isn't yet. `have` is what the
/// other machine's copy of the repository already has.
pub fn pack(s: &Session<'_>, id: &str, have: &[String]) -> Result<MoveManifest, String> {
    if !valid_id(id) {
        return Err(format!("{id:?} isn't a move id"));
    }
    let done = dir(s.state, id);
    if let Ok(m) = read_manifest(&done) {
        return Ok(m);
    }
    let staging = s.state.join("moves").join(format!("{id}.staging"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let packed = fill(s, id, have, &staging);
    let m = match packed {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    std::fs::rename(&staging, &done).map_err(|e| e.to_string())?;
    if let Ok(f) = std::fs::File::open(done.parent().unwrap_or(&done)) {
        let _ = f.sync_all();
    }
    Ok(m)
}

fn fill(s: &Session<'_>, id: &str, have: &[String], out: &Path) -> Result<MoveManifest, String> {
    let row = s.row;
    let cwd = Path::new(&row.cwd);
    // Where it works: a repository, or dent's scratch folder.
    let repo = if cwd.starts_with(s.scratch) {
        None
    } else {
        let Some((root, subdir)) = super::git::repo_of(cwd) else {
            return Err(
                "it works in a folder that isn't a Git repository, and only those (and dent's scratch folder) can move"
                    .into(),
            );
        };
        let mut r = super::git::pack(&root, id, have, &out.join("git.bundle"))?;
        r.remotes = super::git::remotes(&root);
        r.subdir = subdir;
        Some(r)
    };
    // The agent's own conversation, which it resumes from there.
    let agent_session = row.agent_session.clone().unwrap_or_default();
    if !agent_session.is_empty() {
        let Some(files) = s.driver.conversation(&agent_session) else {
            return Err(format!(
                "{}'s conversations can't move between machines yet",
                s.driver.name()
            ));
        };
        for (from, name) in files {
            copy_in(&from, &out.join(&name))?;
        }
    }
    // dent's own record of it, with when each thing happened.
    let mut lines = Vec::new();
    let mut title = String::new();
    let mut effort = None;
    for e in s.events {
        match &e.body {
            dent_protocol::Body::Created { title: t, .. } if title.is_empty() => title = t.clone(),
            dent_protocol::Body::Settings { title: Some(t), .. } => title = t.clone(),
            _ => {}
        }
        if let dent_protocol::Body::Settings {
            effort: Some(x), ..
        } = &e.body
        {
            effort = Some(x.clone());
        }
        // The move's own steps aren't history to carry.
        if matches!(e.body, dent_protocol::Body::Move { .. }) {
            continue;
        }
        let line = serde_json::json!({ "at": e.at, "body": e.body });
        lines.push(line.to_string());
    }
    write_part(
        &out.join("events.jsonl"),
        (lines.join("\n") + "\n").as_bytes(),
    )?;
    // What dent saved for it: pictures pasted, files carried in.
    let saved = s.state.join("attachments").join(&row.id);
    for f in files_under(&saved) {
        let rel = f.strip_prefix(&saved).map_err(|e| e.to_string())?;
        copy_in(&f, &out.join("files").join(rel))?;
    }
    let mut parts = Vec::new();
    for f in files_under(out) {
        let name = f
            .strip_prefix(out)
            .map_err(|e| e.to_string())?
            .display()
            .to_string();
        parts.push(MovePart {
            name,
            bytes: std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0),
            sha256: sha256_file(&f).map_err(|e| e.to_string())?,
        });
    }
    parts.sort_by(|a, b| a.name.cmp(&b.name));
    let m = MoveManifest {
        id: id.to_string(),
        from_machine: s.machine.to_string(),
        from_session: row.id.clone(),
        agent: row.agent.clone(),
        agent_session,
        cwd: row.cwd.clone(),
        title,
        model: row.model.clone(),
        permission: row.permission,
        effort,
        repo,
        files_from: saved.display().to_string(),
        parts,
    };
    let json = serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?;
    write_part(&out.join("manifest.json"), &json)?;
    Ok(m)
}

pub fn read_manifest(dir: &Path) -> std::io::Result<MoveManifest> {
    let bytes = std::fs::read(dir.join("manifest.json"))?;
    serde_json::from_slice(&bytes).map_err(std::io::Error::other)
}

/// A file copied in whole and flushed.
fn copy_in(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(p) = to.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::copy(from, to).map_err(|e| format!("copy {}: {e}", from.display()))?;
    std::fs::File::open(to)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

fn write_part(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}

pub fn files_under(dir: &Path) -> Vec<PathBuf> {
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

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex::encode(h.finalize()))
}
