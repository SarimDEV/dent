//! Directories to offer for a new session: where sessions ran before, and
//! the projects Claude Code already knows from the terminal.

use std::collections::HashMap;
use std::path::Path;

use dent_protocol::Project;

/// `recent` is each session directory with when it was last used.
pub fn list(recent: &[(String, u64)]) -> Vec<Project> {
    let mut by_path: HashMap<String, u64> = HashMap::new();
    for (p, t) in recent {
        let e = by_path.entry(p.clone()).or_default();
        *e = (*e).max(*t);
    }
    for p in claude_projects() {
        by_path.entry(p).or_default();
    }
    let mut out: Vec<Project> = by_path
        .into_iter()
        .filter(|(p, _)| Path::new(p).is_dir())
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

/// The project paths in `~/.claude.json`. Only the keys are read.
fn claude_projects() -> Vec<String> {
    let path = crate::paths::home().join(".claude.json");
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    v["projects"]
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}
