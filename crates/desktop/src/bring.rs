//! Bringing a project from another machine to the one a new session goes
//! to. The project picker lists your other machines' projects that aren't
//! on this one; picking one asks its machine what a clone needs and lists
//! the files Git ignores there, environment files ticked, and Bring reads
//! the ticked ones. The session starts on the chosen machine as soon as
//! it's sent: its daemon clones the project, writes the files in, and the
//! first message goes once that's done ([daemon.md](../../../docs/daemon.md)).

use std::collections::BTreeSet;
use std::rc::Rc;

use dent_client_core::machine::State;
use dent_protocol::{Bring, Pack, Project, Request};
use gpui_kit::{Context, Window};

use crate::app::{Dent, Waiting};

/// How long a machine has to say what copying a project needs.
const BRING_PATIENCE: std::time::Duration = std::time::Duration::from_secs(15);
use crate::chat::{Act, Entry};
use crate::picker::Mode;

/// A project being looked at in the picker, before Bring.
pub struct Draft {
    pub from: String,
    pub dir: String,
    pub pack: Pack,
    pub files: FileChoice,
}

/// The ignored files chosen to go with a clone (Bring, or a move cloning
/// its project there), and how much of their list shows.
#[derive(Clone, Default)]
pub struct FileChoice {
    pub ticked: BTreeSet<String>,
    /// Every environment file listed, not only the first few.
    pub all_env: bool,
    /// The other ignored files listed at all.
    pub others: bool,
}

/// Environment files listed before "Show all".
const ENV_SHOWN: usize = 8;

impl FileChoice {
    /// As offered: the files its machine marks to carry.
    pub fn of(pack: &Pack) -> Self {
        Self {
            ticked: pack
                .ignored
                .iter()
                .filter(|i| i.carry)
                .map(|i| i.path.clone())
                .collect(),
            ..Self::default()
        }
    }

    /// What the ticked files come to.
    pub fn bytes(&self, pack: &Pack) -> u64 {
        pack.ignored
            .iter()
            .filter(|i| self.ticked.contains(&i.path))
            .map(|i| i.size)
            .sum()
    }

    /// More than may go with it.
    pub fn too_much(&self, pack: &Pack) -> bool {
        self.bytes(pack) > dent_protocol::CARRIED_MAX as u64
    }

    /// "and 3 files", for the line that says what a clone takes.
    pub fn and_files(&self) -> String {
        match self.ticked.len() {
            0 => String::new(),
            1 => " and 1 file".into(),
            n => format!(" and {n} files"),
        }
    }
}

/// Where a menu's [`FileChoice`] lives, for its rows to change it.
pub type Choice = fn(&mut Dent) -> Option<&mut FileChoice>;

