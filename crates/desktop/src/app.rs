//! The window's state and everything that changes it. Rendering lives in
//! sidebar.rs, chat.rs and viewer.rs.

use crate::ui::FocusField;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::base::input::{InlineToken, InputEvent, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use dent_client_core::Sessions;
use dent_protocol::{
    Answer, Command, Model, Out, Permission, Project, RecentSession, Reply, Request,
};

use crate::complete;
use crate::data::{Artifact, FileDiff, Item, Session};
use crate::live::{Incoming, Live};
use crate::markdown::Block;
use crate::theme::{Status, Theme};
use crate::web::{PageEvent, Preview, Source};

actions!(
    dent,
    [
        NewSession,
        FindMine,
        SendAndNew,
        StartAside,
        AttachFiles,
        CloseSession,
        UndoClose,
        NextNeedsYou,
        TogglePanel,
        ShowFirst,
        ShowChanges,
        ShowTerminal,
        Escape,
        StopSession,
        SteerSend,
        Search,
        SelectPrev,
        SelectNext,
        InboxUp,
        InboxDown,
        InboxOpen,
        InboxClose,
        SideChat,
        FindInSession,
        FindNext,
        FindPrev,
        SwitchNext,
        SwitchPrev,
        TextBigger,
        TextSmaller,
        TextActual,
        LookSystem,
        LookLight,
        LookDark,
        ShowKeys,
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        ToggleSidebar,
        ClearTerminal,
        ChatPageUp,
        ChatPageDown,
        ChatTop,
        ChatBottom,
        NotifySound,
        KeepAwake,
        HotkeyOptShiftSpace,
        HotkeyCtrlOptCmdN,
        HotkeyOff,
        FullScreen,
        About,
    ]
);

/// The menu bar: the app menu, Edit for text fields, and Window.
pub fn set_menus(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &About, _| crate::notify::about());
    refresh_menus(cx);
}

/// The menus as things stand: the sound's tick follows your choice.
pub fn refresh_menus(cx: &mut App) {
    use gpui_kit::base::input;
    let sound = crate::notify::SOUND.load(std::sync::atomic::Ordering::Relaxed);
    let awake = crate::notify::AWAKE.load(std::sync::atomic::Ordering::Relaxed);
    cx.set_menus([
        gpui_kit::Menu {
            name: "dent".into(),
            items: vec![
                MenuItem::action("About dent", About),
                MenuItem::separator(),
                MenuItem::action("Notification Sound", NotifySound).checked(sound),
                MenuItem::action("Keep Mac Awake While Working", KeepAwake).checked(awake),
                MenuItem::submenu(gpui_kit::Menu {
                    name: "Global Shortcut".into(),
                    items: {
                        let now = crate::hotkey::chosen();
                        vec![
                            MenuItem::action("⌥⇧Space: New Session", HotkeyOptShiftSpace)
                                .checked(now == "opt-shift-space"),
                            MenuItem::action("⌃⌥⌘N: New Session", HotkeyCtrlOptCmdN)
                                .checked(now == "ctrl-opt-cmd-n"),
                            MenuItem::action("Off", HotkeyOff).checked(now == "off"),
                        ]
                    },
                    disabled: false,
                }),
                MenuItem::separator(),
                MenuItem::action("Hide dent", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit dent", Quit),
            ],
            disabled: false,
        },
        gpui_kit::Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Session", NewSession),
                MenuItem::action("Attach Files…", AttachFiles),
                MenuItem::action("Search Sessions…", Search),
                MenuItem::separator(),
                MenuItem::action("Close Session", CloseSession),
            ],
            disabled: false,
        },
        gpui_kit::Menu {
            name: "Edit".into(),
            items: vec![
                // Tied to the system's own edit actions too, so a page in
                // the viewer copies and pastes like any web view.
                MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action("Find…", FindInSession),
                MenuItem::action("Find Next", FindNext),
                MenuItem::action("Find Previous", FindPrev),
            ],
            disabled: false,
        },
        gpui_kit::Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Show or Hide Sidebar", ToggleSidebar),
                MenuItem::action("Show or Hide Viewer", TogglePanel),
                MenuItem::action("Changes", ShowChanges),
                MenuItem::action("Terminal", ShowTerminal),
                MenuItem::action("Side Chat", SideChat),
                MenuItem::separator(),
                MenuItem::action("Bigger Text", TextBigger),
                MenuItem::action("Smaller Text", TextSmaller),
                MenuItem::action("Actual Text Size", TextActual),
                MenuItem::separator(),
                MenuItem::action("Match the Mac's Appearance", LookSystem),
                MenuItem::action("Light Appearance", LookLight),
                MenuItem::action("Dark Appearance", LookDark),
                MenuItem::separator(),
                crate::ambient::menu(),
            ],
            disabled: false,
        },
        gpui_kit::Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
                MenuItem::separator(),
                MenuItem::action("Enter Full Screen", FullScreen),
            ],
            disabled: false,
        },
        gpui_kit::Menu {
            name: "Help".into(),
            items: vec![MenuItem::action("Keyboard Shortcuts", ShowKeys)],
            disabled: false,
        },
    ]);
}

/// Every key binding, from the one key registry (`keys::KEYS`).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys(crate::keys::bindings());
}

/// A thought's key for being opened: its session and which thought of
/// that session it is. Thoughts are only ever added at the end (a rewind
/// drops later ones), so the count before it doesn't change, and two that
/// start alike, here or in another session, open on their own.
pub fn thought_key(session: &str, nth: usize) -> String {
    format!("thought:{session}:{nth}")
}

/// What keeps a command's answer opened or folded.
pub fn command_key(name: &str, text: &str) -> String {
    format!(
        "command:{name}:{}:{}",
        text.len(),
        text.chars().take(48).collect::<String>()
    )
}

/// Make a field one line: a typed or pasted line break becomes a space, and
/// any Return submits. For names and searches.
pub fn single_line(field: &Entity<TextareaState>, window: &mut Window, cx: &mut App) {
    window
        .subscribe(field, cx, |input, ev: &InputEvent, window, cx| {
            if let InputEvent::Change = ev {
                let v = input.read(cx).value().to_string();
                if v.contains('\n') {
                    let one = v.replace('\n', " ");
                    input.update(cx, |i, cx| put(i, one, window, cx));
                }
            }
        })
        .detach();
}

/// Put text in a field with the caret after it, as typing would leave it.
pub fn put(
    field: &mut TextareaState,
    text: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut Context<TextareaState>,
) {
    // The field's own `set_value` tells no one, so this says it changed,
    // as typing would: whatever follows the field (a menu, a search) never
    // misses it. A prompt's words go in with `draft::put_in` instead, which
    // keeps its chips.
    let text: SharedString = text.into();
    let end = text.len();
    field.set_value(text, window, cx);
    field.set_selected_range(end..end, cx);
    cx.emit(InputEvent::Change);
}

/// Put text in a field as an edit you can undo: ⌘Z brings back what was
/// there. The caret goes after it.
pub fn put_undoable(
    field: &mut TextareaState,
    text: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut Context<TextareaState>,
) {
    let text: SharedString = text.into();
    if field.value() == text {
        return;
    }
    let end = text.len();
    field.replace_all(text, window, cx);
    field.set_selected_range(end..end, cx);
    cx.emit(InputEvent::Change);
}

/// How long Send stays inert after sending, so a double press can't land
/// on Stop.
pub const JUST_SENT: Duration = Duration::from_millis(2500);
/// How long after sending Esc and ⌘. can stop the turn a message starts,
/// before it shows as running.
pub const STOPPABLE: Duration = Duration::from_secs(3);
/// How long a waiting message being taken back, or a session being made,
/// waits for the daemon before dent stops waiting.
pub const DAEMON_WAIT: Duration = Duration::from_secs(30);
/// How long a permission mode just asked for shows before the daemon's
/// answer.
pub const ASKED_WAIT: Duration = Duration::from_secs(3);

/// Where the new-session page keeps its draft and files among the
/// sessions' (in `drafts` and the prefs).
pub const NEW_DRAFT: &str = "";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Composer,
    Starter,
    Aside,
    Comment,
    Search,
    Rename,
    Find,
    /// The search at the top of a menu that has one (projects).
    Menu,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Art(&'static str),
    Changes,
    Terminal,
    /// Side questions about the session.
    Aside,
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
    Model,
    Permission,
    Effort,
    /// The open session's model and permissions, switchable mid-session.
    SessionModel,
    SessionPermission,
    SessionEffort,
    /// Sessions started in a terminal, to carry on here.
    Pickup,
    /// Drafts put aside with ⌘S, to bring one back.
    Stash,
}

impl Menu {
    /// Menus with a field to type in: the project picker.
    pub fn searches(self) -> bool {
        self == Menu::Project
    }

    /// Menus with longer rows: paths, session titles.
    pub fn wide(self) -> bool {
        matches!(self, Menu::Project | Menu::Pickup | Menu::Stash)
    }

    /// Menus from a chip at the row's end line up with its right edge.
    #[allow(clippy::wrong_self_convention)] // reads as "opens from the end"
    pub fn from_end(self) -> bool {
        self == Menu::Pickup
    }

    /// Menus under the composer open upward.
    pub fn upward(self) -> bool {
        matches!(
            self,
            Menu::SessionModel | Menu::SessionPermission | Menu::SessionEffort
        )
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum ToastAction {
    Undo(String),
    Resume(String),
    /// Put a project taken off the list back on it.
    UnhideProject(String),
    /// A waiting message removed from the queue: Undo sends it again and
    /// puts it back where it waited, before `place`.
    Unremove {
        session: String,
        back: Draft,
        place: Option<String>,
    },
    /// Just a message, like a request the daemon refused.
    Info,
}

impl ToastAction {
    /// ⌘Z, with nothing else to undo, does what its button does.
    pub fn undoes(&self) -> bool {
        matches!(self, Self::Undo(_) | Self::UnhideProject(_))
    }

    /// Its button takes something back, so a passing message doesn't
    /// push it away.
    pub fn keeps(&self) -> bool {
        self.undoes() || matches!(self, Self::Unremove { .. })
    }
}

#[derive(Clone)]
pub struct Toast {
    pub text: String,
    pub action: ToastAction,
    pub at: Instant,
}

/// How far an approval reaches.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Scope {
    /// This call.
    Once,
    /// This kind of call, for the rest of the session.
    Session,
    /// This kind of call from now on in the project, saved where the agent
    /// keeps its rules.
    Project,
}

/// One of your messages brought back with ↑.
pub struct Recall {
    pub session: String,
    /// How far back: 1 is your newest message.
    pub back: usize,
    /// The composer's text as it came back; once edited, arrows move the caret again.
    pub shown: String,
    /// Files it attached, to take off again if you step past it.
    pub atts: Vec<String>,
}

/// What becomes of a waiting message once the daemon has taken it out of
/// its queue.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Then {
    /// Back to the reply box, files attached again.
    Restore,
    /// Into the running turn now.
    Steer,
}

/// A waiting message being taken back, held until the daemon says it's out
/// of the queue: it may have gone to the agent first, and then nothing is
/// sent twice or brought back.
pub struct TakingBack {
    pub session: String,
    /// The message as sent, to steer with.
    pub text: String,
    pub files: Vec<String>,
    /// As it comes back to the box: chips, files and its place.
    pub back: Draft,
    pub then: Then,
    pub at: Instant,
}

pub use crate::draft::{Attachment, Draft, Place};

/// Settings for a new session: chosen for you, each one click to change.
#[derive(Clone)]
pub struct Defaults {
    /// A folder; `None` starts in a scratch folder. Never guessed: the
    /// session you came from, else the last one you started or picked.
    pub project: Option<String>,
    pub model: Option<String>,
    pub permission: Permission,
    /// The agent's effort level; `None` is its default.
    pub effort: Option<String>,
}

/// A dev server's address as the web view can load it. WebKit won't load
/// `localhost` by name here, but loads an address: whichever of IPv4 and
/// IPv6 the server answers on.
/// Whether something answers at a page address's host and port.
pub fn server_answers(url: &str) -> bool {
    use std::net::ToSocketAddrs;
    let rest = url.split("://").nth(1).unwrap_or(url);
    let hostport = rest.split('/').next().unwrap_or(rest);
    let hostport = if hostport.contains(':') {
        hostport.to_string()
    } else {
        format!("{hostport}:80")
    };
    hostport.to_socket_addrs().is_ok_and(|mut addrs| {
        addrs.any(|a| {
            std::net::TcpStream::connect_timeout(&a, std::time::Duration::from_millis(300)).is_ok()
        })
    })
}

fn reachable(url: &str) -> String {
    let Some(rest) = url.strip_prefix("http://localhost:") else {
        return url.to_string();
    };
    let port: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let Ok(p) = port.parse::<u16>() else {
        return url.to_string();
    };
    let tail = &rest[port.len()..];
    let answers = |addr: std::net::SocketAddr| {
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(150)).is_ok()
    };
    let host = if answers(([127, 0, 0, 1], p).into()) {
        "127.0.0.1"
    } else if answers((std::net::Ipv6Addr::LOCALHOST, p).into()) {
        "[::1]"
    } else {
        // Not up yet: IPv4 is the usual answer.
        "127.0.0.1"
    };
    format!("http://{host}:{p}{tail}")
}

/// A session's search text, with the size it was made at.
type SearchText = ((usize, u64), Rc<str>);

/// Where `commands` keeps the slash commands every session starts with.
const EVERY: &str = "";

/// What the prompt's menu offers for the word at its caret.
pub struct Completion {
    pub token: complete::Token,
    pub items: Vec<complete::Row>,
    /// The list isn't here yet (the agent's commands, a folder's files).
    pub loading: bool,
    /// For a command's arguments: what the command does, as a caption.
    pub title: Option<String>,
}

/// What a request's reply is for.
pub enum Waiting {
    Models,
    Files(String),
    Recent,
    Usage,
    Commands(String),
    Projects,
    /// A session's changes: all of them, or one turn's.
    Changes(String, Option<String>),
    /// Putting back a file or hunk, in this session.
    Discard(String),
    /// A new session; one started from the new-session page carries what
    /// was typed and attached there, to put back if it fails.
    Create(Option<Draft>),
    /// A new session started from what was typed, without leaving where
    /// you are: where it was typed (a session, or the new-session page),
    /// and what, to put back if it fails.
    CreateAside(Option<String>, Draft),
    /// The folders for a typed path, as typed.
    Folders(String),
    ProjectMade,
    /// Going back for Try again, in this session.
    Rewind(String),
    /// Taking a waiting message (by id) back out of the queue.
    Unqueue(String),
    /// A message edited out of the queue, put back in its place: if it
    /// went at once instead (the turn ended meanwhile), there's no place
    /// to keep and nothing to say.
    Requeue,
}

/// The home folder, as a path string.
pub fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

