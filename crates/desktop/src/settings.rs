//! Settings (⌘,) and the first-run welcome, which share their rows: how
//! each agent stands (installed, version, signed in), how this Mac is set
//! up (Git, who commits, the GitHub CLI, starting at login,
//! notifications), and the app's own preferences. Rows are drawn as every
//! list of things you act on is (design-system.md): a mark in the margin,
//! the name, how it stands beneath, its controls at the end.
//!
//! Which agents there are and how each stands is the daemon's to say
//! (`Request::Agents`, `Request::Machine`); a preference row does exactly
//! what its menu item does, by dispatching the same action.

use gpui_kit::prelude::*;
use gpui_kit::*;

use dent_protocol::{Gh, Machine, Mark, Request};

use crate::agents::{Standing, standing};
use crate::app::{Dent, Waiting};
use crate::ds::{CODE, DISPLAY, LABEL, Typed, UI, motion, radius, space};
use crate::ui::{self, Arrive, From, Press, Tipped, probe, tip_owned};

/// Which sheet is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Settings,
    /// The first launch: what dent needs, and the choices worth making once.
    Welcome,
}

/// The pages of Settings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    General,
    Agents,
    Machines,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::General, Tab::Agents, Tab::Machines];

    pub fn name(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Agents => "Agents",
            Tab::Machines => "Machines",
        }
    }
}

/// Where Git, the GitHub CLI and their sign-in are read about.
const GIT_URL: &str = "https://git-scm.com/downloads";
const GH_URL: &str = "https://cli.github.com";

/// One row, before it's drawn.
pub(crate) struct Line {
    id: String,
    mark: Option<Mark>,
    label: String,
    /// Under the name: how it stands, and more. `true` sets it in the
    /// code face (a command, a path).
    under: Vec<(String, bool)>,
    /// With no controls, how it stands, at the row's end.
    value: Option<String>,
    controls: Vec<AnyElement>,
    /// Under it, a value and who has it, a row each (a setting that
    /// differs between machines).
    groups: Vec<(String, String)>,
    /// Under the line before, as one of its own (a project's choices).
    indent: bool,
}

impl Line {
    pub(crate) fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            mark: None,
            label: label.into(),
            under: Vec::new(),
            value: None,
            controls: Vec::new(),
            groups: Vec::new(),
            indent: false,
        }
    }

    pub(crate) fn indent(mut self) -> Self {
        self.indent = true;
        self
    }

    pub(crate) fn group(mut self, value: impl Into<String>, who: impl Into<String>) -> Self {
        self.groups.push((value.into(), who.into()));
        self
    }

    pub(crate) fn mark(mut self, m: Option<Mark>) -> Self {
        self.mark = m;
        self
    }

    pub(crate) fn under(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        if !text.is_empty() {
            self.under.push((text, false));
        }
        self
    }

    pub(crate) fn code(mut self, text: impl Into<String>) -> Self {
        self.under.push((text.into(), true));
        self
    }

    pub(crate) fn value(mut self, text: impl Into<String>) -> Self {
        self.value = Some(text.into());
        self
    }

    pub(crate) fn control(mut self, el: impl IntoElement) -> Self {
        self.controls.push(el.into_any_element());
        self
    }
}

