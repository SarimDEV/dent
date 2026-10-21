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
use crate::chat::{Act, Entry};
use crate::picker::Mode;

/// A project being looked at in the picker, before Bring.
pub struct Draft {
    pub from: String,
    pub dir: String,
    pub pack: Pack,
    pub ticked: BTreeSet<String>,
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
fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", bytes.div_ceil(1024)),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

/// A remote's address without its scheme or `.git`, to say where a clone
/// comes from: `github.com/ada/app`.
fn short(url: &str) -> String {
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
                        &p.name,
                        Some(format!("On {}", self.machine_name(&m))),
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
            "From your other machines, to bring to {}",
            self.machine_name(&self.defaults.machine)
        )));
        v.extend(rows.into_iter().take(12));
    }

    /// Ask a project's machine what bringing it needs.
    fn look_to_bring(&mut self, from: &str, dir: &str, name: &str, cx: &mut Context<Self>) {
        self.project_error = None;
        self.project_busy = Some(format!("Looking at {name} on {}…", self.machine_name(from)));
        let n = self.live.send_to(
            from,
            Request::Pack {
                dir: dir.to_string(),
            },
        );
        self.waiting_insert(n, Waiting::Pack(from.to_string(), dir.to_string()));
        cx.notify();
    }

    /// What bringing a project needs came: its files to choose, or why it
    /// can't come.
    pub fn pack_came(
        &mut self,
        from: String,
        dir: String,
        pack: Result<Option<Pack>, String>,
        cx: &mut Context<Self>,
    ) {
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
                let ticked = pack
                    .ignored
                    .iter()
                    .filter(|i| i.carry)
                    .map(|i| i.path.clone())
                    .collect();
                self.bring_draft = Some(Draft {
                    from,
                    dir,
                    pack,
                    ticked,
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
        let files = match d.ticked.len() {
            0 => String::new(),
            1 => " and 1 file".into(),
            n => format!(" and {n} files"),
        };
        let from = short(&p.remotes[0].url);
        let note = match &p.branch {
            Some(b) => format!("Clones {from} on {b}{files}"),
            None => format!("Clones {from}{files}"),
        };
        v.push(Entry::row(
            "b-go".into(),
            &format!("Bring {} to {here}", p.name),
            Some(note),
            false,
            act(|this, _, cx| this.bring_chosen(cx)),
        ));
        if p.ignored.is_empty() {
            return;
        }
        v.push(Entry::separator());
        v.push(Entry::note(&format!(
            "Files Git ignores on {}, which a clone won't have",
            self.machine_name(&d.from)
        )));
        for i in &p.ignored {
            let path = i.path.clone();
            v.push(
                Entry::row(
                    format!("b-f-{}", i.path),
                    &i.path,
                    Some(size(i.size)),
                    d.ticked.contains(&i.path),
                    act(move |this, _, cx| {
                        if let Some(d) = this.bring_draft.as_mut()
                            && !d.ticked.remove(&path)
                        {
                            d.ticked.insert(path.clone());
                        }
                        cx.notify();
                    }),
                )
                .keeps_open(),
            );
        }
    }

    /// Bring: read the ticked files on their machine, then it's chosen.
    fn bring_chosen(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &self.bring_draft else {
            return;
        };
        let bring = Bring {
            name: d.pack.name.clone(),
            remotes: d.pack.remotes.clone(),
            branch: d.pack.branch.clone(),
            files: Vec::new(),
        };
        if d.ticked.is_empty() {
            self.bring_ready(bring);
        } else {
            let (from, dir) = (d.from.clone(), d.dir.clone());
            let paths: Vec<String> = d.ticked.iter().cloned().collect();
            self.project_error = None;
            self.project_busy = Some(match paths.len() {
                1 => format!("Reading {} on {}…", paths[0], self.machine_name(&from)),
                n => format!("Reading {n} files on {}…", self.machine_name(&from)),
            });
            let n = self.live.send_to(&from, Request::ReadFiles { dir, paths });
            self.waiting_insert(n, Waiting::BringFiles(bring));
        }
        cx.notify();
    }

    /// The ticked files came, or couldn't be read.
    pub fn bring_files_came(
        &mut self,
        mut bring: Bring,
        files: Result<Vec<dent_protocol::Carried>, String>,
        cx: &mut Context<Self>,
    ) {
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
        assert_eq!(short("ssh://git@host/x/"), "git@host/x");
    }

    #[test]
    fn sizes_in_a_word() {
        assert_eq!(size(312), "312 B");
        assert_eq!(size(4000), "4 KB");
        assert_eq!(size(1_300_000), "1.2 MB");
    }
}
