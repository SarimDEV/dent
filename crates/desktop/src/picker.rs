//! The project picker: the new session's project chip opens a menu you can
//! type into. Words search the projects; a path (`/…` or `~…`) goes to a
//! folder, with its subfolders to complete; "New project…" makes one from a
//! name and "Clone a Git URL…" clones one, both into `~/dent/projects`.

use crate::ui::FocusField;
use std::rc::Rc;

use dent_protocol::{Project, Request};
use gpui_kit::{App, Context, Window};

use crate::app::{Dent, Menu, Waiting, home, put};
use crate::chat::{Act, ActFn, Entry};

/// What the picker's field is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    /// Search the projects, or type a path.
    #[default]
    Pick,
    /// Name a new project.
    New,
    /// Paste a Git URL to clone.
    Clone,
}

/// A path is being typed rather than a search.
pub fn is_path(q: &str) -> bool {
    q.starts_with('/') || q.starts_with('~')
}

/// A Git URL is being typed.
pub fn is_url(q: &str) -> bool {
    ["https://", "http://", "ssh://", "git://", "git@"]
        .iter()
        .any(|p| q.starts_with(p))
}

/// Projects matching `q`, best first: name start, a word in the name, the
/// name anywhere, a folder on its path, then the name's letters in order;
/// ties keep the list's order (newest first).
pub fn search<'a>(projects: &'a [Project], q: &str) -> Vec<&'a Project> {
    let q = q.trim().to_lowercase();
    let mut hits: Vec<(u8, usize, &Project)> = projects
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let name = p.name.to_lowercase();
            let path = p.path.to_lowercase();
            let rank = if q.is_empty() || name.starts_with(&q) {
                0
            } else if name
                .split(|c: char| !c.is_alphanumeric())
                .any(|w| w.starts_with(&q))
            {
                1
            } else if name.contains(&q) {
                2
            } else if path.split('/').any(|seg| seg.starts_with(&q)) {
                // A folder on its path: `active` finds what's in it.
                3
            } else {
                let mut it = name.chars();
                if !q.chars().all(|c| it.any(|x| x == c)) {
                    return None;
                }
                4
            };
            Some((rank, i, p))
        })
        .collect();
    hits.sort_by_key(|(rank, i, _)| (*rank, *i));
    hits.into_iter().map(|(_, _, p)| p).collect()
}