pub struct Dent {
    pub t: Theme,
    /// Shared so a redraw can hold a session without copying its history.
    pub sessions: Vec<Rc<Session>>,
    pub arts: Vec<Artifact>,
    /// The open session; None shows the new-session page.
    pub selected: Option<String>,
    pub focus: FocusHandle,
    pub inbox_focus: FocusHandle,
    /// Your own terminals, one per session, started when first opened.
    pub shells: HashMap<String, crate::shell::Shell>,
    pub shell_tx:
        futures::channel::mpsc::UnboundedSender<(String, alacritty_terminal::event::Event)>,
    pub term_focus: FocusHandle,
    /// The terminal pane's size when last drawn, to start the next at.
    pub shell_pane: Rc<std::cell::Cell<Option<Size<Pixels>>>>,
    pub inbox_focused: bool,
    pub palette: Option<usize>,
    /// The palette lists your messages in the open session instead.
    pub palette_mine: bool,
    pub search: Entity<TextareaState>,
    pub composer: Entity<TextareaState>,
    pub starter: Entity<TextareaState>,
    pub comment_input: Entity<TextareaState>,
    /// Where side questions are typed.
    pub aside_input: Entity<TextareaState>,
    pub rename_input: Entity<TextareaState>,
    /// The session whose name is being edited in the header.
    pub renaming: Option<String>,
    /// Each session's unsent draft, and under `NEW_DRAFT` the new-session
    /// page's: the open session's (and the new-session field's) words live
    /// in its field and are copied here as they change; its files are in
    /// `attachments` while it shows.
    pub drafts: HashMap<String, Draft>,
    /// Long pastes folded into a line in a prompt, by number: they open back
    /// up when sent.
    pub pastes: Rc<std::collections::BTreeMap<u32, String>>,
    /// Files being dragged in from another app, until they're let go.
    pub dragging_files: Option<Vec<std::path::PathBuf>>,
    /// Where those files are over the window, last we heard.
    pub drag_at: Option<Point<Pixels>>,
    /// The appearance you chose, and one a flag forces for this run.
    pub appearance: crate::prefs::Appearance,
    /// Sessions pinned to the top of the sidebar.
    pub pinned: Vec<String>,
    pub forced_dark: Option<bool>,
    /// A message of yours brought back to the composer with ↑, while it's
    /// shown as it came back: ↑ and ↓ step on through them.
    pub recall: Option<Recall>,
    pub defaults: Defaults,
    pub menu: Option<Menu>,
    /// The model menu shows pinned older versions.
    pub older_models: bool,
    /// The window's height, for fitting menus.
    pub window_h: f32,
    /// Where the open menu is scrolled.
    pub menu_scroll: ScrollHandle,
    /// The menu row the keyboard is on.
    pub menu_ix: Option<usize>,
    /// A picture open large.
    pub lightbox: Option<crate::lightbox::Lightbox>,
    /// What's typed in a menu that searches (the project picker).
    pub menu_query: Entity<TextareaState>,
    /// ⌘F's field, and what it found.
    pub find_input: Entity<TextareaState>,
    pub find: Option<crate::find::Find>,
    /// Sessions you opened, most recent first, and Ctrl+Tab's list while
    /// Ctrl is held.
    pub lately: Vec<String>,
    pub switcher: Option<crate::switcher::Switcher>,
    /// The keyboard shortcuts sheet is up.
    pub keys_open: bool,
    pub project_mode: crate::picker::Mode,
    /// A project being made or cloned, said in the picker meanwhile.
    pub project_busy: Option<String>,
    /// Why the last make or clone failed.
    pub project_error: Option<String>,
    /// The folders for the path last asked about, or why there are none.
    pub folders: Option<(String, Result<Vec<String>, String>)>,
    pub toast: Option<Toast>,
    pub show_closed: bool,
    pub stopped: HashSet<String>,
    pub finished_at: HashMap<String, Instant>,
    pub arrived_at: HashMap<String, Instant>,
    pub working_since: HashMap<String, Instant>,
    /// Birth time of each streamed word, by session: (item index, byte offset, time).
    pub births: HashMap<String, (usize, Vec<(usize, Instant)>)>,
    /// Everything the daemon has said, folded into timelines.
    pub core: Sessions,
    pub live: Live,
    /// This machine's name, once connected; why not, when not.
    pub machine: String,
    pub connection: Result<(), String>,
    /// The window is in front, so the person sees what changes.
    pub window_active: bool,
    /// Some of the window is on screen (not minimised, covered or on
    /// another Space), so animation is worth drawing.
    pub window_visible: bool,
    /// The ambient background behind the new-session page.
    pub ambient: Rc<RefCell<crate::ambient::State>>,
    /// Times the connection dropped, for the walkthrough.
    pub drops: u32,
    pub models: Vec<Model>,
    pub projects: Vec<Project>,
    /// Requests whose replies matter, by request number.
    waiting: HashMap<u64, Waiting>,
    /// Each session's changes, from the daemon.
    pub changes: HashMap<String, Rc<Vec<FileDiff>>>,
    /// Choices made so far on a question, by question id.
    pub picks: HashMap<String, Vec<Option<String>>>,
    /// Choices ticked so far in a question that takes several, by
    /// `ask/question`.
    pub ticks: HashMap<String, Vec<String>>,
    /// A choice just picked, shown chosen for a moment before it goes.
    pub flash: Option<(String, usize, String)>,
    /// The question card, when the keyboard is on it.
    pub ask_focus: FocusHandle,
    /// Whether the waiting ask's card had the keyboard at the last draw.
    pub ask_keyed: bool,
    /// Whether the reply box had the keyboard at the last draw.
    pub composer_keyed: bool,
    /// Sessions started in a terminal that could carry on here.
    pub recent: Vec<RecentSession>,
    /// The account's usage, as the provider last reported it, and when.
    pub usage: Option<(Vec<dent_protocol::Window>, u64)>,
    /// Each session's files, for @ mentions.
    pub files: HashMap<String, Rc<Vec<String>>>,
    /// Slash commands the agent offers, by session; under `EVERY`, the ones
    /// a session starts with.
    pub commands: HashMap<String, Vec<Command>>,
    /// What was last written to the prefs file.
    saved_prefs: Option<crate::prefs::Prefs>,
    /// Tool groups the person unfolded, by session and item.
    /// Tool groups opened, by their first call's id: ids stay put when
    /// items are inserted above.
    pub open_tools: HashSet<String>,
    /// Tool calls opened to show what they did in full, by id.
    pub open_calls: HashSet<String>,
    /// What the bridge built per session, to extend rather than rebuild.
    built: HashMap<String, crate::bridge::Built>,
    /// Each session's search text, with the size it was made at.
    search_cache: RefCell<HashMap<String, SearchText>>,
    /// The plan above the composer is open.
    pub plan_open: bool,
    /// Long messages you've unfolded.
    pub expanded: HashSet<String>,
    /// What this window asked a command's sheet to do, still under way:
    /// session, command, act.
    pub acting: Vec<(String, String, dent_protocol::Act)>,
    /// MCP servers whose sign-in this window opened in the browser, by
    /// session and server: the row says to finish there while it still
    /// offers to sign in.
    pub signing: HashMap<(String, String), std::time::Instant>,
    /// What went wrong with a sheet's action, shown on its row until the
    /// sheet changes or it's tried again.
    pub act_errors: HashMap<(String, String, dent_protocol::Act), String>,
    /// The highlighted row of the slash menu.
    pub slash_ix: usize,
    /// The permission mode last asked for, by session, and when: until the
    /// daemon says it's in effect, ⇧Tab steps on from it.
    pub asked_permission: Option<(String, Permission, Instant)>,
    /// dent's own commands you ran in each session (`/model`, `/rename`…),
    /// with when: they never reach the agent's log, but ↑ brings them back.
    pub ran_own: HashMap<String, Vec<(u64, String)>>,
    /// The waiting approval, plan or question, and when it showed: its
    /// keys answer only once it has been seen.
    pub ask_shown: Option<(String, Instant)>,
    /// The message whose "Edit from here" is asking to confirm.
    pub rewinding: Option<(String, String)>,
    /// A new session is on its way.
    pub creating: bool,
    /// When the session being made was asked for: a daemon that never
    /// answers doesn't hold Start, Fork and the rest off for good.
    pub creating_at: Instant,
    /// The streaming reply last drawn, by session.
    partial_seen: HashMap<String, String>,
    /// Send is inert for a moment after sending, so a double press can't stop.
    pub just_sent: Option<Instant>,
    /// The open box holds a waiting message taken back to edit: the
    /// waiting message it goes back before when sent again (its draft's
    /// place, while it shows).
    pub box_place: Option<String>,
    /// Drafts put aside with ⌘S, oldest first; kept across launches.
    pub stashes: Vec<Draft>,
    /// Messages sent from here with folded pastes, by id, as they were in
    /// the box: a queued one shows its chips, and one coming back to the
    /// box (Edit, Stop, Undo, ↑) comes back with them.
    pub sent_drafts: crate::lru::Lru<String, Draft>,
    /// Something kept in the prefs changed since they were last written.
    prefs_dirty: bool,
    /// A write of the prefs is on its way.
    prefs_timer: bool,
    /// The small prefs (layout, choices) as last seen, hashed.
    prefs_light: u64,
    /// The message last sent, by session and id, and when: until the turn
    /// it starts shows as running, Esc and ⌘. can still stop it.
    pub sent_at: Option<(String, String, Instant)>,
    pub panel_open: bool,
    pub panel_width: f32,
    /// This frame's widths: the viewer gives way before the chat does.
    pub panel_w: f32,
    /// This frame's chat column width.
    pub chat_w: f32,
    pub sidebar_hidden: bool,
    /// You hid it by hand; it stays hidden whatever the width.
    pub sidebar_off: bool,
    /// The person opened the sidebar over a narrow window.
    pub sidebar_forced: bool,
    pub dragging: bool,
    pub dragging_sidebar: bool,
    pub sidebar_w: f32,
    pub tabs: HashMap<String, Tab>,
    /// Files you closed in each session's viewer, with how many versions
    /// each had then: a new version brings the tab back.
    pub closed_tabs: HashMap<String, HashMap<&'static str, usize>>,
    /// The right-click menu, if one is open.
    pub context: Option<crate::context::ContextMenu>,
    pub tab_scroll: ScrollHandle,
    /// The tab last scrolled into view, so it's only done on a change.
    pub revealed_tab: Option<(Tab, usize, u32)>,
    pub reveal_pending: bool,
    pub reveal_tries: u8,
    /// The word whose menu Esc put away; it stays away until the caret
    /// leaves that word.
    pub slash_dismissed: Option<(complete::Kind, usize)>,
    /// The prompt menu's scroll, kept on its highlighted row.
    pub slash_scroll: ScrollHandle,
    pub slash_shown: Option<usize>,
    pub palette_scroll: ScrollHandle,
    /// Where you were reading each session, and whether you were at the end.
    pub read_at: HashMap<String, (ListOffset, bool)>,
    /// Seen marks sent and not yet back from the daemon, by session.
    pub seen_sent: HashMap<String, u64>,
    /// The earlier version being looked at in a session, if any.
    pub old_version: HashMap<String, usize>,
    /// Sessions whose finished replies wait to be parsed ahead.
    pub parse_queue: Vec<String>,
    /// The count last put on the Dock icon.
    pub badged: Option<usize>,
    /// Parsed documents by file and version.
    pub md_docs: crate::lru::Lru<(&'static str, usize), Rc<Vec<Block>>>,
    /// Sessions closed while the Undo has been showing, oldest first.
    pub undo_closes: Vec<Vec<String>>,
    /// Closing several picked sessions at once: they undo together.
    pub closing_together: bool,
    /// Sessions picked in the sidebar with Shift or ⌘, to act on together.
    pub picked: Vec<String>,
    /// The terminal session being picked up.
    pub adopting: Option<String>,
    /// While the pointer is over the sidebar: each session's section and
    /// place there, held (see `shown_sections`).
    pub inbox_hold: Option<HashMap<String, (usize, usize)>>,
    /// The pointer is over the sidebar, judged by where it is (typing
    /// hides the pointer, and with it the hover state).
    pub sidebar_hovered: bool,
    /// A row to scroll into view in the sidebar on its next draw.
    pub reveal_row: Option<String>,
    /// Files you opened from links, per session, beside what the agent made.
    pub opened: HashMap<String, Vec<&'static str>>,
    /// When each opened file last changed, to notice edits on disk.
    pub opened_at: HashMap<&'static str, std::time::SystemTime>,
    /// When each one-shot action was last pressed, by name.
    pub pressed: HashMap<String, Instant>,
    /// The palette row last scrolled to.
    pub palette_shown: Option<usize>,
    pub preview: Option<Rc<RefCell<Preview>>>,
    pub viewing: HashMap<&'static str, usize>,
    pub viewport: Viewport,
    pub picking: bool,
    /// Console errors per artifact version.
    pub errors: HashMap<(&'static str, usize), Vec<String>>,
    pub updated_at: HashMap<&'static str, Instant>,
    /// Each session's working-line words and when they last changed, so
    /// new words ease in.
    pub live_words: std::cell::RefCell<HashMap<String, LiveWords>>,
    /// The context meter's breakdown is open over the composer.
    pub meter_open: bool,
    pub attachments: Vec<Attachment>,
    pub diff_file: HashMap<String, usize>,
    pub commenting: Option<(String, u32)>,
    pub review: Vec<Attachment>,
    /// The conversation draws only the items on screen.
    pub transcript: ListState,
    /// The sidebar's rows, drawn only where they're on screen.
    pub inbox_list: ListState,
    pub inbox_sigs: Vec<u64>,
    /// What the list was last measured for: the session, and a signature per item.
    pub transcript_sigs: (Option<String>, Vec<u64>),
    /// Items that just landed in the open chat, by index: when, and how
    /// far into the stagger they start (ms). They rise into place.
    pub row_born: HashMap<usize, (Instant, f32)>,
    /// The diff draws only the lines on screen.
    pub diff_list: ListState,
    /// A text file's lines, and which file and version they're for.
    pub text_list: ListState,
    pub text_key: Option<(&'static str, usize)>,
    /// The line a link pointed at in each text file.
    pub marked_line: HashMap<&'static str, u32>,
    /// Side chat's place.
    pub aside_scroll: ScrollHandle,
    /// Each document's scroll, so each keeps its own place.
    pub doc_scrolls: HashMap<&'static str, ScrollHandle>,
    pub diff_key: u64,
    pub diff_file_key: u64,
    /// The visible width of the diff, for notes and the comment box.
    pub diff_view_w: f32,
    /// Parsed Markdown by source, so a redraw never parses again; a reply
    /// streaming in parses only its unfinished end.
    pub md: crate::mdstream::Cache,
    /// What rendered Markdown remembers between frames: maths, diagrams,
    /// scroll positions of wide blocks and tall tables.
    pub mdv: crate::markdown::Md,
    /// Decoded pictures by artifact version, so a redraw never decodes or hashes again.
    pub pictures: HashMap<(&'static str, usize), Arc<Image>>,
    /// Pages open in their own windows, by artifact.
    pub popouts: HashMap<&'static str, (AnyWindowHandle, Entity<crate::popout::PopOut>)>,
    /// Dev servers that stopped answering, by tab.
    pub servers_down: std::collections::HashSet<&'static str>,
    /// Checks the dev server on screen, every couple of seconds.
    pub server_watch: Option<(&'static str, Task<()>)>,
    /// Frames drawn, for the checks.
    pub frames: u64,
    /// What a check's script read off the page.
    pub page_probe: Option<String>,
    /// The width of one character of code, measured on first use.
    pub code_em: f32,
    /// The second the timers last showed.
    pub last_second: u64,
    pub probes: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    pub placeholder: &'static str,
    /// What the hovered preview control does, shown in the toolbar: tooltips
    /// would fall behind the live page.
    pub toolbar_hint: Option<&'static str>,
    pub image_actual: bool,
    pub copied_at: Option<Instant>,
    /// A message's or reply's Copy just pressed, by key: it says "Copied".
    pub copied_key: Option<(String, Instant)>,
    /// The turn (your message's id) Changes shows for a session; none: all.
    pub changes_turn: HashMap<String, String>,
    /// A file to show once a session's changes arrive.
    pub diff_want: HashMap<String, String>,
    /// A file, or a hunk of one, waiting for you to confirm putting it back.
    pub discarding: Option<(String, String, Option<usize>)>,
    /// Try again asked whether to undo the last turn's file changes too.
    pub trying_again: Option<String>,
    /// A message to send again once its session has gone back to before it:
    /// the session, the message it went back to, and its text.
    pub resend: Option<(String, String, String, Vec<String>)>,
    /// A session the daemon just created, to open once its events arrive.
    pub pending_select: Option<String>,
    /// Waiting messages being taken back out of the queue, by message id.
    pub taking_back: HashMap<String, TakingBack>,
    /// Messages to put back in a prompt at the next draw (which has the
    /// window): the session (`None`: the new-session page), the words and
    /// what they carried. Nothing is put over a draft; it's added after.
    pub restores: Vec<(Option<String>, Draft)>,
    /// A scripted walkthrough, when running one.
    pub walk: Option<crate::walk::Walk>,
    pub last_frame: Instant,
    /// When `tick_due` last looked.
    last_tick: Instant,
    /// The working square's next breath is on its way.
    pub breath_timer: bool,
}

/// Sessions `@` offers at most.
const SESSION_ROWS: usize = 4;

/// The most of another session attached as context, in characters.
const SESSION_CONTEXT: usize = 24_000;

impl Dent {
    pub fn new(forced: Option<bool>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = crate::prefs::load();
        crate::ds::set_zoom(prefs.zoom.unwrap_or(1.));
        crate::open::load(prefs.open_with.clone());
        crate::ambient::choose(prefs.ambient.as_deref());
        // Follows the Mac's appearance unless you chose one (or --dark or
        // --light asks).
        let system_dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let t = if forced.or(prefs.appearance.dark()).unwrap_or(system_dark) {
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
                .placeholder("Describe a task")
                .submit_on_enter(true)
        });
        let comment_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .placeholder("Comment on this line")
                .submit_on_enter(true)
        });
        let aside_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .placeholder("Ask about this session")
                .submit_on_enter(true)
        });
        cx.subscribe_in(
            &aside_input,
            window,
            |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = ev {
                    this.ask_aside(window, cx);
                }
            },
        )
        .detach();
        let rename_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 1)
                .placeholder("Session name")
                .submit_on_enter(true)
        });
        cx.subscribe_in(
            &rename_input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                // Return saves, and so does clicking away, as in Finder.
                InputEvent::PressEnter { .. } => {
                    this.commit_rename(window, cx);
                    this.composer.update(cx, |c, cx| c.focus_field(window, cx));
                }
                InputEvent::Blur => this.commit_rename(window, cx),
                _ => {}
            },
        )
        .detach();
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
        single_line(&rename_input, window, cx);
        let find_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 1)
                .placeholder("Find in this session")
        });
        cx.subscribe_in(&find_input, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                crate::find::query_changed(this, cx);
            }
        })
        .detach();
        single_line(&find_input, window, cx);
        let menu_query = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 1)
                .placeholder("Search projects, or type a path")
                .submit_on_enter(true)
        });
        cx.subscribe_in(&menu_query, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                this.project_query_changed(cx);
            }
        })
        .detach();
        single_line(&menu_query, window, cx);
        single_line(&search, window, cx);
        // Only edits redraw the window: watching the fields themselves would
        // redraw on every caret blink, even after they lose focus.
        cx.subscribe_in(
            &composer,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                InputEvent::Change => this.prompt_changed(cx),
                _ => {}
            },
        )
        .detach();
        cx.subscribe_in(
            &starter,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { shift: false, .. } => this.start(window, cx),
                InputEvent::Change => this.prompt_changed(cx),
                _ => {}
            },
        )
        .detach();
        cx.subscribe_in(
            &comment_input,
            window,
            |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = ev {
                    this.add_comment(window, cx);
                }
            },
        )
        .detach();
        // A field taking the keyboard takes ⌘A and ⌘C from text selected
        // in the chat (see `intercept`).
        for field in [
            &composer,
            &starter,
            &comment_input,
            &aside_input,
            &search,
            &rename_input,
            &menu_query,
            &find_input,
        ] {
            cx.subscribe(field, |_, _, ev: &InputEvent, _| {
                if let InputEvent::Focus = ev {
                    crate::select::yield_keys();
                }
            })
            .detach();
        }
        // Before any binding: a text field's own Tab or ⌘Z would win otherwise.
        let me = cx.entity().downgrade();
        cx.intercept_keystrokes(move |ev, window, cx| {
            if let Some(me) = me.upgrade()
                && me.update(cx, |this, cx| this.intercept(&ev.keystroke, window, cx))
            {
                cx.stop_propagation();
            }
        })
        .detach();
        cx.observe_window_appearance(window, |this, window, cx| this.apply_theme(window, cx))
            .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            this.window_active = window.is_window_active();
            // Coming back to the window is looking at the open session.
            this.mark_seen();
            cx.notify();
        })
        .detach();
        cx.observe_window_visibility(window, |this, _, window, cx| {
            this.window_visible = window.is_visible();
            cx.notify();
        })
        .detach();
        // A background run never keeps the front: whatever activates dent's
        // window, activation goes straight back to the app you're using.
        #[cfg(target_os = "macos")]
        if crate::BACKGROUND.load(std::sync::atomic::Ordering::Relaxed) {
            cx.observe_window_activation(window, |_, window, _| {
                if window.is_window_active()
                    && let Some(mtm) = objc2::MainThreadMarker::new()
                {
                    objc2_app_kit::NSApplication::sharedApplication(mtm).deactivate();
                }
            })
            .detach();
        }
        // The daemon's feed arrives on a channel; the window applies it as
        // it comes.
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let live = Live::start(crate::live::default_state(), tx);
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Ok(m) = rx.try_recv() {
                    batch.push(m);
                }
                if this.update(cx, |this, cx| this.on_live(batch, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // The shortcut from any app: dent comes forward on a new session.
        let mut presses = crate::hotkey::listen();
        cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt;
            while presses.next().await.is_some() {
                let gone = this.update_in(cx, |this, window, cx| {
                    cx.activate(true);
                    window.activate_window();
                    this.select(None, window, cx);
                });
                if gone.is_err() {
                    break;
                }
            }
        })
        .detach();
        // What your terminals draw arrives from their own threads.
        let (shell_tx, mut shell_rx) = futures::channel::mpsc::unbounded();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while let Some(first) = shell_rx.next().await {
                let mut batch = vec![first];
                while let Ok(m) = shell_rx.try_recv() {
                    batch.push(m);
                }
                let gone = this.update(cx, |this, cx| {
                    for (id, ev) in batch {
                        this.on_shell(id, ev, cx);
                    }
                });
                if gone.is_err() {
                    break;
                }
            }
        })
        .detach();
        // Usage moves however the account is used: look again every few minutes.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(300))
                    .await;
                if this.update(cx, |this, _| this.fetch_usage()).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            t,
            sessions: Vec::new(),
            arts: Vec::new(),
            selected: None,
            focus: cx.focus_handle(),
            inbox_focus: cx.focus_handle(),
            ask_focus: cx.focus_handle(),
            ask_keyed: false,
            composer_keyed: false,
            ticks: HashMap::new(),
            flash: None,
            shells: HashMap::new(),
            shell_tx,
            term_focus: cx.focus_handle(),
            shell_pane: Rc::default(),
            inbox_focused: false,
            palette: None,
            palette_mine: false,
            search,
            composer,
            starter,
            comment_input,
            aside_input,
            rename_input: rename_input.clone(),
            renaming: None,
            drafts: prefs.drafts.clone().into_iter().collect(),
            pastes: Rc::new(prefs.pastes.clone()),
            appearance: prefs.appearance,
            pinned: prefs.pinned.clone(),
            forced_dark: forced,
            recall: None,
            defaults: Defaults {
                project: prefs.project.clone(),
                model: prefs.model.clone(),
                permission: prefs.permission,
                effort: prefs.effort.clone(),
            },
            menu: None,
            older_models: false,
            window_h: 800.,
            menu_scroll: ScrollHandle::new(),
            menu_ix: None,
            menu_query,
            find_input,
            find: None,
            lately: Vec::new(),
            switcher: None,
            keys_open: false,
            lightbox: None,
            project_mode: crate::picker::Mode::Pick,
            project_busy: None,
            project_error: None,
            folders: None,
            closed_tabs: prefs
                .closed_tabs
                .iter()
                .map(|(id, c)| {
                    (
                        id.clone(),
                        c.iter()
                            .map(|(a, n)| (crate::data::intern(a), *n))
                            .collect(),
                    )
                })
                .collect(),
            tab_scroll: ScrollHandle::new(),
            context: None,
            revealed_tab: None,
            reveal_pending: false,
            reveal_tries: 0,
            slash_dismissed: None,
            slash_scroll: ScrollHandle::new(),
            slash_shown: None,
            palette_scroll: ScrollHandle::new(),
            read_at: HashMap::new(),
            seen_sent: HashMap::new(),
            old_version: HashMap::new(),
            badged: None,
            parse_queue: Vec::new(),
            md_docs: crate::lru::Lru::new(64),
            undo_closes: Vec::new(),
            closing_together: false,
            picked: Vec::new(),
            adopting: None,
            inbox_hold: None,
            sidebar_hovered: false,
            reveal_row: None,
            opened: HashMap::new(),
            opened_at: HashMap::new(),
            pressed: HashMap::new(),
            palette_shown: None,
            toast: None,
            show_closed: false,
            stopped: HashSet::new(),
            finished_at: HashMap::new(),
            arrived_at: HashMap::new(),
            working_since: HashMap::new(),
            births: HashMap::new(),
            core: Sessions::default(),
            live,
            machine: String::new(),
            connection: Err("Starting dent…".into()),
            drops: 0,
            window_active: true,
            window_visible: true,
            ambient: Rc::default(),
            models: Vec::new(),
            projects: Vec::new(),
            waiting: HashMap::new(),
            changes: HashMap::new(),
            picks: HashMap::new(),
            creating: false,
            creating_at: Instant::now(),
            rewinding: None,
            commands: HashMap::new(),
            files: HashMap::new(),
            usage: None,
            recent: Vec::new(),
            slash_ix: 0,
            ran_own: HashMap::new(),
            ask_shown: None,
            asked_permission: None,
            expanded: HashSet::new(),
            acting: Vec::new(),
            signing: HashMap::new(),
            act_errors: HashMap::new(),
            plan_open: true,
            search_cache: RefCell::new(HashMap::new()),
            open_tools: HashSet::new(),
            open_calls: HashSet::new(),
            built: HashMap::new(),
            saved_prefs: None,
            partial_seen: HashMap::new(),
            just_sent: None,
            box_place: None,
            stashes: prefs.stashes.clone(),
            sent_drafts: crate::lru::Lru::new(256),
            prefs_dirty: false,
            prefs_timer: false,
            prefs_light: 0,
            sent_at: None,
            panel_open: true,
            panel_width: prefs.panel_width.unwrap_or(600.),
            sidebar_w: prefs
                .sidebar_width
                .unwrap_or(crate::sidebar::WIDTH)
                .clamp(crate::sidebar::MIN_WIDTH, crate::sidebar::MAX_WIDTH),
            dragging_sidebar: false,
            panel_w: 600.,
            chat_w: 800.,
            sidebar_hidden: false,
            sidebar_off: {
                crate::notify::SOUND.store(!prefs.quiet, std::sync::atomic::Ordering::Relaxed);
                crate::notify::AWAKE.store(!prefs.sleepy, std::sync::atomic::Ordering::Relaxed);
                if let Some(h) = &prefs.hotkey {
                    crate::hotkey::choose(h);
                }
                prefs.sidebar_off
            },
            sidebar_forced: false,
            dragging: false,
            tabs: HashMap::new(),
            preview: None,
            viewing: HashMap::new(),
            viewport: Viewport::Fill,
            picking: false,
            errors: HashMap::new(),
            updated_at: HashMap::new(),
            live_words: Default::default(),
            meter_open: false,
            // The new-session page shows first: its files, as you left them.
            attachments: prefs
                .drafts
                .get(NEW_DRAFT)
                .map(|d| d.atts.clone())
                .unwrap_or_default(),
            dragging_files: None,
            drag_at: None,
            diff_file: HashMap::new(),
            commenting: None,
            review: Vec::new(),
            transcript: ListState::new(0, ListAlignment::Top, px(600.)),
            inbox_list: ListState::new(0, ListAlignment::Top, px(400.)),
            inbox_sigs: Vec::new(),
            transcript_sigs: (None, Vec::new()),
            row_born: HashMap::new(),
            diff_list: ListState::new(0, ListAlignment::Top, px(400.)),
            text_list: ListState::new(0, ListAlignment::Top, px(400.)),
            text_key: None,
            marked_line: HashMap::new(),
            aside_scroll: ScrollHandle::new(),
            doc_scrolls: HashMap::new(),
            diff_key: 0,
            diff_file_key: 0,
            diff_view_w: 400.,
            md: Default::default(),
            mdv: crate::markdown::Md::default(),
            pictures: HashMap::new(),
            popouts: HashMap::new(),
            servers_down: Default::default(),
            server_watch: None,
            frames: 0,
            page_probe: None,
            code_em: 0.,
            last_second: 0,
            probes: Rc::default(),
            placeholder: "",
            toolbar_hint: None,
            image_actual: false,
            copied_at: None,
            copied_key: None,
            changes_turn: HashMap::new(),
            diff_want: HashMap::new(),
            discarding: None,
            trying_again: None,
            resend: None,
            pending_select: None,
            taking_back: HashMap::new(),
            restores: Vec::new(),
            walk: None,
            last_frame: Instant::now(),
            last_tick: Instant::now(),
            breath_timer: false,
        };
        this.mdv = crate::markdown::Md::new(this.probes.clone());
        for input in this.fields() {
            crate::style_input(input, &this.t, cx);
        }
        // What you were typing for a new session, as you left it.
        if let Some(draft) = this.drafts.get(NEW_DRAFT).cloned() {
            let pastes = this.pastes.clone();
            this.starter.update(cx, |c, cx| {
                crate::draft::put_in(c, Place::Replace, &draft, &pastes, false, window, cx)
            });
        }
        // What's typed is written before quitting, not lost to the pause
        // that gathers keystrokes into one write.
        cx.on_app_quit(|this, cx| {
            this.save_prefs(cx);
            async {}
        })
        .detach();
        // Files you had opened come back as tabs, if they're still there.
        for (sid, paths) in &prefs.opened {
            for p in paths {
                this.load_opened(sid, p);
            }
        }
        if std::env::var_os("DENT_NO_PREVIEW").is_none() {
            this.preview = Preview::new(window).map(|p| Rc::new(RefCell::new(p)));
        }
        this
    }

    // ---- Looking things up ----

    pub fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id).map(|s| &**s)
    }
    fn session_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions
            .iter_mut()
            .find(|s| s.id == id)
            .map(Rc::make_mut)
    }
    pub fn current(&self) -> Option<&Session> {
        self.selected.as_deref().and_then(|id| self.session(id))
    }
    /// The open session, cheap to hold across a redraw.
    pub fn current_rc(&self) -> Option<Rc<Session>> {
        let id = self.selected.as_deref()?;
        self.sessions.iter().find(|s| s.id == id).cloned()
    }
    pub fn art(&self, id: &str) -> Option<&Artifact> {
        self.arts.iter().find(|a| a.id == id)
    }

    fn open(&self) -> impl Iterator<Item = &Rc<Session>> {
        self.sessions.iter().filter(|s| !s.closed)
    }
    pub fn needs(&self) -> Vec<Rc<Session>> {
        self.open()
            .filter(|s| s.status == Status::Needs)
            .cloned()
            .collect()
    }
    pub fn done(&self) -> Vec<Rc<Session>> {
        self.open()
            .filter(|s| matches!(s.status, Status::Done | Status::Idle))
            .cloned()
            .collect()
    }
    /// Working sessions stay where they are while they stream: in the order
    /// they started working, newest first, not by their latest output.
    pub fn working(&self) -> Vec<Rc<Session>> {
        let mut v: Vec<Rc<Session>> = self
            .open()
            .filter(|s| matches!(s.status, Status::Working | Status::Waiting))
            .cloned()
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.working_since));
        v
    }
    /// The sidebar's sections as shown: needs you, done, working. While
    /// the pointer is over the sidebar, rows hold the places they had, so
    /// a session that finishes doesn't jump away from a click on its way;
    /// new sessions still come in, and closed ones still go.
    /// Pin a session to the top of the sidebar, or let it go back to its
    /// status's section.
    pub fn toggle_pin(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.pinned.iter().any(|p| p == id) {
            self.pinned.retain(|p| p != id);
        } else {
            self.pinned.push(id.to_string());
        }
        self.prefs_changed();
        cx.notify();
    }

    /// Pinned sessions that are still open, in the order pinned.
    pub fn pinned_sessions(&self) -> Vec<Rc<Session>> {
        self.pinned
            .iter()
            .filter_map(|id| self.sessions.iter().find(|s| s.id == *id && !s.closed))
            .cloned()
            .collect()
    }

    /// The sidebar's sections below Pinned: what needs you, what's done,
    /// what's working. Pinned sessions sit in Pinned instead, though they
    /// still count as needing you for ⌘J and the status bar.
    pub fn shown_sections(&self) -> [Vec<Rc<Session>>; 3] {
        let unpinned = |list: Vec<Rc<Session>>| -> Vec<Rc<Session>> {
            list.into_iter()
                .filter(|s| !self.pinned.contains(&s.id))
                .collect()
        };
        let fresh = [
            unpinned(self.needs()),
            unpinned(self.done()),
            unpinned(self.working()),
        ];
        let Some(hold) = &self.inbox_hold else {
            return fresh;
        };
        let mut all: Vec<((usize, bool, usize), Rc<Session>)> = Vec::new();
        for (sec, list) in fresh.into_iter().enumerate() {
            for (ix, s) in list.into_iter().enumerate() {
                let place = match hold.get(&s.id) {
                    Some(&(sec, ix)) => (sec, true, ix),
                    None => (sec, false, ix),
                };
                all.push((place, s));
            }
        }
        all.sort_by_key(|(place, _)| *place);
        let mut out: [Vec<Rc<Session>>; 3] = Default::default();
        for ((sec, ..), s) in all {
            out[sec].push(s);
        }
        out
    }

    /// Whether `p` is over the sidebar.
    pub fn over_sidebar(&self, p: Point<Pixels>, window: &Window) -> bool {
        let (x, y) = (p.x.as_f32(), p.y.as_f32());
        !self.sidebar_hidden
            && (0. ..self.sidebar_w).contains(&x)
            && (0. ..window.viewport_size().height.as_f32()).contains(&y)
    }

    pub fn closed(&self) -> Vec<Rc<Session>> {
        self.sessions.iter().filter(|s| s.closed).cloned().collect()
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
        let open = self.open_arts(s);
        self.tabs
            .get(&s.id)
            .copied()
            .filter(|t| !matches!(t, Tab::Art(a) if !open.contains(a)))
            .or_else(|| open.last().map(|a| Tab::Art(a)))
            .unwrap_or(Tab::Changes)
    }

    /// The files with a tab in this session's viewer: all it made, less
    /// those you closed and that haven't changed since.
    pub fn open_arts(&self, s: &Session) -> Vec<&'static str> {
        let closed = self.closed_tabs.get(&s.id);
        let opened = self.opened.get(&s.id).cloned().unwrap_or_default();
        s.arts
            .iter()
            .copied()
            .chain(opened.into_iter().filter(|a| !s.arts.contains(a)))
            .filter(|a| {
                closed
                    .and_then(|c| c.get(a))
                    .is_none_or(|&n| self.art(a).is_some_and(|x| x.versions.len() > n))
            })
            .collect()
    }

    /// Show a file in the viewer: one the agent made opens its tab; any
    /// other document, page or picture opens as a tab of its own that
    /// follows the file on disk. With no session open, it opens in its app.
    /// A table's Open, pressed since the last frame: it shows in the
    /// viewer, as a document of its own.
    pub fn take_table_request(&mut self, cx: &mut Context<Self>) {
        let Some(md) = crate::markdown::OPEN_TABLE.with(|o| o.borrow_mut().take()) else {
            return;
        };
        let rows = md.lines().count().saturating_sub(2);
        let dir = std::env::temp_dir().join("dent-tables");
        let path = dir.join(format!("Table, {rows} rows.md"));
        if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&path, md).is_ok() {
            self.open_file(&path.display().to_string(), cx);
        }
    }

    pub fn open_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.open_file_at(path, None, cx);
    }

    /// Show a file from a link, at `line` when it names one. A path that
    /// isn't there says so; a folder opens in Finder.
    pub fn open_file_at(&mut self, path: &str, line: Option<u32>, cx: &mut Context<Self>) {
        let target = std::path::Path::new(path);
        if !target.exists() {
            // Named as the reply named it: from the session's folder.
            let shown = self
                .current()
                .and_then(|s| target.strip_prefix(&s.cwd).ok())
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| crate::picker::tilde(path));
            self.toast(ToastAction::Info, format!("Can't find {shown}"));
            cx.notify();
            return;
        }
        if target.is_dir() {
            crate::open::open(path, cx);
            return;
        }
        let Some(s) = self.current().cloned() else {
            crate::open::open(path, cx);
            return;
        };
        let target = std::path::Path::new(path);
        let known = s
            .arts
            .iter()
            .chain(self.opened.get(&s.id).into_iter().flatten())
            .copied()
            .find(|a| {
                self.art(a)
                    .and_then(crate::viewer::art_file)
                    .is_some_and(|f| f == target)
            });
        if let Some(a) = known {
            self.mark_line(a, line);
            self.open_art(a, None, cx);
            return;
        }
        let shown = std::fs::read(path)
            .ok()
            .and_then(|b| crate::data::Kind::opened(path, &b));
        if shown.is_none() {
            crate::open::open(path, cx);
            return;
        }
        match self.load_opened(&s.id, path) {
            Some(id) => {
                self.prefs_changed();
                self.mark_line(id, line);
                self.open_art(id, None, cx);
            }
            None => {
                self.toast(ToastAction::Info, format!("Can't open {path}"));
                cx.notify();
            }
        }
    }

    /// Read a file into a viewer tab of `session`'s; its id, if it could.
    fn load_opened(&mut self, session: &str, path: &str) -> Option<&'static str> {
        let bytes = std::fs::read(path).ok()?;
        let kind = crate::data::Kind::opened(path, &bytes)?;
        let id = crate::data::intern(&format!("{session}/open/{path}"));
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        self.arts.retain(|a| a.id != id);
        self.arts.push(Artifact {
            id,
            kind,
            title: crate::data::intern(&name),
            path: crate::data::intern(path),
            dir: None,
            versions: vec![Rc::from(bytes)],
        });
        if let Ok(m) = std::fs::metadata(path).and_then(|m| m.modified()) {
            self.opened_at.insert(id, m);
        }
        let list = self.opened.entry(session.to_string()).or_default();
        if !list.contains(&id) {
            list.push(id);
        }
        Some(id)
    }

    /// The line a link named in a text file: scrolled to and marked when
    /// the file next shows. Other kinds ignore it.
    fn mark_line(&mut self, art: &'static str, line: Option<u32>) {
        match line {
            Some(l) => {
                self.marked_line.insert(art, l);
                self.text_key = None;
            }
            None => {
                self.marked_line.remove(art);
            }
        }
    }

    /// Whether the opened file showing in the viewer changed on disk.
    fn opened_changed(&self) -> bool {
        let Some(Tab::Art(a)) = self.current().map(|s| self.tab(s)) else {
            return false;
        };
        let (Some(seen), Some(art)) = (self.opened_at.get(a), self.art(a)) else {
            return false;
        };
        self.panel_open
            && std::fs::metadata(art.path)
                .and_then(|m| m.modified())
                .is_ok_and(|m| m != *seen)
    }

    /// A file you opened changed on disk: its tab shows the new version.
    pub fn refresh_opened(&mut self, art: &'static str) {
        let Some(&seen) = self.opened_at.get(art) else {
            return;
        };
        let Some(path) = self.art(art).map(|a| a.path) else {
            return;
        };
        let Ok(m) = std::fs::metadata(path).and_then(|m| m.modified()) else {
            return;
        };
        if m == seen {
            return;
        }
        self.opened_at.insert(art, m);
        if let Ok(bytes) = std::fs::read(path)
            && let Some(a) = self.arts.iter_mut().find(|a| a.id == art)
        {
            a.versions.push(Rc::from(bytes));
            self.updated_at.insert(art, Instant::now());
        }
    }

    /// Close a file's tab. The tab beside it shows instead; with none left
    /// the viewer closes.
    pub fn close_tab(&mut self, art: &'static str, cx: &mut Context<Self>) {
        let Some(s) = self.current().cloned() else {
            return;
        };
        let open = self.open_arts(&s);
        let showing = self.tab(&s) == Tab::Art(art);
        let ix = open.iter().position(|a| *a == art);
        let versions = self.art(art).map_or(0, |a| a.versions.len());
        self.closed_tabs
            .entry(s.id.clone())
            .or_default()
            .insert(art, versions);
        if showing {
            let next = ix.and_then(|i| {
                open.get(i + 1)
                    .or_else(|| i.checked_sub(1).and_then(|j| open.get(j)))
            });
            match next {
                Some(a) => {
                    self.tabs.insert(s.id.clone(), Tab::Art(a));
                }
                None if !s.diff.is_empty() => {
                    self.tabs.insert(s.id.clone(), Tab::Changes);
                }
                None if self.shells.contains_key(&s.id) => {
                    self.tabs.insert(s.id.clone(), Tab::Terminal);
                }
                None if !s.asides.is_empty() => {
                    self.tabs.insert(s.id.clone(), Tab::Aside);
                }
                None => {
                    self.tabs.remove(&s.id);
                    self.panel_open = false;
                }
            }
        }
        self.prefs_changed();
        cx.notify();
    }

    /// Close every file tab in this session but `keep`.
    pub fn close_other_tabs(&mut self, keep: &'static str, cx: &mut Context<Self>) {
        let Some(s) = self.current().cloned() else {
            return;
        };
        for a in self.open_arts(&s) {
            if a != keep {
                let versions = self.art(a).map_or(0, |x| x.versions.len());
                self.closed_tabs
                    .entry(s.id.clone())
                    .or_default()
                    .insert(a, versions);
            }
        }
        self.tabs.insert(s.id.clone(), Tab::Art(keep));
        self.prefs_changed();
        cx.notify();
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
        // Looking at a session deals with what it notified you about.
        if let Some(id) = &id {
            crate::notify::clear(id);
        }
        // Choosing the session already open only puts you in its reply box:
        // attachments, draft and place stay as they are.
        if id.is_some() && id == self.selected {
            self.menu = None;
            self.composer.update(cx, |c, cx| c.focus_field(window, cx));
            cx.notify();
            return;
        }
        // A new session from inside one starts in its folder.
        if id.is_none()
            && let Some(s) = self.current_rc()
        {
            self.defaults.project = self.project_for(&s.cwd);
        }
        if let Some(id) = &id {
            self.used(id);
        }
        // Find belongs to the session it searched.
        self.unmark_page();
        if self.find.take().is_some() {
            crate::select::set_find(None);
        }
        // Each session keeps its own draft, chips, files and place with it.
        let key = |id: &Option<String>| id.clone().unwrap_or_else(|| NEW_DRAFT.to_string());
        let mut leaving = match &self.selected {
            Some(_) => Draft::read(self.composer.read(cx), Vec::new()),
            None => Draft::read(self.starter.read(cx), Vec::new()),
        };
        leaving.atts = std::mem::take(&mut self.attachments);
        leaving.place = self.box_place.take();
        if leaving.is_empty() {
            self.drafts.remove(&key(&self.selected));
        } else {
            self.drafts.insert(key(&self.selected), leaving);
        }
        let coming = self.drafts.get(&key(&id)).cloned().unwrap_or_default();
        self.attachments = coming.atts.clone();
        self.box_place = coming.place.clone();
        if id.is_some() {
            let pastes = self.pastes.clone();
            self.composer.update(cx, |c, cx| {
                crate::draft::put_in(c, Place::Replace, &coming, &pastes, false, window, cx)
            });
        }
        self.prefs_changed();
        if let Some(new) = id.clone()
            && !self.commands.contains_key(&new)
        {
            self.fetch_commands(&new);
        }
        if let Some(new) = id.clone()
            // Stepping through sessions quickly asks once per session, not
            // once per visit while the last answer is still on its way.
            && !self
                .waiting
                .values()
                .any(|w| matches!(w, Waiting::Files(f) if *f == new))
        {
            // Files change as the agent works: look again on each visit.
            let n = self.live.send(Request::Files {
                session: new.clone(),
            });
            self.waiting.insert(n, Waiting::Files(new));
        }
        if let Some(new) = id.clone()
            && !self.changes.contains_key(&new)
            && self.core.live
        {
            self.fetch_changes(&new);
        }
        if id.is_none() && self.connection.is_ok() {
            self.fetch_recent();
        }
        self.reveal_row = id.clone();
        self.selected = id;
        self.mark_seen();
        self.menu = None;
        self.end_pick();
        self.commenting = None;
        match &self.selected {
            Some(_) => self.composer.update(cx, |c, cx| c.focus_field(window, cx)),
            None => self.starter.update(cx, |c, cx| c.focus_field(window, cx)),
        }
        cx.notify();
    }

    /// The open session has been looked at, if the window is in front: what
    /// it finished or asked is no longer new. Sent once per mark.
    pub fn mark_seen(&mut self) {
        if !self.window_active || !self.core.live {
            return;
        }
        let Some(s) = self.current_rc() else { return };
        if !s.unread() || self.seen_sent.get(&s.id).is_some_and(|&n| n >= s.news) {
            return;
        }
        self.seen_sent.insert(s.id.clone(), s.news);
        self.live.send(Request::Seen {
            session: s.id.clone(),
            upto: s.news,
        });
    }

    /// Closing archives: it can always be reopened. The next thing to clear opens.
    /// Save the name being typed, unless it's empty, and close the field.
    pub fn commit_rename(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let title = self.rename_input.read(cx).value().trim().to_string();
        if let Some(id) = self.renaming.take()
            && !title.is_empty()
            && self.session(&id).is_some_and(|s| s.title != title)
        {
            self.live.send(Request::Rename { session: id, title });
        }
        cx.notify();
    }

    pub fn start_rename(
        &mut self,
        id: &str,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.renaming = Some(id.to_string());
        let title = title.to_string();
        self.rename_input.update(cx, |i, cx| {
            put(i, title, window, cx);
            i.focus_field(window, cx);
            i.select_all(window, cx);
        });
        cx.notify();
    }

    /// Close a session; returns whether it could be (one still working
    /// can't).
    pub fn close(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.session(id) else {
            return false;
        };
        if !Self::can_close(s) {
            self.toast(
                ToastAction::Info,
                "Stop it first: a session can be closed once it's done".into(),
            );
            cx.notify();
            return false;
        }
        let order = self.to_clear();
        let at = order.iter().position(|x| x == id).unwrap_or(0);
        if let Some(s) = self.session_mut(id) {
            s.closed = true;
        }
        if let Some(sh) = self.shells.remove(id) {
            sh.shutdown();
        }
        self.live.send(Request::Close {
            session: id.to_string(),
        });
        self.stopped.remove(id);
        // Closes in a row can all be undone, newest first.
        if !self
            .toast
            .as_ref()
            .is_some_and(|t| matches!(t.action, ToastAction::Undo(_)))
        {
            self.undo_closes.clear();
        }
        if !self.undo_closes.iter().flatten().any(|c| c == id) {
            match self.undo_closes.last_mut() {
                Some(group) if self.closing_together => group.push(id.to_string()),
                _ => self.undo_closes.push(vec![id.to_string()]),
            }
        }
        let n = self.undo_closes.iter().map(Vec::len).sum::<usize>();
        self.toast = Some(Toast {
            text: if n == 1 {
                "Session closed".into()
            } else {
                format!("{n} sessions closed")
            },
            action: ToastAction::Undo(id.to_string()),
            at: Instant::now(),
        });
        if self.selected.as_deref() == Some(id) {
            let rest: Vec<String> = order.into_iter().filter(|x| x != id).collect();
            let next = rest.get(at.min(rest.len().saturating_sub(1))).cloned();
            self.select(next, window, cx);
        }
        cx.notify();
        true
    }

    /// Shift-click picks every row from the last one picked (or the open
    /// session) to this one; ⌘-click picks or unpicks this one.
    pub fn pick(&mut self, id: &str, run: bool, cx: &mut Context<Self>) {
        if self.picked.is_empty()
            && let Some(open) = self.selected.clone()
        {
            self.picked.push(open);
        }
        let order = self.order();
        match (run, self.picked.last().cloned()) {
            (true, Some(from)) => {
                let (a, b) = (
                    order.iter().position(|x| *x == from),
                    order.iter().position(|x| x == id),
                );
                if let (Some(a), Some(b)) = (a, b) {
                    for x in &order[a.min(b)..=a.max(b)] {
                        if !self.picked.contains(x) {
                            self.picked.push(x.clone());
                        }
                    }
                }
                // The row clicked is where the next run starts.
                self.picked.retain(|x| x != id);
                self.picked.push(id.to_string());
            }
            _ => {
                if let Some(at) = self.picked.iter().position(|x| x == id) {
                    self.picked.remove(at);
                } else {
                    self.picked.push(id.to_string());
                }
            }
        }
        cx.notify();
    }

    /// Close every picked session that can be closed, as one Undo; those
    /// still working stay, and a toast says so.
    pub fn close_picked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = std::mem::take(&mut self.picked);
        let (can, busy): (Vec<String>, Vec<String>) = picked.into_iter().partition(|id| {
            self.session(id)
                .is_some_and(|s| !s.closed && Self::can_close(s))
        });
        // A fresh group for this batch, then every close joins it.
        let had_undo = self
            .toast
            .as_ref()
            .is_some_and(|t| matches!(t.action, ToastAction::Undo(_)));
        if !had_undo {
            self.undo_closes.clear();
        }
        self.undo_closes.push(Vec::new());
        self.closing_together = true;
        for id in &can {
            self.close(id, window, cx);
        }
        self.closing_together = false;
        if self.undo_closes.last().is_some_and(Vec::is_empty) {
            self.undo_closes.pop();
        }
        if !busy.is_empty() {
            let n = busy.len();
            self.toast(
                ToastAction::Info,
                format!("{n} still working, so left open: a session can be closed once it's done"),
            );
        }
        cx.notify();
    }

    pub fn reopen(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(s) = self.session_mut(id) {
            s.closed = false;
        }
        self.live.send(Request::Reopen {
            session: id.to_string(),
        });
        self.arrived_at.insert(id.to_string(), Instant::now());
        cx.notify();
    }

    pub fn toast(&mut self, action: ToastAction, text: String) {
        // A passing message doesn't push an Undo out of the way.
        if matches!(action, ToastAction::Info)
            && self.toast.as_ref().is_some_and(|t| t.action.keeps())
        {
            return;
        }
        self.toast = Some(Toast {
            text,
            action,
            at: Instant::now(),
        });
    }

    /// Undo the latest close; each Undo (or ⌘Z) steps one further back.
    pub fn toast_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.toast.take() else { return };
        match t.action {
            ToastAction::Undo(_) => {
                // Sessions closed together come back together.
                if let Some(group) = self.undo_closes.pop() {
                    for id in &group {
                        self.reopen(id, cx);
                    }
                    if let Some(id) = group.first() {
                        self.select(Some(id.clone()), window, cx);
                    }
                }
                if let Some(next) = self.undo_closes.last().and_then(|g| g.last()).cloned() {
                    let n = self.undo_closes.iter().map(Vec::len).sum::<usize>();
                    self.toast = Some(Toast {
                        text: if n == 1 {
                            "Session closed".into()
                        } else {
                            format!("{n} sessions closed")
                        },
                        action: ToastAction::Undo(next),
                        at: Instant::now(),
                    });
                }
            }
            ToastAction::Resume(id) => self.resume(&id, cx),
            ToastAction::UnhideProject(path) => self.hide_project(&path, false, cx),
            // Back in the queue where it waited (or, the turn over
            // meanwhile, sent as the next message would be).
            ToastAction::Unremove {
                session,
                back,
                place,
            } => {
                let d = Draft { place, ..back };
                self.send_draft(&session, &d, false);
            }
            ToastAction::Info => {}
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

    // ---- The daemon ----

    /// What the daemon said since the last look.
    pub fn on_live(&mut self, batch: Vec<Incoming>, cx: &mut Context<Self>) {
        if self.walk.as_ref().is_some_and(|w| w.resting.is_some()) {
            eprintln!("      daemon messages while idle: {}", batch.len());
        }
        let mut changed: Vec<String> = Vec::new();
        for m in batch {
            match m {
                Incoming::Up { machine } => {
                    self.machine = machine;
                    self.connection = Ok(());
                    self.live.send(Request::KeepAwake {
                        on: crate::notify::AWAKE.load(std::sync::atomic::Ordering::Relaxed),
                    });
                    self.waiting
                        .insert(self.live.send(Request::Models), Waiting::Models);
                    self.waiting
                        .insert(self.live.send(Request::Projects), Waiting::Projects);
                    self.fetch_usage();
                    self.fetch_recent();
                    self.fetch_commands(EVERY);
                }
                Incoming::Down(why) => {
                    if self.connection.is_ok() {
                        self.drops += 1;
                    }
                    self.connection = Err(why);
                }
                Incoming::Out { req, out } => {
                    if let Out::Events { events } = &out {
                        // A finished turn may have changed files.
                        for e in events {
                            // A waiting message being taken back: once it's
                            // out of the queue it's restored or steered;
                            // gone to the agent instead, it's left be.
                            match &e.body {
                                dent_protocol::Body::Unqueued { id } => {
                                    if let Some(tb) = self.taking_back.remove(id) {
                                        self.took_back(tb);
                                    }
                                }
                                dent_protocol::Body::Delivered { id } => {
                                    self.taking_back.remove(id);
                                }
                                _ => {}
                            }
                            if matches!(e.body, dent_protocol::Body::TurnEnded { .. })
                                && self.core.live
                            {
                                self.fetch_changes(&e.session);
                                // A command may have changed what's in
                                // effect, which the menu marks.
                                self.fetch_commands(&e.session);
                            }
                        }
                    }
                    if let Out::Commands { session, commands } = &out {
                        self.commands.insert(session.clone(), commands.clone());
                    }
                    if let Out::Sheet {
                        session, command, ..
                    } = &out
                    {
                        self.act_errors
                            .retain(|(s, c, _), _| !(s == session && c == command));
                    }
                    if let Out::Acted {
                        session,
                        command,
                        act,
                        url,
                        error,
                    } = &out
                    {
                        self.acted(session, command, act, url.as_deref(), error.as_deref(), cx);
                    }
                    if let Out::Models { models } = &out {
                        self.models = models.clone();
                        changed.extend(self.core.list.keys().cloned());
                    }
                    let first = !self.core.live;
                    for id in self.core.apply(&out) {
                        if !changed.contains(&id) {
                            changed.push(id);
                        }
                    }
                    if first
                        && self.core.live
                        && let Some(id) = self.selected.clone()
                    {
                        // Caught up: the open session's changes; others load
                        // when opened.
                        self.fetch_changes(&id);
                    }
                    if req != 0 {
                        self.reply(req, out, cx);
                    }
                }
            }
        }
        self.live
            .seq
            .store(self.core.last_seq, std::sync::atomic::Ordering::Relaxed);
        changed.sort();
        changed.dedup();
        for id in changed {
            self.rebuild(&id);
        }
        self.parse_ahead(cx);
        cx.notify();
    }

    /// Replies of turns that just finished, parsed on a background thread,
    /// so opening their session later parses nothing on the UI thread.
    fn parse_ahead(&mut self, cx: &mut Context<Self>) {
        let ids = std::mem::take(&mut self.parse_queue);
        let mut todo: Vec<(u64, String)> = Vec::new();
        for id in ids {
            let Some(s) = self.sessions.iter().find(|s| s.id == id).cloned() else {
                continue;
            };
            // The last turn's replies: back to your last message.
            let from = s
                .items
                .iter()
                .rposition(|i| matches!(i, Item::User { .. }))
                .unwrap_or(0);
            for i in &s.items[from..] {
                if let Item::Text(src) = i {
                    let key = crate::mdstream::Cache::key(src);
                    if !self.md.has(key) && !todo.iter().any(|(k, _)| *k == key) {
                        todo.push((key, src.clone()));
                    }
                }
            }
        }
        if todo.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_executor()
                .spawn(async move {
                    todo.into_iter()
                        .map(|(k, src)| (k, crate::markdown::parse(&src)))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, _| {
                for (k, blocks) in parsed {
                    this.md.put(k, blocks);
                }
            });
        })
        .detach();
    }

    fn reply(&mut self, req: u64, out: Out, cx: &mut Context<Self>) {
        let Some(w) = self.waiting.remove(&req) else {
            if let Out::Error { message } = out {
                self.toast(ToastAction::Info, message);
            }
            return;
        };
        match (w, out) {
            (
                Waiting::Models,
                Out::Reply {
                    reply: Reply::Models { models },
                },
            ) => {
                self.models = models;
                let ids: Vec<String> = self.core.list.keys().cloned().collect();
                for id in ids {
                    self.rebuild(&id);
                }
            }
            (
                Waiting::Commands(id),
                Out::Reply {
                    reply: Reply::Commands { commands },
                },
            ) => {
                // An agent that hasn't started yet has none to tell: ask again later.
                if !commands.is_empty() {
                    self.commands.insert(id, commands);
                }
            }
            (
                Waiting::Recent,
                Out::Reply {
                    reply: Reply::Recent { sessions },
                },
            ) => self.recent = sessions,
            (
                Waiting::Files(id),
                Out::Reply {
                    reply: Reply::Files { paths },
                },
            ) => {
                self.files
                    .insert(id, Rc::new(complete::with_folders(paths)));
            }
            (
                Waiting::Usage,
                Out::Reply {
                    reply: Reply::Usage { windows, at },
                },
            ) => {
                if !windows.is_empty() {
                    self.usage = Some((windows, at));
                }
                // The usage reading also learns the commands every session
                // starts with.
                if !self.commands.contains_key(EVERY) {
                    self.fetch_commands(EVERY);
                }
            }
            (
                Waiting::Projects,
                Out::Reply {
                    reply: Reply::Projects { projects },
                },
            ) => {
                self.projects = projects;
            }
            (
                Waiting::Folders(asked),
                Out::Reply {
                    reply: Reply::Folders { folders, .. },
                },
            ) => {
                // New rows for what's typed: the first one lit.
                self.folders = Some((asked, Ok(folders)));
                self.menu_ix = None;
                self.relight_menu(cx);
            }
            // A path being typed often isn't one yet: said in the picker,
            // never as a toast.
            (Waiting::Folders(asked), Out::Error { message }) => {
                self.folders = Some((asked, Err(message)));
                self.relight_menu(cx);
            }
            (
                Waiting::ProjectMade,
                Out::Reply {
                    reply: Reply::ProjectMade { path, warning },
                },
            ) => {
                self.project_busy = None;
                self.defaults.project = Some(path);
                self.menu = None;
                self.waiting
                    .insert(self.live.send(Request::Projects), Waiting::Projects);
                if let Some(w) = warning {
                    self.toast(ToastAction::Info, w);
                }
            }
            (Waiting::ProjectMade, Out::Error { message }) => {
                self.project_busy = None;
                self.project_error = Some(message);
                self.relight_menu(cx);
            }
            (
                Waiting::Changes(id, turn),
                Out::Reply {
                    reply: Reply::Changes { files },
                },
            ) if self.changes_turn.get(&id) == turn.as_ref() => {
                let diff: Vec<FileDiff> = files
                    .iter()
                    .map(|f| crate::data::parse_diff(&f.path, f.added, f.removed, &f.diff))
                    .collect();
                // Nothing left of the turn: back to all changes.
                if turn.is_some() && diff.is_empty() {
                    self.changes_turn.remove(&id);
                    self.fetch_changes(&id);
                    return;
                }
                // A file asked for before the changes came: show it now.
                let want = self.diff_want.remove(&id);
                if let Some(i) = want.and_then(|p| diff.iter().position(|f| f.path == p)) {
                    self.diff_file.insert(id.clone(), i);
                }
                self.changes.insert(id.clone(), Rc::new(diff));
                self.rebuild(&id);
            }
            (
                Waiting::Create(_),
                Out::Reply {
                    reply: Reply::Created { session },
                },
            ) => {
                self.creating = false;
                if let Some(agent) = self.adopting.take() {
                    self.recent.retain(|r| r.agent_session != agent);
                }
                self.pending_select = Some(session);
            }
            (
                Waiting::CreateAside(..),
                Out::Reply {
                    reply: Reply::Created { .. },
                },
            ) => {
                self.toast(
                    ToastAction::Info,
                    "Started a new session; it's in the sidebar".into(),
                );
            }
            (Waiting::CreateAside(from, typed), Out::Error { message }) => {
                self.restores.push((from, typed));
                self.toast(ToastAction::Info, message);
            }
            (Waiting::Create(typed), Out::Error { message }) => {
                self.creating = false;
                self.adopting = None;
                // What you typed and attached comes back, to try again.
                if let Some(d) = typed {
                    self.restores.push((None, d));
                }
                self.toast(ToastAction::Info, message);
            }
            // Put back: what Changes shows is read again.
            (Waiting::Discard(id), Out::Reply { .. }) => self.fetch_changes(&id),
            (Waiting::Discard(_), Out::Error { message }) => self.toast(ToastAction::Info, message),
            (Waiting::Requeue, _) => {}
            // Going back failed: nothing is sent again.
            (Waiting::Rewind(id), Out::Error { message }) => {
                if self.resend.as_ref().is_some_and(|r| r.0 == id) {
                    self.resend = None;
                }
                self.toast(ToastAction::Info, message);
            }
            // Already gone to the agent: it isn't sent again or brought back.
            (Waiting::Unqueue(msg), Out::Error { message }) => {
                // Refused because it went to the agent: let it be. Refused
                // for another reason (a resend after reconnecting, whose
                // first copy worked), its Unqueued may still be on the way,
                // so it stays waiting; `take_restores` brings it back if
                // nothing comes.
                let delivered = self.taking_back.get(&msg).is_some_and(|tb| {
                    self.core.list.get(&tb.session).is_some_and(|v| {
                        v.entries.iter().any(|e| {
                            matches!(e, dent_client_core::Entry::User { id, queued: false, .. } if *id == msg)
                        })
                    })
                });
                if delivered {
                    self.taking_back.remove(&msg);
                    self.toast(
                        ToastAction::Info,
                        "That message had already gone to the agent.".into(),
                    );
                } else if !self.taking_back.contains_key(&msg) {
                    self.toast(ToastAction::Info, message);
                }
            }
            (_, Out::Error { message }) => self.toast(ToastAction::Info, message),
            _ => {}
        }
        cx.notify();
    }

    /// A waiting message the daemon has taken out of its queue: into the
    /// running turn, or back to the reply box with its files.
    fn took_back(&mut self, tb: TakingBack) {
        match tb.then {
            Then::Steer => {
                let d = Draft::with(
                    tb.text,
                    tb.files
                        .into_iter()
                        .map(|path| Attachment::File { path })
                        .collect(),
                );
                self.send_draft(&tb.session, &d, true);
            }
            Then::Restore => self.restores.push((Some(tb.session), tb.back)),
        }
    }

    /// A message as it comes back to a box: as it was in the box when sent
    /// from here (chips and all), else its words and files.
    pub fn back_draft(&mut self, msg: &str, text: &str, files: &[String]) -> Draft {
        self.sent_drafts
            .get(&msg.to_string())
            .unwrap_or_else(|| Draft::of_sent(text, files))
    }

    /// Take waiting messages back out of the daemon's queue, each with its
    /// words, files and (for Edit) the place it goes back to. Nothing more
    /// happens until the daemon says each is out (see `took_back`).
    fn take_back(
        &mut self,
        session: &str,
        msgs: Vec<(String, String, Vec<String>, Option<String>)>,
        then: Then,
    ) {
        for (msg, text, files, place) in msgs {
            let back = Draft {
                place,
                ..self.back_draft(&msg, &text, &files)
            };
            self.taking_back.insert(
                msg,
                TakingBack {
                    session: session.to_string(),
                    text,
                    files,
                    back,
                    then,
                    at: Instant::now(),
                },
            );
        }
    }

    /// Put back what `restores` holds, after whatever is already typed:
    /// the open session's box, another session's draft, or the new-session
    /// field. Files come back attached.
    pub fn take_restores(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // While the daemon can't be reached, nothing waits in vain: its
        // answer comes after reconnecting (the request is sent again), so
        // the wait starts over.
        if self.connection.is_err() {
            for tb in self.taking_back.values_mut() {
                tb.at = Instant::now();
            }
        }
        // One the daemon never answered for comes back to the box rather
        // than being lost: never sent, it's still yours. One the log shows
        // went to the agent stays sent, never in the box as well.
        let stale: Vec<String> = self
            .taking_back
            .iter()
            .filter(|(_, tb)| tb.at.elapsed() >= DAEMON_WAIT)
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            if let Some(mut tb) = self.taking_back.remove(&id) {
                let delivered = self.core.list.get(&tb.session).is_some_and(|v| {
                    v.entries.iter().any(|e| {
                        matches!(e, dent_client_core::Entry::User { id: m, queued: false, .. } if *m == id)
                    })
                });
                if delivered {
                    self.toast(
                        ToastAction::Info,
                        "That message had already gone to the agent.".into(),
                    );
                    continue;
                }
                tb.then = Then::Restore;
                self.took_back(tb);
            }
        }
        for (session, d) in std::mem::take(&mut self.restores) {
            self.put_draft_in(session, Place::After, d, window, cx);
        }
    }

    /// Put draft `d` in a session's box (`None`: the new-session field),
    /// as an edit ⌘Z undoes: in the field if it shows (the open session's
    /// box, or the new-session field, which keeps its words while a session
    /// shows), else into the session's stored draft. Its files are attached
    /// there, and a place it carries goes with it. The one way dent puts
    /// words in a prompt; chips already there stay chips.
    pub fn put_draft_in(
        &mut self,
        session: Option<String>,
        place: Place,
        d: Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = session == self.selected;
        let field = match &session {
            None => Some(self.starter.clone()),
            Some(_) if open => Some(self.composer.clone()),
            Some(_) => None,
        };
        let key = session.clone().unwrap_or_else(|| NEW_DRAFT.to_string());
        match field {
            Some(field) => {
                let held = if open {
                    &mut self.attachments
                } else {
                    &mut self.drafts.entry(key).or_default().atts
                };
                for a in &d.atts {
                    if !held.contains(a) {
                        held.push(a.clone());
                    }
                }
                if open && session.is_some() && d.place.is_some() {
                    self.box_place = d.place.clone();
                }
                let pastes = self.pastes.clone();
                field.update(cx, |c, cx| {
                    crate::draft::put_in(c, place, &d, &pastes, true, window, cx);
                    if open {
                        c.focus_field(window, cx);
                    }
                });
                // It's yours again to send: no double-press pause on it.
                if open {
                    self.just_sent = None;
                }
            }
            None => {
                let first = place == Place::Before;
                self.drafts.entry(key).or_default().join(d, first);
            }
        }
        self.prefs_changed();
        cx.notify();
    }

    fn fetch_changes(&mut self, id: &str) {
        let turn = self.changes_turn.get(id).cloned();
        let n = self.live.send(Request::Changes {
            session: id.to_string(),
            turn: turn.clone(),
        });
        self.waiting
            .insert(n, Waiting::Changes(id.to_string(), turn));
    }

    /// Review one turn's changes (its message's id), or with `None` all of
    /// them, opening `path` once they're here.
    pub fn review_turn(
        &mut self,
        session: &str,
        turn: Option<String>,
        path: Option<String>,
        cx: &mut Context<Self>,
    ) {
        match turn {
            Some(t) => self.changes_turn.insert(session.to_string(), t),
            None => self.changes_turn.remove(session),
        };
        self.discarding = None;
        if let Some(p) = path {
            self.diff_want.insert(session.to_string(), p);
        }
        self.fetch_changes(session);
        self.show_tab(Tab::Changes, cx);
    }

    /// Whether what Changes shows can be put back: everything, or the
    /// latest turn's, and not while the agent is working on the files.
    pub fn can_discard(&self, s: &Session) -> Result<(), &'static str> {
        if s.running() {
            return Err("Wait for the turn to finish");
        }
        match self.changes_turn.get(&s.id) {
            Some(t)
                if s.items.iter().rev().find_map(|i| match i {
                    Item::User { id, .. } => Some(id),
                    _ => None,
                }) != Some(t) =>
            {
                Err("A later turn built on this one; discard from all changes")
            }
            _ => Ok(()),
        }
    }

    /// Put a file (or one hunk of it) back, after you've said so in place.
    pub fn discard(
        &mut self,
        session: &str,
        path: &str,
        hunk: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if self.pressed_again(format!("discard/{session}/{path}/{hunk:?}")) {
            return;
        }
        self.discarding = None;
        let n = self.live.send(Request::Discard {
            session: session.to_string(),
            path: path.to_string(),
            hunk: hunk.map(|h| h as u32),
            turn: self.changes_turn.get(session).cloned(),
        });
        self.waiting
            .insert(n, Waiting::Discard(session.to_string()));
        cx.notify();
    }

    /// Keys the app handles before any text field or binding sees them:
    /// the slash menu, pasting pictures, undoing a close. Returns whether
    /// the key was used.
    pub fn intercept(
        &mut self,
        k: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Your terminal takes every key but ⌘ while it has focus.
        if self.shell_key(k, window, cx) {
            return true;
        }
        if !k.modifiers.platform
            && !k.modifiers.control
            && (k.key.chars().count() == 1 || matches!(k.key.as_str(), "space" | "backspace"))
            && self.field_focused(window, cx) == Some(Field::Starter)
        {
            self.ambient.borrow_mut().key();
        }
        let plain = !k.modifiers.modified();
        // ⇧Tab in the reply box or a new session's field steps the mode:
        // Ask, Edits, Plan, round again. Full is never a keystroke away;
        // it's chosen from the chip.
        if k.key == "tab"
            && k.modifiers.shift
            && !k.modifiers.platform
            && !k.modifiers.control
            && !k.modifiers.alt
            && let Some(f @ (Field::Composer | Field::Starter)) = self.field_focused(window, cx)
        {
            // From the mode last asked for, if the daemon hasn't said yet:
            // two quick presses step twice.
            let now = match (f, self.current()) {
                (Field::Composer, Some(s)) => self.permission_shown(s),
                _ => self.defaults.permission,
            };
            let next = match now {
                Permission::Ask => Permission::Edits,
                Permission::Edits => Permission::Plan,
                Permission::Plan | Permission::Full => Permission::Ask,
            };
            if f == Field::Composer && self.selected.is_some() {
                self.set_permission(next, cx);
            } else {
                self.defaults.permission = next;
                cx.notify();
            }
            return true;
        }
        // ? with no text field in use: the shortcuts sheet.
        if k.key_char.as_deref() == Some("?")
            && !k.modifiers.platform
            && !k.modifiers.control
            && self.field_focused(window, cx).is_none()
        {
            self.toggle_keys(cx);
            return true;
        }
        // In ⌘F's field, Return goes to the next match and ⇧Return back.
        if k.key == "enter"
            && !k.modifiers.platform
            && self.find_input.read(cx).focus_handle(cx).is_focused(window)
        {
            self.step_find(k.modifiers.shift, window, cx);
            return true;
        }
        // A picture open large: Space closes it (Esc is the Escape action's),
        // ← and → step through its set, and plain keys don't reach what's
        // behind it.
        if self.lightbox.is_some() && plain && k.key != "escape" {
            match k.key.as_str() {
                "space" => {
                    self.close_lightbox(cx);
                }
                // Through the set, a picture at a time.
                "left" => {
                    self.step_lightbox(-1, cx);
                }
                "right" => {
                    self.step_lightbox(1, cx);
                }
                _ => {}
            }
            return true;
        }
        let field = self.field_focused(window, cx);
        let in_composer = field == Some(Field::Composer);
        // The field a prompt is typed in, whichever page it's on.
        let in_prompt = field.is_some_and(|f| self.field_entity(f) == self.prompt());
        // The ⌘K palette: arrows move through the results, and Tab stays put
        // rather than leaving for the composer behind it.
        if self.palette.is_some() && plain && matches!(k.key.as_str(), "up" | "down" | "tab") {
            let n = self.palette_len(cx);
            let at = self.palette.unwrap_or(0);
            self.palette = Some(match k.key.as_str() {
                "up" => at.saturating_sub(1),
                "down" => (at + 1).min(n.saturating_sub(1)),
                _ => at,
            });
            cx.notify();
            return true;
        }
        // A page in the viewer with the keyboard does its own editing keys.
        if k.modifiers.platform
            && !k.modifiers.alt
            && !k.modifiers.control
            && matches!(k.key.as_str(), "c" | "x" | "v" | "a" | "z")
            && let Some(p) = &self.preview
        {
            let clip = (k.key == "v")
                .then(|| cx.read_from_clipboard().and_then(|c| c.text()))
                .flatten();
            if p.borrow().edit(&k.key, k.modifiers.shift, clip) {
                return true;
            }
        }
        // The editing keys (⌘A, ⌘C, ⌘X) go where you last were: text
        // selected in the chat or viewer if that was the last thing you
        // pressed, else the field with the keyboard, as in any Mac app.
        let cmd_only =
            k.modifiers.platform && !k.modifiers.shift && !k.modifiers.alt && !k.modifiers.control;
        let editing = cmd_only && matches!(k.key.as_str(), "a" | "c" | "x");
        let in_text = field.is_some() || self.term_focus.is_focused(window);
        // Typing in a field, or any key there but these, makes it the
        // field's turn.
        if in_text && !editing {
            crate::select::yield_keys();
        }
        let selection_keys = !in_text || crate::select::holds_keys();
        // ⌘A takes all of what's selected in; with no text field in use,
        // all of the chat.
        if cmd_only && k.key == "a" && selection_keys {
            let area = crate::select::selected_area().or_else(|| {
                field
                    .is_none()
                    .then(|| self.selected.clone())
                    .flatten()
                    .map(|id| crate::select::area(&format!("chat/{id}")))
            });
            if let Some(area) = area
                && crate::select::select_all(area, window)
            {
                return true;
            }
        }
        // ⌘C copies the selection in the chat or viewer.
        if cmd_only && k.key == "c" && selection_keys && crate::select::has_selection() {
            let text = crate::select::copied();
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                return true;
            }
        }
        // ⌘C or ⌘X in a field with nothing selected does nothing: the
        // clipboard keeps what you copied before.
        if cmd_only
            && matches!(k.key.as_str(), "c" | "x")
            && let Some(f) = field
            && self.field_entity(f).read(cx).selected_range().is_empty()
        {
            return true;
        }
        // A menu that searches: Tab goes into the lit folder, ⌘1–⌘9 choose
        // the rows showing them, ⌘⌫ with nothing typed takes the lit row off
        // the list.
        if let Some(which) = self.menu
            && which.searches()
        {
            if plain && k.key == "tab" {
                self.project_tab(window, cx);
                cx.notify();
                return true;
            }
            let cmd = k.modifiers.platform
                && !k.modifiers.shift
                && !k.modifiers.alt
                && !k.modifiers.control;
            if cmd && let Some(d) = k.key.parse::<usize>().ok().filter(|d| (1..=9).contains(d)) {
                let cap = format!("⌘ {d}");
                if let Some(e) = self
                    .menu_entries(which, cx)
                    .into_iter()
                    .find(|e| e.keycap.as_deref() == Some(cap.as_str()))
                {
                    self.choose_entry(&e, window, cx);
                }
                return true;
            }
            // Only with nothing typed: there ⌘⌫ deletes what you typed.
            if cmd
                && k.key == "backspace"
                && self.menu_query.read(cx).value().is_empty()
                && let Some(i) = self.menu_ix
                && let Some((_, remove)) = self
                    .menu_entries(which, cx)
                    .get(i)
                    .and_then(|e| e.remove.clone())
            {
                remove(self, window, cx);
                return true;
            }
        }
        // A chip's menu over a prompt: typing there puts the menu away and
        // types; the menu keeps its own keys, Space (it chooses) included.
        if let Some(which) = self.menu
            && !which.searches()
            && matches!(field, Some(Field::Composer | Field::Starter))
            && k.key != "space"
            && !k.modifiers.platform
            && !k.modifiers.control
            && k.key_char
                .as_deref()
                .is_some_and(|c| !c.is_empty() && !c.chars().any(char::is_control))
        {
            self.menu = None;
            self.menu_ix = None;
            cx.notify();
            return false;
        }
        // An open menu: arrows move, Return chooses, and Space too unless
        // the menu has a field to type it in, or was opened by ⌘S from the
        // box you're typing in (there a space is a space).
        if let Some(which) = self.menu
            && plain
            && matches!(k.key.as_str(), "up" | "down" | "enter" | "space")
            && !(k.key == "space" && (which.searches() || which == Menu::Stash))
        {
            let entries = self.menu_entries(which, cx);
            let ok: Vec<usize> = (0..entries.len())
                .filter(|i| entries[*i].choosable())
                .collect();
            if ok.is_empty() {
                return true;
            }
            let at = self.menu_ix.and_then(|i| ok.iter().position(|x| *x == i));
            match k.key.as_str() {
                "up" => {
                    let n = at.map_or(ok.len() - 1, |p| p.saturating_sub(1));
                    self.menu_ix = Some(ok[n]);
                    self.menu_scroll.scroll_to_item(ok[n]);
                }
                "down" => {
                    let n = at.map_or(0, |p| (p + 1).min(ok.len() - 1));
                    self.menu_ix = Some(ok[n]);
                    self.menu_scroll.scroll_to_item(ok[n]);
                }
                _ => {
                    if let Some(e) = self.menu_ix.and_then(|i| entries.get(i)).cloned() {
                        self.choose_entry(&e, window, cx);
                    }
                }
            }
            cx.notify();
            return true;
        }
        // ⌥⌘↑ and ⌥⌘↓ step between sessions, from a text field too.
        if k.modifiers.alt
            && k.modifiers.platform
            && !k.modifiers.shift
            && !k.modifiers.control
            && matches!(k.key.as_str(), "up" | "down")
        {
            self.step_selection(if k.key == "up" { -1 } else { 1 }, false, window, cx);
            return true;
        }
        // The prompt's menu: arrows move round it, Tab or Return picks.
        // Return sends instead when the command is typed in full, or when
        // the lit row was only found by its description: Return never
        // swaps what you typed for a different name unless you chose it
        // (0043). An `@` word has no exception: Return picks the lit file.
        if plain
            && matches!(k.key.as_str(), "up" | "down" | "tab" | "enter")
            && in_prompt
            && let Some(c) = self.completion(cx)
            && !c.items.is_empty()
        {
            let n = c.items.len();
            let at = self.slash_ix.min(n - 1);
            let typed = &self.prompt().read(cx).value()[c.token.start..c.token.end];
            let slash = c.token.kind == complete::Kind::Slash;
            let arg = c.token.kind == complete::Kind::Arg;
            // Typed in full, Return sends what's typed.
            let whole = slash && c.items.iter().any(|i| typed == format!("/{}", i.name))
                || arg
                    && !typed.is_empty()
                    && c.items
                        .iter()
                        .any(|i| typed == i.name && !i.more && !i.open);
            let named = c.items[at]
                .name
                .to_lowercase()
                .contains(&c.token.query.to_lowercase());
            let chosen = self.slash_ix > 0;
            let send = whole || slash && !named && !chosen;
            if !(k.key == "enter" && send) {
                match k.key.as_str() {
                    "up" => self.slash_ix = (at + n - 1) % n,
                    "down" => self.slash_ix = (at + 1) % n,
                    _ => {
                        let pick = c.items[at].name.clone();
                        self.pick_completion(&pick, window, cx);
                    }
                }
                cx.notify();
                return true;
            }
        }
        // Moving the caret can move it into or out of a word to complete.
        if in_prompt
            && matches!(
                k.key.as_str(),
                "left" | "right" | "home" | "end" | "up" | "down" | "backspace" | "delete"
            )
        {
            cx.notify();
        }
        // Up in an empty composer brings back what you sent, to edit.
        if plain
            && in_composer
            && matches!(k.key.as_str(), "up" | "down")
            && self.step_recall(k.key == "up", window, cx)
        {
            return true;
        }
        // Alt+Up and Alt+Down move between your own messages, while the
        // composer has nothing to edit.
        if k.modifiers.alt
            && !k.modifiers.platform
            && !k.modifiers.shift
            && matches!(k.key.as_str(), "up" | "down")
            && (in_composer || field.is_none())
            && self.composer.read(cx).value().is_empty()
            && self.jump_to_mine(k.key == "up", cx)
        {
            return true;
        }
        // ⌘S in a prompt puts the draft aside, or with the box empty brings
        // one back. Always claimed, so it never does anything else there.
        if k.key == "s"
            && cmd_only
            && let Some(f @ (Field::Composer | Field::Starter)) = field
        {
            self.stash_key(f, window, cx);
            return true;
        }
        // ⇧⌘V in a prompt pastes text as it is, never folded into a chip:
        // a long paste to edit in place. Pictures and files attach as with ⌘V.
        if k.key == "v"
            && k.modifiers.platform
            && k.modifiers.shift
            && !k.modifiers.alt
            && !k.modifiers.control
            && self.palette.is_none()
            && let Some(f @ (Field::Composer | Field::Starter)) = field
        {
            // Pictures and files first, in the same order as ⌘V.
            if self.paste_attachment(window, cx) {
                return true;
            }
            if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                self.field_entity(f).clone().update(cx, |c, cx| {
                    c.replace(text, window, cx);
                    cx.emit(InputEvent::Change);
                });
                return true;
            }
            return false;
        }
        // ⌘V with a picture or files on the clipboard attaches them.
        if k.key == "v"
            && k.modifiers.platform
            && !k.modifiers.shift
            && self.palette.is_none()
            && matches!(field, None | Some(Field::Composer | Field::Starter))
            && (self.paste_attachment(window, cx) || self.paste_folded(field, window, cx))
        {
            return true;
        }
        // Right after closing, ⌘Z brings the session back even from a
        // text field, as long as the field has nothing to undo.
        let undo = k.key == "z" && k.modifiers.platform && !k.modifiers.shift;
        let closing = self.toast.as_ref().is_some_and(|t| t.action.undoes());
        // Only from no field, or from an empty composer or new-session box:
        // anywhere else ⌘Z is the field's own undo.
        let typed = match field {
            None => false,
            Some(Field::Composer) => !self.composer.read(cx).value().is_empty(),
            Some(Field::Starter) => !self.starter.read(cx).value().is_empty(),
            Some(_) => true,
        };
        if undo && closing && !typed {
            self.toast_action(window, cx);
            return true;
        }
        false
    }

    /// A waiting card's keys: a question's numbers (outside a text field),
    /// and an approval's or plan's keys while its card has the keyboard.
    /// Called where the key's own `held` flag is known (the window's
    /// capture of key-downs), which is the one test for a held key: a card
    /// is never answered by one, nor in its first moment on screen, since
    /// what it allows can't be taken back. Returns whether the key was used.
    pub fn card_key(
        &mut self,
        k: &Keystroke,
        held: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let plain = !k.modifiers.modified();
        let card_keys = !held
            && self
                .ask_shown
                .as_ref()
                .is_some_and(|(_, at)| at.elapsed() >= Duration::from_millis(150));
        // A question's options answer to their number keys, but only while
        // the keyboard isn't in a text field: there a number is a number,
        // so an answer like "2 days" types.
        if plain
            && self.field_focused(window, cx).is_none()
            && k.key.len() == 1
            && card_keys
            && let Some(d) = k.key.chars().next().and_then(|c| c.to_digit(10))
            && d > 0
            && let Some(id) = self.selected.clone()
            && let Some(Item::Question { id: ask, questions }) = self.open_ask(&id)
        {
            let picks = self.picks.get(&ask).cloned().unwrap_or_default();
            let q = (0..questions.len())
                .find(|q| picks.get(*q).is_none_or(Option::is_none))
                .unwrap_or(0);
            if let Some((label, _)) = questions.get(q).and_then(|q| q.options.get(d as usize - 1)) {
                let label = label.clone();
                self.pick_option(&id, &ask, q, label, cx);
                return true;
            }
        }
        // An approval or a plan answers to its keys while its card has the
        // keyboard (Esc from the empty reply box gets there): never from
        // a stray key elsewhere, as what it allows can't be taken back.
        if plain
            && self.ask_focus.is_focused(window)
            && let Some(id) = self.selected.clone()
            && let Some(item @ (Item::Approval { .. } | Item::Plan { .. })) = self.open_ask(&id)
        {
            // A held or early key does nothing here, and goes nowhere else.
            if !card_keys {
                return matches!(k.key.as_str(), "enter" | "1" | "2" | "3" | "4");
            }
            let plan = matches!(item, Item::Plan { .. });
            let keeps = matches!(item, Item::Approval { keeps: true, .. });
            // The keys follow the buttons, left to right.
            match (k.key.as_str(), plan) {
                ("enter" | "1", _) => self.allow(&id, None, Scope::Once, cx),
                ("2", false) => self.allow(&id, None, Scope::Session, cx),
                ("3", false) if keeps => self.allow(&id, None, Scope::Project, cx),
                ("2", true) | ("4", false) => self.deny(&id, None, window, cx),
                _ => return false,
            }
            return true;
        }
        // On the card, Return finishes a question that takes several.
        if plain
            && k.key == "enter"
            && self.ask_focus.is_focused(window)
            && card_keys
            && let Some(id) = self.selected.clone()
            && let Some(Item::Question { id: ask, questions }) = self.open_ask(&id)
        {
            let picks = self.picks.get(&ask).cloned().unwrap_or_default();
            if let Some(q) = (0..questions.len())
                .find(|q| picks.get(*q).is_none_or(Option::is_none))
                .filter(|q| questions[*q].multi)
            {
                self.finish_ticks(&id, &ask, q, cx);
                return true;
            }
        }
        false
    }

    /// Notes when the waiting approval, plan or question first showed.
    pub fn note_ask_shown(&mut self) {
        let ask = self
            .selected
            .clone()
            .and_then(|id| self.open_ask(&id))
            .and_then(|i| match i {
                Item::Approval { id, .. } | Item::Plan { id, .. } | Item::Question { id, .. } => {
                    Some(id)
                }
                _ => None,
            });
        if self.ask_shown.as_ref().map(|(a, _)| a) != ask.as_ref() {
            self.ask_shown = ask.map(|a| (a, Instant::now()));
        }
    }

    /// Which of dent's text fields has the keyboard, if any.
    pub fn field_focused(&self, window: &Window, cx: &App) -> Option<Field> {
        let on = |e: &Entity<TextareaState>| e.read(cx).focus_handle(cx).is_focused(window);
        if on(&self.composer) {
            Some(Field::Composer)
        } else if on(&self.starter) {
            Some(Field::Starter)
        } else if on(&self.aside_input) {
            Some(Field::Aside)
        } else if on(&self.comment_input) {
            Some(Field::Comment)
        } else if on(&self.search) {
            Some(Field::Search)
        } else if on(&self.rename_input) {
            Some(Field::Rename)
        } else if on(&self.find_input) {
            Some(Field::Find)
        } else if on(&self.menu_query) {
            Some(Field::Menu)
        } else {
            None
        }
    }

    /// Every text field, for what touches them all (the theme's colours).
    pub fn fields(&self) -> [&Entity<TextareaState>; 8] {
        [
            &self.composer,
            &self.starter,
            &self.comment_input,
            &self.aside_input,
            &self.search,
            &self.rename_input,
            &self.menu_query,
            &self.find_input,
        ]
    }

    /// Wear the theme you chose, or the Mac's: everything retints, and
    /// what was laid out in the old colours is laid out again.
    pub fn apply_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let system = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let dark = self
            .forced_dark
            .or(self.appearance.dark())
            .unwrap_or(system);
        if dark == self.t.dark {
            return;
        }
        self.t = if dark {
            Theme::carbon()
        } else {
            Theme::paper()
        };
        for input in self.fields() {
            crate::style_input(input, &self.t, cx);
        }
        self.md.clear();
        cx.notify();
    }

    /// Choose light, dark, or the Mac's appearance.
    pub fn set_appearance(
        &mut self,
        a: crate::prefs::Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance = a;
        self.forced_dark = None;
        self.apply_theme(window, cx);
        self.prefs_changed();
        cx.notify();
    }

    /// ⌘+ and ⌘−: one step bigger or smaller; ⌘0 (`by` 0): as designed.
    pub fn step_zoom(&mut self, by: i32, cx: &mut Context<Self>) {
        let now = crate::ds::zoom();
        let at = crate::ds::ZOOMS
            .iter()
            .position(|z| (*z - now).abs() < 0.001)
            .unwrap_or(2) as i32;
        let next = if by == 0 {
            1.
        } else {
            crate::ds::ZOOMS[(at + by).clamp(0, crate::ds::ZOOMS.len() as i32 - 1) as usize]
        };
        if (next - now).abs() < 0.001 {
            return;
        }
        crate::ds::set_zoom(next);
        // Everything measured at the old size is measured again.
        self.md.clear();
        self.transcript.remeasure();
        self.diff_list.remeasure();
        self.prefs_changed();
        cx.notify();
    }

    pub fn field_entity(&self, f: Field) -> &Entity<TextareaState> {
        match f {
            Field::Composer => &self.composer,
            Field::Starter => &self.starter,
            Field::Aside => &self.aside_input,
            Field::Comment => &self.comment_input,
            Field::Search => &self.search,
            Field::Rename => &self.rename_input,
            Field::Find => &self.find_input,
            Field::Menu => &self.menu_query,
        }
    }

    /// Rows in the ⌘K palette: the matches, then "Start a session" when
    /// something is typed.
    pub fn palette_len(&self, cx: &App) -> usize {
        if self.palette_mine {
            return self.palette_messages(cx).len();
        }
        let typed = !self.search.read(cx).value().trim().is_empty();
        self.palette_results(cx).len() + self.palette_files(cx).len() + usize::from(typed)
    }

    /// Files in the open session's project matching what's typed in ⌘K,
    /// listed under the sessions; one opens in the viewer.
    pub fn palette_files(&self, cx: &App) -> Vec<String> {
        let q = self.search.read(cx).value().trim().to_string();
        if q.is_empty() || q.contains(char::is_whitespace) {
            return Vec::new();
        }
        let Some(list) = self.current().and_then(|s| self.files.get(&s.id)) else {
            return Vec::new();
        };
        complete::files(list, &q)
            .into_iter()
            .map(|c| c.name)
            .filter(|p| !p.ends_with('/'))
            .take(6)
            .collect()
    }

    /// Scroll to your previous or next message. Returns whether there was one.
    pub fn jump_to_mine(&mut self, up: bool, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.current() else {
            return false;
        };
        let mine: Vec<usize> = s
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| matches!(i, Item::User { .. }))
            .map(|(ix, _)| ix)
            .collect();
        let top = self.transcript.logical_scroll_top().item_ix;
        let at_end = self.transcript.is_following_tail();
        let to = if up {
            mine.iter().rev().find(|ix| **ix < top).copied()
        } else if at_end {
            None
        } else {
            mine.iter().find(|ix| **ix > top).copied()
        };
        let Some(ix) = to else {
            // Down past your last message: on to the end, following again.
            if !up && !at_end {
                self.follow();
                cx.notify();
                return true;
            }
            return false;
        };
        // Reading back pauses following; reaching the end picks it up again.
        self.transcript.set_follow_mode(FollowMode::Tail);
        self.transcript.pause_following_tail();
        self.transcript.scroll_to(ListOffset {
            item_ix: ix,
            offset_in_item: px(0.),
        });
        cx.notify();
        true
    }

    /// Ask for sessions started in a terminal.
    pub fn fetch_recent(&mut self) {
        let n = self.live.send(Request::Recent);
        self.waiting.insert(n, Waiting::Recent);
    }

    /// Carry on a terminal session here; it opens once it's in.
    /// The row says "Opening…" until it's in, and stays if it fails.
    pub fn adopt(&mut self, agent_session: &str, cx: &mut Context<Self>) {
        if self.creating() {
            return;
        }
        let n = self.live.send(Request::Adopt {
            agent_session: agent_session.to_string(),
        });
        self.waiting.insert(n, Waiting::Create(None));
        self.creating = true;
        self.creating_at = Instant::now();
        self.adopting = Some(agent_session.to_string());
        cx.notify();
    }

    /// Ask the daemon for the account's usage.
    pub fn fetch_usage(&mut self) {
        let n = self.live.send(Request::Usage);
        self.waiting.insert(n, Waiting::Usage);
    }

    /// The newest usage reading, from the provider or a session's stream.
    /// A usage limit that's used up: what it's called and when it starts
    /// over (Unix seconds). With several, the one that ends last.
    pub fn limit_hit(&self) -> Option<(&'static str, i64)> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        self.limits()
            .iter()
            .filter(|w| w.used >= 0.995 && w.resets_at > now)
            .max_by_key(|w| w.resets_at)
            .map(|w| {
                let name = if w.name == "five_hour" {
                    "5-hour limit"
                } else if w.name.contains("opus") {
                    "weekly Opus limit"
                } else if w.name.contains("sonnet") {
                    "weekly Sonnet limit"
                } else if w.name.starts_with("seven_day") {
                    "weekly limit"
                } else {
                    "usage limit"
                };
                (name, w.resets_at)
            })
    }

    pub fn limits(&self) -> Vec<dent_protocol::Window> {
        let stream = self.core.limits.as_ref();
        match (&self.usage, stream) {
            (Some((w, at)), Some((sw, sat))) => {
                if at >= sat {
                    w.clone()
                } else {
                    sw.clone()
                }
            }
            (Some((w, _)), None) => w.clone(),
            (None, Some((sw, _))) => sw.clone(),
            (None, None) => Vec::new(),
        }
    }

    /// Ask for a session's slash commands, once it has some; for `EVERY`,
    /// the ones a session starts with.
    pub fn fetch_commands(&mut self, id: &str) {
        let n = self.live.send(Request::Commands {
            session: (id != EVERY).then(|| id.to_string()),
        });
        self.waiting.insert(n, Waiting::Commands(id.to_string()));
    }

    /// The field a prompt is typed in: the open session's composer, or the
    /// new session's.
    pub fn prompt(&self) -> &Entity<TextareaState> {
        if self.current_rc().is_some() {
            &self.composer
        } else {
            &self.starter
        }
    }

    /// Where a new session's files are kept, by its folder.
    fn folder_key(&self) -> String {
        let dir = self
            .defaults
            .project
            .clone()
            .unwrap_or_else(|| crate::scratch_dir().display().to_string());
        format!("dir:{dir}")
    }

    /// The files an `@` offers: the session's, or the new session's folder's.
    fn mention_files(&self) -> Option<&Rc<Vec<String>>> {
        match self.current_rc() {
            Some(s) => self.files.get(&s.id),
            None => self.files.get(&self.folder_key()),
        }
    }

    /// What the prompt could complete at its caret: commands for a `/`
    /// opening it, files and folders for an `@` anywhere. `None` while
    /// nothing is being typed, or Esc put this one away.
    pub fn completion(&self, cx: &App) -> Option<Completion> {
        let field = self.prompt().read(cx);
        let token = complete::token(&field.value(), field.cursor())?;
        if self.slash_dismissed == Some((token.kind, token.start)) {
            return None;
        }
        let mut title = None;
        let (items, loading) = match token.kind {
            complete::Kind::Slash => {
                // The agent's own list for this session, or the one every
                // session starts with; dent's commands are there either way.
                let agent = self
                    .selected
                    .as_ref()
                    .and_then(|s| self.commands.get(s))
                    .or_else(|| self.commands.get(EVERY));
                let all = complete::offered(
                    agent.map(Vec::as_slice).unwrap_or_default(),
                    self.current_rc().is_some(),
                );
                let rows = complete::commands(&all, &token.query)
                    .into_iter()
                    .map(complete::Row::from)
                    .collect();
                (rows, agent.is_none())
            }
            complete::Kind::Mention => {
                // The project's files, then other sessions whose title
                // matches: one picked goes with the message as context.
                let sessions = self.session_rows(&token.query);
                match self.mention_files() {
                    Some(files) => {
                        let mut rows: Vec<complete::Row> = complete::files(files, &token.query)
                            .into_iter()
                            .map(complete::Row::from)
                            .collect();
                        rows.extend(sessions);
                        (rows, false)
                    }
                    None => (sessions, true),
                }
            }
            // What the command takes here; words of its own open nothing.
            complete::Kind::Arg => {
                let cmd = self
                    .selected
                    .as_ref()
                    .and_then(|s| self.commands.get(s))
                    .or_else(|| self.commands.get(EVERY))?
                    .iter()
                    .find(|c| c.name == token.command)?;
                title = Some(cmd.description.clone()).filter(|d| !d.is_empty());
                (
                    complete::arg_rows(&cmd.args, &token.before, &token.query)?,
                    false,
                )
            }
        };
        Some(Completion {
            token,
            items,
            loading,
            title,
        })
    }

    /// Esc on the prompt's menu: it stays away until the caret leaves its
    /// word. True if there was one to put away. Esc is handled in one place,
    /// the Escape action, because macOS hands a field's Esc back as a
    /// command after any interceptor has seen it.
    pub fn dismiss_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(c) = self.completion(cx) else {
            return false;
        };
        self.slash_dismissed = Some((c.token.kind, c.token.start));
        cx.notify();
        true
    }

    /// After the prompt changes: the highlight goes back to the top, a
    /// put-away menu comes back once its word is gone or erased to its sign,
    /// and a new session's
    /// files are asked for when an `@` needs them.
    pub fn prompt_changed(&mut self, cx: &mut Context<Self>) {
        self.slash_ix = 0;
        let field = self.prompt().read(cx);
        let token = complete::token(&field.value(), field.cursor());
        // A waiting message taken back to edit and then cleared away has
        // no place to go back to: what's typed next is a new message.
        if self.selected.is_some() && self.composer.read(cx).value().trim().is_empty() {
            self.box_place = None;
        }
        self.prefs_dirty = true;
        // Erased back to its bare `/` or `@`, the word starts over.
        if token.as_ref().map(|t| (t.kind, t.start)) != self.slash_dismissed
            || token.as_ref().is_some_and(|t| t.query.is_empty())
        {
            self.slash_dismissed = None;
        }
        if token.is_some_and(|t| t.kind == complete::Kind::Mention) && self.current_rc().is_none() {
            let key = self.folder_key();
            let asked = self
                .waiting
                .values()
                .any(|w| matches!(w, Waiting::Files(f) if *f == key));
            if !self.files.contains_key(&key) && !asked {
                let dir = key.trim_start_matches("dir:").to_string();
                let n = self.live.send(Request::FilesIn { dir });
                self.waiting.insert(n, Waiting::Files(key));
            }
        }
        cx.notify();
    }

    /// Sessions other than the open one whose title holds every word typed
    /// after `@`, newest first, a few at most; none until something's typed.
    fn session_rows(&self, query: &str) -> Vec<complete::Row> {
        let q = query.to_lowercase();
        if q.trim().is_empty() || q.contains('/') {
            return Vec::new();
        }
        self.sessions
            .iter()
            .filter(|s| self.selected.as_deref() != Some(s.id.as_str()))
            .filter(|s| {
                let title = s.title.to_lowercase();
                q.split_whitespace().all(|w| title.contains(w))
            })
            .take(SESSION_ROWS)
            .map(|s| complete::Row {
                name: format!("session:{}", s.id),
                label: s.title.clone(),
                about: format!("Session · {} · {}", s.project, crate::data::ago(s.updated)),
                session: Some(s.id.clone()),
                ..Default::default()
            })
            .collect()
    }

    /// Attach another session as context: what was said in it, as Markdown,
    /// with where it ran, so the agent can read it (and look further).
    pub fn attach_session(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.session(id) else { return };
        let mut md = format!(
            "# {}\n\nAnother dent session, attached as context. It worked in `{}`; its id is `{}`{}.\n",
            s.title,
            s.cwd,
            s.id,
            s.resume_command()
                .map(|c| format!(", and `{c}` carries it on in a terminal"))
                .unwrap_or_default(),
        );
        for i in s.items.iter() {
            match i {
                Item::User { text, .. } => md.push_str(&format!("\n## You\n\n{text}\n")),
                Item::Text(t) => md.push_str(&format!("\n## Agent\n\n{t}\n")),
                _ => {}
            }
        }
        // The start and the end say most; a long one keeps both.
        if md.len() > SESSION_CONTEXT {
            let head: String = md.chars().take(SESSION_CONTEXT / 4).collect();
            let tail: String = {
                let n = md.chars().count();
                md.chars().skip(n - SESSION_CONTEXT * 3 / 4).collect()
            };
            md = format!("{head}\n\n… (the middle is left out) …\n\n{tail}");
        }
        let dir = std::env::temp_dir().join("dent-sessions");
        let name: String = s
            .title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let path = dir.join(format!(
            "{}.md",
            if name.is_empty() { "Session" } else { &name }
        ));
        if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&path, md).is_ok() {
            self.attach_paths(&[path], window, cx);
        } else {
            self.toast(ToastAction::Info, "Couldn't attach that session".into());
        }
    }

    /// Use the chosen completion: a file or command takes the word's place;
    /// a folder too, leaving the menu open inside it; dent's own commands
    /// do their thing.
    pub fn pick_completion(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.completion(cx) else {
            return;
        };
        let field = self.prompt().clone();
        let text = field.read(cx).value().to_string();
        let row = c
            .items
            .iter()
            .find(|r| r.name == name)
            .cloned()
            .unwrap_or_default();
        // A session goes as an attachment; its `@words` leave the box.
        if let Some(id) = &row.session {
            let pastes = self.pastes.clone();
            field.update(cx, |f, cx| {
                crate::draft::put_in(
                    f,
                    Place::At(c.token.start..c.token.end),
                    &Draft::default(),
                    &pastes,
                    true,
                    window,
                    cx,
                )
            });
            self.attach_session(id, window, cx);
            cx.notify();
            return;
        }
        // A value of your own: the menu steps aside while you type it.
        if c.token.kind == complete::Kind::Arg && row.open && !row.name.ends_with('=') {
            self.slash_dismissed = Some((c.token.kind, c.token.start));
            cx.notify();
            return;
        }
        let own =
            c.token.kind == complete::Kind::Slash && complete::OWN.iter().any(|(n, ..)| *n == name);
        // A choice with nothing after it is the whole command: it runs.
        let runs = c.token.kind == complete::Kind::Arg
            && !row.more
            && !row.then_text
            && !row.name.ends_with('=');
        let (with, finished) = match c.token.kind {
            _ if own => (String::new(), false),
            complete::Kind::Slash => (format!("/{name}"), true),
            complete::Kind::Mention => (complete::mention(name), !name.ends_with('/')),
            // `key=` stays in its word, for the value.
            complete::Kind::Arg => (name.to_string(), !name.ends_with('=')),
        };
        // Only the word changes, so chips elsewhere in the box stay chips
        // and ⌘Z takes the pick back.
        let (put, caret) = complete::apply_at(&text, &c.token, &with, finished);
        let lead = text.len() - text.trim_start().len();
        let pastes = self.pastes.clone();
        field.update(cx, |f, cx| {
            let word = c.token.start..c.token.end;
            crate::draft::put_in(
                f,
                Place::At(word),
                &Draft::words(put),
                &pastes,
                true,
                window,
                cx,
            );
            // A command opens the message: spaces before it go.
            if lead > 0 && c.token.start >= lead {
                crate::draft::put_in(
                    f,
                    Place::At(0..lead),
                    &Draft::default(),
                    &pastes,
                    true,
                    window,
                    cx,
                );
            }
            let caret = caret - if c.token.start >= lead { lead } else { 0 };
            f.set_selected_range(caret..caret, cx);
            f.focus_field(window, cx);
        });
        self.slash_ix = 0;
        if own {
            self.run_own(name, "", window, cx);
        } else if runs {
            self.submit_prompt(window, cx);
        }
        cx.notify();
    }

    /// Send what the prompt holds: into the open session, or as a new one.
    pub fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.current_rc().is_some() {
            self.send(window, cx);
        } else {
            self.start(window, cx);
        }
    }

    /// Do one of dent's own commands, with what was typed after it.
    pub fn run_own(&mut self, name: &str, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let session = self.current_rc();
        let which = |open, new| if session.is_some() { open } else { new };
        match name {
            "model" => self.open_menu(which(Menu::SessionModel, Menu::Model), window, cx),
            "effort" => self.open_menu(which(Menu::SessionEffort, Menu::Effort), window, cx),
            "rename" => {
                if let Some(s) = session {
                    if arg.is_empty() {
                        self.start_rename(&s.id, &s.title, window, cx);
                    } else if s.title != arg {
                        self.live.send(Request::Rename {
                            session: s.id.clone(),
                            title: arg.to_string(),
                        });
                    }
                }
            }
            "clear" => {
                // A fresh start in the same folder (as any new session from
                // here is); this session stays.
                self.select(None, window, cx);
            }
            _ => {}
        }
    }

    /// A sent message that is one of dent's own commands, done here rather
    /// than sent to the agent. True if it was one.
    pub fn sent_own(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // One line that is a command, by the one rule for that.
        let Some(name) = crate::data::command_name(text) else {
            return false;
        };
        let arg = &text[1 + name.len()..];
        let in_session = self.current_rc().is_some();
        if !complete::OWN
            .iter()
            .any(|(n, _, needs)| *n == name && (in_session || !needs))
        {
            return false;
        }
        let name = name.to_string();
        // ↑ brings it back like anything else you sent here, in order
        // among them: stamped on the daemon's clock (just after its last
        // event), which is what your sent messages carry, not this
        // machine's. Only the latest few are kept.
        if let Some(id) = self.selected.clone() {
            let at = self.session(&id).map_or(0, |s| s.updated + 1);
            let mine = self.ran_own.entry(id).or_default();
            mine.push((at, text.to_string()));
            if mine.len() > 50 {
                mine.remove(0);
            }
        }
        self.prompt()
            .clone()
            .update(cx, |f, cx| put(f, "", window, cx));
        self.run_own(&name, arg.trim(), window, cx);
        true
    }

    /// Send a request, noting what its reply is for.
    pub fn ask(&mut self, req: Request, w: Waiting) {
        let n = self.live.send(req);
        self.waiting.insert(n, w);
    }

    /// Open a chip's menu, at its top with the current choice lit; one
    /// that searches with its field empty and the keyboard in it.
    pub fn open_menu(&mut self, which: Menu, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_scroll = ScrollHandle::new();
        if which.searches() {
            self.open_project_picker(window, cx);
        }
        self.menu_ix = self.menu_entries(which, cx).iter().position(|e| e.on);
        self.menu = Some(which);
        cx.notify();
    }

    /// After a menu's rows change under it: the current choice lit, or
    /// the first that can be.
    pub fn relight_menu(&mut self, cx: &mut Context<Self>) {
        let Some(which) = self.menu else { return };
        let entries = self.menu_entries(which, cx);
        let lit = self
            .menu_ix
            .and_then(|i| entries.get(i))
            .is_some_and(|e| e.choosable());
        if !lit {
            self.menu_ix = entries
                .iter()
                .position(|e| e.on && e.choosable())
                .or_else(|| entries.iter().position(|e| e.choosable()));
        }
    }

    /// Draw a session again from its timeline.
    fn rebuild(&mut self, id: &str) {
        let Some(v) = self.core.list.get_mut(id) else {
            return;
        };
        let from = v.take_changes();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let prev = self.sessions.iter().position(|s| s.id == id);
        // Drop the old drawing's hold on the items so they extend in place.
        let old = prev.map(|i| {
            let old = &self.sessions[i];
            (old.status, old.items.len(), old.made_sig)
        });
        if let Some(i) = prev {
            self.sessions[i] = Rc::new(Session::placeholder(id));
        }
        let first = !self.built.contains_key(id);
        let built = self.built.entry(id.to_string()).or_default();
        let v = &self.core.list[id];
        let mut s = crate::bridge::session(
            v,
            crate::bridge::Context {
                machine: &self.machine,
                models: &self.models,
                diff: self.changes.get(id).cloned(),
                now_ms,
            },
            built,
            if first { Some(0) } else { from },
        );
        // An earlier version, being looked at: drawn from that version,
        // read-only, the session's state otherwise as it is.
        if let Some(&b) = self.old_version.get(id)
            && let Some(old) = v.with_branch(b)
        {
            let then = crate::bridge::session(
                &old,
                crate::bridge::Context {
                    machine: &self.machine,
                    models: &self.models,
                    diff: None,
                    now_ms,
                },
                &mut crate::bridge::Built::default(),
                Some(0),
            );
            s.items = then.items;
            s.versions = crate::bridge::versions(v, Some(b));
            s.old = Some(v.branches[b].seq);
        }
        let made_changed = old.is_none_or(|(_, _, sig)| sig != s.made_sig);
        let partial_len = v.partial.len();
        let partial_now = v.partial.clone();
        let arts = if made_changed {
            crate::bridge::artifacts(v)
        } else {
            Vec::new()
        };
        // Its dev servers are pages too: live, with no versions.
        for url in &s.servers {
            let id = crate::bridge::art_id(&s.id, url);
            if !self.arts.iter().any(|a| a.id == id) {
                self.arts.push(Artifact {
                    id,
                    kind: crate::data::Kind::Server,
                    title: crate::data::intern(&format!(
                        "Dev server · {}",
                        url.trim_start_matches("http://").trim_end_matches('/')
                    )),
                    path: crate::data::intern(&reachable(url)),
                    dir: None,
                    versions: Vec::new(),
                });
            }
        }
        // New versions of what it made show straight away when followed.
        for a in arts {
            match self.arts.iter_mut().find(|x| x.id == a.id) {
                Some(old) if old.versions.len() != a.versions.len() => {
                    self.viewing.remove(a.id);
                    self.updated_at.insert(a.id, Instant::now());
                    self.pictures.retain(|(art, _), _| *art != a.id);
                    *old = a;
                }
                Some(old) => *old = a,
                None => {
                    // A file it just made comes forward, unless you're in
                    // your terminal or a side chat.
                    let fresh = a.id;
                    self.arts.push(a);
                    if !first && !matches!(self.tabs.get(id), Some(Tab::Terminal | Tab::Aside)) {
                        self.tabs.insert(id.to_string(), Tab::Art(fresh));
                        if let Some(c) = self.closed_tabs.get_mut(id) {
                            c.remove(fresh);
                        }
                    }
                }
            }
        }
        // A reply streaming in fades in word by word.
        let len = partial_len;
        let was = self
            .partial_seen
            .insert(id.to_string(), partial_now.clone())
            .unwrap_or_default();
        // More of the same reply, or a new one (its words all new).
        let more = crate::mdstream::new_words(&was, &partial_now);
        if len > 0 && (more.is_none() || len > was.len()) {
            let ix = s
                .items
                .iter()
                .rposition(|i| matches!(i, Item::Text(_)))
                .unwrap_or(0);
            let entry = self
                .births
                .entry(id.to_string())
                .or_insert((ix, Vec::new()));
            if entry.0 != ix || more.is_none() || was.is_empty() {
                *entry = (ix, Vec::new());
            }
            let now = Instant::now();
            let words = more
                .unwrap_or_else(|| partial_now.matches([' ', '\n']).count())
                .max(1);
            for _ in 0..words {
                entry.1.push((len, now));
            }
        } else if len == 0 {
            self.births.remove(id);
        }
        let old_status = old.map(|o| o.0);
        // Once stopped, the next turn can be stopped at once: the guard
        // against a second Stop is only for while the first is landing.
        if !s.running() {
            self.pressed.remove(&format!("stop/{id}"));
        }
        // Tell the person when something needs them or is done, unless
        // they're already looking at it.
        let looking = self.window_active && self.selected.as_deref() == Some(id);
        if self.core.live && !looking && old_status.is_some() && old_status != Some(s.status) {
            match s.status {
                Status::Needs => {
                    crate::notify::post("Needs you", &format!("{} · {}", s.title, s.detail), id)
                }
                // After the turn and anything it left running in the background.
                Status::Done if matches!(old_status, Some(Status::Working | Status::Waiting)) => {
                    crate::notify::post("Done", &format!("{} · {}", s.title, s.detail), id)
                }
                _ => {}
            }
        }
        if old_status != Some(s.status) {
            match s.status {
                Status::Done | Status::Needs if old_status.is_some() => {
                    self.finished_at.insert(id.to_string(), Instant::now());
                    // Not open: parse its replies ahead, off the UI thread.
                    if self.selected.as_deref() != Some(id) {
                        self.parse_queue.push(id.to_string());
                    }
                }
                _ => {}
            }
            if old_status.is_none() && self.core.live {
                self.arrived_at.insert(id.to_string(), Instant::now());
            }
        }
        match s.working_since {
            Some(at) => {
                self.working_since.insert(id.to_string(), at);
            }
            None => {
                self.working_since.remove(id);
                // Its working line is gone: words waiting to show would
                // otherwise keep asking for frames with nothing to draw.
                self.live_words.borrow_mut().remove(id);
            }
        }
        let following = self.selected.as_deref() == Some(id);
        match prev {
            Some(i) => self.sessions[i] = Rc::new(s),
            None => self.sessions.push(Rc::new(s)),
        }
        if following {
            self.mark_seen();
        }
        // The Dock says how many need you, as the status bar does.
        let needs = self.needs().len();
        if self.badged != Some(needs) {
            self.badged = Some(needs);
            crate::notify::badge(needs);
        }
        // Newest first; the inbox groups them by what they need.
        self.sessions.sort_by_key(|s| std::cmp::Reverse(s.updated));
        if following && self.transcript.is_following_tail() {
            self.follow();
        }
    }

    /// Every draft as it stands: the stored ones, with the open box's and
    /// the new-session field's read from their fields.
    pub fn drafts_now(&self, cx: &App) -> std::collections::BTreeMap<String, Draft> {
        let mut all: std::collections::BTreeMap<String, Draft> = self
            .drafts
            .iter()
            .map(|(k, d)| (k.clone(), d.clone()))
            .collect();
        let new_atts = match &self.selected {
            None => self.attachments.clone(),
            Some(_) => all
                .get(NEW_DRAFT)
                .map(|d| d.atts.clone())
                .unwrap_or_default(),
        };
        all.insert(
            NEW_DRAFT.to_string(),
            Draft::read(self.starter.read(cx), new_atts),
        );
        if let Some(id) = &self.selected {
            all.insert(
                id.clone(),
                Draft::read(self.composer.read(cx), self.attachments.clone()),
            );
        }
        all.retain(|_, d| !d.is_empty());
        all
    }

    /// Something the prefs keep changed: they're written shortly, once,
    /// however many changes come meanwhile.
    pub fn prefs_changed(&mut self) {
        self.prefs_dirty = true;
    }

    /// Each frame: notice a change to the small prefs (layout, choices)
    /// cheaply, and write the prefs half a second after the last change,
    /// never while a menu is open or something is dragged.
    pub fn prefs_tick(&mut self, cx: &mut Context<Self>) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            &self.defaults.project,
            &self.defaults.model,
            format!("{:?}", self.defaults.permission),
            &self.defaults.effort,
        )
            .hash(&mut h);
        (self.panel_width.to_bits(), self.sidebar_w.to_bits()).hash(&mut h);
        (
            self.sidebar_off,
            format!("{:?}", self.appearance),
            &self.pinned,
        )
            .hash(&mut h);
        crate::ds::zoom().to_bits().hash(&mut h);
        let mut opened: Vec<_> = self.opened.iter().collect();
        opened.sort();
        opened.hash(&mut h);
        let mut closed: Vec<(&String, Vec<(&&str, &usize)>)> = self
            .closed_tabs
            .iter()
            .map(|(k, v)| {
                let mut v: Vec<_> = v.iter().collect();
                v.sort();
                (k, v)
            })
            .collect();
        closed.sort();
        closed.hash(&mut h);
        self.attachments.len().hash(&mut h);
        for a in &self.attachments {
            a.path().hash(&mut h);
        }
        let light = h.finish();
        if light != self.prefs_light {
            self.prefs_light = light;
            self.prefs_dirty = true;
        }
        if !self.prefs_dirty || self.prefs_timer || self.walk.is_some() {
            return;
        }
        self.prefs_timer = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.prefs_timer = false;
                if this.menu.is_none() && !this.dragging && !this.dragging_sidebar {
                    this.save_prefs(cx);
                } else {
                    // Tried again after the menu closes or the drag ends.
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Write the prefs now, if they changed.
    pub fn save_prefs(&mut self, cx: &App) {
        self.prefs_dirty = false;
        let drafts = self.drafts_now(cx);
        let held: std::collections::HashSet<u32> = drafts
            .values()
            .chain(&self.stashes)
            .flat_map(|d| d.pastes())
            .collect();
        let saved = |d: &Draft| Draft {
            // Only files are kept across quitting; notes go with the run.
            atts: d
                .atts
                .iter()
                .filter(|a| a.path().is_some())
                .cloned()
                .collect(),
            ..d.clone()
        };
        let p = crate::prefs::Prefs {
            version: crate::prefs::VERSION,
            project: self.defaults.project.clone(),
            model: self.defaults.model.clone(),
            permission: self.defaults.permission,
            effort: self.defaults.effort.clone(),
            panel_width: Some(self.panel_width),
            sidebar_width: Some(self.sidebar_w),
            sidebar_off: self.sidebar_off,
            quiet: !crate::notify::SOUND.load(std::sync::atomic::Ordering::Relaxed),
            sleepy: !crate::notify::AWAKE.load(std::sync::atomic::Ordering::Relaxed),
            hotkey: Some(crate::hotkey::chosen()).filter(|h| h != crate::hotkey::DEFAULT),
            appearance: self.appearance,
            pinned: self.pinned.clone(),
            open_with: crate::open::chosen(),
            ambient: crate::ambient::chosen().map(|s| s.id.to_string()),
            zoom: Some(crate::ds::zoom()).filter(|z| *z != 1.),
            opened: self
                .opened
                .iter()
                .map(|(sid, ids)| {
                    (
                        sid.clone(),
                        ids.iter()
                            .filter_map(|a| self.art(a).map(|x| x.path.to_string()))
                            .collect(),
                    )
                })
                .collect(),
            closed_tabs: self
                .closed_tabs
                .iter()
                .filter(|(_, c)| !c.is_empty())
                .map(|(id, c)| {
                    (
                        id.clone(),
                        c.iter().map(|(a, n)| (a.to_string(), *n)).collect(),
                    )
                })
                .collect(),
            drafts: drafts
                .iter()
                .map(|(k, d)| (k.clone(), saved(d)))
                .filter(|(_, d)| !d.is_empty())
                .collect(),
            attached: Default::default(),
            // Only the pastes a draft still holds (kept in files of their
            // own, beside the prefs).
            pastes: self
                .pastes
                .iter()
                .filter(|(n, _)| held.contains(n))
                .map(|(n, full)| (*n, full.clone()))
                .collect(),
            stashes: self.stashes.iter().map(saved).collect(),
        };
        if self.saved_prefs.as_ref() != Some(&p) {
            crate::prefs::save(&p);
            self.saved_prefs = Some(p);
        }
    }

    /// A session's folder as a new session's project: the scratch folder is
    /// "No project".
    pub fn project_for(&self, cwd: &str) -> Option<String> {
        (std::path::Path::new(cwd) != crate::scratch_dir()).then(|| cwd.to_string())
    }

    /// Models to offer: the agent's own list, from the daemon.
    pub fn models_or_default(&self) -> Vec<Model> {
        self.models.clone()
    }

    pub fn project_name(path: &str) -> String {
        path.rsplit('/').next().unwrap_or(path).to_string()
    }

    // ---- Sending, starting, stopping ----

    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.send_with(false, window, cx);
    }

    /// Send the composer's message. While the agent works it waits for the
    /// turn to end, unless `steer` sends it into the turn now.
    pub fn send_with(&mut self, steer: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let mut d = self.box_draft(cx);
        // Folded pastes go in full.
        let text = d.words_out(&self.pastes);
        if text.is_empty() && d.atts.is_empty() {
            return;
        }
        // `/model` and the like are dent's to do, not the agent's.
        if d.atts.is_empty() && self.sent_own(&text, window, cx) {
            return;
        }
        self.attachments.clear();
        self.composer.update(cx, |c, cx| put(c, "", window, cx));
        self.drafts.remove(&id);
        self.recall = None;
        d.place = self.box_place.take().filter(|_| !steer);
        // Send wakes up again when the pause ends (see `deadlines`).
        self.just_sent = Some(Instant::now());
        // Typing while it asks a question answers it; while it asks to run
        // something, the text is the reason for denying; on a plan, it's
        // what to change. That's Return's (and the send button's): ⌘↩ only
        // ever steers.
        if !steer && d.atts.is_empty() {
            if self.answer_with_text(&id, &text, cx) {
                return;
            }
            if let Some(Item::Approval { id: ask, .. } | Item::Plan { id: ask, .. }) =
                self.open_ask(&id)
            {
                self.reply_to_ask(&id, ask, Answer::Deny { message: text }, cx);
                return;
            }
        }
        self.send_draft(&id, &d, steer);
        cx.notify();
    }

    /// What the open prompt holds, with what rides along: the one reader
    /// every send, deny and answer uses, so a folded paste always goes in
    /// full.
    pub fn box_draft(&self, cx: &App) -> Draft {
        Draft::read(self.prompt().read(cx), self.attachments.clone())
    }

    /// Send a message to a session: the one way a message leaves, from the
    /// box, Send now, Try again, Undo, a command or one dent writes. Its
    /// chips open in full, its notes become lines after its words, its
    /// files go as attachments; one with a place waits before that
    /// message again, while it's still waiting and the turn still runs.
    /// Returns the message's id.
    pub fn send_draft(&mut self, session: &str, d: &Draft, steer: bool) -> String {
        let msg = crate::message_id();
        self.live.send(Request::Send {
            session: session.to_string(),
            id: msg.clone(),
            text: d.message(&self.pastes),
            steer,
            attachments: d.files(),
        });
        if !d.chips.is_empty() {
            self.sent_drafts.insert(
                msg.clone(),
                Draft {
                    place: None,
                    ..d.clone()
                },
            );
        }
        self.sent_at = Some((session.to_string(), msg.clone(), Instant::now()));
        let place = d.place.clone().filter(|next| {
            !steer
                && self.session(session).is_some_and(|s| {
                    s.running()
                        && s.items
                            .iter()
                            .any(|i| matches!(i, Item::Queued { id, .. } if id == next))
                })
        });
        if let Some(next) = place {
            let n = self.live.send(Request::Requeue {
                session: session.to_string(),
                id: msg.clone(),
                before: Some(next),
            });
            self.waiting.insert(n, Waiting::Requeue);
        }
        self.follow();
        msg
    }

    /// Send a command to a session as if typed, leaving any draft alone.
    pub fn run_command(
        &mut self,
        session: &str,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_draft(session, &Draft::words(text), false);
        cx.notify();
    }

    /// Start a new session from the starter box with the chosen defaults.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.box_draft(cx);
        // Folded pastes go in full.
        let text = typed.words_out(&self.pastes);
        // Files alone can start one, as they can be sent in a session.
        if text.is_empty() && typed.atts.is_empty() || self.creating() {
            return;
        }
        if typed.atts.is_empty() && self.sent_own(&text, window, cx) {
            return;
        }
        self.ambient.borrow_mut().submit();
        let d = self.defaults.clone();
        let cwd = match d.project {
            Some(p) => p,
            None => {
                let scratch = crate::scratch_dir();
                let _ = std::fs::create_dir_all(&scratch);
                scratch.display().to_string()
            }
        };
        self.attachments.clear();
        let n = self.live.send(Request::Create {
            cwd,
            model: d.model,
            permission: d.permission,
            effort: d.effort,
            id: crate::message_id(),
            text: typed.message(&self.pastes),
            attachments: typed.files(),
        });
        // Back to the field, chips and files, if it can't be made.
        self.waiting.insert(n, Waiting::Create(Some(typed)));
        self.creating = true;
        self.creating_at = Instant::now();
        self.starter.update(cx, |c, cx| put(c, "", window, cx));
        cx.notify();
    }

    /// Keep a message as a Claude Code skill, named from its first words:
    /// in `project`'s `.claude/skills`, or your own (`~/.claude/skills`) for
    /// every project. It's in the `/` menu from the next look.
    pub fn save_skill(&mut self, words: &str, project: Option<&str>, cx: &mut Context<Self>) {
        let root = match project {
            Some(p) => std::path::PathBuf::from(p),
            None => std::path::PathBuf::from(home()),
        };
        match crate::skills::save(&root.join(".claude").join("skills"), words) {
            Ok(name) => {
                self.toast(
                    ToastAction::Info,
                    format!("Saved as /{name}; it's in the / menu"),
                );
                self.commands.clear();
                if let Some(id) = self.selected.clone() {
                    self.fetch_commands(&id);
                }
                self.fetch_commands(EVERY);
            }
            Err(e) => self.toast(ToastAction::Info, format!("Couldn't save the skill: {e}")),
        }
        cx.notify();
    }

    /// Step to the version before (`older`) or after the one showing of
    /// your message at timeline place `at`; past the last, the latest.
    pub fn step_version(&mut self, id: &str, at: usize, older: bool, cx: &mut Context<Self>) {
        let Some(v) = self.core.list.get(id) else {
            return;
        };
        let group: Vec<usize> = (0..v.branches.len())
            .filter(|&i| v.branches[i].at == at)
            .collect();
        let now = match self.old_version.get(id) {
            Some(b) => group.iter().position(|i| i == b).unwrap_or(group.len()),
            None => group.len(),
        };
        let next = if older {
            now.checked_sub(1)
        } else {
            Some(now + 1)
        };
        match next {
            Some(n) if n < group.len() => {
                self.old_version.insert(id.to_string(), group[n]);
            }
            Some(_) => {
                self.old_version.remove(id);
            }
            None => return,
        }
        self.read_at.remove(id);
        self.rebuild(id);
        cx.notify();
    }

    /// Back to the latest version from an earlier one.
    pub fn latest_version(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.old_version.remove(id).is_some() {
            self.rebuild(id);
            cx.notify();
        }
    }

    /// Carry on from the earlier version showing, as a new session.
    pub fn fork_version(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(seq) = self.session(id).and_then(|s| s.old) else {
            return;
        };
        if self.creating() {
            return;
        }
        self.creating = true;
        self.creating_at = Instant::now();
        let n = self.live.send(Request::Fork {
            session: id.to_string(),
            before: None,
            until: Some(seq),
            id: Some(crate::message_id()),
        });
        self.waiting.insert(n, Waiting::Create(None));
        self.old_version.remove(id);
        self.rebuild(id);
        cx.notify();
    }

    /// ⇧⌘↩ in a session: send, then straight to a new session's field.
    pub fn send_and_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_none() {
            return self.start_aside(window, cx);
        }
        self.send_with(false, window, cx);
        self.select(None, window, cx);
    }

    /// ⌥⌘↩: what's typed starts a new session in the same project, and you
    /// stay where you are (in a session, or on the new-session page, ready
    /// for the next).
    pub fn start_aside(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.box_draft(cx);
        if typed.words_out(&self.pastes).is_empty() && typed.atts.is_empty() {
            return;
        }
        let from = self.selected.clone();
        let d = self.defaults.clone();
        let cwd = match from.as_deref().and_then(|id| self.session(id)) {
            Some(s) => s.cwd.clone(),
            None => match d.project.clone() {
                Some(p) => p,
                None => {
                    let scratch = crate::scratch_dir();
                    let _ = std::fs::create_dir_all(&scratch);
                    scratch.display().to_string()
                }
            },
        };
        self.attachments.clear();
        let n = self.live.send(Request::Create {
            cwd,
            model: d.model,
            permission: d.permission,
            effort: d.effort,
            id: crate::message_id(),
            text: typed.message(&self.pastes),
            attachments: typed.files(),
        });
        self.waiting.insert(n, Waiting::CreateAside(from, typed));
        let field = self.prompt().clone();
        field.update(cx, |c, cx| put(c, "", window, cx));
        cx.notify();
    }

    /// Whether session `id` has a turn to stop: one running, or one a
    /// message just sent is starting (the daemon starts it before it reads
    /// a stop sent after).
    pub fn stoppable(&self, id: &str) -> bool {
        self.session(id).is_some_and(|s| s.running())
            || self
                .sent_at
                .as_ref()
                .is_some_and(|(s, _, at)| s == id && at.elapsed() < STOPPABLE)
    }

    pub fn stop(&mut self, id: &str, by: &str, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.stoppable(id) {
            return;
        }
        let Some(s) = self.sessions.iter().find(|s| s.id == id).cloned() else {
            return;
        };
        // One stop a turn: a second press before it has stopped does
        // nothing, but a turn started since (a correction) stops at once.
        let pending = self
            .sent_at
            .as_ref()
            .filter(|(s, ..)| s == id)
            .map(|(_, m, _)| m.clone());
        let turn = pending.or_else(|| {
            s.items.iter().rev().find_map(|i| match i {
                Item::User { id, .. } => Some(id.clone()),
                _ => None,
            })
        });
        if self.pressed_again(format!("stop/{id}/{}", turn.unwrap_or_default())) {
            return;
        }
        // Stopping takes messages still waiting their turn out of the queue:
        // each the daemon says it took out comes back to the composer, files
        // and all, rather than vanishing.
        let waiting: Vec<(String, String, Vec<String>, Option<String>)> = s
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Queued {
                    id,
                    text,
                    attachments,
                } => Some((id.clone(), text.clone(), attachments.clone(), None)),
                _ => None,
            })
            .collect();
        self.take_back(id, waiting, Then::Restore);
        // The header offers Resume once it has stopped.
        self.live.send(Request::Interrupt {
            session: id.to_string(),
            by: Some(by.to_string()),
        });
        cx.notify();
    }

    /// After a stop: carry on from where it was.
    /// For actions that send something: true when `key` was pressed in the
    /// last few seconds, so a double-click doesn't do it twice.
    /// A new session is on its way (asked for in the last 30 seconds).
    pub fn creating(&self) -> bool {
        self.creating && self.creating_at.elapsed() < DAEMON_WAIT
    }

    pub fn pressed_again(&mut self, key: String) -> bool {
        let again = self.pressed_lately(&key);
        if !again {
            self.pressed
                .retain(|_, at| at.elapsed() < Duration::from_secs(3));
            self.pressed.insert(key, Instant::now());
        }
        again
    }

    /// Whether `key` was pressed in the last few seconds.
    pub fn pressed_lately(&self, key: &str) -> bool {
        self.pressed
            .get(key)
            .is_some_and(|at| at.elapsed() < Duration::from_secs(3))
    }

    pub fn resume(&mut self, id: &str, cx: &mut Context<Self>) {
        // A second press before it's running again would send it twice.
        if self.pressed_again(format!("resume/{id}")) {
            return;
        }
        self.live.send(Request::Send {
            session: id.to_string(),
            id: crate::message_id(),
            text: "Continue from where you stopped.".into(),
            steer: false,
            attachments: Vec::new(),
        });
        if self
            .toast
            .as_ref()
            .is_some_and(|t| t.action == ToastAction::Resume(id.to_string()))
        {
            self.toast = None;
        }
        cx.notify();
    }

    /// The session's open question or approval, by id.
    pub fn open_ask(&self, id: &str) -> Option<Item> {
        self.session(id)?
            .items
            .iter()
            .rev()
            .find(|i| {
                matches!(
                    i,
                    Item::Approval { .. } | Item::Question { .. } | Item::Plan { .. }
                )
            })
            .cloned()
    }

    fn reply_to_ask(&mut self, session: &str, ask: String, answer: Answer, cx: &mut Context<Self>) {
        // One answer per ask: the card stays until the agent hears it.
        if self.pressed_again(format!("ask/{session}/{ask}")) {
            return;
        }
        self.live.send(Request::Answer {
            session: session.to_string(),
            id: ask,
            answer,
        });
        cx.notify();
    }

    /// The ask a button belongs to (by its id), or with none given, the
    /// latest: buttons answer their own card, typing answers the newest.
    fn ask_for(&self, id: &str, ask: Option<&str>) -> Option<Item> {
        match ask {
            None => self.open_ask(id),
            Some(want) => self.session(id)?.items.iter().find_map(|i| match i {
                Item::Approval { id: a, .. }
                | Item::Plan { id: a, .. }
                | Item::Question { id: a, .. }
                    if a == want =>
                {
                    Some(i.clone())
                }
                _ => None,
            }),
        }
    }

    pub fn allow(&mut self, id: &str, ask: Option<&str>, scope: Scope, cx: &mut Context<Self>) {
        if let Some(Item::Approval { id: ask, .. } | Item::Plan { id: ask, .. }) =
            self.ask_for(id, ask)
        {
            let (always, project) = match scope {
                Scope::Once => (false, false),
                Scope::Session => (true, false),
                Scope::Project => (true, true),
            };
            self.reply_to_ask(id, ask, Answer::Allow { always, project }, cx);
        }
    }

    /// Deny, with what's in the composer as the reason the agent hears.
    pub fn deny(
        &mut self,
        id: &str,
        ask: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The box's words, folded pastes in full, as any send reads them.
        let reason = self.box_draft(cx).words_out(&self.pastes);
        if let Some(Item::Approval { id: ask, .. } | Item::Plan { id: ask, .. }) =
            self.ask_for(id, ask)
        {
            if self.pressed_lately(&format!("ask/{id}/{ask}")) {
                return;
            }
            if !reason.is_empty() {
                self.composer.update(cx, |c, cx| put(c, "", window, cx));
            }
            self.reply_to_ask(id, ask, Answer::Deny { message: reason }, cx);
        }
    }

    /// Pick an option for question `q`; when every question has an answer,
    /// the answers go.
    pub fn answer(
        &mut self,
        id: &str,
        ask: Option<&str>,
        q: usize,
        option: String,
        cx: &mut Context<Self>,
    ) {
        let Some(Item::Question { id: ask, questions }) = self.ask_for(id, ask) else {
            return;
        };
        let picks = self
            .picks
            .entry(ask.clone())
            .or_insert_with(|| vec![None; questions.len()]);
        if let Some(p) = picks.get_mut(q) {
            *p = Some(option);
        }
        if picks.iter().all(Option::is_some) {
            let answers = picks.iter().flatten().cloned().collect();
            self.picks.remove(&ask);
            self.reply_to_ask(id, ask, Answer::Choose { answers }, cx);
        }
        cx.notify();
    }

    /// A choice picked by click or number key. One of several is ticked or
    /// unticked; a single answer shows as chosen for a moment, then goes.
    pub fn pick_option(
        &mut self,
        id: &str,
        ask: &str,
        q: usize,
        option: String,
        cx: &mut Context<Self>,
    ) {
        let Some(Item::Question { questions, .. }) = self.ask_for(id, Some(ask)) else {
            return;
        };
        if questions.get(q).is_some_and(|q| q.multi) {
            let ticks = self.ticks.entry(format!("{ask}/{q}")).or_default();
            match ticks.iter().position(|t| *t == option) {
                Some(i) => {
                    ticks.remove(i);
                }
                None => ticks.push(option),
            }
            cx.notify();
            return;
        }
        // A second pick while one is showing waits for it.
        if self.flash.is_some() {
            return;
        }
        self.flash = Some((ask.to_string(), q, option.clone()));
        cx.notify();
        let (id, ask) = (id.to_string(), ask.to_string());
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.flash = None;
                this.answer(&id, Some(&ask), q, option, cx);
            });
        })
        .detach();
    }

    /// The ticked choices answer a question that takes several, in the
    /// order they're listed.
    pub fn finish_ticks(&mut self, id: &str, ask: &str, q: usize, cx: &mut Context<Self>) {
        let Some(Item::Question { questions, .. }) = self.ask_for(id, Some(ask)) else {
            return;
        };
        let ticked = self
            .ticks
            .get(&format!("{ask}/{q}"))
            .cloned()
            .unwrap_or_default();
        if ticked.is_empty() {
            return;
        }
        let answer = questions[q]
            .options
            .iter()
            .filter(|(label, _)| ticked.contains(label))
            .map(|(label, _)| label.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        self.ticks.remove(&format!("{ask}/{q}"));
        self.answer(id, Some(ask), q, answer, cx);
    }

    /// Typed text answers the next unanswered question.
    fn answer_with_text(&mut self, id: &str, text: &str, cx: &mut Context<Self>) -> bool {
        let Some(Item::Question { questions, id: ask }) = self.open_ask(id) else {
            return false;
        };
        let q = self
            .picks
            .get(&ask)
            .and_then(|p| p.iter().position(Option::is_none))
            .unwrap_or(0);
        if q >= questions.len() {
            return false;
        }
        // Only its choices answer it; the words are kept for after.
        if questions[q].fixed {
            return false;
        }
        self.answer(id, Some(&ask), q, text.to_string(), cx);
        true
    }

    /// ⌘V of long text in a prompt folds it into one line, which opens back
    /// up when sent (or when clicked). Returns whether it did.
    pub fn paste_folded(
        &mut self,
        field: Option<Field>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(f @ (Field::Composer | Field::Starter)) = field else {
            return false;
        };
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return false;
        };
        if !crate::compose::folds(&text) {
            return false;
        }
        let n = self.pastes.keys().next_back().map_or(1, |n| n + 1);
        let label = crate::compose::fold_label(n, &text);
        let token = InlineToken::new(format!("paste-{n}"), label)
            .with_label(crate::compose::chip_label(&text));
        Rc::make_mut(&mut self.pastes).insert(n, text);
        self.field_entity(f)
            .clone()
            .update(cx, |c, cx| c.replace_with_token(token, window, cx))
            .is_ok()
    }

    /// A folded paste clicked in a prompt opens to its full text there.
    pub fn open_paste(
        &mut self,
        field: &Entity<TextareaState>,
        id: &str,
        at: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(full) = id
            .strip_prefix("paste-")
            .and_then(|n| n.parse::<u32>().ok())
            .and_then(|n| self.pastes.get(&n))
            .cloned()
        else {
            return;
        };
        field.update(cx, |c, cx| {
            // On lines of its own, not run into the words around it.
            let (at, full) = crate::compose::opened(&c.value(), &at, &full);
            c.set_selected_range(at, cx);
            c.replace(full, window, cx);
            cx.emit(InputEvent::Change);
        });
    }

    /// Session `id`'s waiting message `msg`: its words and files.
    fn queued(&self, id: &str, msg: &str) -> Option<(String, Vec<String>)> {
        self.session(id)?.items.iter().find_map(|i| match i {
            Item::Queued {
                id,
                text,
                attachments,
            } if id == msg => Some((text.clone(), attachments.clone())),
            _ => None,
        })
    }

    /// Take back a message that hasn't gone yet. Once the daemon has it out
    /// of the queue, its words return to the composer, after anything
    /// already typed there, and its files come back attached; if it went
    /// to the agent first, it stays sent and a toast says so.
    pub fn unqueue(&mut self, id: &str, msg: &str, cx: &mut Context<Self>) {
        if self.pressed_again(format!("queued/{msg}")) {
            return;
        }
        let Some((text, files)) = self.queued(id, msg) else {
            return;
        };
        // Its place: sent again, it goes back before the one after it.
        let next = self.queued_after(id, msg);
        let n = self.live.send(Request::Unqueue {
            session: id.to_string(),
            id: msg.to_string(),
        });
        self.waiting.insert(n, Waiting::Unqueue(msg.to_string()));
        self.take_back(
            id,
            vec![(msg.to_string(), text, files, next)],
            Then::Restore,
        );
        cx.notify();
    }

    /// ⌘S in a prompt: a draft (its words, folded pastes and files) is put
    /// aside and the box emptied; in an empty box the one put aside comes
    /// back, or with several a menu of them, newest first.
    pub fn stash_key(&mut self, f: Field, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.field_entity(f).clone();
        // Notes (a comment, a picked element) stay with the box.
        let files: Vec<Attachment> = self
            .attachments
            .iter()
            .filter(|a| a.path().is_some())
            .cloned()
            .collect();
        let d = Draft::read(field.read(cx), files);
        if !d.is_empty() {
            self.stashes.push(d);
            self.attachments.retain(|a| a.path().is_none());
            field.update(cx, |c, cx| put(c, "", window, cx));
            self.recall = None;
            self.toast(
                ToastAction::Info,
                "Draft put aside. ⌘S in an empty box brings it back.".into(),
            );
            self.save_prefs(cx);
        } else {
            match self.stashes.len() {
                0 => self.toast(
                    ToastAction::Info,
                    "Nothing put aside: ⌘S with words in the box puts them aside.".into(),
                ),
                1 => self.unstash(0, window, cx),
                _ => {
                    self.menu_scroll = ScrollHandle::new();
                    self.menu = Some(Menu::Stash);
                    self.menu_ix = None;
                    self.relight_menu(cx);
                }
            }
        }
        cx.notify();
    }

    /// Bring back draft `i` put aside, into the prompt (after anything
    /// typed there since), chips and files with it.
    pub fn unstash(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i >= self.stashes.len() {
            return;
        }
        let d = self.stashes.remove(i);
        let target = self.selected.clone();
        self.put_draft_in(target, Place::After, d, window, cx);
        self.save_prefs(cx);
        cx.notify();
    }

    /// Take a waiting message out of the queue for good. A toast's Undo
    /// puts it back where it waited.
    pub fn remove_queued(&mut self, id: &str, msg: &str, cx: &mut Context<Self>) {
        if self.pressed_again(format!("queued/{msg}")) {
            return;
        }
        let Some((text, files)) = self.queued(id, msg) else {
            return;
        };
        let place = self.queued_after(id, msg);
        // As it was sent, chips and notes too: Undo sends it again so.
        let back = match self.sent_drafts.get(&msg.to_string()) {
            Some(d) => d,
            None => Draft::with(
                text,
                files
                    .into_iter()
                    .map(|path| Attachment::File { path })
                    .collect(),
            ),
        };
        let n = self.live.send(Request::Unqueue {
            session: id.to_string(),
            id: msg.to_string(),
        });
        self.waiting.insert(n, Waiting::Unqueue(msg.to_string()));
        self.toast = Some(Toast {
            text: "Removed from the queue".into(),
            action: ToastAction::Unremove {
                session: id.to_string(),
                back,
                place,
            },
            at: Instant::now(),
        });
        cx.notify();
    }

    /// The waiting message after `msg` in session `id`'s queue: where `msg`
    /// goes back to.
    fn queued_after(&self, id: &str, msg: &str) -> Option<String> {
        self.session(id).and_then(|s| {
            s.items
                .iter()
                .filter_map(|i| match i {
                    Item::Queued { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .skip_while(|q| q != msg)
                .nth(1)
        })
    }

    /// Go back to just before message `msg`: the chat, and with `files` the
    /// working tree. The message returns to the composer to edit.
    pub fn rewind(&mut self, msg: &str, files: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if self.pressed_again(format!("rewind/{msg}")) {
            return;
        }
        let sent = self.session(&id).and_then(|s| {
            s.items.iter().find_map(|i| match i {
                Item::User {
                    id: m, text, files, ..
                } if m == msg => Some((text.clone(), files.clone())),
                _ => None,
            })
        });
        self.live.send(Request::Rewind {
            session: id,
            to: msg.to_string(),
            files,
        });
        self.rewinding = None;
        if let Some((text, files)) = sent {
            let d = self.back_draft(msg, &text, &files);
            self.restore_message(d, true, window, cx);
        }
        cx.notify();
    }

    /// A message you sent, back in the composer to edit: the words come
    /// back to the box (with `keep_draft`, before what was typed there), and
    /// attached files come back attached; other attachment lines stay
    /// behind. Returns the files it attached.
    fn restore_message(
        &mut self,
        d: Draft,
        keep_draft: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let added: Vec<String> = d
            .files()
            .into_iter()
            .filter(|p| !self.attachments.iter().any(|a| a.path() == Some(p)))
            .collect();
        // Whatever was typed stays, after it, its pastes still folded; and
        // ⌘Z undoes the change.
        let place = if keep_draft {
            Place::Before
        } else {
            Place::Replace
        };
        let target = self.selected.clone();
        self.put_draft_in(target, place, d, window, cx);
        added
    }

    /// ↑ and ↓ in an empty composer, or while it shows a message brought
    /// back unchanged: a waiting message comes out of the queue first,
    /// then your messages, newest first. ↓ past the newest empties the box.
    /// Returns whether the key was used.
    fn step_recall(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.current_rc() else {
            return false;
        };
        let value = self.composer.read(cx).value().to_string();
        let browsing = self
            .recall
            .as_ref()
            .filter(|r| r.session == s.id && r.shown == value)
            .map(|r| r.back);
        let empty = value.is_empty() && self.attachments.is_empty();
        if browsing.is_none() && !(up && empty) {
            return false;
        }
        // A message of several lines, or one long enough to wrap: the
        // arrows walk its lines as they show first, and step through what
        // you sent only from the first line (↑) or the last (↓). The field
        // knows where its lines wrap, so it's asked to move the caret: one
        // already on its first or last line stays put.
        if !value.is_empty() {
            let before = self.composer.read(cx).cursor();
            let handle = self.composer.read(cx).focus_handle(cx);
            if up {
                handle.dispatch_action(&gpui_kit::base::input::MoveUp, window, cx);
            } else {
                handle.dispatch_action(&gpui_kit::base::input::MoveDown, window, cx);
            }
            if self.composer.read(cx).cursor() != before {
                return true;
            }
        }
        if up
            && browsing.is_none()
            && let Some(msg) = s.items.iter().rev().find_map(|i| match i {
                Item::Queued { id, .. } => Some(id.clone()),
                _ => None,
            })
        {
            self.unqueue(&s.id, &msg, cx);
            return true;
        }
        // Everything you sent, commands included (in the chat a command
        // that answered shows as its card, not your message), and dent's
        // own commands run here, newest first.
        let mut mine: Vec<(u64, Draft)> = self
            .core
            .list
            .get(&s.id)
            .map(|v| {
                v.entries
                    .iter()
                    .filter_map(|e| match e {
                        dent_client_core::Entry::User {
                            id,
                            text,
                            attachments,
                            queued: false,
                            at,
                            ..
                        } => Some((*at, id.clone(), text.clone(), attachments.clone())),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter()
            .map(|(at, id, text, files)| (at, self.back_draft(&id, &text, &files)))
            .collect();
        mine.extend(
            self.ran_own
                .get(&s.id)
                .into_iter()
                .flatten()
                .map(|(at, t)| (*at, Draft::words(t.clone()))),
        );
        // Newest first; where times tie, the later one written first.
        mine.reverse();
        mine.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        let mine: Vec<Draft> = mine.into_iter().map(|(_, d)| d).collect();
        let back = browsing.unwrap_or(0);
        let to = if up { back + 1 } else { back.saturating_sub(1) };
        if up && to > mine.len() {
            // Already at your first message: stay on it.
            return !mine.is_empty();
        }
        // What the last step attached goes with it.
        if let Some(r) = self.recall.take() {
            self.attachments
                .retain(|a| !matches!(a, Attachment::File { path } if r.atts.contains(path)));
        }
        if to == 0 {
            self.composer.update(cx, |c, cx| put(c, "", window, cx));
        } else {
            let atts = self.restore_message(mine[to - 1].clone(), false, window, cx);
            let shown = self.composer.read(cx).value().to_string();
            self.recall = Some(Recall {
                session: s.id.clone(),
                back: to,
                shown,
                atts,
            });
        }
        cx.notify();
        true
    }

    /// A new session carrying on from this one: everything so far, or up to
    /// just before message `before`. It opens when the daemon has made it.
    pub fn fork(&mut self, session: &str, before: Option<String>, cx: &mut Context<Self>) {
        // One fork per press, however fast the presses come.
        if self.creating() {
            return;
        }
        self.creating = true;
        self.creating_at = Instant::now();
        let n = self.live.send(Request::Fork {
            until: None,
            session: session.to_string(),
            before,
            id: Some(crate::message_id()),
        });
        self.waiting.insert(n, Waiting::Create(None));
        cx.notify();
    }

    /// Carry on after a turn that went wrong: a message asking the agent to
    /// pick up where it was cut off, naming any background work that
    /// stopped with it.
    pub fn carry_on(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.pressed_again(format!("carry-on/{id}")) {
            return;
        }
        let Some(s) = self.session(id) else { return };
        let lost = s
            .items
            .iter()
            .rev()
            .take_while(|i| !matches!(i, Item::User { .. }))
            .find_map(|i| match i {
                Item::Note { text, .. } => text
                    .strip_prefix("Background work stopped with the agent: ")
                    .map(|w| w.trim_end_matches('.').to_string()),
                _ => None,
            });
        let mut text = "You were cut off before the turn finished. Carry on from where you stopped, without redoing finished work.".to_string();
        if let Some(lost) = lost {
            text.push_str(&format!(
                " This background work stopped with you and isn't running now: {lost}. Start it again if it's still needed."
            ));
        }
        self.send_again(id, &text, &[], cx);
    }

    /// Have the session carry on by itself when the used-up limit resets
    /// (`at`, Unix seconds), or not (`None`).
    pub fn resume_at(&mut self, id: &str, at: Option<u64>, cx: &mut Context<Self>) {
        self.live.send(Request::Resume {
            session: id.to_string(),
            at,
        });
        cx.notify();
    }

    /// Send the last message again, after a turn that failed.
    pub fn retry(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.pressed_again(format!("retry/{id}")) {
            return;
        }
        let last = self.session(id).and_then(|s| {
            s.items.iter().rev().find_map(|i| match i {
                Item::User { text, files, .. } => Some((text.clone(), files.clone())),
                _ => None,
            })
        });
        if let Some((text, files)) = last {
            self.send_again(id, &text, &files, cx);
        }
    }

    /// Try again on a reply that went fine: back to before your last
    /// message, and send it again. When the turn changed files, it first
    /// asks whether to undo them too (`files` is the answer).
    pub fn try_again(&mut self, id: &str, files: Option<bool>, cx: &mut Context<Self>) {
        let Some(s) = self.session(id) else { return };
        let Some((msg, text, sent_files)) = s.items.iter().rev().find_map(|i| match i {
            Item::User {
                id, text, files, ..
            } => Some((id.clone(), text.clone(), files.clone())),
            _ => None,
        }) else {
            return;
        };
        let changed = s.checkpoints.contains(&msg)
            && s.items
                .iter()
                .rev()
                .take_while(|i| !matches!(i, Item::User { .. }))
                .any(|i| matches!(i, Item::Changed { .. } | Item::Made { .. }));
        let files = match files {
            Some(f) => f,
            None if changed => {
                self.trying_again = Some(id.to_string());
                cx.notify();
                return;
            }
            None => false,
        };
        if self.pressed_again(format!("try-again/{id}/{msg}")) {
            return;
        }
        self.trying_again = None;
        let n = self.live.send(Request::Rewind {
            session: id.to_string(),
            to: msg.clone(),
            files,
        });
        self.waiting.insert(n, Waiting::Rewind(id.to_string()));
        self.resend = Some((id.to_string(), msg, text, sent_files));
        cx.notify();
    }

    /// Once a session has gone back for Try again, its message goes again.
    pub fn take_resend(&mut self, cx: &mut Context<Self>) {
        let Some((id, msg, text, files)) = self.resend.clone() else {
            return;
        };
        let back = self.session(&id).is_some_and(|s| {
            !s.items
                .iter()
                .any(|i| matches!(i, Item::User { id, .. } if *id == msg))
        });
        if back {
            self.resend = None;
            self.send_again(&id, &text, &files, cx);
        }
    }

    /// A message of yours sent again as it was: attached files go as
    /// attachments, and comments or picked elements stay in its text.
    fn send_again(&mut self, id: &str, text: &str, files: &[String], cx: &mut Context<Self>) {
        let d = Draft::with(
            text,
            files
                .iter()
                .map(|p| Attachment::File { path: p.clone() })
                .collect(),
        );
        self.send_draft(id, &d, false);
        cx.notify();
    }

    /// Move a waiting message to just before another waiting one (or to the
    /// end): the order they'll go in.
    pub fn requeue(&mut self, id: &str, msg: &str, before: Option<String>, cx: &mut Context<Self>) {
        if self.pressed_again(format!("requeue/{id}/{msg}")) {
            return;
        }
        self.live.send(Request::Requeue {
            session: id.to_string(),
            id: msg.to_string(),
            before,
        });
        cx.notify();
    }

    /// A waiting message goes into the running turn now instead, files and
    /// all: once the daemon has taken it out of the queue, so a message
    /// that went meanwhile is never sent twice.
    pub fn steer_queued(&mut self, id: &str, msg: &str, cx: &mut Context<Self>) {
        if self.pressed_again(format!("queued/{msg}")) {
            return;
        }
        let Some((text, files)) = self.queued(id, msg) else {
            return;
        };
        let n = self.live.send(Request::Unqueue {
            session: id.to_string(),
            id: msg.to_string(),
        });
        self.waiting.insert(n, Waiting::Unqueue(msg.to_string()));
        self.take_back(id, vec![(msg.to_string(), text, files, None)], Then::Steer);
        cx.notify();
    }

    /// Files dropped on the window or pasted ride along with the next message.
    pub fn attach_paths(
        &mut self,
        paths: &[std::path::PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for p in paths {
            let path = p.display().to_string();
            if self
                .attachments
                .iter()
                .any(|a| matches!(a, Attachment::File { path: x } if *x == path))
            {
                continue;
            }
            // A picture the agent can't see as it is (an iPhone's HEIC, a
            // TIFF, one over 5 MB) goes as a JPEG copy, made off the main
            // thread; if that fails it goes by its path, and a toast says so.
            let size = std::fs::metadata(p).map_or(0, |m| m.len());
            if crate::compose::wants_jpeg(&path, size) {
                self.attach_as_jpeg(p.clone(), cx);
                continue;
            }
            self.attachments.push(Attachment::File { path });
        }
        match &self.selected {
            Some(_) => self.composer.update(cx, |c, cx| c.focus_field(window, cx)),
            None => self.starter.update(cx, |c, cx| c.focus_field(window, cx)),
        }
        cx.notify();
    }

    /// Attach a JPEG copy of the picture at `src` to the message being
    /// written where it was dropped, once made; or the file itself, said
    /// so, if it can't be.
    fn attach_as_jpeg(&mut self, src: std::path::PathBuf, cx: &mut Context<Self>) {
        let session = self.selected.clone();
        let dir = crate::scratch_dir()
            .with_file_name("attachments")
            .join(crate::message_id());
        let made = cx.background_executor().spawn({
            let src = src.clone();
            async move { crate::compose::to_jpeg(&src, &dir) }
        });
        cx.spawn(async move |this, cx| {
            let made = made.await;
            let _ = this.update(cx, |this, cx| {
                let name = src
                    .file_name()
                    .map_or(String::new(), |n| n.to_string_lossy().into_owned());
                let path = match made {
                    Ok(jpg) => jpg.display().to_string(),
                    Err(why) => {
                        this.toast(
                            ToastAction::Info,
                            format!(
                                "Couldn't turn {name} into a JPEG the agent can see ({why}), so it gets the file's path."
                            ),
                        );
                        src.display().to_string()
                    }
                };
                let held = if this.selected == session {
                    &mut this.attachments
                } else {
                    let key = session.unwrap_or_else(|| NEW_DRAFT.to_string());
                    &mut this.drafts.entry(key).or_default().atts
                };
                let a = Attachment::File { path };
                if !held.contains(&a) {
                    held.push(a);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether `at` is over the terminal drawn this frame.
    fn over_shell(&self, at: Point<Pixels>) -> bool {
        let frame = crate::ui::FRAME.load(std::sync::atomic::Ordering::Relaxed);
        crate::ui::PROBE_FRAME
            .with(|f| f.borrow().get("shell").copied())
            .is_some_and(|f| f + 1 >= frame)
            && self
                .probes
                .borrow()
                .get("shell")
                .is_some_and(|b| b.contains(&at))
    }

    /// Where files being dragged would go if let go now: the terminal under
    /// the pointer, else the message. Only that one says so.
    pub fn drop_target(&self, cx: &App) -> Option<DropTarget> {
        cx.has_active_drag().then(|| match self.drag_at {
            Some(at) if self.over_shell(at) => DropTarget::Shell,
            _ => DropTarget::Message,
        })
    }

    /// Dragged files let go: on your terminal they're typed as paths, as
    /// in any terminal; anywhere else they ride along with the message.
    pub fn drop_files(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.drag_at = None;
        let Some(paths) = self.dragging_files.take() else {
            return;
        };
        let shell = self
            .selected
            .as_ref()
            .and_then(|id| self.shells.get(id))
            .filter(|_| self.over_shell(at));
        match shell {
            Some(sh) => {
                let typed: Vec<String> = paths
                    .iter()
                    .map(|p| crate::shell::shell_quote(&p.display().to_string()))
                    .collect();
                sh.paste(&format!("{} ", typed.join(" ")));
                self.term_focus.focus(window, cx);
                cx.notify();
            }
            None => self.attach_paths(&paths, window, cx),
        }
    }

    /// Files picked with the system's open panel ride along, as dropped
    /// ones do.
    pub fn pick_attachments(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.attach_paths(&paths, window, cx);
            });
        })
        .detach();
    }

    /// A pasted picture is saved where the agent can read it, then attached.
    /// Returns whether the clipboard held something to attach.
    pub fn paste_attachment(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let mut paths = Vec::new();
        for entry in item.entries() {
            match entry {
                ClipboardEntry::String(_) => {}
                ClipboardEntry::ExternalPaths(p) => paths.extend(p.paths().iter().cloned()),
                ClipboardEntry::Image(img) => {
                    // Saved as what it is: kinds the agent reads as they
                    // came, others (TIFF, BMP) as PNG, and one too big to
                    // be shown scaled down. An SVG goes by its path.
                    let ready = match img.format() {
                        ImageFormat::Svg => Ok((img.bytes().to_vec(), "svg")),
                        f => crate::compose::ready_picture(
                            img.bytes(),
                            match f {
                                ImageFormat::Png => Some("png"),
                                ImageFormat::Jpeg => Some("jpg"),
                                ImageFormat::Gif => Some("gif"),
                                ImageFormat::Webp => Some("webp"),
                                _ => None,
                            },
                        ),
                    };
                    let (bytes, ext) = match ready {
                        Ok(r) => r,
                        Err(why) => {
                            self.toast(ToastAction::Info, format!("The pasted picture {why}."));
                            cx.notify();
                            return true;
                        }
                    };
                    let dir = crate::scratch_dir().with_file_name("attachments");
                    let path = dir.join(format!("pasted-{}.{ext}", crate::message_id()));
                    if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&path, bytes).is_ok()
                    {
                        paths.push(path);
                    }
                }
            }
        }
        // Text pastes as text; pictures and files attach. Files copied in
        // Finder come with their names as text too: they still attach.
        if paths.is_empty() {
            return false;
        }
        self.attach_paths(&paths, window, cx);
        true
    }

    pub fn set_model(&mut self, model: String, cx: &mut Context<Self>) {
        if let Some(id) = self.selected.clone() {
            self.live.send(Request::SetModel {
                session: id,
                model: model.clone(),
            });
        }
        self.defaults.model = Some(model);
        cx.notify();
    }

    pub fn set_effort(&mut self, effort: Option<String>, cx: &mut Context<Self>) {
        if let Some(id) = self.selected.clone() {
            self.live.send(Request::SetEffort {
                session: id,
                effort: effort.clone().unwrap_or_default(),
            });
        }
        self.defaults.effort = effort;
        cx.notify();
    }

    /// The mode a session's chip shows: the one just asked for (⇧Tab, the
    /// menu) while the daemon hasn't answered yet, so the chip follows the
    /// key at once; then what's in effect, which a refusal leaves as it was.
    pub fn permission_shown(&self, s: &Session) -> Permission {
        self.asked_permission
            .as_ref()
            .filter(|(id, _, at)| *id == s.id && at.elapsed() < ASKED_WAIT)
            .map_or(s.permission, |(_, p, _)| *p)
    }

    pub fn set_permission(&mut self, p: Permission, cx: &mut Context<Self>) {
        if let Some(id) = self.selected.clone() {
            // A refusal shows once the wait is over (see `deadlines`): the
            // chip goes back.
            self.asked_permission = Some((id.clone(), p, Instant::now()));
            self.live.send(Request::SetPermission {
                session: id,
                permission: p,
            });
        }
        self.defaults.permission = p;
        cx.notify();
    }

    // ---- The viewer ----

    /// Open the side chat for the open session, ready to type.
    pub fn open_side_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_none() {
            return;
        }
        self.show_tab(Tab::Aside, cx);
        self.aside_input
            .update(cx, |i, cx| i.focus_field(window, cx));
    }

    /// Ask what's in the side chat's box.
    pub fn ask_aside(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let text = self.aside_input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.live.send(Request::Aside {
            session: id,
            id: crate::message_id(),
            text,
        });
        self.aside_input.update(cx, |i, cx| put(i, "", window, cx));
        cx.notify();
    }

    /// A side question and its answer become a session of their own.
    pub fn promote_aside(&mut self, session: &str, id: &str, cx: &mut Context<Self>) {
        // One new session at a time, like Start and Fork.
        if self.creating() {
            return;
        }
        self.creating = true;
        self.creating_at = Instant::now();
        let n = self.live.send(Request::PromoteAside {
            session: session.to_string(),
            id: id.to_string(),
        });
        self.waiting.insert(n, Waiting::Create(None));
        cx.notify();
    }

    /// Open Changes at one file of the session's diff.
    pub fn review(&mut self, session: &str, path: &str, cx: &mut Context<Self>) {
        if let Some(i) = self
            .session(session)
            .and_then(|s| s.diff.iter().position(|f| f.path == path))
        {
            self.diff_file.insert(session.to_string(), i);
        }
        self.show_tab(Tab::Changes, cx);
    }

    pub fn show_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if let Some(id) = self.selected.clone() {
            self.tabs.insert(id, tab);
        }
        self.panel_open = true;
        self.end_pick();
        cx.notify();
    }

    pub fn open_art(&mut self, art: &'static str, version: Option<usize>, cx: &mut Context<Self>) {
        if let Some(id) = &self.selected
            && let Some(c) = self.closed_tabs.get_mut(id)
            && c.remove(art).is_some()
        {
            self.prefs_changed();
        }
        // A version whose text wasn't captured (only edited) shows the latest.
        let captured = |a: &Artifact, v: usize| {
            a.versions
                .get(v.wrapping_sub(1))
                .is_some_and(|b| !b.is_empty())
        };
        match version {
            Some(v)
                if self
                    .art(art)
                    .is_some_and(|a| v < a.versions.len() && captured(a, v)) =>
            {
                self.viewing.insert(art, v);
            }
            _ => {
                self.viewing.remove(art);
            }
        }
        self.show_tab(Tab::Art(art), cx);
    }

    /// Pick any folder as the new session's project, with the system's
    /// folder picker.
    pub fn choose_project_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Use as project".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            let path = dir.display().to_string();
            let _ = this.update(cx, |this, cx| {
                // On the list from now on, as one typed or made here is.
                this.use_project(&path, false);
                cx.notify();
            });
        })
        .detach();
    }

    /// Leave Point mode, telling the page too, so it doesn't stay armed.
    pub fn end_pick(&mut self) {
        if self.picking {
            self.picking = false;
            if let Some(p) = &self.preview {
                p.borrow().pick(false);
            }
        }
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
        let (events, showing, retired) = {
            let p = p.borrow();
            (p.drain(), p.showing.clone(), p.retired())
        };
        for image in retired {
            let _ = window.drop_image(image);
        }
        if events.is_empty() {
            return;
        }
        let art = self.current().and_then(|s| match self.tab(s) {
            Tab::Art(a) => Some(a),
            _ => None,
        });
        if let Some(a) = art {
            self.page_events(a, showing, events, window, cx);
        }
        cx.notify();
    }

    /// What a page said, from the viewer or a page in its own window.
    pub fn page_events(
        &mut self,
        art: &'static str,
        showing: Option<Source>,
        events: Vec<PageEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let v = match &showing {
            Some(Source::File { version, .. }) => *version,
            _ => 1,
        };
        for ev in events {
            match ev {
                // Each load starts clean: what the last one reported is gone.
                PageEvent::Started => {
                    self.errors.remove(&(art, v));
                }
                PageEvent::Loaded => {}
                PageEvent::Logged(msg) => {
                    let list = self.errors.entry((art, v)).or_default();
                    if !list.contains(&msg) {
                        list.push(msg);
                    }
                }
                PageEvent::Error(msg) => {
                    let list = self.errors.entry((art, v)).or_default();
                    if !list.contains(&msg) {
                        list.push(msg);
                    }
                    // A broken latest page needs you, like any other error;
                    // a line it only logged doesn't.
                    let latest = self.art(art).map(|x| x.versions.len().max(1)) == Some(v);
                    if latest
                        && let Some(s) = self
                            .sessions
                            .iter_mut()
                            .find(|s| {
                                s.arts.contains(&art)
                                    && matches!(s.status, Status::Done | Status::Idle)
                            })
                            .map(Rc::make_mut)
                    {
                        s.status = Status::Needs;
                        s.detail = "The page has an error".into();
                    }
                }
                PageEvent::Pick {
                    tag,
                    text,
                    selector,
                } => {
                    self.picking = false;
                    let file = self
                        .art(art)
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
                    // A pick from a page in its own window lands in the main window's composer.
                    if self.popouts.contains_key(art) {
                        cx.notify();
                    } else {
                        self.composer.update(cx, |c, cx| c.focus_field(window, cx));
                    }
                }
                PageEvent::PickCancel => self.picking = false,
                PageEvent::Probe(text) => self.page_probe = Some(text),
                PageEvent::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                PageEvent::Found(n) => self.page_found(n),
                PageEvent::Dropped(paths) => self.attach_paths(&paths, window, cx),
                PageEvent::External(url) => cx.open_url(&url),
            }
        }
        cx.notify();
    }

    /// Parsed Markdown for `src`, parsed once.
    pub fn markdown(&mut self, src: &str) -> Rc<Vec<Block>> {
        self.md.get(src)
    }

    /// A reply still streaming in at `at`: its finished blocks are kept and
    /// only its end is parsed again, tidied so it doesn't flip from raw to
    /// formatted as it arrives.
    pub fn markdown_live(&mut self, at: &str, src: &str) -> Rc<Vec<Block>> {
        self.md.live(at, src)
    }

    /// A document version, parsed once: a big file isn't copied and hashed
    /// each frame.
    pub fn doc_blocks(&mut self, art: &'static str, v: usize) -> Rc<Vec<Block>> {
        if let Some(b) = self.md_docs.get(&(art, v)) {
            return b;
        }
        let src = self
            .art(art)
            .and_then(|a| a.versions.get(v.wrapping_sub(1)).cloned())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let blocks = Rc::new(crate::markdown::parse(&src));
        self.md_docs.insert((art, v), blocks.clone());
        blocks
    }

    /// The decoded picture for an artifact version, decoded once.
    pub fn picture(&mut self, art: &'static str, v: usize) -> Option<Arc<Image>> {
        if let Some(p) = self.pictures.get(&(art, v)) {
            return Some(p.clone());
        }
        let a = self.art(art)?;
        let bytes = a.versions.get(v.checked_sub(1)?)?.clone();
        let format = match a
            .path
            .rsplit('.')
            .next()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("jpg" | "jpeg") => ImageFormat::Jpeg,
            Some("gif") => ImageFormat::Gif,
            Some("webp") => ImageFormat::Webp,
            Some("svg") => ImageFormat::Svg,
            _ => ImageFormat::Png,
        };
        let p = Arc::new(Image::from_bytes(format, bytes.to_vec()));
        self.pictures.insert((art, v), p.clone());
        Some(p)
    }

    /// Keep the conversation list's measurements in step with its items:
    /// a new session starts at the end; changed items are measured again.
    pub fn sync_transcript(&mut self, s: &Session) {
        let mut thoughts = 0;
        let sigs: Vec<u64> = s
            .items
            .iter()
            .enumerate()
            .map(|(ix, i)| {
                let thought = matches!(i, Item::Thinking(..)).then(|| {
                    thoughts += 1;
                    thought_key(&s.id, thoughts - 1)
                });
                self.item_sig(&s.id, ix, i, thought.as_deref())
            })
            .chain((s.status == Status::Working).then_some(u64::MAX))
            .collect();
        // The first queued message carries the working line above it, so
        // its height changes with the working state.
        let mut sigs = sigs;
        if let Some(q) = crate::chat::working_host(s) {
            sigs[q] ^= 0x9e37_79b9_7f4a_7c15;
        }
        let (id, old) = &self.transcript_sigs;
        if id.as_deref() != Some(s.id.as_str()) {
            // Leaving a session remembers where you were reading it.
            if let Some(prev) = id.clone() {
                let at = self.transcript.logical_scroll_top();
                let following = self.transcript.is_following_tail();
                self.read_at.insert(prev, (at, following));
            }
            self.transcript.reset(sigs.len());
            self.row_born.clear();
            match self.read_at.get(&s.id).copied() {
                // Back where you left it, unless you were at the end.
                Some((at, false)) if at.item_ix < sigs.len() => {
                    self.transcript.set_follow_mode(FollowMode::Tail);
                    self.transcript.pause_following_tail();
                    self.transcript.scroll_to(at);
                }
                _ => {
                    self.transcript.set_follow_mode(FollowMode::Tail);
                    self.transcript.scroll_to_end();
                }
            }
        } else if *old != sigs {
            let first = old
                .iter()
                .zip(&sigs)
                .position(|(a, b)| a != b)
                .unwrap_or(old.len().min(sigs.len()));
            self.transcript.splice(first..old.len(), sigs.len() - first);
            // A few items added at the end arrive; the working line's
            // signature trails the items and isn't one of them. A batch
            // (history loading) just appears.
            let items = |v: &[u64]| v.len() - usize::from(v.last() == Some(&u64::MAX));
            let (was, now) = (items(old), items(&sigs));
            if first >= was && now > was && now - was <= ARRIVE_AT_ONCE {
                let at = Instant::now();
                for (k, ix) in (was..now).enumerate() {
                    let delay =
                        (k as f32 * crate::ds::motion::STAGGER).min(crate::ds::motion::STAGGER_CAP);
                    self.row_born.insert(ix, (at, delay));
                }
            }
        }
        let settle = Duration::from_millis(
            (crate::ds::motion::BASE + crate::ds::motion::STAGGER_CAP) as u64,
        );
        self.row_born.retain(|_, (at, _)| at.elapsed() < settle);
        self.transcript_sigs = (Some(s.id.clone()), sigs);
    }

    /// Anything about an item that changes its height.
    /// `thought` is a thought's key, for whether it's open.
    fn item_sig(&self, sid: &str, ix: usize, item: &Item, thought: Option<&str>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::mem::discriminant(item).hash(&mut h);
        // Every fold it has changes its height.
        for f in crate::data::Fold::of(sid, ix, item) {
            self.is_open(&f).hash(&mut h);
        }
        let fp = crate::data::fingerprint::<std::collections::hash_map::DefaultHasher>;
        match item {
            Item::User { text: t, id, .. } => {
                fp(t, &mut h);
                self.rewinding
                    .as_ref()
                    .is_some_and(|(_, m)| m == id)
                    .hash(&mut h)
            }
            Item::Text(t) => fp(t, &mut h),
            Item::Queued {
                text, attachments, ..
            } => {
                fp(text, &mut h);
                attachments.hash(&mut h);
            }
            Item::Thinking(t, _) => {
                fp(t, &mut h);
                thought
                    .is_some_and(|k| self.open_calls.contains(k))
                    .hash(&mut h);
            }
            Item::Tools { lines, summary, .. } => {
                (lines, summary).hash(&mut h);
                lines
                    .first()
                    .is_some_and(|l| self.open_tools.contains(&l.id))
                    .hash(&mut h);
                for l in lines.iter().filter(|l| l.detail.is_some()) {
                    self.open_calls.contains(&l.id).hash(&mut h);
                }
            }
            Item::Made {
                art,
                version,
                review,
            } => {
                // Older versions fold to one line.
                let latest = self.art(art).map(|a| a.versions.len()).unwrap_or(0);
                (art, version, *version < latest, review).hash(&mut h);
            }
            Item::Approval {
                id, command, why, ..
            } => (id, command, why).hash(&mut h),
            Item::Plan { id, plan } => {
                id.hash(&mut h);
                fp(plan, &mut h);
            }
            Item::Question { id, questions } => {
                (id, questions.len()).hash(&mut h);
                self.picks
                    .get(id)
                    .map(|p| p.iter().flatten().count())
                    .hash(&mut h);
            }
            Item::Note { text, .. } => fp(text, &mut h),
            Item::Changed { paths } => paths.hash(&mut h),
            Item::TurnEnd { at, took } => (at, took).hash(&mut h),
            Item::Compacted {
                auto,
                before,
                after,
            } => (auto, before, after).hash(&mut h),
            Item::Command {
                name, args, text, ..
            } => {
                (name, args).hash(&mut h);
                fp(text, &mut h);
            }
        }
        h.finish()
    }

    /// Whether a fold in the chat is open.
    pub fn is_open(&self, f: &crate::data::Fold) -> bool {
        self.expanded.contains(&f.key())
    }

    /// Open a fold, or close it again, by its key.
    pub fn toggle_fold(&mut self, key: &str) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_string());
        }
    }

    /// Open a page in a window of its own, or bring that window forward.
    pub fn pop_out(&mut self, art: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((h, _)) = self.popouts.get(art) {
            if !crate::BACKGROUND.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = h.update(cx, |_, w, _| w.activate_window());
            }
            return;
        }
        self.end_pick();
        let me = cx.entity();
        let at = window.bounds();
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(at.origin.x + px(120.), at.origin.y + px(60.)),
                size: size(px(900.), px(760.)),
            })),
            titlebar: Some(TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: Some(point(px(16.), px(16.))),
            }),
            window_min_size: Some(size(px(420.), px(360.))),
            focus: !crate::BACKGROUND.load(std::sync::atomic::Ordering::Relaxed),
            ..Default::default()
        };
        let t = self.t;
        // The new window draws straight away and reads this state, so open it
        // once this update is over.
        cx.defer(move |cx| {
            let dent = me.clone();
            if let Ok((h, view)) = gpui_kit::open_window(opts, cx, move |window, cx| {
                cx.new(|cx| crate::popout::PopOut::new(dent, art, t, window, cx))
            }) {
                me.update(cx, |d, cx| {
                    d.popouts.insert(art, (h, view));
                    cx.notify();
                });
            }
        });
    }

    /// Close a page's own window; it goes back to the viewer.
    pub fn bring_back(&mut self, art: &'static str, cx: &mut Context<Self>) {
        if let Some((h, view)) = self.popouts.remove(art) {
            crate::popout::close(h, view, cx);
        }
        cx.notify();
    }

    /// Send the preview's error to the agent, straight away.
    pub fn ask_to_fix(
        &mut self,
        art: &'static str,
        v: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pressed_again(format!("fix/{art}/{v}")) {
            return;
        }
        let Some(msg) = self.errors.get(&(art, v)).and_then(|e| e.first()).cloned() else {
            return;
        };
        let file = self
            .art(art)
            .map(|a| a.path.to_string())
            .unwrap_or_default();
        let Some(id) = self.selected.clone() else {
            return;
        };
        // Its own message: the draft and what's attached to it stay put.
        self.send_aside_draft(
            &id,
            "Fix this error.",
            &[Attachment::Error { file, message: msg }],
            cx,
        );
    }

    /// Send a message dent wrote for you (fix this error, address these
    /// comments) with only its own attachments, leaving the draft and the
    /// files attached to it as they are.
    fn send_aside_draft(
        &mut self,
        id: &str,
        text: &str,
        atts: &[Attachment],
        cx: &mut Context<Self>,
    ) {
        self.send_draft(id, &Draft::with(text, atts.to_vec()), false);
        cx.notify();
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
            put(c, "", window, cx);
            c.focus_field(window, cx);
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
            .update(cx, |c, cx| put(c, "", window, cx));
        self.composer.update(cx, |c, cx| c.focus_field(window, cx));
        cx.notify();
    }

    pub fn send_review(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.review.is_empty() {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        let comments = std::mem::take(&mut self.review);
        let text = if comments.len() == 1 {
            "Please address this comment."
        } else {
            "Please address these comments."
        };
        self.send_aside_draft(&id, text, &comments, cx);
    }

    // ---- Keyboard and search ----

    /// Every open session in inbox order, then what's working.
    /// The sidebar's sessions top to bottom, closed ones too while shown.
    pub fn order(&self) -> Vec<String> {
        let closed = if self.show_closed {
            self.closed()
        } else {
            Vec::new()
        };
        let [needs, done, working] = self.shown_sections();
        self.pinned_sessions()
            .into_iter()
            .chain(needs)
            .chain(done)
            .chain(working)
            .chain(closed)
            .map(|s| s.id.clone())
            .collect()
    }

    /// Keyboard focus to the inbox, where arrows, Enter and E work. With
    /// the sidebar hidden it goes to the window instead: a hidden element
    /// holding focus would leave every shortcut with nowhere to go.
    pub fn focus_inbox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar_hidden {
            window.focus(&self.focus, cx);
        } else {
            window.focus(&self.inbox_focus, cx);
        }
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
            self.focus_inbox(window, cx);
        }
    }

    /// ⇧⌘K or the header's list: your messages in the open session, to
    /// search and jump to.
    pub fn open_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.current().is_none() {
            return;
        }
        self.open_palette(window, cx);
        self.palette_mine = true;
        self.search.update(cx, |s, cx| {
            s.set_placeholder("Find your messages here", window, cx)
        });
    }

    /// Your messages in the open session, newest first, matching every word
    /// typed: where each is in the conversation, its words and when.
    pub fn palette_messages(&self, cx: &App) -> Vec<(usize, String, u64)> {
        let q = self.search.read(cx).value().to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        let Some(s) = self.current() else {
            return Vec::new();
        };
        s.items
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(ix, i)| match i {
                Item::User { text, at, .. } => {
                    let lower = text.to_lowercase();
                    words
                        .iter()
                        .all(|w| lower.contains(w))
                        .then(|| (ix, text.clone(), *at))
                }
                _ => None,
            })
            .collect()
    }

    /// Scroll to the message at `ix`, reading back from there.
    pub fn scroll_to_item(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.transcript.set_follow_mode(FollowMode::Tail);
        self.transcript.pause_following_tail();
        self.transcript.scroll_to(ListOffset {
            item_ix: ix,
            offset_in_item: px(0.),
        });
        cx.notify();
    }

    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.palette_mine = false;
        self.search.update(cx, |s, cx| {
            s.set_placeholder("Search sessions, or type a task to start one", window, cx)
        });
        self.palette = Some(0);
        self.search.update(cx, |s, cx| {
            put(s, "", window, cx);
            s.focus_field(window, cx);
        });
        cx.notify();
    }

    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.palette_mine = false;
        match &self.selected {
            Some(_) => self.composer.update(cx, |c, cx| c.focus_field(window, cx)),
            None => self.starter.update(cx, |c, cx| c.focus_field(window, cx)),
        }
        cx.notify();
    }

    /// Sessions matching the query, closed ones included, best first.
    /// A session's words, lowercased, for search: what you sent and what
    /// came back. Kept until the session changes.
    fn search_text(&self, s: &Session) -> Rc<str> {
        let key = (s.items.len(), s.updated);
        if let Some((k, text)) = self.search_cache.borrow().get(&s.id)
            && *k == key
        {
            return text.clone();
        }
        let mut text = String::new();
        for i in s.items.iter() {
            match i {
                Item::User { text: t, .. } | Item::Text(t) => {
                    text.push_str(&t.to_lowercase());
                    text.push('\n');
                }
                _ => {}
            }
        }
        let text: Rc<str> = text.into();
        self.search_cache
            .borrow_mut()
            .insert(s.id.clone(), (key, text.clone()));
        text
    }

    /// Sessions matching the query, closed ones included, best first: titles
    /// count most, then what was said.
    pub fn palette_results(&self, cx: &App) -> Vec<String> {
        let q = self.search.read(cx).value().to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        let mut hits: Vec<(usize, String)> = self
            .sessions
            .iter()
            .filter_map(|s| {
                let meta =
                    format!("{} {} {} {}", s.title, s.project, s.machine, s.detail).to_lowercase();
                let body = if words.is_empty() {
                    Rc::from("")
                } else {
                    self.search_text(s)
                };
                let mut score = 0;
                for w in &words {
                    // A whole word ranks above part of one: "number 7"
                    // finds "hello number 7" before "hello number 17".
                    if has_word(&meta, w) {
                        score += 4;
                    } else if meta.contains(w) {
                        score += 3;
                    } else if body.contains(w) {
                        score += 1;
                    } else {
                        return None;
                    }
                }
                Some((score + usize::from(!s.closed) * 10, s.id.clone()))
            })
            .collect();
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
        hits.into_iter().take(40).map(|(_, id)| id).collect()
    }

    /// Where in a session the query was found, when it isn't in the title.
    pub fn search_snippet(&self, s: &Session, cx: &App) -> Option<String> {
        let q = self.search.read(cx).value().to_lowercase();
        let w = q.split_whitespace().next()?;
        if s.title.to_lowercase().contains(w) {
            return None;
        }
        for i in s.items.iter() {
            let (Item::User { text, .. } | Item::Text(text)) = i else {
                continue;
            };
            let lower = text.to_lowercase();
            if let Some(at) = lower.find(w) {
                // A few words either side, on a char boundary.
                let mut start = at.saturating_sub(40);
                while !text.is_char_boundary(start) {
                    start -= 1;
                }
                let mut end = (at + w.len() + 60).min(text.len());
                while !text.is_char_boundary(end) {
                    end += 1;
                }
                let piece = text[start..end]
                    .replace('\n', " ")
                    .replace("**", "")
                    .replace('`', "");
                let lead = if start > 0 { "…" } else { "" };
                let tail = if end < text.len() { "…" } else { "" };
                return Some(format!("{lead}{}{tail}", piece.trim()));
            }
        }
        None
    }

    /// Open session `id` from the palette: the one clicked, wherever the
    /// results have moved since.
    pub fn palette_open(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().trim().to_string();
        self.palette = None;
        self.open_at_match(id, &query, window, cx);
        cx.notify();
    }

    /// Open a session from search. Found inside the conversation rather than
    /// its title, it opens at the first place it was said, with ⌘F holding
    /// the words so the next match is a ↩ away.
    fn open_at_match(
        &mut self,
        id: String,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inside = !query.is_empty()
            && self
                .session(&id)
                .is_some_and(|s| self.search_snippet(s, cx).is_some());
        self.select(Some(id), window, cx);
        if inside {
            let q = query.to_string();
            self.find_input.update(cx, |f, cx| put(f, q, window, cx));
            self.open_find(window, cx);
        }
    }

    pub fn palette_open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_none() {
            return;
        }
        if self.palette_mine {
            let at = self.palette.unwrap_or(0);
            let picked = self.palette_messages(cx).get(at).map(|m| m.0);
            self.close_palette(window, cx);
            if let Some(ix) = picked {
                self.scroll_to_item(ix, cx);
            }
            return;
        }
        let results = self.palette_results(cx);
        let files = self.palette_files(cx);
        let at = self.palette.unwrap_or(0);
        let query = self.search.read(cx).value().trim().to_string();
        self.palette = None;
        if let Some(file) = at.checked_sub(results.len()).and_then(|j| files.get(j)) {
            let full = self.current().map(|s| {
                std::path::Path::new(&s.cwd)
                    .join(file)
                    .display()
                    .to_string()
            });
            self.close_palette(window, cx);
            if let Some(full) = full {
                self.open_file(&full, cx);
            }
        } else if let Some(id) = results.get(at).cloned() {
            // A closed session opens to read, as from the sidebar; Reopen
            // is in its footer.
            self.open_at_match(id, &query, window, cx);
        } else if !query.is_empty() && at >= results.len() + files.len() {
            // The last row: start a session with what was typed.
            self.select(None, window, cx);
            // Over a draft as an edit ⌘Z undoes.
            self.starter.update(cx, |s, cx| {
                put_undoable(s, query.clone(), window, cx);
                s.focus_field(window, cx);
            });
        } else {
            self.close_palette(window, cx);
        }
        cx.notify();
    }

    /// Hide the sidebar, or bring it back: hidden by hand it stays hidden;
    /// brought back, the viewer closes to make room, as the header's
    /// sidebar button does.
    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_hidden {
            self.sidebar_off = false;
            self.sidebar_forced = true;
            // Short of room, the viewer gives way, so the chat keeps its width.
            self.panel_open = false;
        } else {
            self.sidebar_off = true;
            self.sidebar_forced = false;
        }
        self.prefs_changed();
        cx.notify();
    }

    /// Page Up, Page Down, Home and End in the chat.
    pub fn scroll_chat(&mut self, by: ChatScroll, cx: &mut Context<Self>) {
        let page = (self.window_h - crate::ds::size::HEADER - 160.).max(120.) * 0.9;
        match by {
            ChatScroll::Up => {
                self.transcript.set_follow_mode(FollowMode::Normal);
                self.transcript.scroll_by(px(-page));
            }
            ChatScroll::Down => self.transcript.scroll_by(px(page)),
            ChatScroll::Top => {
                self.transcript.set_follow_mode(FollowMode::Normal);
                self.transcript.scroll_to(ListOffset::default());
            }
            ChatScroll::Bottom => self.follow(),
        }
        cx.notify();
    }

    /// Keep the newest line in view while following the conversation.
    fn follow(&mut self) {
        self.transcript.set_follow_mode(FollowMode::Tail);
        self.transcript.scroll_to_end();
    }

    /// Whether the next tick needs a frame. Nothing on screen moving means
    /// nothing is drawn.
    /// When each timed state that changes what shows ends: one list, so
    /// the window wakes for every one of them (`tick_due`), not only those
    /// that remembered to set a timer.
    fn deadlines(&self) -> Vec<Instant> {
        let mut d = Vec::new();
        d.extend(self.just_sent.map(|at| at + JUST_SENT));
        d.extend(self.sent_at.as_ref().map(|(_, _, at)| *at + STOPPABLE));
        d.extend(
            self.asked_permission
                .as_ref()
                .map(|(_, _, at)| *at + ASKED_WAIT),
        );
        d.extend(self.taking_back.values().map(|tb| tb.at + DAEMON_WAIT));
        if self.creating {
            d.push(self.creating_at + DAEMON_WAIT);
        }
        d
    }

    pub fn tick_due(&mut self) -> bool {
        let now = Instant::now();
        let since = std::mem::replace(&mut self.last_tick, now);
        let ended = self
            .deadlines()
            .iter()
            .any(|end| *end > since && *end <= now);
        let page = self.preview.as_ref().is_some_and(|p| {
            let p = p.borrow();
            let owed = p.tick();
            p.pending() || owed
        });
        let toast_expired = self
            .toast
            .as_ref()
            .is_some_and(|t| t.at.elapsed() > Duration::from_secs(8));
        let copied = self
            .copied_at
            .is_some_and(|at| (1.5..1.7).contains(&at.elapsed().as_secs_f32()));
        let working_shown = self.working_shown();
        // Working squares draw every frame through `animating`; with Reduce
        // Motion they hold still and only the timers move, once a second.
        let second = crate::PROCESS_START
            .get()
            .map(|p| p.elapsed().as_secs())
            .unwrap_or(0);
        let new_second = second != self.last_second;
        self.last_second = second;
        // A walkthrough wakes when an idle stretch it's measuring ends.
        let rest_over = self
            .walk
            .as_ref()
            .and_then(|d| d.resting)
            .is_some_and(|(until, ..)| Instant::now() >= until);
        // Once a minute, so "now" and "5m" in the sidebar stay true.
        let new_minute = new_second && second.is_multiple_of(60);
        // Once a second, whether the file you opened in the viewer changed.
        let disk = new_second && self.opened_changed();
        let due = page
            || ended
            || toast_expired
            || copied
            || (working_shown && new_second)
            || new_minute
            || disk
            || rest_over;
        if due && self.walk.as_ref().is_some_and(|w| w.resting.is_some()) {
            eprintln!(
                "      tick: page={page} toast={toast_expired} copied={copied} second={}",
                working_shown && new_second
            );
        }
        due
    }

    /// Whether a working session's square is on screen: in the sidebar,
    /// or the open session (its working line and plan) when the sidebar is
    /// hidden.
    fn working_shown(&self) -> bool {
        self.sessions.iter().any(|s| {
            !s.closed
                && s.status == Status::Working
                && (!self.sidebar_hidden || self.selected.as_deref() == Some(s.id.as_str()))
        })
    }

    pub fn animating(&self) -> bool {
        let fresh = |m: &HashMap<String, Instant>| {
            m.values().any(|t| t.elapsed() < Duration::from_millis(700))
        };
        // Words fade in for a moment after they arrive; the agent's own updates
        // redraw when they happen, so a running agent alone needs no frames.
        let streaming = self.births.values().any(|(_, b)| {
            b.last().is_some_and(|(_, at)| {
                at.elapsed() < Duration::from_millis(crate::ds::motion::BASE as u64 + 40)
            })
        });
        // A working square's breath is drawn at its own rate (`breathing`).
        streaming
            || !self.row_born.is_empty()
            || fresh(&self.finished_at)
            || fresh(&self.arrived_at)
            || self
                .updated_at
                .values()
                .any(|t| t.elapsed() < Duration::from_secs(3))
            || self.dragging
            // Words waiting their turn need frames only where they'll show:
            // the open session's working line.
            || self.live_words.borrow().iter().any(|(id, w)| {
                (w.next.is_some() && self.selected.as_deref() == Some(id.as_str()))
                    || w.at.elapsed() < Duration::from_millis(crate::ds::motion::BASE as u64 + 40)
            })
    }

    /// A working square breathes, smoothly, but only while one shows and
    /// motion is on: an idle window still draws nothing.
    pub fn breathing(&self) -> bool {
        !crate::reduce_motion() && self.working_shown()
    }

    /// The working line's words for `session` as shown: what it was, what
    /// it is now, and when it changed. New words wait until the old ones
    /// have been up long enough to read and have themselves held still a
    /// moment, so a run of quick calls never flickers.
    pub fn live_words(&self, session: &str, kind: &str, words: &str, step: bool) -> LiveWords {
        let key = format!("{kind}\u{1f}{words}");
        let now = Instant::now();
        let mut all = self.live_words.borrow_mut();
        let w = all.entry(session.to_string()).or_insert_with(|| LiveWords {
            key: key.clone(),
            words: words.to_string(),
            was: String::new(),
            // The first words are just there.
            at: now - Duration::from_secs(1),
            next: None,
            step,
        });
        if w.key == key {
            w.next = None;
        } else {
            match &w.next {
                Some((k, _)) if *k == key => {}
                _ => w.next = Some((key.clone(), now)),
            }
            let up = w.at.elapsed().as_millis() as f32;
            let still = w
                .next
                .as_ref()
                .map_or(0., |(_, t)| t.elapsed().as_millis() as f32);
            if swap_due(w.step, step, up, still) {
                w.was = std::mem::replace(&mut w.words, words.to_string());
                w.key = key;
                w.at = now;
                w.next = None;
                w.step = step;
            }
        }
        w.clone()
    }

    /// Put a suggested command in the composer, ready to send.
    pub fn suggest_command(
        &mut self,
        session: &str,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected.as_deref() != Some(session) {
            return;
        }
        // Over a draft as an edit ⌘Z undoes: the draft is a keystroke back.
        let text = format!("/{name} ");
        self.composer.clone().update(cx, |f, cx| {
            put_undoable(f, text.clone(), window, cx);
            f.focus_field(window, cx);
        });
        cx.notify();
    }
}

