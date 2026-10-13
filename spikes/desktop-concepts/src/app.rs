//! The window's state and everything that changes it. Rendering lives in
//! sidebar.rs, chat.rs and viewer.rs.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::data::{self, Artifact, Item, Session};
use crate::theme::{Status, Theme};
use crate::web::{PageEvent, Preview};

actions!(
    dent,
    [
        NewSession,
        CloseSession,
        UndoClose,
        NextNeedsYou,
        TogglePanel,
        ShowFirst,
        ShowChanges,
        ShowTerminal,
        Escape,
        StopSession,
        Search,
        SelectPrev,
        SelectNext,
        InboxUp,
        InboxDown,
        InboxOpen,
        InboxClose,
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewSession, None),
        KeyBinding::new("cmd-w", CloseSession, None),
        KeyBinding::new("cmd-z", UndoClose, Some("Dent")),
        KeyBinding::new("cmd-j", NextNeedsYou, None),
        KeyBinding::new("cmd-\\", TogglePanel, None),
        KeyBinding::new("cmd-1", ShowFirst, None),
        KeyBinding::new("cmd-2", ShowChanges, None),
        KeyBinding::new("cmd-3", ShowTerminal, None),
        KeyBinding::new("escape", Escape, None),
        KeyBinding::new("cmd-.", StopSession, None),
        KeyBinding::new("cmd-k", Search, None),
        KeyBinding::new("alt-cmd-up", SelectPrev, None),
        KeyBinding::new("alt-cmd-down", SelectNext, None),
        // With the inbox focused (Esc from an empty composer), it works like mail.
        KeyBinding::new("up", InboxUp, Some("Inbox")),
        KeyBinding::new("down", InboxDown, Some("Inbox")),
        KeyBinding::new("enter", InboxOpen, Some("Inbox")),
        KeyBinding::new("e", InboxClose, Some("Inbox")),
        KeyBinding::new("backspace", InboxClose, Some("Inbox")),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Art(&'static str),
    Changes,
    Terminal,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Viewport {
    Fill,
    Tablet,
    Phone,
}

impl Viewport {
    pub fn width(self) -> Option<f32> {
        match self {
            Viewport::Fill => None,
            Viewport::Tablet => Some(768.),
            Viewport::Phone => Some(390.),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Menu {
    Project,
    Machine,
    Model,
    Permission,
    Worktree,
    /// The open session's model and permissions, switchable mid-session.
    SessionModel,
    SessionPermission,
}

impl Menu {
    /// Menus under the composer open upward.
    pub fn upward(self) -> bool {
        matches!(self, Menu::SessionModel | Menu::SessionPermission)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum ToastAction {
    Undo(String),
    Resume(String),
}

#[derive(Clone)]
pub struct Toast {
    pub text: String,
    pub action: ToastAction,
    pub at: Instant,
}

/// Something riding along with the next message.
#[derive(Clone, PartialEq, Debug)]
pub enum Attachment {
    /// An element picked in the preview.
    Element {
        file: String,
        tag: String,
        text: String,
        selector: String,
    },
    /// An error from the preview's console.
    Error { file: String, message: String },
    /// A review comment on a changed line.
    Comment {
        file: String,
        line: u32,
        text: String,
    },
}

impl Attachment {
    pub fn label(&self) -> String {
        match self {
            Attachment::Element { tag, text, .. } if !text.is_empty() => format!("{tag} “{text}”"),
            Attachment::Element { tag, .. } => tag.clone(),
            Attachment::Error { message, .. } => message.clone(),
            Attachment::Comment { file, line, .. } => {
                format!("{}:{line}", file.rsplit('/').next().unwrap_or(file))
            }
        }
    }
    pub fn icon(&self) -> &'static str {
        match self {
            Attachment::Element { .. } => "square-dashed-mouse-pointer",
            Attachment::Error { .. } => "circle-alert",
            Attachment::Comment { .. } => "message-square",
        }
    }
}

/// Settings for a new session: chosen for you, each one click to change.
#[derive(Clone)]
pub struct Defaults {
    pub project: Option<&'static str>,
    /// Once you pick a project, typing stops changing it.
    pub project_picked: bool,
    pub machine: &'static str,
    pub model: &'static str,
    pub permission: &'static str,
    pub worktree: &'static str,
}

/// One scripted step of a fake agent.
#[derive(Clone)]
pub enum Step {
    Wait(u64),
    Detail(&'static str),
    Tools(&'static str, &'static [&'static str]),
    Say(String),
    Make(&'static str, Rc<[u8]>),
    Finish(String),
}

pub struct Dent {
    pub t: Theme,
    pub sessions: Vec<Session>,
    pub arts: Vec<Artifact>,
    /// The open session; None shows the new-session page.
    pub selected: Option<String>,
    pub focus: FocusHandle,
    pub inbox_focus: FocusHandle,
    pub inbox_focused: bool,
    pub palette: Option<usize>,
    pub search: Entity<TextareaState>,
    pub composer: Entity<TextareaState>,
    pub starter: Entity<TextareaState>,
    pub comment_input: Entity<TextareaState>,
    pub drafts: HashMap<String, String>,
    pub defaults: Defaults,
    pub menu: Option<Menu>,
    pub toast: Option<Toast>,
    pub show_closed: bool,
    pub stopped: HashSet<String>,
    pub finished_at: HashMap<String, Instant>,
    pub arrived_at: HashMap<String, Instant>,
    pub working_since: HashMap<String, Instant>,
    /// Birth time of each streamed word, by session: (item index, byte offset, time).
    pub births: HashMap<String, (usize, Vec<(usize, Instant)>)>,
    tasks: HashMap<String, Task<()>>,
    /// Send is inert for a moment after sending, so a double press can't stop.
    pub just_sent: Option<Instant>,
    pub panel_open: bool,
    pub panel_width: f32,
    /// This frame's widths: the viewer gives way before the chat does.
    pub panel_w: f32,
    pub sidebar_hidden: bool,
    /// The person opened the sidebar over a narrow window.
    pub sidebar_forced: bool,
    pub dragging: bool,
    pub tabs: HashMap<String, Tab>,
    pub preview: Option<Rc<RefCell<Preview>>>,
    pub viewing: HashMap<&'static str, usize>,
    pub viewport: Viewport,
    pub picking: bool,
    /// Console errors per artifact version.
    pub errors: HashMap<(&'static str, usize), Vec<String>>,
    pub updated_at: HashMap<&'static str, Instant>,
    pub attachments: Vec<Attachment>,
    pub diff_file: HashMap<String, usize>,
    pub commenting: Option<(String, u32)>,
    pub review: Vec<Attachment>,
    pub chat_scroll: ScrollHandle,
    pub probes: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    pub hovered_row: Option<String>,
    pub placeholder: &'static str,
    /// What the hovered preview control does, shown in the toolbar: tooltips
    /// would fall behind the live page.
    pub toolbar_hint: Option<&'static str>,
    pub image_actual: bool,
    pub copied_at: Option<Instant>,
    pub pace: f32,
    pub demo: Option<crate::demo::Demo>,
    pub last_frame: Instant,
}

impl Dent {
    pub fn new(
        dark: bool,
        pace: f32,
        demo: Option<crate::demo::Demo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let t = if dark {
            Theme::carbon()
        } else {
            Theme::paper()
        };
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .placeholder("Reply")
                .submit_on_enter(true)
        });
        let starter = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 14)
                .placeholder("Describe a task. dent picks the project as you type.")
                .submit_on_enter(true)
        });
        let comment_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .placeholder("Comment on this line")
                .submit_on_enter(true)
        });
        let search = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 1)
                .placeholder("Search sessions, or type a task to start one")
                .submit_on_enter(true)
        });
        cx.subscribe_in(
            &search,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => this.palette_open_selected(window, cx),
                InputEvent::Change if this.palette.is_some() => {
                    this.palette = Some(0);
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
        cx.observe(&composer, |_, _, cx| cx.notify()).detach();
        cx.observe(&starter, |_, _, cx| cx.notify()).detach();
        for input in [&composer, &starter, &comment_input, &search] {
            crate::style_input(input, &t, cx);
        }
        cx.subscribe_in(&composer, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.send(window, cx);
            }
        })
        .detach();
        cx.subscribe_in(
            &starter,
            window,
            |this, s, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => this.start(window, cx),
                InputEvent::Change => {
                    // The project follows what you type, a beat behind, until you pick one.
                    let text = s.read(cx).value().to_string();
                    cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(300))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            if !this.defaults.project_picked
                                && this.starter.read(cx).value() == text.as_str()
                            {
                                this.defaults.project = data::guess_project(&text);
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }
                _ => {}
            },
        )
        .detach();
        cx.subscribe_in(
            &comment_input,
            window,
            |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.add_comment(window, cx);
                }
            },
        )
        .detach();
        let mut this = Self {
            t,
            sessions: data::sessions(),
            arts: data::artifacts(),
            selected: Some("pricing".into()),
            focus: cx.focus_handle(),
            inbox_focus: cx.focus_handle(),
            inbox_focused: false,
            palette: None,
            search,
            composer,
            starter,
            comment_input,
            drafts: HashMap::new(),
            defaults: Defaults {
                project: None,
                project_picked: false,
                machine: "laptop",
                model: "Claude · Opus 5.5",
                permission: "Full access",
                worktree: "New worktree",
            },
            menu: None,
            toast: None,
            show_closed: false,
            stopped: HashSet::new(),
            finished_at: HashMap::new(),
            arrived_at: HashMap::new(),
            working_since: HashMap::new(),
            births: HashMap::new(),
            tasks: HashMap::new(),
            just_sent: None,
            panel_open: true,
            panel_width: 600.,
            panel_w: 600.,
            sidebar_hidden: false,
            sidebar_forced: false,
            dragging: false,
            tabs: HashMap::new(),
            preview: None,
            viewing: HashMap::new(),
            viewport: Viewport::Fill,
            picking: false,
            errors: HashMap::new(),
            updated_at: HashMap::new(),
            attachments: Vec::new(),
            diff_file: HashMap::new(),
            commenting: None,
            review: Vec::new(),
            chat_scroll: ScrollHandle::new(),
            probes: Rc::default(),
            hovered_row: None,
            placeholder: "",
            toolbar_hint: None,
            image_actual: false,
            copied_at: None,
            pace,
            demo,
            last_frame: Instant::now(),
        };
        let now = Instant::now();
        for s in &this.sessions {
            if s.status == Status::Working {
                this.working_since.insert(
                    s.id.clone(),
                    now - Duration::from_secs(if s.id == "deps" { 240 } else { 38 }),
                );
            }
        }
        this.preview = Preview::new(window).map(|p| Rc::new(RefCell::new(p)));
        this
    }

    // ---- Looking things up ----

    pub fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }
    fn session_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }
    pub fn current(&self) -> Option<&Session> {
        self.selected.as_deref().and_then(|id| self.session(id))
    }
    pub fn art(&self, id: &str) -> Option<&Artifact> {
        self.arts.iter().find(|a| a.id == id)
    }

    fn open(&self) -> impl Iterator<Item = &Session> {
        self.sessions.iter().filter(|s| !s.closed)
    }
    pub fn needs(&self) -> Vec<&Session> {
        self.open().filter(|s| s.status == Status::Needs).collect()
    }
    pub fn done(&self) -> Vec<&Session> {
        self.open()
            .filter(|s| matches!(s.status, Status::Done | Status::Idle))
            .collect()
    }
    pub fn working(&self) -> Vec<&Session> {
        self.open()
            .filter(|s| matches!(s.status, Status::Working | Status::Waiting))
            .collect()
    }
    pub fn closed(&self) -> Vec<&Session> {
        self.sessions.iter().filter(|s| s.closed).collect()
    }
    /// Inbox order: what needs you, then what's done.
    fn to_clear(&self) -> Vec<String> {
        self.needs()
            .into_iter()
            .chain(self.done())
            .map(|s| s.id.clone())
            .collect()
    }
    pub fn can_close(s: &Session) -> bool {
        !s.closed && matches!(s.status, Status::Done | Status::Idle)
    }

    pub fn tab(&self, s: &Session) -> Tab {
        self.tabs
            .get(&s.id)
            .copied()
            .or_else(|| s.arts.last().map(|a| Tab::Art(a)))
            .unwrap_or(if s.diff.is_empty() {
                Tab::Terminal
            } else {
                Tab::Changes
            })
    }
    pub fn version(&self, art: &Artifact) -> usize {
        self.viewing
            .get(art.id)
            .copied()
            .unwrap_or(art.versions.len())
            .min(art.versions.len())
            .max(1)
    }

    // ---- Selecting and closing ----

    pub fn select(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        // Each session keeps its own draft.
        let text = self.composer.read(cx).value().to_string();
        if let Some(old) = &self.selected {
            self.drafts.insert(old.clone(), text);
        }
        let draft = id
            .as_ref()
            .and_then(|i| self.drafts.get(i))
            .cloned()
            .unwrap_or_default();
        self.composer
            .update(cx, |c, cx| c.set_value(draft, window, cx));
        self.selected = id;
        self.menu = None;
        self.picking = false;
        self.attachments.clear();
        self.commenting = None;
        self.chat_scroll.scroll_to_bottom();
        match &self.selected {
            Some(_) => self.composer.update(cx, |c, cx| c.focus(window, cx)),
            None => self.starter.update(cx, |c, cx| c.focus(window, cx)),
        }
        cx.notify();
    }

    /// Closing archives: it can always be reopened. The next thing to clear opens.
    pub fn close(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.session(id) else { return };
        if !Self::can_close(s) {
            return;
        }
        let order = self.to_clear();
        let at = order.iter().position(|x| x == id).unwrap_or(0);
        if let Some(s) = self.session_mut(id) {
            s.closed = true;
        }
        self.stopped.remove(id);
        self.toast(ToastAction::Undo(id.to_string()), "Session closed".into());
        if self.selected.as_deref() == Some(id) {
            let rest: Vec<String> = order.into_iter().filter(|x| x != id).collect();
            let next = rest.get(at.min(rest.len().saturating_sub(1))).cloned();
            self.select(next, window, cx);
        }
        cx.notify();
    }

    pub fn reopen(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(s) = self.session_mut(id) {
            s.closed = false;
        }
        self.arrived_at.insert(id.to_string(), Instant::now());
        cx.notify();
    }

    fn toast(&mut self, action: ToastAction, text: String) {
        self.toast = Some(Toast {
            text,
            action,
            at: Instant::now(),
        });
    }

    pub fn toast_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.toast.take() else { return };
        match t.action {
            ToastAction::Undo(id) => {
                self.reopen(&id, cx);
                self.select(Some(id), window, cx);
            }
            ToastAction::Resume(id) => self.resume(&id, cx),
        }
    }

    pub fn next_needs_you(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let needs: Vec<String> = self.needs().iter().map(|s| s.id.clone()).collect();
        if needs.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|c| needs.iter().position(|x| x == c));
        let next = needs[at.map_or(0, |i| (i + 1) % needs.len())].clone();
        self.select(Some(next), window, cx);
    }

    // ---- Sending, starting, stopping ----

    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        let atts = std::mem::take(&mut self.attachments);
        let message = with_attachments(&text, &atts);
        self.composer
            .update(cx, |c, cx| c.set_value("", window, cx));
        self.drafts.remove(&id);
        self.just_sent = Some(Instant::now());
        let Some(s) = self.session(&id) else { return };
        if matches!(s.status, Status::Working) {
            // It waits its turn, below the reply in progress.
            if let Some(s) = self.session_mut(&id) {
                s.items.push(Item::Queued(message));
            }
            cx.notify();
            return;
        }
        if let Some(s) = self.session_mut(&id) {
            s.items.push(Item::User(message));
        }
        let script = self.reply_to(&id, &text, &atts);
        self.run(&id, script, cx);
        self.chat_scroll.scroll_to_bottom();
    }

    /// What the fake agent does with a message.
    fn reply_to(&self, id: &str, text: &str, atts: &[Attachment]) -> Vec<Step> {
        let has = |f: fn(&Attachment) -> bool| atts.iter().any(f);
        if id == "pricing" && has(|a| matches!(a, Attachment::Error { .. })) {
            return vec![
                Step::Detail("Reading site/pricing.html"),
                Step::Wait(700),
                Step::Tools("Read site/pricing.html, edited it", &["Read site/pricing.html", "Edited site/pricing.html · +9 −2"]),
                Step::Say("`formatPrices` was called before it existed. I replaced it with the toggle handler, which updates every price in place.".into()),
                Step::Make("pricing", data::pricing(3)),
                Step::Finish("Fixed the toggle · v3".into()),
            ];
        }
        if id == "pricing" && has(|a| matches!(a, Attachment::Element { .. })) {
            return vec![
                Step::Detail("Editing site/pricing.html"),
                Step::Wait(600),
                Step::Tools("Edited site/pricing.html", &["Edited site/pricing.html · +4 −1"]),
                Step::Say("The Pro button is now the filled brand colour, taller, and says “Start your 14-day trial”.".into()),
                Step::Make("pricing", data::pricing(4)),
                Step::Finish("Bigger trial button · v4".into()),
            ];
        }
        if has(|a| matches!(a, Attachment::Comment { .. })) {
            return vec![
                Step::Detail("Addressing review comments"),
                Step::Wait(700),
                Step::Tools("Edited 1 file", &["Edited src/middleware/limit.rs · +3 −1"]),
                Step::Say("Done: the window is now a constant with a doc comment, and the comment's example matches the code.".into()),
                Step::Finish("Review comments addressed".into()),
            ];
        }
        let _ = text;
        vec![
            Step::Detail("Thinking"),
            Step::Wait(500),
            Step::Tools("Read 2 files", &["Read README.md", "Read src/lib.rs"]),
            Step::Say(ON_IT.into()),
            Step::Finish("Replied".into()),
        ]
    }

    /// Start a new session from the starter box with the chosen defaults.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.starter.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let n = self.sessions.len();
        let id = format!("new-{n}");
        let d = self.defaults.clone();
        let s = Session {
            id: id.clone(),
            title: title_from(&text),
            project: d.project.unwrap_or("No project").to_string(),
            machine: d.machine.to_string(),
            agent: d.model.to_string(),
            permission: d.permission.to_string(),
            status: Status::Working,
            detail: "Starting".into(),
            age: "now".into(),
            closed: false,
            items: vec![Item::User(text)],
            arts: Vec::new(),
            diff: Vec::new(),
            terminal: Vec::new(),
        };
        self.sessions.insert(0, s);
        self.arrived_at.insert(id.clone(), Instant::now());
        self.starter.update(cx, |c, cx| c.set_value("", window, cx));
        self.defaults.project = None;
        self.defaults.project_picked = false;
        self.select(Some(id.clone()), window, cx);
        let script = vec![
            Step::Detail("Reading the repo"),
            Step::Wait(900),
            Step::Tools(
                "Read 3 files",
                &["Read README.md", "Read package.json", "Read src/index.ts"],
            ),
            Step::Detail("Planning"),
            Step::Say(ON_IT.into()),
            Step::Finish("Plan ready for review".into()),
        ];
        self.run(&id, script, cx);
    }

    pub fn run(&mut self, id: &str, script: Vec<Step>, cx: &mut Context<Self>) {
        let id = id.to_string();
        self.stopped.remove(&id);
        if self
            .toast
            .as_ref()
            .is_some_and(|t| t.action == ToastAction::Resume(id.clone()))
        {
            self.toast = None;
        }
        self.working_since.insert(id.clone(), Instant::now());
        if let Some(s) = self.session_mut(&id) {
            s.status = Status::Working;
            s.age = "now".into();
        }
        let pace = self.pace;
        let key = id.clone();
        let task = cx.spawn(async move |this, cx| {
            for step in script {
                match step {
                    Step::Wait(ms) => {
                        cx.background_executor()
                            .timer(Duration::from_millis((ms as f32 * pace) as u64))
                            .await;
                    }
                    Step::Say(text) => {
                        let ix = this
                            .update(cx, |this, cx| {
                                let ix = this
                                    .session_mut(&id)
                                    .map(|s| insert_reply(s, Item::Text(String::new())));
                                if let Some(ix) = ix {
                                    this.births.insert(id.clone(), (ix, Vec::new()));
                                }
                                cx.notify();
                                ix
                            })
                            .ok()
                            .flatten();
                        let Some(ix) = ix else { return };
                        let words: Vec<String> =
                            text.split_inclusive(' ').map(String::from).collect();
                        for w in words {
                            cx.background_executor()
                                .timer(Duration::from_millis((38. * pace) as u64))
                                .await;
                            if this
                                .update(cx, |this, cx| {
                                    if let Some(Item::Text(t)) =
                                        this.session_mut(&id).and_then(|s| s.items.get_mut(ix))
                                    {
                                        let at = t.len();
                                        t.push_str(&w);
                                        if let Some((_, b)) = this.births.get_mut(&id) {
                                            b.push((at, Instant::now()));
                                        }
                                    }
                                    this.follow();
                                    cx.notify();
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    step => {
                        let wait = matches!(step, Step::Tools(..) | Step::Make(..));
                        if this
                            .update(cx, |this, cx| this.apply(&id, step, cx))
                            .is_err()
                        {
                            return;
                        }
                        if wait {
                            cx.background_executor()
                                .timer(Duration::from_millis((500. * pace) as u64))
                                .await;
                        }
                    }
                }
            }
            let _ = this.update(cx, |this, cx| this.finished(&id, cx));
        });
        self.tasks.insert(key, task);
        cx.notify();
    }

    fn apply(&mut self, id: &str, step: Step, cx: &mut Context<Self>) {
        let now = Instant::now();
        match step {
            Step::Detail(d) => {
                if let Some(s) = self.session_mut(id) {
                    s.detail = d.into();
                }
            }
            Step::Tools(summary, lines) => {
                if let Some(s) = self.session_mut(id) {
                    insert_reply(
                        s,
                        Item::Tools {
                            summary: summary.into(),
                            lines: lines.iter().map(|l| l.to_string()).collect(),
                            open: false,
                        },
                    );
                }
            }
            Step::Make(art, bytes) => {
                let v = {
                    let Some(a) = self.arts.iter_mut().find(|a| a.id == art) else {
                        return;
                    };
                    a.versions.push(bytes);
                    a.versions.len()
                };
                // Following the latest: a new version shows straight away.
                self.viewing.remove(art);
                self.updated_at.insert(art, now);
                if let Some(s) = self.session_mut(id) {
                    if !s.arts.contains(&art) {
                        s.arts.push(art);
                    }
                    insert_reply(s, Item::Made { art, version: v });
                }
            }
            Step::Finish(detail) => {
                if let Some(s) = self.session_mut(id) {
                    s.detail = detail;
                }
            }
            Step::Wait(_) | Step::Say(_) => {}
        }
        self.follow();
        cx.notify();
    }

    fn finished(&mut self, id: &str, cx: &mut Context<Self>) {
        self.tasks.remove(id);
        self.births.remove(id);
        self.working_since.remove(id);
        // A queued message runs next.
        let queued = self.session_mut(id).and_then(|s| {
            let at = s.items.iter().position(|i| matches!(i, Item::Queued(_)))?;
            let Item::Queued(text) = s.items.remove(at) else {
                return None;
            };
            s.items.push(Item::User(text.clone()));
            Some(text)
        });
        if let Some(text) = queued {
            let script = self.reply_to(id, &text, &[]);
            self.run(id, script, cx);
            return;
        }
        if let Some(s) = self.session_mut(id)
            && s.status == Status::Working
        {
            s.status = Status::Done;
            s.age = "now".into();
        }
        self.finished_at.insert(id.to_string(), Instant::now());
        cx.notify();
    }

    pub fn stop(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(s) = self.session(id) else { return };
        if s.status != Status::Working {
            return;
        }
        // Dropping the task cancels it: nothing more arrives.
        self.tasks.remove(id);
        self.births.remove(id);
        self.working_since.remove(id);
        self.stopped.insert(id.to_string());
        if let Some(s) = self.session_mut(id) {
            s.status = Status::Idle;
            s.detail = "Stopped · Resume picks up where it left off".into();
            // Queued messages were never sent.
            for item in s.items.iter_mut() {
                if let Item::Queued(t) = item {
                    *item = Item::Note {
                        icon: "circle-slash",
                        text: format!("Not sent: {t}"),
                    };
                }
            }
            s.items.push(Item::Note {
                icon: "square",
                text: "Stopped".into(),
            });
        }
        self.toast(ToastAction::Resume(id.to_string()), "Stopped".into());
        cx.notify();
    }

    pub fn resume(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.stopped.contains(id) {
            return;
        }
        if let Some(s) = self.session_mut(id) {
            s.items.push(Item::Note {
                icon: "play",
                text: "Resumed".into(),
            });
        }
        let script = vec![
            Step::Detail("Picking up where it left off"),
            Step::Wait(500),
            Step::Say(
                "Picking up where I stopped: the rest of the change is in, and the tests pass."
                    .into(),
            ),
            Step::Finish("Finished after resuming".into()),
        ];
        self.run(id, script, cx);
    }

    pub fn allow(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(s) = self.session_mut(id) else {
            return;
        };
        let Some(at) = s
            .items
            .iter()
            .rposition(|i| matches!(i, Item::Approval { .. }))
        else {
            return;
        };
        let Item::Approval { command, .. } = s.items[at].clone() else {
            return;
        };
        s.items[at] = Item::Note {
            icon: "check",
            text: format!("Allowed {command}"),
        };
        let script = vec![
            Step::Detail("Running cargo test -p auth"),
            Step::Wait(900),
            Step::Tools("Ran cargo test -p auth · 14 passed", &["cargo test -p auth", "test result: ok. 14 passed; 0 failed"]),
            Step::Say("All 14 tests pass, including the two new ones: the sixth try in a minute gets a **429** with `Retry-After`, and a different IP still gets through.".into()),
            Step::Finish("Rate limiting is in · 14 tests pass".into()),
        ];
        self.run(id, script, cx);
    }

    pub fn deny(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(s) = self.session_mut(id) else {
            return;
        };
        let Some(at) = s
            .items
            .iter()
            .rposition(|i| matches!(i, Item::Approval { .. }))
        else {
            return;
        };
        let Item::Approval { command, .. } = s.items[at].clone() else {
            return;
        };
        s.items[at] = Item::Note {
            icon: "x",
            text: format!("Denied {command}"),
        };
        s.status = Status::Idle;
        s.detail = "Waiting for your next message".into();
        cx.notify();
    }

    pub fn answer(&mut self, id: &str, option: String, cx: &mut Context<Self>) {
        let Some(s) = self.session_mut(id) else {
            return;
        };
        let Some(at) = s
            .items
            .iter()
            .rposition(|i| matches!(i, Item::Question { .. }))
        else {
            return;
        };
        s.items[at] = Item::Note {
            icon: "corner-down-right",
            text: format!("You chose {option}"),
        };
        let script = vec![
            Step::Detail("Applying the layout"),
            Step::Wait(700),
            Step::Tools(
                "Edited 2 files",
                &[
                    "Edited site/changelog.html · +44 −61",
                    "Edited site/styles.css · +18 −30",
                ],
            ),
            Step::Say(format!(
                "Kept **{option}** and removed the other layout. Older releases fold under each year so the page stays short."
            )),
            Step::Finish(format!("{option} layout kept")),
        ];
        self.run(id, script, cx);
    }

    // ---- The viewer ----

    pub fn show_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if let Some(id) = self.selected.clone() {
            self.tabs.insert(id, tab);
        }
        self.panel_open = true;
        self.picking = false;
        cx.notify();
    }

    pub fn open_art(&mut self, art: &'static str, version: Option<usize>, cx: &mut Context<Self>) {
        match version {
            Some(v) if self.art(art).is_some_and(|a| v < a.versions.len()) => {
                self.viewing.insert(art, v);
            }
            _ => {
                self.viewing.remove(art);
            }
        }
        self.show_tab(Tab::Art(art), cx);
    }

    pub fn toggle_pick(&mut self, cx: &mut Context<Self>) {
        self.picking = !self.picking;
        if let Some(p) = &self.preview {
            p.borrow().pick(self.picking);
        }
        cx.notify();
    }

    /// Page events arrive from the web view; turn them into state.
    pub fn drain_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.preview else { return };
        let (events, showing) = {
            let p = p.borrow();
            (p.drain(), p.showing.clone())
        };
        if events.is_empty() {
            return;
        }
        let art = self.current().and_then(|s| match self.tab(s) {
            Tab::Art(a) => Some(a),
            _ => None,
        });
        for ev in events {
            match ev {
                PageEvent::Loaded => {}
                PageEvent::Error(msg) => {
                    if let (Some(a), Some((_, v))) = (art, &showing) {
                        let list = self.errors.entry((a, *v)).or_default();
                        if !list.contains(&msg) {
                            list.push(msg);
                        }
                        // A broken latest page needs you, like any other error.
                        let latest = self.art(a).map(|x| x.versions.len()) == Some(*v);
                        if latest
                            && let Some(s) = self.sessions.iter_mut().find(|s| {
                                s.arts.contains(&a)
                                    && matches!(s.status, Status::Done | Status::Idle)
                            })
                        {
                            s.status = Status::Needs;
                            s.detail = "The page has an error".into();
                        }
                    }
                }
                PageEvent::Pick {
                    tag,
                    text,
                    selector,
                } => {
                    self.picking = false;
                    let file = art
                        .and_then(|a| self.art(a))
                        .map(|a| a.path.to_string())
                        .unwrap_or_default();
                    self.attachments
                        .retain(|a| !matches!(a, Attachment::Element { .. }));
                    self.attachments.push(Attachment::Element {
                        file,
                        tag,
                        text,
                        selector,
                    });
                    self.composer.update(cx, |c, cx| c.focus(window, cx));
                }
            }
        }
        cx.notify();
    }

    /// Send the preview's error to the agent, straight away.
    pub fn ask_to_fix(
        &mut self,
        art: &'static str,
        v: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(msg) = self.errors.get(&(art, v)).and_then(|e| e.first()).cloned() else {
            return;
        };
        let file = self
            .art(art)
            .map(|a| a.path.to_string())
            .unwrap_or_default();
        self.attachments
            .retain(|a| !matches!(a, Attachment::Error { .. }));
        self.attachments
            .push(Attachment::Error { file, message: msg });
        self.composer
            .update(cx, |c, cx| c.set_value("Fix this error.", window, cx));
        self.send(window, cx);
    }

    pub fn start_comment(
        &mut self,
        file: String,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commenting = Some((file, line));
        self.comment_input.update(cx, |c, cx| {
            c.set_value("", window, cx);
            c.focus(window, cx);
        });
        cx.notify();
    }

    pub fn add_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.comment_input.read(cx).value().trim().to_string();
        let Some((file, line)) = self.commenting.take() else {
            return;
        };
        if !text.is_empty() {
            self.review.push(Attachment::Comment { file, line, text });
        }
        self.comment_input
            .update(cx, |c, cx| c.set_value("", window, cx));
        cx.notify();
    }

    pub fn send_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.review.is_empty() {
            return;
        }
        let n = self.review.len();
        self.attachments.extend(std::mem::take(&mut self.review));
        self.composer.update(cx, |c, cx| {
            c.set_value(
                if n == 1 {
                    "Please address this comment."
                } else {
                    "Please address these comments."
                },
                window,
                cx,
            )
        });
        self.send(window, cx);
    }

    // ---- Keyboard and search ----

    /// Every open session in inbox order, then what's working.
    pub fn order(&self) -> Vec<String> {
        self.needs()
            .into_iter()
            .chain(self.done())
            .chain(self.working())
            .map(|s| s.id.clone())
            .collect()
    }

    pub fn step_selection(
        &mut self,
        by: isize,
        keep_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let order = self.order();
        if order.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|c| order.iter().position(|x| x == c));
        let next = match at {
            Some(i) => (i as isize + by).clamp(0, order.len() as isize - 1) as usize,
            None => 0,
        };
        self.select(Some(order[next].clone()), window, cx);
        if keep_focus {
            window.focus(&self.inbox_focus, cx);
        }
    }

    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.palette = Some(0);
        self.search.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        match &self.selected {
            Some(_) => self.composer.update(cx, |c, cx| c.focus(window, cx)),
            None => self.starter.update(cx, |c, cx| c.focus(window, cx)),
        }
        cx.notify();
    }

    /// Sessions matching the query, closed ones included, best first.
    pub fn palette_results(&self, cx: &App) -> Vec<String> {
        let q = self.search.read(cx).value().to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        let mut hits: Vec<(usize, String)> = self
            .sessions
            .iter()
            .filter_map(|s| {
                let hay =
                    format!("{} {} {} {}", s.title, s.project, s.machine, s.detail).to_lowercase();
                let score = words.iter().filter(|w| hay.contains(*w)).count();
                (words.is_empty() || score == words.len())
                    .then(|| (score + usize::from(!s.closed) * 10, s.id.clone()))
            })
            .collect();
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
        hits.into_iter().take(8).map(|(_, id)| id).collect()
    }

    pub fn palette_open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_none() {
            return;
        }
        let results = self.palette_results(cx);
        let at = self.palette.unwrap_or(0);
        let query = self.search.read(cx).value().trim().to_string();
        self.palette = None;
        if let Some(id) = results.get(at).cloned() {
            if self.session(&id).is_some_and(|s| s.closed) {
                self.reopen(&id, cx);
            }
            self.select(Some(id), window, cx);
        } else if !query.is_empty() {
            // Nothing matched: start a session with what was typed.
            self.select(None, window, cx);
            self.starter.update(cx, |s, cx| {
                s.set_value(query.clone(), window, cx);
                s.focus(window, cx);
            });
            if !self.defaults.project_picked {
                self.defaults.project = data::guess_project(&query);
            }
        } else {
            self.close_palette(window, cx);
        }
        cx.notify();
    }

    /// Keep the newest line in view while following the conversation.
    fn follow(&mut self) {
        self.chat_scroll.scroll_to_bottom();
    }

    pub fn animating(&self) -> bool {
        let fresh = |m: &HashMap<String, Instant>| {
            m.values().any(|t| t.elapsed() < Duration::from_millis(700))
        };
        !self.tasks.is_empty()
            || fresh(&self.finished_at)
            || fresh(&self.arrived_at)
            || self
                .updated_at
                .values()
                .any(|t| t.elapsed() < Duration::from_secs(3))
            || self.dragging
    }
}