/// The rows choosing what goes with a clone, under the row that goes, keys
/// starting `key`: how many are ticked and what they come to, environment
/// files (all at once, the first few, the rest on asking), then the other
/// ignored files on asking. Long lists stay short until asked.
pub fn file_entries(key: &str, pack: &Pack, c: &FileChoice, choice: Choice, v: &mut Vec<Entry>) {
    if pack.ignored.is_empty() {
        return;
    }
    v.push(Entry::separator());
    let bytes = c.bytes(pack);
    let listed = pack.ignored.len() + pack.more as usize;
    v.push(Entry::note(&format!(
        "{} of {listed} file{} Git ignores ticked{}. Folders of what's installed or built, like node_modules, never go.",
        c.ticked.len(),
        if listed == 1 { "" } else { "s" },
        if bytes > 0 {
            format!(" · {}", size(bytes))
        } else {
            String::new()
        }
    )));
    let file = |i: &dent_protocol::Ignored| {
        let path = i.path.clone();
        Entry::row(
            format!("{key}-f-{}", i.path),
            &i.path,
            Some(size(i.size)),
            c.ticked.contains(&i.path),
            act(move |this, _, cx| {
                if let Some(c) = choice(this)
                    && !c.ticked.remove(&path)
                {
                    c.ticked.insert(path.clone());
                }
                cx.notify();
            }),
        )
        .keeps_open()
    };
    let (env, rest): (Vec<_>, Vec<_>) = pack.ignored.iter().partition(|i| i.carry || i.env);
    if !env.is_empty() {
        // Ticked all at once: those small enough to be settings (one over
        // 1 MB is ticked by hand, if at all).
        let all: Vec<String> = env
            .iter()
            .filter(|i| i.carry)
            .map(|i| i.path.clone())
            .collect();
        let ticked = all.iter().filter(|p| c.ticked.contains(*p)).count();
        if !all.is_empty() {
            v.push(
                Entry::row(
                    format!("{key}-env-all"),
                    &format!("Environment files · {}", env.len()),
                    Some(match ticked {
                        n if n == all.len() => "All ticked · untick them all".into(),
                        0 => "None ticked · tick them all".into(),
                        n => format!("{n} ticked · tick them all"),
                    }),
                    ticked == all.len(),
                    act(move |this, _, cx| {
                        if let Some(c) = choice(this) {
                            if all.iter().all(|p| c.ticked.contains(p)) {
                                for p in &all {
                                    c.ticked.remove(p);
                                }
                            } else {
                                c.ticked.extend(all.iter().cloned());
                            }
                        }
                        cx.notify();
                    }),
                )
                .keeps_open(),
            );
        }
        let shown = if c.all_env {
            env.len()
        } else {
            env.len().min(ENV_SHOWN)
        };
        v.extend(env[..shown].iter().map(|i| file(i)));
        if env.len() > ENV_SHOWN {
            v.push(Entry::fold(
                format!("{key}-env-more"),
                &if c.all_env {
                    "Show fewer".to_string()
                } else {
                    format!("Show all {}", env.len())
                },
                c.all_env,
                act(move |this, _, cx| {
                    if let Some(c) = choice(this) {
                        c.all_env = !c.all_env;
                    }
                    // The rows after it moved: no row stays lit by
                    // where it was.
                    this.menu_ix = None;
                    cx.notify();
                }),
            ));
        }
    }
    if !rest.is_empty() {
        v.push(Entry::fold(
            format!("{key}-others"),
            &format!("Other files Git ignores · {}", rest.len()),
            c.others,
            act(move |this, _, cx| {
                if let Some(c) = choice(this) {
                    c.others = !c.others;
                }
                this.menu_ix = None;
                cx.notify();
            }),
        ));
        if c.others {
            v.extend(rest.iter().map(|i| file(i)));
        }
    }
    if pack.more > 0 {
        v.push(Entry::note(&format!("{} more aren't listed.", pack.more)));
    }
}

/// A project the next session brings with it.
#[derive(Clone)]
pub struct Chosen {
    /// The machine it goes to; a different one drops it.
    pub to: String,
    pub bring: Bring,
}

fn act(f: impl Fn(&mut Dent, &mut Window, &mut Context<Dent>) + 'static) -> Option<Act> {
    Some(Rc::new(f))
}

/// File sizes in a word: 312 B, 4 KB, 1.2 MB.
pub fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", bytes.div_ceil(1024)),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

/// A remote's address without its scheme or `.git`, to say where a clone
/// comes from: `github.com/ada/app`.
pub fn short(url: &str) -> String {
    let s = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("ssh://")
        .trim_start_matches("git://");
    let s = s
        .strip_prefix("git@")
        .map_or(s.to_string(), |r| r.replacen(':', "/", 1));
    s.trim_end_matches('/').trim_end_matches(".git").to_string()
}

impl Dent {
    /// The machines a project could be brought from: every other one
    /// that's online.
    fn others_online(&self) -> Vec<String> {
        let here = &self.defaults.machine;
        let mut v: Vec<String> = self
            .added_machines()
            .filter(|m| matches!(m.state, State::Online | State::Slow))
            .map(|m| m.id.clone())
            .collect();
        if self.connection.is_ok() {
            v.insert(0, String::new());
        }
        v.retain(|m| m != here);
        v
    }

    /// Ask the other machines for their projects, as the picker opens.
    pub fn ask_elsewhere(&mut self) {
        self.elsewhere.clear();
        if !self.has_machines() {
            return;
        }
        for m in self.others_online() {
            let n = self.live.send_to(&m, Request::Projects);
            self.waiting_insert(n, Waiting::Elsewhere(m));
        }
    }

    /// Another machine's projects came.
    pub fn elsewhere_came(&mut self, machine: String, projects: Vec<Project>) {
        if machine != self.defaults.machine {
            self.elsewhere.insert(machine, projects);
        }
    }

