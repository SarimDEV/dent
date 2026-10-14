//! Choices the app remembers between launches: the defaults for a new
//! session and how the window was laid out. Kept beside the daemon's state.

use dent_protocol::Permission;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
#[serde(default)]
pub struct Prefs {
    pub project: Option<String>,
    pub model: Option<String>,
    pub permission: Permission,
    pub effort: Option<String>,
    pub panel_width: Option<f32>,
    pub dark: Option<bool>,
    /// Viewer tabs you closed, per session: file and its versions then.
    pub closed_tabs: std::collections::BTreeMap<String, std::collections::BTreeMap<String, usize>>,
}

fn path() -> std::path::PathBuf {
    crate::live::default_state().join("desktop.json")
}

pub fn load() -> Prefs {
    std::fs::read(path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(p: &Prefs) {
    if let Ok(json) = serde_json::to_vec_pretty(p) {
        let path = path();
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
        // Write then rename, so a crash never leaves half a file.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}
