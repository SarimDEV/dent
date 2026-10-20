//! Choices the app remembers between launches: the defaults for a new
//! session and how the window was laid out. Kept beside the daemon's state.

use crate::draft::Draft;
use dent_protocol::Permission;
use serde::{Deserialize, Serialize};

/// The prefs' shape: 1 keeps drafts whole (chips by paste number, files
/// with them) and pastes in files of their own.
pub const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
#[serde(default)]
pub struct Prefs {
    /// Which shape these were written in (0: before `VERSION` 1).
    pub version: u32,
    /// The agent new sessions run; none is the daemon's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The first-run welcome was seen. Prefs from before it existed were
    /// someone already using dent, who doesn't need it.
    #[serde(default = "yes")]
    pub onboarded: bool,
    pub project: Option<String>,
    pub model: Option<String>,
    /// The permission a new session starts with: Full access until you pick
    /// another, which then sticks (decision 0048).
    pub permission: Permission,
    pub effort: Option<String>,
    /// New sessions get a worktree of their own (decision 0053).
    pub worktree: bool,
    pub panel_width: Option<f32>,
    pub sidebar_width: Option<f32>,
    /// You hid the sidebar (View › Hide Sidebar, ⌃⌘S); it stays hidden.
    pub sidebar_off: bool,
    /// Notifications come without their sound.
    pub quiet: bool,
    /// The Mac may sleep while agents work.
    #[serde(default)]
    pub sleepy: bool,
    /// The shortcut that brings dent forward from any app (a name from
    /// `hotkey`'s choices); none kept means the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    /// Light, dark, or as the Mac is.
    pub appearance: Appearance,
    /// The text size you chose with ⌘+ and ⌘−, as a factor.
    pub zoom: Option<f32>,
    /// Viewer tabs you closed, per session: file and its versions then.
    /// Files opened from links in each session's viewer, by path.
    pub opened: std::collections::BTreeMap<String, Vec<String>>,
    pub closed_tabs: std::collections::BTreeMap<String, std::collections::BTreeMap<String, usize>>,
    /// What you'd typed in each session and not sent, with its chips and
    /// files, so quitting loses nothing; under "" what was typed for a new
    /// session.
    pub drafts: std::collections::BTreeMap<String, Draft>,
    /// Before version 1: files attached to each draft, by the same keys.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub attached: std::collections::BTreeMap<String, Vec<String>>,
    /// Long pastes the drafts hold, by number. Kept in files of their own
    /// beside the prefs (before version 1, in here).
    #[serde(skip_serializing)]
    pub pastes: std::collections::BTreeMap<u32, String>,
    /// Drafts put aside with ⌘S, oldest first.
    pub stashes: Vec<Draft>,
    /// What each project's last new session started with, by its folder
    /// ("" for no project): a project brings these back when chosen.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub projects: std::collections::BTreeMap<String, ProjectPrefs>,
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

fn yes() -> bool {
    true
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
    if p.version < VERSION {
        migrate(&mut p);
        // Pastes kept in the prefs go to files of their own.
        for (n, full) in &p.pastes {
            write_paste(*n, full);
        }
    }
    p.pastes = load_pastes(p.drafts.values().chain(&p.stashes));
    // Files deleted since are no longer attached.
    for d in p.drafts.values_mut().chain(p.stashes.iter_mut()) {
        d.atts
            .retain(|a| a.path().is_none_or(|f| std::path::Path::new(f).exists()));
    }
    let marker = path().with_file_name(FULL_ACCESS_MARKER);
    if upgrade(&mut p, marker.exists()) {
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&marker, "");
    }
    p
}

/// A project's own new-session choices.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
#[serde(default)]
pub struct ProjectPrefs {
    pub model: Option<String>,
    pub permission: Permission,
    pub effort: Option<String>,
    pub worktree: bool,
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

/// Older prefs into this shape: a draft's chips were found again by their
/// labels and its files kept apart. No files are touched here.
fn migrate(p: &mut Prefs) {
    if p.version >= VERSION {
        return;
    }
    for d in p.drafts.values_mut().chain(p.stashes.iter_mut()) {
        if d.chips.is_empty() {
            d.chips = crate::compose::folded_in(&d.text, &p.pastes)
                .into_iter()
                .map(|(r, paste)| crate::draft::Chip {
                    start: r.start,
                    end: r.end,
                    paste,
                })
                .collect();
        }
    }
    for (k, files) in std::mem::take(&mut p.attached) {
        let d = p.drafts.entry(k).or_default();
        for path in files {
            let a = crate::draft::Attachment::File { path };
            if !d.atts.contains(&a) {
                d.atts.push(a);
            }
        }
    }
    p.version = VERSION;
}

fn pastes_dir() -> std::path::PathBuf {
    path().with_file_name("desktop-pastes")
}

thread_local! {
    /// Pastes known to be in their files already.
    static WRITTEN: std::cell::RefCell<std::collections::HashSet<u32>> = Default::default();
}

fn write_paste(n: u32, full: &str) {
    let dir = pastes_dir();
    let _ = std::fs::create_dir_all(&dir);
    if std::fs::write(dir.join(n.to_string()), full).is_ok() {
        WRITTEN.with(|w| w.borrow_mut().insert(n));
    }
}

/// The pastes these drafts hold, read from their files.
fn load_pastes<'a>(
    drafts: impl Iterator<Item = &'a Draft>,
) -> std::collections::BTreeMap<u32, String> {
    let dir = pastes_dir();
    let mut out = std::collections::BTreeMap::new();
    for n in drafts.flat_map(|d| d.pastes()) {
        if let Ok(full) = std::fs::read_to_string(dir.join(n.to_string())) {
            WRITTEN.with(|w| w.borrow_mut().insert(n));
            out.insert(n, full);
        }
    }
    out
}

/// Each paste the prefs hold goes in its own file, once; files of pastes
/// no draft holds any more go.
fn save_pastes(pastes: &std::collections::BTreeMap<u32, String>) {
    for (n, full) in pastes {
        if !WRITTEN.with(|w| w.borrow().contains(n)) {
            write_paste(*n, full);
        }
    }
    if let Ok(dir) = std::fs::read_dir(pastes_dir()) {
        for e in dir.flatten() {
            let n = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok());
            if n.is_none_or(|n| !pastes.contains_key(&n)) {
                let _ = std::fs::remove_file(e.path());
                if let Some(n) = n {
                    WRITTEN.with(|w| w.borrow_mut().remove(&n));
                }
            }
        }
    }
}

pub fn save(p: &Prefs) {
    save_pastes(&p.pastes);
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
    fn older_drafts_keep_their_chips_and_files() {
        let long: String = (0..60).map(|i| format!("line {i}\n")).collect();
        let label = crate::compose::fold_label(1, &long);
        let mut p: Prefs = serde_json::from_str(
            &serde_json::json!({
                "drafts": {"s": format!("see {label}")},
                "attached": {"s": ["/a.png"]},
                "pastes": {"1": long},
                "stashes": [{"text": "aside", "files": ["/b"]}],
            })
            .to_string(),
        )
        .unwrap();
        migrate(&mut p);
        let d = &p.drafts["s"];
        assert_eq!(&d.text[d.chips[0].range()], label);
        assert_eq!(d.chips[0].paste, 1);
        assert_eq!(d.files(), ["/a.png"]);
        assert_eq!(p.stashes[0].files(), ["/b"]);
        assert_eq!(p.version, VERSION);
        // Written now, pastes aren't in the prefs file.
        let json = serde_json::to_value(&p).unwrap();
        assert!(json.get("pastes").is_none() && json.get("attached").is_none());
    }

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
