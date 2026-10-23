//! A move arriving, on the machine it goes to ([decision 0064]): its parts
//! land in `moves-in/<id>/`, each written at the offset both sides agree
//! on, so a carrier stopped anywhere picks up where it left off. Once all
//! are there and match the manifest, the session is made here. Until then
//! nothing outside the move's folder is touched, so "not accepted" is true
//! whenever it's said.
//!
//! [decision 0064]: ../../../../docs/decisions/0064-moving-sessions.md

use std::io::{Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use dent_protocol::MoveManifest;

/// Where move `id` arrives.
pub fn inbox(state: &Path, id: &str) -> PathBuf {
    state.join("moves-in").join(id)
}

/// Accepted already: the session it made here.
pub fn accepted(state: &Path, id: &str) -> Option<String> {
    std::fs::read_to_string(inbox(state, id).join("accepted"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// How much of each part is here, by name.
pub fn have(state: &Path, m: &MoveManifest) -> Vec<(String, u64)> {
    let dir = inbox(state, &m.id).join("parts");
    m.parts
        .iter()
        .map(|p| {
            let n = std::fs::metadata(dir.join(&p.name))
                .map(|f| f.len())
                .unwrap_or(0);
            (p.name.clone(), n.min(p.bytes))
        })
        .collect()
}

/// A part's name stays inside the move: plain names, no `..`, no root.
fn safe(name: &str) -> bool {
    !name.is_empty()
        && Path::new(name)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// Start receiving move `m` (again): its manifest kept, and how much of
/// each part is here. The same id with another manifest is refused.
pub fn begin(state: &Path, m: &MoveManifest) -> Result<Vec<(String, u64)>, String> {
    if !super::pack::valid_id(&m.id) {
        return Err(format!("{:?} isn't a move id", m.id));
    }
    if let Some(p) = m.parts.iter().find(|p| !safe(&p.name)) {
        return Err(format!("{:?} isn't a part's name", p.name));
    }
    let dir = inbox(state, &m.id);
    std::fs::create_dir_all(dir.join("parts")).map_err(|e| e.to_string())?;
    let file = dir.join("manifest.json");
    match std::fs::read(&file) {
        Ok(had) => {
            let had: MoveManifest = serde_json::from_slice(&had).map_err(|e| e.to_string())?;
            if had != *m {
                return Err("another move has this id here".into());
            }
        }
        Err(_) => {
            let json = serde_json::to_vec_pretty(m).map_err(|e| e.to_string())?;
            let tmp = dir.join("manifest.json.part");
            std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
        }
    }
    Ok(have(state, m))
}

pub fn manifest(state: &Path, id: &str) -> Result<MoveManifest, String> {
    let bytes = std::fs::read(inbox(state, id).join("manifest.json"))
        .map_err(|_| "this move hasn't started here".to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// Write `data` into part `name` at `offset`, which must be where it
/// stands (a chunk sent twice lands once). Returns how much is there now.
pub fn write(state: &Path, id: &str, name: &str, offset: u64, data: &[u8]) -> Result<u64, String> {
    let m = manifest(state, id)?;
    let Some(part) = m.parts.iter().find(|p| p.name == name) else {
        return Err(format!("{name:?} isn't part of this move"));
    };
    let path = inbox(state, id).join("parts").join(name);
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(false)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let size = f.metadata().map_err(|e| e.to_string())?.len();
    // Already have it (a resend): nothing to do.
    if offset + data.len() as u64 <= size {
        return Ok(size);
    }
    if offset > size {
        return Err(format!("{name}: {size} bytes here, not {offset}"));
    }
    if offset + data.len() as u64 > part.bytes {
        return Err(format!("{name}: more than its {} bytes", part.bytes));
    }
    let skip = (size - offset) as usize;
    f.seek(SeekFrom::Start(size)).map_err(|e| e.to_string())?;
    f.write_all(&data[skip..]).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    Ok(size + (data.len() - skip) as u64)
}

/// Every part here, whole and as the manifest says: why not, if not.
pub fn verify(state: &Path, m: &MoveManifest) -> Result<(), String> {
    let dir = inbox(state, &m.id).join("parts");
    for p in &m.parts {
        let path = dir.join(&p.name);
        let size = std::fs::metadata(&path).map(|f| f.len()).unwrap_or(0);
        if size != p.bytes {
            return Err(format!(
                "{} isn't all here yet ({size} of {} bytes)",
                p.name, p.bytes
            ));
        }
        let sum = super::pack::sha256_file(&path).map_err(|e| e.to_string())?;
        if sum != p.sha256 {
            // Damaged on the way: it's sent again.
            let _ = std::fs::remove_file(&path);
            return Err(format!(
                "{} didn't arrive as it left; it's sent again",
                p.name
            ));
        }
    }
    Ok(())
}

/// The same repository, however its address is written:
/// `git@host:org/repo.git`, `ssh://git@host/org/repo` and
/// `https://host/org/repo` are one.
pub fn same_remote(a: &str, b: &str) -> bool {
    fn norm(u: &str) -> String {
        let u = u.trim().trim_end_matches('/');
        let u = u.strip_suffix(".git").unwrap_or(u);
        let rest = match u.split_once("://") {
            Some((_, r)) => r.to_string(),
            // scp-like `user@host:path`.
            None => match u.split_once(':') {
                Some((host, path)) if !host.contains('/') => format!("{host}/{path}"),
                _ => u.to_string(),
            },
        };
        let rest = rest.rsplit_once('@').map_or(rest.as_str(), |(_, r)| r);
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        format!("{}/{}", host.to_lowercase(), path)
    }
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_repository_however_its_address_is_written() {
        let all = [
            "git@github.com:SarimDEV/dent.git",
            "ssh://git@github.com/SarimDEV/dent",
            "https://github.com/SarimDEV/dent.git",
            "https://GitHub.com/SarimDEV/dent/",
        ];
        for a in all {
            for b in all {
                assert!(same_remote(a, b), "{a} {b}");
            }
        }
        assert!(!same_remote(
            "git@github.com:SarimDEV/dent.git",
            "git@github.com:SarimDEV/other.git"
        ));
        assert!(!same_remote("/srv/a.git", "/srv/b.git"));
        assert!(same_remote("/srv/a.git", "/srv/a"));
    }

    #[test]
    fn parts_arrive_once_at_the_offset_agreed_and_are_checked() {
        let state = std::env::temp_dir().join(format!("dent-accept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        let data = b"hello, moved world".to_vec();
        let m = MoveManifest {
            id: "m1".into(),
            from_machine: "a".into(),
            from_session: "s1".into(),
            agent: "claude".into(),
            agent_session: "x".into(),
            cwd: "/w".into(),
            title: "t".into(),
            model: None,
            permission: Default::default(),
            effort: None,
            repo: None,
            files_from: String::new(),
            parts: vec![dent_protocol::MovePart {
                name: "events.jsonl".into(),
                bytes: data.len() as u64,
                sha256: {
                    use sha2::Digest;
                    hex::encode(sha2::Sha256::digest(&data))
                },
            }],
        };
        assert_eq!(
            begin(&state, &m).unwrap(),
            [("events.jsonl".to_string(), 0)]
        );
        assert_eq!(
            write(&state, "m1", "events.jsonl", 0, &data[..5]).unwrap(),
            5
        );
        // The same chunk again, or one overlapping: lands once.
        assert_eq!(
            write(&state, "m1", "events.jsonl", 0, &data[..5]).unwrap(),
            5
        );
        assert_eq!(
            write(&state, "m1", "events.jsonl", 3, &data[3..9]).unwrap(),
            9
        );
        // A gap is refused; so is a part that isn't in the move.
        assert!(write(&state, "m1", "events.jsonl", 12, &data[12..]).is_err());
        assert!(write(&state, "m1", "nope", 0, b"x").is_err());
        assert!(verify(&state, &m).unwrap_err().contains("isn't all here"));
        // Begun again (a carrier starting over): it says what it has.
        assert_eq!(
            begin(&state, &m).unwrap(),
            [("events.jsonl".to_string(), 9)]
        );
        write(&state, "m1", "events.jsonl", 9, &data[9..]).unwrap();
        verify(&state, &m).unwrap();
        // Another move under the same id is refused.
        let other = MoveManifest {
            title: "other".into(),
            ..m.clone()
        };
        assert!(begin(&state, &other).is_err());
        // A part named to leave the folder is refused up front.
        let bad = MoveManifest {
            id: "m2".into(),
            parts: vec![dent_protocol::MovePart {
                name: "../x".into(),
                bytes: 1,
                sha256: String::new(),
            }],
            ..m.clone()
        };
        assert!(begin(&state, &bad).is_err());
        let _ = std::fs::remove_dir_all(&state);
    }
}
