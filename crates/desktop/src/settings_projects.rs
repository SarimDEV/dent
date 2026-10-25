//! Projects with their own defaults, on a machine's tab in Settings ›
//! Agents (decision 0076): each says what it sets, opens to change any
//! choice (or leave it to the machine), and any project there can be given
//! its own.

use dent_protocol::{Defaults, Project, Request};
use gpui_kit::{App, Context, Window};

use crate::app::{Dent, Menu, Waiting};
use crate::chat::Entry;
use crate::defaults::Field;
use crate::settings::Line;
use crate::settings_agents::field_title;

/// Projects with their own shown before "Show all".
const PROJECTS_SHOWN: usize = 8;

/// How long a machine has to say its projects.
const PROJECTS_PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

/// The choices a project can have its own of, as on the tab.
const FIELDS: [Field; 4] = [
    Field::Model,
    Field::Permission,
    Field::Effort,
    Field::Worktree,
];

impl Dent {
    /// The projects on `m` with their own, sorted by name, with the one
    /// open even before anything is set for it.
    fn own_projects(&self, m: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .setups
            .get(m)
            .map(|s| s.project_defaults.keys().cloned().collect())
            .unwrap_or_default();
        v.extend(
            self.pending_defaults
                .iter()
                .filter(|p| p.machine == m)
                .filter_map(|p| p.project.clone()),
        );
        v.sort_by_key(|p| (Dent::project_name(p).to_lowercase(), p.clone()));
        v.dedup();
        let open = self.open_project(m);
        v.retain(|p| Some(p.as_str()) == open || !self.project_own(m, p).is_empty());
        if let Some(p) = open
            && !v.iter().any(|x| x == p)
        {
            v.push(p.to_string());
        }
        v
    }

    /// The project open on `m`'s tab.
    fn open_project(&self, m: &str) -> Option<&str> {
        self.agents_project
            .as_ref()
            .filter(|(on, _)| on == m)
            .map(|(_, p)| p.as_str())
    }

    /// What project `p` on `m` sets of its own, changes on their way
    /// counted.
    fn project_own(&self, m: &str, p: &str) -> Defaults {
        self.project_defaults(m, p)
    }

    /// The lines for `m`'s projects: each with what it sets, the open one
    /// a choice a line, then a way to give another its own.
    pub fn project_lines(&mut self, m: &str, cx: &mut Context<Self>) -> Vec<Line> {
        // Giving another its own comes first: below a long list it's
        // out of reach.
        let mut lines = vec![
            Line::new("proj-add", "Give a project its own")
                .under("New sessions in it start differently from the rest of the machine")
                .chip(self.chip(Menu::KeepProject, "", "Choose a project", true, cx)),
        ];
        let all = self.own_projects(m);
        // Past a few, the rest behind "Show all"; the open one always shows.
        let shown: Vec<String> = if self.projects_all_shown || all.len() <= PROJECTS_SHOWN + 1 {
            all.clone()
        } else {
            all.iter()
                .enumerate()
                .filter(|(i, p)| *i < PROJECTS_SHOWN || self.open_project(m) == Some(p.as_str()))
                .map(|(_, p)| p.clone())
                .collect()
        };
        let hidden = all.len() - shown.len();
        // Two of one name (~/a/app, ~/b/app) say where each is, under it.
        let twin = |p: &str| {
            let name = Dent::project_name(p);
            all.iter().filter(|x| Dent::project_name(x) == name).count() > 1
        };
        for p in shown {
            let own = self.project_own(m, &p);
            let open = self.open_project(m) == Some(p.as_str());
            let said: Vec<String> = FIELDS
                .iter()
                .filter(|f| !f.is_unset(&own))
                .map(|f| self.field_words(*f, &own, m))
                .collect();
            let mut line = Line::new(format!("proj-{p}"), Dent::project_name(&p));
            if twin(&p) {
                line = line.under(crate::picker::tilde(&p));
            }
            line = line.under(if said.is_empty() {
                "Nothing of its own yet: as the machine".to_string()
            } else {
                said.join(" · ")
            });
            let (to, at) = (m.to_string(), p.clone());
            line = line.opens(open, cx, move |this, _, cx| {
                this.agents_project = if open {
                    None
                } else {
                    Some((to.clone(), at.clone()))
                };
                cx.notify();
            });
            if open {
                let base = self.defaults_for(m, None);
                let mut inner = Vec::new();
                for f in FIELDS {
                    let words = if f.is_unset(&own) {
                        format!("As the machine · {}", self.field_words(f, &base, m))
                    } else {
                        self.field_words(f, &base.under(&own), m)
                    };
                    let chip =
                        self.chip(Menu::SetProjectDefault(f), "", &words, f.is_unset(&own), cx);
                    inner.push(
                        Line::new(format!("proj-{f:?}").to_lowercase(), field_title(f)).chip(chip),
                    );
                }
                if !said.is_empty() {
                    let (to, path) = (m.to_string(), p.clone());
                    inner.push(
                        Line::new("proj-clear", "Start as the machine again")
                            .under("Its own choices go; new sessions in it start like the rest")
                            .control(self.small_button(
                                &format!("proj-remove-{p}"),
                                "Remove",
                                cx,
                                move |this, _, cx| {
                                    this.put_defaults(
                                        &to,
                                        Some(path.clone()),
                                        Defaults::default(),
                                        cx,
                                    );
                                    this.agents_project = None;
                                },
                            )),
                    );
                }
                line = line.holding(inner);
            }
            lines.push(line);
        }
        if hidden > 0 {
            lines.push(
                Line::new("proj-more", format!("{hidden} more with their own")).control(
                    self.small_button(
                        "proj-show-all",
                        &format!("Show all {}", all.len()),
                        cx,
                        |this, _, cx| {
                            this.projects_all_shown = true;
                            cx.notify();
                        },
                    ),
                ),
            );
        }
        lines
    }