pub const ON_IT: &str = "On it. I read the code around this first, and I'll make the change in small steps you can review. I'll show anything visual in the viewer as soon as it renders.";

/// Replies land above anything queued, which always stays last.
fn insert_reply(s: &mut Session, item: Item) -> usize {
    let at = s
        .items
        .iter()
        .position(|i| matches!(i, Item::Queued(_)))
        .unwrap_or(s.items.len());
    s.items.insert(at, item);
    at
}

fn title_from(text: &str) -> String {
    let first = text.lines().next().unwrap_or(text);
    let mut t: String = first.chars().take(60).collect();
    if first.chars().count() > 60 {
        t = t.trim_end().to_string() + "…";
    }
    let mut c = t.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => t,
    }
}

fn with_attachments(text: &str, atts: &[Attachment]) -> String {
    let mut lines: Vec<String> = atts
        .iter()
        .map(|a| match a {
            Attachment::Element {
                file, tag, text, ..
            } => format!("› {file} · {tag} “{text}”"),
            Attachment::Error { file, message } => format!("› {file} · {message}"),
            Attachment::Comment { file, line, text } => format!("› {file}:{line} · {text}"),
        })
        .collect();
    if !text.is_empty() {
        lines.push(text.to_string());
    }
    lines.join("\n")
}