    /// The picker's rows for projects on other machines that aren't on
    /// this one (by name), matching what's typed.
    pub fn elsewhere_entries(&self, q: &str, v: &mut Vec<Entry>) {
        let here: BTreeSet<&str> = self.projects.iter().map(|p| p.name.as_str()).collect();
        let mut rows = Vec::new();
        let mut seen = BTreeSet::new();
        for m in self.others_online() {
            let Some(list) = self.elsewhere.get(&m) else {
                continue;
            };
            let list: Vec<Project> = list
                .iter()
                .filter(|p| !crate::app::is_scratch(&m, &p.path) && !here.contains(p.name.as_str()))
                .cloned()
                .collect();
            for p in crate::picker::search(&list, q) {
                // One row per name: the first machine that has it.
                if !seen.insert(p.name.clone()) {
                    continue;
                }
                let (from, dir, name) = (m.clone(), p.path.clone(), p.name.clone());
                rows.push(
                    Entry::row(
                        format!("b-{m}-{}", p.path),
                        &format!("Copy {} from {}…", p.name, self.machine_name(&m)),
                        Some(format!(
                            "Clones it on {}; you choose its files next",
                            self.machine_name(&self.defaults.machine)
                        )),
                        false,
                        act(move |this, _, cx| this.look_to_bring(&from, &dir, &name, cx)),
                    )
                    .keeps_open(),
                );
            }
        }
        if rows.is_empty() {
            return;
        }
        if !v.is_empty() {
            v.push(Entry::separator());
        }
        v.push(Entry::note(&format!(
            "Copy a project from your other machines to {}",
            self.machine_name(&self.defaults.machine)
        )));
        v.extend(rows.into_iter().take(12));
    }

