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
    pub sidebar_width: Option<f32>,
    /// You hid the sidebar (View › Hide Sidebar, ⌃⌘S); it stays hidden.
    pub sidebar_off: bool,
    /// Notifications come without their sound.
    pub quiet: bool,
    /// Light, dark, or as the Mac is.
    pub appearance: Appearance,
    /// The text size you chose with ⌘+ and ⌘−, as a factor.
    pub zoom: Option<f32>,
    /// Viewer tabs you closed, per session: file and its versions then.
    /// Files opened from links in each session's viewer, by path.
    pub opened: std::collections::BTreeMap<String, Vec<String>>,
    pub closed_tabs: std::collections::BTreeMap<String, std::collections::BTreeMap<String, usize>>,
    /// What you'd typed in each session and not sent, so quitting loses nothing.
    pub drafts: std::collections::BTreeMap<String, String>,
    /// Sessions kept at the top of the sidebar, in the order pinned.
    pub pinned: Vec<String>,
    /// The app you picked to open each kind of file (by extension).
    pub open_with: std::collections::BTreeMap<String, String>,
}

/// Which theme dent wears.
#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// Paper by day, Carbon by night, as the Mac is set.
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    /// Dark or not when chosen outright; `None` follows the Mac.
    pub fn dark(self) -> Option<bool> {
        match self {
            Appearance::System => None,
            Appearance::Light => Some(false),
            Appearance::Dark => Some(true),
        }
    }
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