/// A Git URL as people say it: `owner/repo`.
pub fn repo(url: &str) -> String {
    let parts: Vec<&str> = url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .take(2)
        .collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

/// A path as shown: the home folder as `~`, however the path spells it
/// (`/var` or `/private/var`), and only as whole folders.
pub fn tilde(path: &str) -> String {
    tilde_in(path, &home())
}

fn tilde_in(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return path.to_string();
    }
    let real = std::fs::canonicalize(home)
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    for h in [home, real.as_str()].into_iter().filter(|h| !h.is_empty()) {
        if path == h {
            return "~".into();
        }
        if let Some(rest) = path.strip_prefix(h)
            && rest.starts_with('/')
        {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// A path as typed, with `~` for the home folder, as the daemon reads it.
fn full(path: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) => format!("{}{rest}", home()),
        None => path.to_string(),
    }
}

fn act(f: Box<ActFn>) -> Option<Act> {
    Some(Rc::from(f))
}

impl Dent {
    /// The picker's rows for what's typed.
    pub fn project_entries(&self, cx: &App) -> Vec<Entry> {
        let q = self.menu_query.read(cx).value().trim().to_string();
        let mut v = Vec::new();
        if let Some(busy) = &self.project_busy {
            v.push(Entry::note(busy));
            return v;
        }
        if let Some(e) = &self.project_error {
            v.push(Entry::note(e));
        }
        match self.project_mode {
            Mode::New if q.is_empty() => v.push(Entry::note("Name the new project")),
            Mode::New => v.push(Entry::row(
                "p-new".into(),
                &format!("Create “{q}”"),
                Some("A Git repository in ~/dent/projects".into()),
                false,
                act(Box::new(move |this, _, _| this.new_project(&q))),
            )),
            Mode::Clone if !is_url(&q) => {
                v.push(Entry::note("Paste a Git URL: https://… or git@…"))
            }
            Mode::Clone => v.push(Entry::row(
                "p-clone".into(),
                &format!("Clone {}", repo(&q)),
                Some("Into ~/dent/projects".into()),
                false,
                act(Box::new(move |this, _, _| this.clone_project(&q))),
            )),
            Mode::Pick if is_path(&q) => self.folder_entries(&q, &mut v),
            Mode::Pick => self.search_entries(&q, &mut v),
        }
        v
    }

    /// Projects matching a search, "No project", then the ways to add one.
    fn search_entries(&self, q: &str, v: &mut Vec<Entry>) {
        let d = &self.defaults;
        let scratch = crate::scratch_dir().display().to_string();
        let projects: Vec<Project> = self
            .projects
            .iter()
            // The scratch folder is "No project", below.
            .filter(|p| p.path != scratch)
            .cloned()
            .collect();
        for (n, p) in search(&projects, q).into_iter().enumerate() {
            let path = p.path.clone();
            let on = d.project.as_deref() == Some(p.path.as_str());
            // One line: a long path keeps its start and end.
            let note = crate::viewer::middle_ellipsis(&tilde(&p.path), 38);
            let gone = p.path.clone();
            v.push(
                Entry::row(
                    format!("m-{}", p.path),
                    &p.name,
                    Some(note),
                    on,
                    act(Box::new(move |this, _, _| this.use_project(&path, false))),
                )
                .keycap((n < 9).then(|| format!("⌘ {}", n + 1)))
                .removable(
                    "Remove from the list (the folder stays)",
                    Rc::new(move |this, _, cx| this.hide_project(&gone, true, cx)),
                ),
            );
        }
        let none = q.is_empty() || "no project".contains(&q.to_lowercase());
        if none {
            v.push(Entry::row(
                "m-none".into(),
                "No project",
                Some("A scratch folder, ~/dent/scratch".into()),
                d.project.is_none(),
                act(Box::new(|this, _, _| {
                    this.defaults.project = None;
                })),
            ));
        }
        if v.iter().all(|e| !e.choosable()) && !q.is_empty() && !is_url(q) {
            v.push(Entry::note(&format!("No project matches “{q}”")));
        }
        v.push(Entry::separator());
        // A pasted URL clones straight away; a name makes a project.
        if is_url(q) {
            let url = q.to_string();
            v.push(Entry::row(
                "p-clone".into(),
                &format!("Clone {}", repo(q)),
                Some("Into ~/dent/projects".into()),
                false,
                act(Box::new(move |this, _, _| this.clone_project(&url))),
            ));
        } else if !q.is_empty() {
            // What's typed names it: one Return makes it.
            let name = q.to_string();
            v.push(Entry::row(
                "p-new".into(),
                &format!("New project “{q}”"),
                Some("A Git repository in ~/dent/projects".into()),
                false,
                act(Box::new(move |this, _, _| this.new_project(&name))),
            ));
        } else {
            v.push(
                Entry::row(
                    "p-new".into(),
                    "New project…",
                    Some("A Git repository from a name".into()),
                    false,
                    act(Box::new(|this, window, cx| {
                        this.project_mode_to(Mode::New, window, cx)
                    })),
                )
                .keeps_open(),
            );
        }
        if !is_url(q) {
            v.push(
                Entry::row(
                    "p-clone".into(),
                    "Clone a Git URL…",
                    None,
                    false,
                    act(Box::new(|this, window, cx| {
                        this.project_mode_to(Mode::Clone, window, cx)
                    })),
                )
                .keeps_open(),
            );
        }
        v.push(Entry::row(
            "m-choose".into(),
            "Choose a folder…",
            Some("Or type a path starting with / or ~".into()),
            false,
            act(Box::new(|this, _, cx| this.choose_project_folder(cx))),
        ));
    }

    /// A typed path: the folder itself when it's there, its subfolders, or
    /// making it when it isn't.
    fn folder_entries(&self, q: &str, v: &mut Vec<Entry>) {
        let Some((asked, found)) = &self.folders else {
            v.push(Entry::note("Looking…"));
            return;
        };
        let typed = q.trim_end_matches('/');
        let folders: Vec<String> = match found {
            // An answer for what was typed a moment ago stands in until the
            // new one comes, narrowed to what's typed now.
            Ok(f) => f
                .iter()
                .filter(|f| asked == q || f.to_lowercase().starts_with(&q.to_lowercase()))
                .cloned()
                .collect(),
            // A folder that can't be read: nothing to complete; one that
            // isn't there can be made.
            Err(_) => Vec::new(),
        };
        // The folder typed in full, ending in `/`, is the one listed.
        let here = q.ends_with('/') && found.is_ok() && asked == q;
        if here || folders.iter().any(|f| f == typed) {
            let path = typed.to_string();
            v.push(Entry::row(
                "p-here".into(),
                &format!("Use {}", crate::viewer::middle_ellipsis(&path, 40)),
                None,
                false,
                act(Box::new(move |this, _, _| {
                    this.use_project(&full(&path), false)
                })),
            ));
        }
        for f in folders.iter().filter(|f| f.as_str() != typed) {
            let path = f.clone();
            let name = f.rsplit('/').next().unwrap_or(f).to_string();
            // One line: the field already says where these are.
            v.push(Entry::row(
                format!("p-dir-{f}"),
                &format!("{name}/"),
                None,
                false,
                act(Box::new(move |this, _, _| {
                    this.use_project(&full(&path), false)
                })),
            ));
        }
        let missing = !here && !folders.iter().any(|f| f == typed);
        if missing && typed.len() > 1 && !typed.ends_with('~') {
            let path = typed.to_string();
            v.push(Entry::separator());
            v.push(Entry::row(
                "p-make".into(),
                &format!("Make {}", crate::viewer::middle_ellipsis(&path, 40)),
                Some("A new, empty folder".into()),
                false,
                act(Box::new(move |this, _, _| {
                    this.use_project(&full(&path), true)
                })),
            ));
        }
        if let Err(e) = found
            && asked == q
            && !missing
        {
            v.push(Entry::note(e));
        }
        if v.is_empty() {
            v.push(Entry::note("No folders here"));
        }
    }

    /// Open the picker afresh: searching, the field empty and focused.
    pub fn open_project_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project_mode = Mode::Pick;
        self.project_error = None;
        self.folders = None;
        self.menu_query.update(cx, |f, cx| {
            put(f, "", window, cx);
            f.focus_field(window, cx);
        });
    }

    /// Switch what the field is for, keeping what's typed.
    pub fn project_mode_to(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.project_mode = mode;
        self.project_error = None;
        self.menu_ix = None;
        self.menu_query
            .update(cx, |f, cx| f.focus_field(window, cx));
        self.project_query_changed(cx);
    }

    /// Esc in the picker: from naming or cloning back to the search first.
    pub fn project_back(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.menu != Some(Menu::Project) || self.project_mode == Mode::Pick {
            return false;
        }
        self.project_mode_to(Mode::Pick, window, cx);
        true
    }

    /// After the field changes: the first row lit, and a typed path's
    /// folders asked for.
    pub fn project_query_changed(&mut self, cx: &mut Context<Self>) {
        let q = self.menu_query.read(cx).value().trim().to_string();
        self.project_error = None;
        if self.project_mode == Mode::Pick && is_path(&q) {
            self.ask(Request::Folders { partial: q.clone() }, Waiting::Folders(q));
        }
        self.menu_ix = self.project_entries(cx).iter().position(|e| e.choosable());
        self.menu_scroll.scroll_to_item(0);
        cx.notify();
    }

    /// Tab on a lit folder: go into it, to pick one inside.
    pub fn project_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let q = self.menu_query.read(cx).value().trim().to_string();
        if self.project_mode != Mode::Pick || !is_path(&q) {
            return false;
        }
        let entries = self.project_entries(cx);
        let Some(e) = self.menu_ix.and_then(|i| entries.get(i)) else {
            return false;
        };
        let Some(dir) = e.key.strip_prefix("p-dir-") else {
            return true;
        };
        let into = format!("{dir}/");
        self.menu_query.update(cx, |f, cx| put(f, into, window, cx));
        true
    }

    /// Use a folder for the new session, and keep it on the list.
    pub fn use_project(&mut self, path: &str, create: bool) {
        self.ask(
            Request::AddProject {
                path: path.to_string(),
                create,
            },
            Waiting::Projects,
        );
        self.defaults.project = Some(path.to_string());
    }

    /// Take a project off the list, or put it back (Undo).
    pub fn hide_project(&mut self, path: &str, hidden: bool, cx: &mut Context<Self>) {
        self.ask(
            Request::HideProject {
                path: path.to_string(),
                hidden,
            },
            Waiting::Projects,
        );
        if hidden {
            // Gone from the list now, not when the daemon answers.
            self.projects.retain(|p| p.path != path);
            if self.defaults.project.as_deref() == Some(path) {
                self.defaults.project = None;
            }
            let name = crate::app::Dent::project_name(path);
            self.toast(
                crate::app::ToastAction::UnhideProject(path.to_string()),
                format!("Removed {name}"),
            );
        }
        // The row that slid into its place is lit, as in a Finder list.
        let entries = self.project_entries(cx);
        let at = self.menu_ix.unwrap_or(0);
        self.menu_ix = (at..entries.len())
            .chain((0..at).rev())
            .find(|i| entries[*i].choosable());
        cx.notify();
    }

    /// Make a project from a name; the picker waits, then picks it.
    pub fn new_project(&mut self, name: &str) {
        self.project_busy = Some(format!("Making {name}…"));
        self.ask(
            Request::NewProject {
                name: name.to_string(),
            },
            Waiting::ProjectMade,
        );
    }

    /// Clone a Git URL; the picker waits, then picks it.
    pub fn clone_project(&mut self, url: &str) {
        let short = repo(url);
        self.project_busy = Some(format!("Cloning {short}…"));
        self.ask(
            Request::CloneProject {
                url: url.to_string(),
            },
            Waiting::ProjectMade,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_takes_whole_folders_only() {
        assert_eq!(tilde_in("/Users/al/code", "/Users/al"), "~/code");
        assert_eq!(
            tilde_in("/Users/alice/code", "/Users/al"),
            "/Users/alice/code"
        );
        assert_eq!(tilde_in("/Users/al", "/Users/al/"), "~");
    }

    fn p(name: &str, path: &str) -> Project {
        Project {
            name: name.into(),
            path: path.into(),
            last_used: 0,
        }
    }

    #[test]
    fn searches_names_then_paths() {
        let list = [
            p("beacon", "/u/code/beacon"),
            p("dent", "/u/projects/active/dent"),
            p("design-kit", "/u/code/design-kit"),
            p("quartz", "/u/active/quartz"),
        ];
        let names = |q: &str| -> Vec<&str> {
            search(&list, q)
                .into_iter()
                .map(|p| p.name.as_str())
                .collect()
        };
        assert_eq!(names(""), ["beacon", "dent", "design-kit", "quartz"]);
        assert_eq!(names("de"), ["dent", "design-kit"]);
        assert_eq!(names("kit"), ["design-kit"]);
        // The path finds it when the name doesn't.
        assert_eq!(names("active"), ["dent", "quartz"]);
        // Letters in order.
        assert_eq!(names("bcn"), ["beacon"]);
        assert!(names("zzz").is_empty());
    }

    #[test]
    fn names_repos() {
        assert_eq!(
            repo("https://github.com/octocat/Hello-World.git"),
            "octocat/Hello-World"
        );
        assert_eq!(repo("git@github.com:acme/tool"), "acme/tool");
    }

    #[test]
    fn tells_paths_and_urls() {
        assert!(is_path("~/code") && is_path("/tmp") && !is_path("code"));
        assert!(is_url("git@github.com:a/b") && is_url("https://x/y") && !is_url("dent"));
    }
}