    /// Ask a project's machine what bringing it needs.
    fn look_to_bring(&mut self, from: &str, dir: &str, name: &str, cx: &mut Context<Self>) {
        self.project_error = None;
        self.bring_asked = Some((from.to_string(), dir.to_string()));
        self.project_busy = Some(format!(
            "Checking what copying {name} from {} to {} needs…",
            self.machine_name(from),
            self.machine_name(&self.defaults.machine)
        ));
        let n = self.live.send_to(
            from,
            Request::Pack {
                dir: dir.to_string(),
            },
        );
        let to = self.defaults.machine.clone();
        self.waiting_insert(
            n,
            Waiting::Pack(from.to_string(), dir.to_string(), to.clone()),
        );
        // A machine that doesn't answer is said, rather than waited on for
        // good.
        let (from, dir) = (from.to_string(), dir.to_string());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BRING_PATIENCE).await;
            let _ = this.update(cx, |this, cx| {
                if this.bring_asked.as_ref() == Some(&(from.clone(), dir.clone())) {
                    let why = format!(
                        "{} didn't answer in {} seconds, so nothing was copied",
                        this.machine_name(&from),
                        BRING_PATIENCE.as_secs()
                    );
                    this.pack_came(from, dir, to, Err(why), cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// What bringing a project needs came: its files to choose, or why it
    /// can't come.
    pub fn pack_came(
        &mut self,
        from: String,
        dir: String,
        to: String,
        pack: Result<Option<Pack>, String>,
        cx: &mut Context<Self>,
    ) {
        // Asked for another machine, or the picker's closed or moved on:
        // old news.
        let looking = self.menu == Some(crate::app::Menu::Project)
            && self.project_mode == Mode::Pick
            && self.bring_asked.as_ref() == Some(&(from.clone(), dir.clone()));
        if to != self.defaults.machine || from == to || !looking {
            return;
        }
        self.bring_asked = None;
        self.project_busy = None;
        let there = self.machine_name(&from);
        let here = self.machine_name(&self.defaults.machine);
        let name = crate::app::Dent::project_name(&dir);
        match pack {
            Err(e) if e.contains("unknown variant") => {
                self.project_error = Some(format!(
                    "{there} runs an older dent, which can't share its projects: update dent there"
                ))
            }
            Err(e) => self.project_error = Some(e),
            Ok(None) => {
                self.project_error = Some(format!(
                    "{name} isn't a Git repository, so it can't be cloned to {here}"
                ))
            }
            Ok(Some(p)) if p.remotes.is_empty() => {
                self.project_error = Some(format!(
                    "{name} has no remote {here} can clone from: push it somewhere first"
                ))
            }
            Ok(Some(pack)) => {
                self.bring_draft = Some(Draft {
                    from,
                    dir,
                    files: FileChoice::of(&pack),
                    pack,
                });
                self.project_mode = Mode::Bring;
                // Bring is lit, not the first ticked file: Return brings.
                let mut v = Vec::new();
                self.bring_entries(&mut v);
                self.menu_ix = v.iter().position(|e| e.key == "b-go");
            }
        }
        self.relight_menu(cx);
    }

    /// The picker's rows while a project's being looked at: Bring, then
    /// the files Git ignores there, each ticked to come along or not.
    pub fn bring_entries(&self, v: &mut Vec<Entry>) {
        let Some(d) = &self.bring_draft else {
            v.push(Entry::note("Pick a project to bring"));
            return;
        };
        let p = &d.pack;
        let here = self.machine_name(&self.defaults.machine);
        let files = d.files.and_files();
        let from = short(&p.remotes[0].url);
        let note = match &p.branch {
            Some(b) => format!("Clones {from} on {b}{files}"),
            None => format!("Clones {from}{files}"),
        };
        let mut go = Entry::row(
            "b-go".into(),
            &format!("Bring {} to {here}", p.name),
            Some(note),
            false,
            act(|this, _, cx| this.bring_chosen(cx)),
        )
        // Open while its files are read; it closes once they're here.
        .keeps_open();
        if d.files.too_much(p) {
            go.note = Some(format!(
                "The ticked files come to {}; at most {} go along",
                size(d.files.bytes(p)),
                size(dent_protocol::CARRIED_MAX as u64)
            ));
            go.off = Some("Untick some files first");
            go.act = None;
        }
        v.push(go);
        // Work here a clone won't have: said before it's brought.
        if let Some(missing) = p.missing() {
            v.push(Entry::note(&missing));
        }
        file_entries(
            "b",
            p,
            &d.files,
            |this| this.bring_draft.as_mut().map(|d| &mut d.files),
            v,
        );
    }

    /// Bring: read the ticked files on their machine, then it's chosen.
    fn bring_chosen(&mut self, cx: &mut Context<Self>) {
        // Its files are already being read.
        if self.project_busy.is_some() {
            return;
        }
        let Some(d) = &self.bring_draft else {
            return;
        };
        let bring = Bring {
            name: d.pack.name.clone(),
            remotes: d.pack.remotes.clone(),
            branch: d.pack.branch.clone(),
            files: Vec::new(),
        };
        if d.files.too_much(&d.pack) {
            return;
        }
        if d.files.ticked.is_empty() {
            self.bring_ready(bring);
        } else {
            let (from, dir) = (d.from.clone(), d.dir.clone());
            let paths: Vec<String> = d.files.ticked.iter().cloned().collect();
            self.project_error = None;
            self.project_busy = Some(match paths.len() {
                1 => format!(
                    "Reading {} to copy from {}…",
                    paths[0],
                    self.machine_name(&from)
                ),
                n => format!(
                    "Reading the {n} files to copy from {}…",
                    self.machine_name(&from)
                ),
            });
            let n = self.live.send_to(&from, Request::ReadFiles { dir, paths });
            let to = self.defaults.machine.clone();
            self.waiting_insert(n, Waiting::BringFiles(bring, to));
        }
        cx.notify();
    }

    /// The ticked files came, or couldn't be read.
    pub fn bring_files_came(
        &mut self,
        mut bring: Bring,
        to: String,
        files: Result<Vec<dent_protocol::Carried>, String>,
        cx: &mut Context<Self>,
    ) {
        // Only while that project's still being looked at in the picker,
        // for the machine it was asked for.
        let looking = self.menu == Some(crate::app::Menu::Project)
            && self.project_mode == Mode::Bring
            && self
                .bring_draft
                .as_ref()
                .is_some_and(|d| d.pack.name == bring.name);
        if to != self.defaults.machine || !looking {
            return;
        }
        self.project_busy = None;
        match files {
            Ok(files) => {
                bring.files = files;
                self.bring_ready(bring);
            }
            Err(e) => self.project_error = Some(e),
        }
        self.relight_menu(cx);
    }

    fn bring_ready(&mut self, bring: Bring) {
        self.set_project(None);
        self.bringing = Some(Chosen {
            to: self.defaults.machine.clone(),
            bring,
        });
        self.bring_draft = None;
        self.project_mode = Mode::Pick;
        self.menu = None;
    }

    /// The project the next session brings, if it goes where it was
    /// chosen for.
    pub fn bringing_here(&self) -> Option<&Chosen> {
        self.bringing
            .as_ref()
            .filter(|c| c.to == self.defaults.machine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_where_a_clone_comes_from() {
        assert_eq!(
            short("https://github.com/ada/app.git"),
            "github.com/ada/app"
        );
        assert_eq!(short("git@github.com:ada/app.git"), "github.com/ada/app");
        assert_eq!(short("ssh://git@host/x/"), "host/x");
    }

    #[test]
    fn sizes_in_a_word() {
        assert_eq!(size(312), "312 B");
        assert_eq!(size(4000), "4 KB");
        assert_eq!(size(1_300_000), "1.2 MB");
    }
}