    /// Ask the open tab's machine which projects it has, for the menu.
    pub fn ask_tab_projects(&mut self, cx: &mut Context<Self>) {
        let m = self.tab_machine();
        // What it failed to say last time is asked anew, not repeated.
        if self.tab_projects.get(&m).is_some_and(Result::is_err) {
            self.tab_projects.remove(&m);
        }
        let n = self.live.send_to(&m, Request::Projects);
        self.waiting_insert(n, Waiting::TabProjects(m.clone()));
        // Asked again each time the menu opens: what it said last stands
        // meanwhile, and a machine that never answers is said to.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PROJECTS_PATIENCE).await;
            let _ = this.update(cx, |this, cx| {
                // Still awaited: said not to have answered, while an answer
                // that comes later still lands.
                if this.is_waiting(n) && !this.tab_projects.contains_key(&m) {
                    let why = format!("{} didn't answer", this.machine_name(&m));
                    this.tab_projects.insert(m.clone(), Err(why));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn tab_projects_came(
        &mut self,
        m: String,
        projects: Result<Vec<Project>, String>,
        cx: &mut Context<Self>,
    ) {
        self.tab_projects.insert(m, projects);
        if self.menu == Some(Menu::KeepProject) {
            self.menu_ix = self
                .keep_project_entries(cx)
                .iter()
                .position(|e| e.choosable());
        }
        cx.notify();
    }

    /// The open tab's projects without their own yet, searched by name or
    /// folder.
    pub fn keep_project_entries(&self, cx: &App) -> Vec<Entry> {
        let m = self.tab_machine();
        let all = match self.tab_projects.get(&m) {
            None => return vec![Entry::note("Asking which projects it has…")],
            Some(Err(why)) => return vec![Entry::note(why)],
            Some(Ok(all)) => all,
        };
        let q = self.menu_query.read(cx).value().trim().to_lowercase();
        let own = self.own_projects(&m);
        let mut v: Vec<Entry> = all
            .iter()
            .filter(|p| !own.contains(&p.path))
            .filter(|p| {
                q.is_empty()
                    || p.name.to_lowercase().contains(&q)
                    || p.path.to_lowercase().contains(&q)
            })
            .map(|p| {
                let (path, on) = (p.path.clone(), m.clone());
                Entry::row(
                    format!("keep-{}", p.path),
                    &p.name,
                    Some(crate::picker::tilde(&p.path)),
                    false,
                    Some(std::rc::Rc::new(
                        move |this: &mut Dent, _: &mut Window, cx: &mut Context<Dent>| {
                            this.agents_project = Some((on.clone(), path.clone()));
                            this.menu = None;
                            cx.notify();
                        },
                    )),
                )
            })
            .collect();
        if v.is_empty() {
            v.push(Entry::note(&if q.is_empty() {
                "Every project there has its own already".to_string()
            } else {
                format!("No project matches “{q}”")
            }));
        }
        v
    }

    /// Choice `f` for the open project: the machine's, or a value of its
    /// own.
    pub fn project_default_entries(&self, f: Field) -> Vec<Entry> {
        let m = self.tab_machine();
        let Some(p) = self.open_project(&m).map(str::to_string) else {
            return Vec::new();
        };
        let own = self.project_own(&m, &p);
        let base = self.defaults_for(&m, None);
        let unset = f.is_unset(&own);
        let set = |value: Option<Defaults>| {
            let (m, p) = (m.clone(), p.clone());
            Some(std::rc::Rc::new(
                move |this: &mut Dent, _: &mut Window, cx: &mut Context<Dent>| {
                    this.set_project_field(f, &m, &p, value.as_ref(), cx);
                    this.menu = None;
                },
            ) as crate::chat::Act)
        };
        let mut v = vec![
            Entry::row(
                format!("proj-set-{f:?}-machine").to_lowercase(),
                &format!("As the machine · {}", self.field_words(f, &base, &m)),
                None,
                unset,
                set(None),
            ),
            Entry::separator(),
        ];
        let now = serde_json::to_string(&f.effective(&own)).unwrap_or_default();
        for (words, value) in self.field_values(f, &m) {
            let on =
                !unset && serde_json::to_string(&f.effective(&value)).unwrap_or_default() == now;
            v.push(Entry::row(
                format!("proj-set-{f:?}-{words}").to_lowercase(),
                &words,
                None,
                on,
                set(Some(value)),
            ));
        }
        v
    }

    /// Project `p` on `m` takes `value` for choice `f`, or leaves it to
    /// the machine.
    fn set_project_field(
        &mut self,
        f: Field,
        m: &str,
        p: &str,
        value: Option<&Defaults>,
        cx: &mut Context<Self>,
    ) {
        let mut own = self.project_own(m, p);
        f.take(&mut own, value.unwrap_or(&Defaults::default()));
        self.put_defaults(m, Some(p.to_string()), own, cx);
    }
}