/// Whether the working line's words give way to new ones, `up` ms after
/// they came and with the new ones still for `still` ms. A plan step
/// (`was_step`) is up for a while, so it reads before a call's words
/// replace it; a call's words only long enough not to flicker. The plan's
/// next step (`step`) never waits on the last one: the line keeps up with
/// the plan above it.
fn swap_due(was_step: bool, step: bool, up: f32, still: f32) -> bool {
    use crate::ds::motion::{DWELL, DWELL_STEP, SETTLE};
    let held = if was_step && !step { DWELL_STEP } else { DWELL };
    up >= held && still >= SETTLE
}

/// More items than this landing at once (history arriving) just appear.
const ARRIVE_AT_ONCE: usize = 3;

/// What the working line shows (see `App::live_words`).
#[derive(Clone)]
pub struct LiveWords {
    key: String,
    pub words: String,
    /// The words before, leaving as these arrive.
    pub was: String,
    pub at: Instant,
    next: Option<(String, Instant)>,
    /// The words shown are the plan's current step.
    step: bool,
}

/// Where dragged files would land.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropTarget {
    Message,
    Shell,
}

/// Where the chat scrolls to by key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChatScroll {
    Up,
    Down,
    Top,
    Bottom,
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_working_line_keeps_up_with_the_plan() {
        // A step gives way to the plan's next step at the usual pace...
        assert!(super::swap_due(true, true, 700., 300.));
        // ...but holds against a call's words for its full while.
        assert!(!super::swap_due(true, false, 700., 300.));
        assert!(super::swap_due(true, false, 2000., 300.));
        // New words always settle first.
        assert!(!super::swap_due(false, true, 5000., 100.));
    }
}

/// Whether `w` stands in `hay` as a word of its own, not inside a longer one.
fn has_word(hay: &str, w: &str) -> bool {
    let alnum = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    hay.match_indices(w).any(|(at, _)| {
        !alnum(hay[..at].chars().next_back()) && !alnum(hay[at + w.len()..].chars().next())
    })
}

#[cfg(test)]
mod word_tests {
    #[test]
    fn a_whole_word_is_told_from_part_of_one() {
        assert!(super::has_word("hello number 7", "7"));
        assert!(!super::has_word("hello number 17", "7"));
        assert!(super::has_word("fix (cache) now", "cache"));
        assert!(!super::has_word("caches", "cache"));
        assert!(super::has_word("日本語 タイトル", "タイトル"));
    }
}
