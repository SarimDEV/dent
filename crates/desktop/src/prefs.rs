//! Choices the app remembers between launches: the defaults for a new
//! session and how the window was laid out. Kept beside the daemon's state.

use dent_protocol::Permission;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
#[serde(default)]
pub struct Prefs {
    pub project: Option<String>,
    pub model: Option<String>,
    /// The permission a new session starts with: Full access until you pick
    /// another, which then sticks (decision 0048).
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
    /// What you'd typed in each session and not sent, so quitting loses
    /// nothing; under "" what was typed for a new session.
    pub drafts: std::collections::BTreeMap<String, String>,
    /// Files attached to each draft and not yet sent, by the same keys.
    pub attached: std::collections::BTreeMap<String, Vec<String>>,
    /// Long pastes folded into the drafts, by number.
    pub pastes: std::collections::BTreeMap<u32, String>,
    /// Sessions kept at the top of the sidebar, in the order pinned.
    pub pinned: Vec<String>,
    /// The ambient background on the new-session page, by id; none when unset
    /// or no longer known.
    pub ambient: Option<String>,
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
    let mut p: Prefs = std::fs::read(path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    // A marker beside the prefs, not a field in them: an older build
    // rewriting the prefs can't drop it and so undo a choice made since.
    let marker = path().with_file_name(FULL_ACCESS_MARKER);
    if upgrade(&mut p, marker.exists()) {
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&marker, "");
    }
    p
}

/// Present once new sessions have been moved to Full access.
const FULL_ACCESS_MARKER: &str = "desktop-full-access-default";

/// Moves older (or no) prefs to Full access, once: `done` says it
/// happened before. True when it moved them now; a choice made after that
/// is yours and stays.
fn upgrade(p: &mut Prefs, done: bool) -> bool {
    if done {
        return false;
    }
    p.permission = Permission::Full;
    true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sessions_start_in_full_access_until_you_choose() {
        // No prefs yet, or prefs from before: Full access.
        let mut fresh = Prefs::default();
        assert!(upgrade(&mut fresh, false));
        assert_eq!(fresh.permission, Permission::Full);
        let mut old: Prefs = serde_json::from_str(r#"{"permission":"ask"}"#).unwrap();
        assert!(upgrade(&mut old, false));
        assert_eq!(old.permission, Permission::Full);
        // A choice made since sticks, whatever rewrote the prefs meanwhile.
        let mut chosen: Prefs = serde_json::from_str(r#"{"permission":"ask"}"#).unwrap();
        assert!(!upgrade(&mut chosen, true));
        assert_eq!(chosen.permission, Permission::Ask);
    }
}
