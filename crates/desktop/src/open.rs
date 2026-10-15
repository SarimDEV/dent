//! Opening a file outside dent: in the app you picked for its kind (by
//! extension), or the Mac's default. "Open with" lists the apps the Mac
//! says can open it; picking one opens the file and remembers the choice.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;

thread_local! {
    /// Your pick per extension: the app's path.
    static CHOSEN: RefCell<BTreeMap<String, String>> = const { RefCell::new(BTreeMap::new()) };
}

/// What a file's kind is called for remembering: its extension, lowercase,
/// or its whole name when it has none (`Makefile`).
fn kind(path: &str) -> String {
    let p = Path::new(path);
    p.extension()
        .or_else(|| p.file_name())
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn load(chosen: BTreeMap<String, String>) {
    CHOSEN.with(|c| *c.borrow_mut() = chosen);
}

/// Every pick, for saving.
pub fn chosen() -> BTreeMap<String, String> {
    CHOSEN.with(|c| c.borrow().clone())
}

/// The app you picked for this kind of file, if it's still there.
pub fn app_for(path: &str) -> Option<String> {
    CHOSEN
        .with(|c| c.borrow().get(&kind(path)).cloned())
        .filter(|a| Path::new(a).exists())
}

/// An app's name as the Finder shows it.
pub fn app_name(app: &str) -> String {
    Path::new(app)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.to_string())
}

/// Open `path` outside dent, in your pick for its kind or the default app.
pub fn open(path: &str, cx: &mut gpui_kit::App) {
    match app_for(path) {
        Some(app) => {
            let _ = std::process::Command::new("open")
                .arg("-a")
                .arg(&app)
                .arg(path)
                .spawn();
        }
        None => cx.open_url(&format!("file://{path}")),
    }
}

/// Open `path` with `app` and remember it for this kind of file; `None`
/// goes back to the Mac's default.
pub fn open_with(path: &str, app: Option<String>, cx: &mut gpui_kit::App) {
    CHOSEN.with(|c| match &app {
        Some(a) => {
            c.borrow_mut().insert(kind(path), a.clone());
        }
        None => {
            c.borrow_mut().remove(&kind(path));
        }
    });
    open(path, cx);
}

/// The apps the Mac says can open `path`, its default first.
pub fn apps(path: &str) -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSString, NSURL};
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        let ws = objc2_app_kit::NSWorkspace::sharedWorkspace();
        let mut out: Vec<String> = Vec::new();
        if let Some(d) = ws.URLForApplicationToOpenURL(&url)
            && let Some(p) = d.path()
        {
            out.push(p.to_string());
        }
        for u in ws.URLsForApplicationsToOpenURL(&url).iter() {
            if let Some(p) = u.path() {
                let p = p.to_string();
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
        out
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_extensions_or_names() {
        assert_eq!(kind("/a/b/Notes.MD"), "md");
        assert_eq!(kind("/a/Makefile"), "makefile");
    }

    #[test]
    fn a_pick_holds_only_while_its_app_is_there() {
        let gone = "/Applications/Nothing Here.app".to_string();
        load(BTreeMap::from([("rs".to_string(), gone)]));
        assert_eq!(app_for("/x/main.rs"), None);
        let here = std::env::temp_dir().display().to_string();
        load(BTreeMap::from([("rs".to_string(), here.clone())]));
        assert_eq!(app_for("/x/main.rs"), Some(here));
        assert_eq!(app_for("/x/notes.md"), None);
    }

    #[test]
    fn app_names_drop_the_bundle_suffix() {
        assert_eq!(app_name("/Applications/TextEdit.app"), "TextEdit");
    }
}