impl Dent {
    /// Open Settings, or the welcome; each asks again how things stand.
    pub fn open_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.pane = Some(pane);
        self.check_again();
        // Opened onto Machines: as when that page is picked, an away
        // machine is tried again now rather than at its next backoff.
        if pane == Pane::Settings && self.settings_tab == Tab::Machines {
            self.ask(Request::Devices, Waiting::Devices);
            self.live
                .signal(None, dent_client_core::machine::Signal::Retry);
        }
        cx.notify();
    }

    pub fn close_pane(&mut self, cx: &mut Context<Self>) -> bool {
        let was = self.pane.take();
        if was == Some(Pane::Welcome) {
            // Seen once is enough; the banner keeps saying what's missing.
            self.onboarded = true;
            self.prefs_changed();
        }
        cx.notify();
        was.is_some()
    }

    /// Ask the daemon again how the agents and this Mac stand.
    pub fn check_again(&mut self) {
        self.ask(Request::Agents { fresh: true }, Waiting::Agents);
        self.ask(Request::Machine, Waiting::Machine);
        self.checking = true;
        crate::notify::read_allowed();
    }

    /// Open a page outside dent. Walkthroughs note it instead.
    pub fn open_outside(&mut self, url: &str, cx: &mut Context<Self>) {
        if self.walk.is_some() {
            self.opened_outside.push(url.to_string());
            return;
        }
        cx.open_url(url);
    }

    /// Run `line` in Terminal, for what signs in there. Walkthroughs note
    /// it instead.
    pub fn run_in_terminal(&mut self, line: &str, cx: &mut Context<Self>) {
        if self.walk.is_some() {
            self.opened_outside.push(format!("terminal: {line}"));
            cx.notify();
            return;
        }
        let dir = crate::live::default_state().join("terminal");
        let file = dir.join("sign-in.command");
        let script = format!("#!/bin/zsh -l\n{line}\n");
        let wrote = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, script));
        #[cfg(unix)]
        let wrote = wrote.and_then(|_| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700))
        });
        let opened = wrote.and_then(|_| {
            std::process::Command::new("open")
                .arg("-a")
                .arg("Terminal")
                .arg(&file)
                .spawn()
                .map(|_| ())
        });
        let said = match opened {
            Ok(()) => "Finish in Terminal, then Check again".to_string(),
            Err(e) => format!("Couldn't open Terminal: {e}. Run {line} yourself."),
        };
        self.toast(crate::ToastAction::Info, said);
        cx.notify();
    }

    /// The new-session page's line when the default agent can't work: not
    /// installed or signed out. `None` while it can, or before it's known.
    pub fn agent_banner(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // About the machine new sessions go to: its agents are its own.
        let machine = self.defaults.machine.clone();
        let a = self.agents_on(&machine).first()?.clone();
        // Before the daemon has said how it stands, nothing is known.
        let known = if machine.is_empty() {
            self.agents_known
        } else {
            self.machines
                .iter()
                .any(|m| m.id == machine && m.agents_known)
        };
        if !known {
            return None;
        }
        let far = (!machine.is_empty()).then(|| {
            self.machines
                .iter()
                .find(|m| m.id == machine)
                .map_or_else(|| machine.clone(), |m| m.name().to_string())
        });
        let on = far.clone().unwrap_or_else(|| "this Mac".into());
        let t = self.t;
        let (said, action): (String, Option<(&str, AnyElement)>) = match standing(&a) {
            Standing::Ready => return None,
            Standing::Missing => (
                format!("{} isn't installed on {on}", a.name),
                Some(("banner-install", {
                    let url = a.install_url.clone();
                    self.small_button(
                        "banner-install",
                        "How to install",
                        cx,
                        move |this, _, cx| this.open_outside(&url, cx),
                    )
                })),
            ),
            Standing::SignedOut => (
                match &far {
                    Some(name) => format!("{} is signed out on {name}: sign in there", a.name),
                    None => format!("{} is signed out", a.name),
                },
                // Signing in runs in a terminal here, which reaches only
                // this Mac.
                a.sign_in.clone().filter(|_| far.is_none()).map(|line| {
                    (
                        "banner-sign-in",
                        self.small_button("banner-sign-in", "Sign in", cx, move |this, _, cx| {
                            this.run_in_terminal(&line, cx)
                        }),
                    )
                }),
            ),
        };
        // Another machine's agents are asked again each time it connects.
        let again = far.is_none().then(|| {
            self.small_button("banner-again", "Check again", cx, |this, _, cx| {
                this.check_again();
                cx.notify();
            })
        });
        Some(
            div()
                .id("agent-banner")
                .relative()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .ty(UI)
                .text_color(t.ink)
                .child(ui::dot(Some(t.needs.0), &t))
                .child(div().flex_1().min_w_0().child(said))
                .children(action.map(|(_, el)| el))
                .children(again)
                .child(probe(&self.probes, "agent-banner"))
                .into_any_element(),
        )
    }

    /// An outlined button at a row's end.
    pub(crate) fn small_button(
        &self,
        id: &str,
        label: &str,
        cx: &mut Context<Self>,
        press: impl Fn(&mut Dent, &mut Window, &mut Context<Dent>) + 'static,
    ) -> AnyElement {
        let t = self.t;
        ui::inner_button(
            SharedString::from(id.to_string()),
            label.to_string(),
            false,
            &t,
        )
        .h(px(crate::ds::size::CONTROL_SM))
        .px(px(space::S2))
        .relative()
        .child(probe(&self.probes, id.to_string()))
        .on_press(cx.listener(move |this, _, window, cx| press(this, window, cx)))
        .into_any_element()
    }

    /// A switch at a row's end; `action` is what its menu item dispatches.
    fn switch_for(
        &self,
        id: &str,
        on: bool,
        tip: &str,
        cx: &mut Context<Self>,
        flip: impl Fn(&mut Dent, &mut Window, &mut Context<Dent>) + 'static,
    ) -> AnyElement {
        let t = self.t;
        let el = div()
            .id(SharedString::from(id.to_string()))
            .with_tip(tip_owned(tip.to_string(), t))
            .on_press(cx.listener(move |this, _, window, cx| {
                flip(this, window, cx);
                cx.notify();
            }));
        div()
            .relative()
            .child(ui::switch(el, on, &t))
            .child(probe(&self.probes, id.to_string()))
            .into_any_element()
    }

    /// The agents' section: each one's version, account and command, and
    /// what to do when it can't work yet.
    pub(crate) fn agent_lines(&mut self, machine: &str, cx: &mut Context<Self>) -> Vec<Line> {
        let here = machine.is_empty();
        let (agents, known) = if here {
            (self.agents.clone(), self.agents_known)
        } else {
            self.machines
                .iter()
                .find(|m| m.id == machine)
                .map_or((Vec::new(), false), |m| (m.agents.clone(), m.agents_known))
        };
        let agents = if agents.is_empty() {
            vec![crate::bridge::agent_info(&[], "")]
        } else {
            agents
        };
        let several = agents.iter().filter(|a| a.installed).count() > 1;
        // The one new sessions there run: its default.
        let chosen = self
            .defaults_for(machine, None)
            .agent
            .unwrap_or_else(|| self.agent_id(self.new_agent_on(machine).as_deref()));
        let owner = machine.to_string();
        agents
            .iter()
            .map(|a| {
                let st = standing(a);
                let mut line = Line::new(format!("agent-{}", a.id), a.name.clone())
                    .mark(match (known, &st) {
                        (false, _) => Some(Mark::Waiting),
                        (true, Standing::Ready) if a.untested() => Some(Mark::Attention),
                        (true, Standing::Ready) => None,
                        (true, Standing::SignedOut) => Some(Mark::Attention),
                        (true, Standing::Missing) => Some(Mark::Bad),
                    })
                    .under(if known {
                        crate::agents::version_line(a)
                    } else {
                        "Checking…".into()
                    })
                    .under(crate::agents::account_line(a))
                    // A path keeps its end, the part that says what it is.
                    .code(crate::cards::elide_start(&a.command, 64));
                if known && st == Standing::Missing && here {
                    let url = a.install_url.clone();
                    line = line.control(self.small_button(
                        &format!("install-{}", a.id),
                        "How to install",
                        cx,
                        move |this, _, cx| this.open_outside(&url, cx),
                    ));
                }
                if known
                    && st == Standing::SignedOut
                    && !here
                    && let Some(sign_in) = a.sign_in.clone()
                {
                    line = line.under("To sign in, run this on it:").code(sign_in);
                } else if known
                    && st == Standing::SignedOut
                    && let Some(sign_in) = a.sign_in.clone()
                {
                    line = line.control(self.small_button(
                        &format!("sign-in-{}", a.id),
                        "Sign in",
                        cx,
                        move |this, _, cx| this.run_in_terminal(&sign_in, cx),
                    ));
                }
                // With more than one, new sessions run the one picked here.
                if several && st == Standing::Ready {
                    if chosen == a.id {
                        line = line.value("New sessions");
                    } else {
                        let id = a.id.clone();
                        let owner = owner.clone();
                        line = line.control(self.small_button(
                            &format!("use-{}", a.id),
                            "Use for new sessions",
                            cx,
                            move |this, _, cx| {
                                // Its own models, so the model is one it has.
                                if this
                                    .setups
                                    .get(&owner)
                                    .is_some_and(|s| s.defaults.is_some())
                                {
                                    let mut d = this.defaults_for(&owner, None);
                                    d.agent = Some(id.clone());
                                    d.model = None;
                                    this.put_defaults(&owner, None, d, cx);
                                } else {
                                    // Keeps none (older, or not said yet):
                                    // this device's next sessions, as before.
                                    this.defaults.agent = Some(id.clone());
                                    this.defaults.model = None;
                                    this.last_used.agent = Some(id.clone());
                                    this.last_used.model = None;
                                    this.prefs_changed();
                                    cx.notify();
                                }
                            },
                        ));
                    }
                }
                line
            })
            .collect()
    }

    /// This Mac: Git, who commits, the GitHub CLI, starting at login and
    /// notifications.
    pub(crate) fn machine_lines(&mut self, machine: &str, cx: &mut Context<Self>) -> Vec<Line> {
        let here = machine.is_empty();
        let known = if here {
            self.machine_check
                .clone()
                .or_else(|| self.setups.get("").cloned())
        } else {
            self.setups.get(machine).cloned()
        };
        let Some(m) = known else {
            return vec![
                Line::new("machine-checking", self.machine_name(machine))
                    .mark(Some(Mark::Waiting))
                    .under("Checking…"),
            ];
        };
        let mut lines = Vec::new();
        lines.push(match &m.git {
            Some(v) => {
                Line::new("git", "Git").value(v.trim_start_matches("git version ").to_string())
            }
            None if here => Line::new("git", "Git")
                .mark(Some(Mark::Bad))
                .under("Not installed. Worktrees, checkpoints and commits need it.")
                .control(
                    self.small_button("install-git", "How to install", cx, |this, _, cx| {
                        this.open_outside(GIT_URL, cx)
                    }),
                ),
            None => Line::new("git", "Git")
                .mark(Some(Mark::Bad))
                .under("Not installed there. Worktrees, checkpoints and commits need it."),
        });
        if m.git.is_some() {
            lines.push(self.git_user_line(machine, &m, cx));
        }
        lines.push(match &m.gh {
            Gh::SignedIn { user } => Line::new("gh", "GitHub CLI").value(match user {
                Some(u) => format!("Signed in as {u}"),
                None => "Signed in".into(),
            }),
            Gh::SignedOut if here => Line::new("gh", "GitHub CLI")
                .mark(Some(Mark::Attention))
                .under("Signed out. Pull requests and their checks need it.")
                .control(
                    self.small_button("sign-in-gh", "Sign in", cx, |this, _, cx| {
                        this.run_in_terminal("gh auth login", cx)
                    }),
                ),
            Gh::SignedOut => Line::new("gh", "GitHub CLI")
                .mark(Some(Mark::Attention))
                .under("Signed out there. To sign in, run this on it:")
                .code("gh auth login"),
            Gh::Missing if here => Line::new("gh", "GitHub CLI")
                .mark(Some(Mark::Off))
                .under("Not installed. Pull requests and their checks need it.")
                .control(
                    self.small_button("install-gh", "How to install", cx, |this, _, cx| {
                        this.open_outside(GH_URL, cx)
                    }),
                ),
            Gh::Missing => Line::new("gh", "GitHub CLI")
                .mark(Some(Mark::Off))
                .under("Not installed there. Pull requests and their checks need it."),
        });
        lines.push(self.login_line(machine, &m, cx));
        lines.push(self.lan_line(machine, &m, cx));
        lines
    }

    /// Who commits are by, changed in place: the field holds until the
    /// machine says it's saved.
    fn git_user_line(&mut self, machine: &str, m: &Machine, cx: &mut Context<Self>) -> Line {
        let line = Line::new("git-user", "Commits by");
        let editing = self.git_user_edit.as_ref().filter(|e| e.machine == machine);
        if let Some(e) = editing {
            let (saving, error) = (e.sent.is_some(), e.error.clone());
            let line = match error {
                Some(why) => line.mark(Some(Mark::Bad)).under(why),
                None if saving => line.under("Saving…"),
                None => line.under("As Git writes it: Name <email>. Return saves, Esc cancels"),
            };
            return line
                .control(
                    self.field(&self.git_user_input, false)
                        .w(px(300.))
                        .h(px(crate::ds::size::CONTROL_SM)),
                )
                .control(
                    self.small_button("git-user-save", "Save", cx, |this, _, cx| {
                        this.save_git_user(cx)
                    }),
                );
        }
        let (now, there) = (m.git_user.clone(), machine.to_string());
        let change = self.small_button(
            "git-user-change",
            if now.is_some() { "Change" } else { "Set" },
            cx,
            move |this, window, cx| this.edit_git_user(&there, now.clone(), window, cx),
        );
        match &m.git_user {
            Some(who) => line.under(who.clone()).control(change),
            None => line
                .mark(Some(Mark::Attention))
                .under("Git has no name and email for commits, so committing fails")
                .control(change),
        }
    }

    fn login_line(&mut self, machine: &str, m: &Machine, cx: &mut Context<Self>) -> Line {
        let machine = machine.to_string();
        let line = Line::new("login-item", "Start at login")
            .under("Sessions carry on after a restart without opening dent");
        match &m.login_item_blocked {
            Some(why) => line.mark(Some(Mark::Off)).under(why.clone()),
            None => {
                let on = m.login_item;
                line.control(self.switch_for(
                    "login-item-switch",
                    on,
                    if on {
                        "Don't start at login"
                    } else {
                        "Start at login"
                    },
                    cx,
                    move |this, _, _| {
                        this.ask_machine(&machine, Request::SetLoginItem { on: !on });
                    },
                ))
            }
        }
    }

    fn lan_line(&mut self, machine: &str, m: &Machine, cx: &mut Context<Self>) -> Line {
        let on = m.lan;
        let here = machine.is_empty();
        let machine = machine.to_string();
        Line::new(
            "lan",
            if here {
                "Devices on this network"
            } else {
                "Devices on its network"
            },
        )
        .under(if on {
            "Can reach it without Tailscale, to pair with a code"
        } else {
            "Only over Tailscale, unless already paired"
        })
        .control(self.switch_for(
            "lan-switch",
            on,
            if on {
                "Only over Tailscale"
            } else {
                "Let devices on this network pair"
            },
            cx,
            move |this, _, _| {
                this.ask_machine(&machine, Request::SetLan { on: !on });
            },
        ))
    }

    /// Ask `machine` to change how it's set up; its answer says how it is
    /// now.
    pub(crate) fn ask_machine(&mut self, machine: &str, r: Request) {
        if machine.is_empty() {
            self.ask(r, Waiting::Machine);
        } else {
            let n = self.live.send_to(machine, r);
            self.waiting_insert(n, Waiting::MachineSetup(machine.to_string()));
        }
    }

    pub(crate) fn notification_line(&mut self, cx: &mut Context<Self>) -> Line {
        let line = Line::new("notifications", "Notifications");
        match crate::notify::allowed() {
            Some(true) => line.value("Allowed"),
            Some(false) => line
                .mark(Some(Mark::Attention))
                .under("Turned off for dent, so you won't hear when an agent needs you")
                .control(self.small_button(
                    "notifications-open",
                    "Open Settings",
                    cx,
                    |this, _, cx| {
                        this.open_outside(
                            "x-apple.systempreferences:com.apple.Notifications-Settings.extension",
                            cx,
                        )
                    },
                )),
            None => line.value("Only dent.app can notify"),
        }
    }

    /// The app's own choices, each what its menu item does.
    fn general_lines(&mut self, cx: &mut Context<Self>) -> Vec<Line> {
        use crate::prefs::Appearance;
        use std::sync::atomic::Ordering::Relaxed;
        let now = self.appearance;
        let look = |a: Appearance| match a {
            Appearance::System => "As the Mac",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        };
        let mut appearance = Line::new("appearance", "Appearance").under(look(now));
        for a in [Appearance::Light, Appearance::Dark, Appearance::System] {
            if a == now {
                continue;
            }
            appearance = appearance.control(self.small_button(
                &format!("appearance-{a:?}").to_lowercase(),
                look(a),
                cx,
                move |this, window, cx| this.set_appearance(a, window, cx),
            ));
        }
        let sound = crate::notify::SOUND.load(Relaxed);
        let awake = crate::notify::AWAKE.load(Relaxed);
        let hot = crate::hotkey::chosen();
        let mut shortcut =
            Line::new("hotkey", "New session from any app").under(hotkey_label(&hot));
        for (name, label) in HOTKEYS {
            if *name == hot {
                continue;
            }
            shortcut = shortcut.control(self.small_button(
                &format!("hotkey-{name}"),
                label,
                cx,
                move |this, _, cx| {
                    crate::hotkey::set(name);
                    this.prefs_changed();
                    crate::app::refresh_menus(cx);
                },
            ));
        }
        vec![
            appearance,
            Line::new("sound", "Notification sound").control(self.switch_for(
                "sound-switch",
                sound,
                if sound {
                    "Notify quietly"
                } else {
                    "Play a sound"
                },
                cx,
                |_, window, cx| window.dispatch_action(Box::new(crate::app::NotifySound), cx),
            )),
            Line::new("awake", "Keep the Mac awake while agents work")
                .under("Even with the lid closed, on power")
                .control(self.switch_for(
                    "awake-switch",
                    awake,
                    if awake {
                        "Let the Mac sleep"
                    } else {
                        "Keep it awake"
                    },
                    cx,
                    |_, window, cx| window.dispatch_action(Box::new(crate::app::KeepAwake), cx),
                )),
            Line::new("fold-turns", "Fold each finished turn's work")
                .under("Into one “Worked for 2m” line above its answer, opened with a click")
                .control(self.switch_for(
                    "fold-turns-switch",
                    self.fold_turns,
                    if self.fold_turns {
                        "Show every turn's work"
                    } else {
                        "Fold finished turns"
                    },
                    cx,
                    |this, _, cx| {
                        this.fold_turns = !this.fold_turns;
                        this.prefs_changed();
                        cx.notify();
                    },
                )),
            shortcut,
            Line::new("keys", "Keyboard shortcuts").control(self.small_button(
                "keys-open",
                "Show all",
                cx,
                |this, _, cx| {
                    this.pane = None;
                    this.toggle_keys(cx);
                },
            )),
        ]
    }

    /// One row, drawn as sheets draw theirs.
    fn draw_line(&self, l: Line) -> AnyElement {
        let t = self.t;
        let small = |(text, code): (String, bool)| {
            let d = div()
                .text_size(px(crate::cards::SMALL))
                .line_height(px(crate::cards::SMALL_LINE))
                .text_color(t.quiet)
                .line_clamp(3)
                .child(text);
            if code { d.ty(CODE) } else { d }
        };
        let group = |(value, who): (String, String)| {
            div()
                .mt(px(space::S1))
                .flex()
                .gap(px(space::S3))
                .text_size(px(crate::cards::SMALL))
                .line_height(px(crate::cards::SMALL_LINE))
                .child(
                    div()
                        .w(px(112.))
                        .flex_none()
                        .truncate()
                        .text_color(t.ink)
                        .font_weight(FontWeight::MEDIUM)
                        .child(value),
                )
                .child(div().flex_1().min_w_0().text_color(t.quiet).child(who))
        };
        let words = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(div().min_w_0().truncate().text_color(t.ink).child(l.label))
            .children(l.under.into_iter().map(small));
        // Under the whole row, so a narrow window's controls don't squeeze
        // the machines' names to a sliver.
        let groups = (!l.groups.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .children(l.groups.into_iter().map(group))
        });
        let trailing = if !l.controls.is_empty() {
            Some(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .children(l.controls)
                    .into_any_element(),
            )
        } else {
            l.value.map(|v| {
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(t.quiet)
                    .child(v)
                    .into_any_element()
            })
        };
        let line_h = crate::ds::size::LINE;
        let on_line = |d: AnyElement| {
            div()
                .h(px(line_h))
                .flex_none()
                .flex()
                .items_center()
                .child(d)
        };
        div()
            .id(SharedString::from(format!("row-{}", l.id)))
            .relative()
            .flex()
            .items_start()
            .gap(px(space::S3))
            .py(px(space::S1H))
            .when(l.indent, |d| d.pl(px(space::S4)))
            .ty(UI)
            .children(
                l.mark
                    .and_then(|m| crate::cards::mark_color(m, &t))
                    .map(|c| {
                        div()
                            .absolute()
                            .left(px(-space::S3))
                            .top(px(space::S1H))
                            .h(px(line_h))
                            .flex()
                            .items_center()
                            .child(ui::dot(c, &t))
                    }),
            )
            .child(words)
            .children(trailing.map(on_line))
            .child(probe(&self.probes, format!("row-{}", l.id)))
            .map(|row| match groups {
                Some(g) => div()
                    .flex()
                    .flex_col()
                    .child(row)
                    .child(div().pb(px(space::S1H)).child(g))
                    .into_any_element(),
                None => row.into_any_element(),
            })
    }

    pub(crate) fn section(
        &self,
        title: &str,
        note: Option<&str>,
        lines: Vec<Line>,
        first: bool,
    ) -> Div {
        let t = self.t;
        div()
            .flex()
            .flex_col()
            .when(!first, |d| d.mt(px(space::S4)))
            .child(ui::section_label(title, None, &t))
            .when_some(note, |d, n| {
                d.child(
                    div()
                        .pt(px(space::S1H))
                        .text_size(px(crate::cards::SMALL))
                        .line_height(px(crate::cards::SMALL_LINE))
                        .text_color(t.quiet)
                        .child(n.to_string()),
                )
            })
            .child(div().h(px(space::S2 - space::S1H)))
            .children(lines.into_iter().map(|l| self.draw_line(l)))
    }

    /// Settings, as a page of its own in place of the sessions: its pages
    /// down the left, the open one beside them.
    pub fn settings_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.pane != Some(Pane::Settings) {
            return None;
        }
        let t = self.t;
        let tab = self.settings_tab;
        let nav = div()
            .w(px(220.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(t.hairline)
            .px(px(space::S3))
            .pt(px(crate::ds::size::HEADER))
            .flex()
            .flex_col()
            .gap(px(space::HALF))
            .child(
                div()
                    .id("settings-back")
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .h(px(crate::ds::size::CONTROL))
                    .px(px(space::S2))
                    .mb(px(space::S4))
                    .rounded(px(radius::MD))
                    .ty(UI)
                    .text_color(t.quiet)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover()).text_color(t.ink))
                    .child(ui::icon("chevron-left", 14., t.quiet))
                    .child("Sessions")
                    .child(probe(&self.probes, "settings-back"))
                    .on_press(cx.listener(|this, _, _, cx| {
                        this.close_pane(cx);
                    })),
            )
            .child(
                div()
                    .px(px(space::S2))
                    .pb(px(space::S1H))
                    .child(ui::section_label("Settings", None, &t)),
            )
            .children(Tab::ALL.into_iter().map(|tb| {
                let on = tb == tab;
                div()
                    .id(SharedString::from(format!(
                        "settings-tab-{}",
                        tb.name().to_lowercase()
                    )))
                    .relative()
                    .flex()
                    .items_center()
                    .h(px(crate::ds::size::CONTROL))
                    .px(px(space::S2))
                    .rounded(px(radius::MD))
                    .ty(UI)
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(t.selected())
                            .text_color(t.ink)
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!on, |d| {
                        d.text_color(t.quiet)
                            .hover(|s| s.bg(t.hover()).text_color(t.ink))
                    })
                    .child(tb.name())
                    .child(probe(
                        &self.probes,
                        format!("settings-tab-{}", tb.name().to_lowercase()),
                    ))
                    .on_press(cx.listener(move |this, _, _, cx| {
                        this.open_tab(tb, cx);
                    }))
            }));
        let body: AnyElement = match tab {
            Tab::General => {
                let mut general = vec![self.notification_line(cx)];
                general.extend(self.general_lines(cx));
                self.section("This app", None, general, true)
                    .into_any_element()
            }
            Tab::Agents => {
                let room = window.viewport_size().width.as_f32() - 220. - space::S12 * 2.;
                self.agents_body(room.min(900. - space::S12 * 2.), cx)
            }
            Tab::Machines => self.machines_tab(window, cx),
        };
        let again = (tab != Tab::Machines).then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .when(self.checking, |d| {
                    d.child(div().ty(LABEL).text_color(t.faint).child("Checking…"))
                })
                .child(
                    ui::ghost("pane-again", "Check again", &t)
                        .relative()
                        .child(probe(&self.probes, "pane-again"))
                        .on_press(cx.listener(|this, _, _, cx| {
                            this.check_again();
                            cx.notify();
                        })),
                )
        });
        let head = (tab != Tab::Machines).then(|| {
            div()
                .flex()
                .items_end()
                .pb(px(space::S6))
                .child(
                    div()
                        .flex_1()
                        .ty(DISPLAY)
                        .text_color(t.ink)
                        .child(tab.name()),
                )
                .children(again)
        });
        Some(
            div()
                .id("settings-page")
                .flex_1()
                .min_h_0()
                .flex()
                .bg(t.surface)
                .child(nav)
                .child(
                    div()
                        .id("settings-body")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .restrict_scroll_to_axis()
                        .child(
                            div()
                                .max_w(px(900.))
                                .px(px(space::S12))
                                .pt(px(crate::ds::size::HEADER))
                                .pb(px(space::S12))
                                // Marks hang in the margin, left of the names.
                                .pl(px(space::S12 + space::S1))
                                .children(head)
                                .child(body),
                        ),
                )
                .child(probe(&self.probes, "pane-settings"))
                .into_any_element(),
        )
    }

    /// Show a page of Settings; each asks again how things stand.
    pub fn open_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.settings_tab = tab;
        if tab == Tab::Machines {
            self.ask(Request::Devices, Waiting::Devices);
            self.live
                .signal(None, dent_client_core::machine::Signal::Retry);
        }
        cx.notify();
    }

    /// The welcome, over everything, when open.
    pub fn pane_view(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pane = self.pane.filter(|p| *p == Pane::Welcome)?;
        let t = self.t;
        let agents = self.agent_lines("", cx);
        let mut machine = self.machine_lines("", cx);
        machine.push(self.notification_line(cx));
        let (title, intro) = match pane {
            Pane::Settings => ("Settings", None),
            Pane::Welcome => (
                "Welcome to dent",
                Some(
                    "dent drives the coding agents you already use, signed in as you, and keeps \
                     every session going when the app closes. Here's how this Mac stands.",
                ),
            ),
        };
        let mut body = div()
            .flex()
            .flex_col()
            .child(self.section("Agents", None, agents, true))
            .child(self.section("This Mac", None, machine, false));
        body = match pane {
            Pane::Settings => {
                let general = self.general_lines(cx);
                body.child(self.section("dent", None, general, false))
            }
            Pane::Welcome => {
                let mut l = self.general_lines(cx);
                l.retain(|l| l.id == "hotkey" || l.id == "awake");
                body.child(self.section(
                    "Worth knowing",
                    Some("All of this is in Settings later, with ⌘,"),
                    l,
                    false,
                ))
            }
        };
        let done = (pane == Pane::Welcome).then(|| {
            div().pt(px(space::S6)).flex().justify_end().child(
                ui::inner_button("welcome-done", "Get started", true, &t)
                    .relative()
                    .child(probe(&self.probes, "welcome-done"))
                    .on_press(cx.listener(|this, _, _, cx| {
                        this.close_pane(cx);
                    })),
            )
        });
        let again = div()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .when(self.checking, |d| {
                d.child(div().ty(LABEL).text_color(t.faint).child("Checking…"))
            })
            .child(
                ui::ghost("pane-again", "Check again", &t)
                    .relative()
                    .child(probe(&self.probes, "pane-again"))
                    .on_press(cx.listener(|this, _, _, cx| {
                        this.check_again();
                        cx.notify();
                    })),
            );
        Some(
            div()
                .id("pane-scrim")
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(if t.dark {
                    gpui_kit::black().opacity(0.45)
                } else {
                    t.ink.opacity(0.08)
                })
                .flex()
                .justify_center()
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_pane(cx);
                    }),
                )
                .child(
                    ui::floating(&t)
                        .id("pane")
                        .relative()
                        .w(px(600.))
                        .max_w(relative(0.92))
                        .max_h(relative(0.88))
                        .overflow_y_scroll()
                        .restrict_scroll_to_axis()
                        .rounded(px(radius::XL))
                        .p(px(space::S6))
                        // Marks hang in the margin, left of the names.
                        .pl(px(space::S6 + space::S1))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .pb(px(space::S4))
                                .child(
                                    div()
                                        .flex_1()
                                        .ty(UI)
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(t.ink)
                                        .child(title),
                                )
                                .child(again),
                        )
                        .when_some(intro, |d, text| {
                            d.child(
                                div()
                                    .pb(px(space::S4))
                                    .ty(UI)
                                    .text_color(t.quiet)
                                    .child(text),
                            )
                        })
                        .child(body)
                        .children(done)
                        .child(probe(&self.probes, format!("pane-{pane:?}").to_lowercase()))
                        .arrive("pane-arrive", From::Above, motion::FAST),
                )
                .with_animation(
                    "pane-scrim-in",
                    Animation::new(std::time::Duration::from_millis(motion::FAST as u64))
                        .with_easing(crate::anim::ease_out),
                    |d, p| d.opacity(p),
                )
                .into_any_element(),
        )
    }
}

/// The new-session shortcuts dent offers, as the menu does.
pub const HOTKEYS: &[(&str, &str)] = &[
    ("opt-shift-space", "⌥⇧Space"),
    ("ctrl-opt-cmd-n", "⌃⌥⌘N"),
    ("off", "Off"),
];

fn hotkey_label(name: &str) -> String {
    HOTKEYS
        .iter()
        .find(|(n, _)| *n == name)
        .map_or_else(|| name.to_string(), |(_, l)| (*l).to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_shortcut_choice_is_one_the_hotkey_takes() {
        for (name, _) in super::HOTKEYS {
            assert!(
                *name == "off" || crate::hotkey::DEFAULT == *name || name.contains('-'),
                "{name}"
            );
        }
        assert!(
            super::HOTKEYS
                .iter()
                .any(|(n, _)| *n == crate::hotkey::DEFAULT)
        );
    }
}
