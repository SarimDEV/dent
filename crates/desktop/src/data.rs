//! What the window draws: sessions, their items, and what they made. Built
//! from the daemon's events by [`crate::bridge`].

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use dent_protocol::Permission;

use crate::theme::Status;

/// A string that lives for the whole run. Artifact ids and paths are used as
/// keys everywhere; there are few of them, so they're kept once.
pub fn intern(s: &str) -> &'static str {
    thread_local! {
        static SEEN: RefCell<HashSet<&'static str>> = RefCell::new(HashSet::new());
    }
    SEEN.with(|seen| {
        if let Some(x) = seen.borrow().get(s) {
            return *x;
        }
        let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
        seen.borrow_mut().insert(leaked);
        leaked
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A web page, shown live in the preview.
    Html,
    /// A Markdown document, rendered natively.
    Doc,
    /// A picture: a screenshot, a chart.
    Image,
    /// A dev server the agent started.
    Server,
    /// Any other text file you opened from a link: shown read-only, by line.
    Text,
    /// A video, a sound or a PDF: played or shown by the web view, read
    /// from disk as it plays, never held in memory whole.
    Media,
}

/// A visual's title, for a page shown with dent's own tool (kept under
/// its folder of visuals); `None` for any other file.
pub fn visual_title(path: &str) -> Option<&str> {
    path.strip_prefix(dent_client_core::timeline::VISUAL_DIR)?
        .strip_prefix('/')?
        .strip_suffix(".html")
}

/// A media file's own icon, by its name.
pub fn file_icon(path: &str) -> Option<&'static str> {
    Some(match media_kind(path)? {
        "Video" => "film",
        "Sound" => "audio-lines",
        _ => "file-text",
    })
}

/// What a media file is, by its name: "Video", "Sound" or "PDF".
pub fn media_kind(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "mp4" | "m4v" | "mov" | "webm" => "Video",
        "mp3" | "m4a" | "aac" | "wav" | "aiff" | "aif" | "flac" | "ogg" | "oga" => "Sound",
        "pdf" => "PDF",
        _ => return None,
    })
}

impl Kind {
    pub fn of(path: &str) -> Option<Kind> {
        let ext = path.rsplit('.').next()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "html" | "htm" => Kind::Html,
            "md" | "markdown" => Kind::Doc,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => Kind::Image,
            _ if media_kind(path).is_some() => Kind::Media,
            _ => return None,
        })
    }
    /// How a file you open is shown: as its kind, else as text when it reads
    /// as text (no NUL in its start, valid UTF-8) and isn't huge.
    pub fn opened(path: &str, bytes: &[u8]) -> Option<Kind> {
        Kind::of(path).or_else(|| {
            let head = &bytes[..bytes.len().min(8192)];
            (bytes.len() <= 8 << 20 && !head.contains(&0) && std::str::from_utf8(bytes).is_ok())
                .then_some(Kind::Text)
        })
    }
    /// Shown live in the web view.
    pub fn is_page(self) -> bool {
        matches!(self, Kind::Html | Kind::Server | Kind::Media)
    }
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Html => "app-window",
            Kind::Doc => "file-text",
            Kind::Image => "image",
            Kind::Server => "globe",
            Kind::Text => "file-code",
            Kind::Media => "file-play",
        }
    }
}

/// Something an agent made that is worth looking at, with every version it
/// wrote. Writing the same path again adds a version; nothing is replaced.
#[derive(Clone)]
pub struct Artifact {
    pub id: &'static str,
    pub kind: Kind,
    pub title: &'static str,
    /// As the agent named it.
    pub path: &'static str,
    /// The folder it's in, for a page's own images and styles.
    pub dir: Option<PathBuf>,
    pub versions: Vec<Rc<[u8]>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub text: String,
    pub options: Vec<(String, String)>,
    /// Only its choices answer it (Claude Code's own questions): no
    /// answer of your own.
    pub fixed: bool,
    /// Several of its choices can be picked together.
    pub multi: bool,
}

#[derive(Clone)]
pub enum Item {
    /// Something you sent: its id, for rewinding to it, and its text.
    User {
        id: String,
        /// As sent: the words, and any notes on lines after them.
        text: String,
        /// The files that went with it, by full path.
        files: Vec<String>,
        /// When you sent it, in Unix milliseconds.
        at: u64,
    },
    /// A reply, in Markdown.
    Text(String),
    /// What the agent thought before acting, folded to one line, and how
    /// long it thought in milliseconds (0 if unknown).
    Thinking(String, u64),
    /// Tool calls folded to one line, expandable.
    Tools {
        summary: String,
        lines: Vec<ToolLine>,
        /// The summary counts several calls ("Read 3 files") rather than
        /// being one call's own line.
        counted: bool,
        /// Pictures the calls brought back (screenshots, images read), by
        /// path: shown under the line, folded or not.
        pictures: Vec<String>,
    },
    /// The agent wrote a viewable file: a card that opens it.
    Made {
        art: &'static str,
        version: usize,
        /// The file as Changes lists it, when this turn's edits include
        /// it: the card offers Review instead of a separate line.
        review: Option<String>,
    },
    Approval {
        id: String,
        title: String,
        command: String,
        why: String,
        /// The command is an edit: its lines read as removed and added.
        diff: bool,
        /// The agent has a rule it could save, so it can be allowed for
        /// good in this project.
        keeps: bool,
    },
    /// The agent's plan, waiting for a go-ahead.
    Plan { id: String, plan: String },
    /// Questions with choices; the reply is one click each.
    Question {
        id: String,
        questions: Vec<Question>,
    },
    /// The agent asks for a secret, typed into a private card: `name` is
    /// what it's saved as, `why` what it's for.
    Secret {
        id: String,
        name: String,
        why: String,
    },
    /// One quiet line: Allowed, Stopped, an error…
    Note {
        icon: &'static str,
        text: String,
        /// What it says, for what follows it: the icon is only its look.
        kind: NoteKind,
    },
    /// A message waiting for the turn to end, with the files it carries.
    Queued {
        id: String,
        text: String,
        attachments: Vec<String>,
    },
    /// A turn finished: when and how long it took, both in milliseconds.
    /// Drawn as the footer under the turn's last reply, not a row of its own.
    TurnEnd { at: u64, took: u64 },
    /// Files a turn edited; opens Changes.
    Changed { paths: Vec<String> },
    /// The conversation was summarised to make room: a divider.
    Compacted {
        auto: bool,
        before: u64,
        after: Option<u64>,
    },
    /// A command the agent ran itself, and its answer.
    Command {
        name: String,
        args: String,
        text: String,
        outcome: dent_protocol::CommandOutcome,
        view: Option<dent_protocol::View>,
    },
}

/// What a note in the chat is, apart from how it looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum NoteKind {
    /// The turn was stopped: its footer goes after this note.
    Stopped,
    /// The turn ended in an error: it carries its own Try again.
    Error,
    Other,
}

/// A text's fingerprint, cheap to take each frame: its length and its
/// first and last 32 bytes. Streaming only appends, which changes the
/// length; hashing a long session's text whole would cost megabytes a
/// frame.
pub fn fingerprint<H: std::hash::Hasher>(t: &str, h: &mut H) {
    use std::hash::Hash;
    t.len().hash(h);
    let b = t.as_bytes();
    b[..b.len().min(32)].hash(h);
    b[b.len().saturating_sub(32)..].hash(h);
}

/// Everything in the chat that folds, and the key it's kept open under
/// (in `Dent::expanded`). One place, so the row signature that decides
/// when a row is measured again (`item_sig`) and Find's counts see every
/// fold an item has: a new fold added here can't be missed by either.
pub enum Fold<'a> {
    /// A long message of yours, sent or waiting.
    Message(&'a str),
    /// A long plan's card.
    Plan(&'a str),
    /// A subagent's report, by its call.
    Report(&'a str),
    /// A call's input and output shown whole: session, call.
    CallIn(&'a str, &'a str),
    CallOut(&'a str, &'a str),
    /// A subagent's earlier steps, by the key its lines carry.
    Steps(&'a str),
    /// A command's long answer: name, text.
    Command(&'a str, &'a str),
    /// A section of a command's sheet: session, item, section.
    Sheet(&'a str, usize, usize),
}

impl Fold<'_> {
    pub fn key(&self) -> String {
        match self {
            Fold::Message(id) => id.to_string(),
            Fold::Plan(ask) => format!("plan-open-{ask}"),
            Fold::Report(call) => format!("report-{call}"),
            Fold::CallIn(s, call) => format!("call-in-all-{s}/{call}"),
            Fold::CallOut(s, call) => format!("call-out-all-{s}/{call}"),
            Fold::Steps(k) => k.to_string(),
            Fold::Command(name, text) => crate::app::command_key(name, text),
            Fold::Sheet(s, ix, si) => format!("sheet/{s}/{ix}/{si}"),
        }
    }

    /// Every fold item `ix` of session `sid` has.
    pub fn of<'a>(sid: &'a str, ix: usize, item: &'a Item) -> Vec<Fold<'a>> {
        match item {
            Item::User { id, .. } | Item::Queued { id, .. } => vec![Fold::Message(id)],
            Item::Plan { id, .. } => vec![Fold::Plan(id)],
            Item::Tools { lines, .. } => lines
                .iter()
                .flat_map(|l| {
                    let mut f = Vec::new();
                    if l.report.is_some() {
                        f.push(Fold::Report(&l.id));
                    }
                    if l.detail.is_some() {
                        f.push(Fold::CallIn(sid, &l.id));
                        f.push(Fold::CallOut(sid, &l.id));
                    }
                    if let Some((k, _)) = &l.opens {
                        f.push(Fold::Steps(k));
                    }
                    f
                })
                .collect(),
            Item::Command {
                name, text, view, ..
            } => {
                let mut f = vec![Fold::Command(name, text)];
                if let Some(dent_protocol::View::Sheet { sections }) = view {
                    f.extend((0..sections.len()).map(|si| Fold::Sheet(sid, ix, si)));
                }
                f
            }
            _ => Vec::new(),
        }
    }

    /// The fold that decides how much of an item's words show (and so
    /// what Find searches): a message's or a plan's.
    pub fn words<'a>(item: &'a Item) -> Option<Fold<'a>> {
        match item {
            Item::User { id, .. } | Item::Queued { id, .. } => Some(Fold::Message(id)),
            Item::Plan { id, .. } => Some(Fold::Plan(id)),
            _ => None,
        }
    }
}

impl Item {
    /// What it reads as in plain text, for copying when it isn't drawn:
    /// your messages and the replies, the plan and notes. Folded work
    /// (thinking, tools) has none.
    pub fn plain(&self) -> Option<String> {
        match self {
            Item::User { text, .. } => Some(crate::draft::split_notes(text).0.to_string()),
            Item::Text(t) | Item::Note { text: t, .. } | Item::Plan { plan: t, .. } => {
                Some(t.clone())
            }
            Item::Command {
                name, args, text, ..
            } => Some(format!("/{name} {args}").trim_end().to_string() + "\n" + text),
            _ => None,
        }
    }
}

pub use dent_protocol::command_name;

/// Whether `path` is a file, asked of the disk at most every few seconds:
/// drawing asks every frame, for every line that names one.
pub fn is_file(path: &std::path::Path) -> bool {
    const FRESH: std::time::Duration = std::time::Duration::from_secs(3);
    thread_local! {
        static SEEN: RefCell<crate::lru::Lru<PathBuf, (bool, Instant)>> =
            RefCell::new(crate::lru::Lru::new(4096));
    }
    SEEN.with(|seen| {
        let now = Instant::now();
        let path = path.to_path_buf();
        if let Some((is, at)) = seen.borrow_mut().get(&path)
            && now.duration_since(at) < FRESH
        {
            return is;
        }
        let is = path.is_file();
        seen.borrow_mut().insert(path, (is, now));
        is
    })
}

/// One line of an opened tool group. Subagents' work sits under the call
/// that started it, a level deeper each time.
#[derive(Clone, Debug, PartialEq, Hash)]
pub struct ToolLine {
    pub text: String,
    pub depth: usize,
    /// A finished subagent's report, in Markdown, shown under its steps.
    pub report: Option<String>,
    /// The call it belongs to.
    pub id: String,
    /// Everything about the call, for when the line is opened.
    pub detail: Option<std::rc::Rc<CallDetail>>,
    /// A subagent's earlier step: shown only once this fold is opened.
    pub fold: Option<String>,
    /// The line that opens a fold, and how many steps it holds.
    pub opens: Option<(String, usize)>,
}

/// What a tool call did in full: what it was given, what came back, and
/// the file it worked on.
#[derive(Debug, PartialEq)]
pub struct CallDetail {
    /// The command, the edit as removed and added lines, or the input.
    pub input: String,
    /// A shell command, bare (no prompt sign): what Copy Command copies
    /// and Run in My Terminal types.
    pub command: Option<String>,
    /// What came back, and whether it failed.
    pub output: Option<(bool, String)>,
    /// The file, made whole from the session's folder.
    pub path: Option<String>,
    /// It changed that file.
    pub edits: bool,
}

/// Hashed by length and ends, not whole: a command's output can be large,
/// and items are fingerprinted every frame. Output only ever arrives whole.
impl std::hash::Hash for CallDetail {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.input.len().hash(h);
        self.output.as_ref().map(|(f, o)| (*f, o.len())).hash(h);
        self.path.hash(h);
        self.edits.hash(h);
    }
}

#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub title: String,
    /// The folder it works in.
    pub cwd: String,
    /// Its repository's name: in a worktree, the main checkout's.
    pub project: String,
    /// The branch it works on, as the daemon last said.
    pub branch: Option<String>,
    /// In a linked worktree: the project's main checkout.
    pub root: Option<String>,
    pub machine: String,
    /// "Claude · Opus", for the composer.
    pub agent: String,
    pub model: Option<String>,
    pub permission: Permission,
    pub effort: Option<String>,
    pub status: Status,
    /// What it is doing, or what it wants, in plain words.
    pub detail: String,
    /// Unix milliseconds of its last event.
    pub updated: u64,
    pub working_since: Option<Instant>,
    pub closed: bool,
    pub stopped: bool,
    /// Shared with the bridge, which extends it in place.
    pub items: Rc<Vec<Item>>,
    pub arts: Vec<&'static str>,
    pub diff: Rc<Vec<FileDiff>>,
    /// Changes whenever a file it wrote gets a new version.
    pub made_sig: usize,
    /// The agent's plan, as its latest todo list: (state, what, doing-form).
    pub todos: Vec<(String, String, String)>,
    /// Dev servers it started, by address.
    pub servers: Vec<String>,
    /// Videos, sounds and PDFs its replies named, by full path.
    pub media: Vec<String>,
    /// The agent's own id for it, for `claude --resume`.
    pub agent_session: Option<String>,
    /// Messages with a snapshot of the files from just before them.
    pub checkpoints: std::collections::HashSet<String>,
    /// Tokens in context, and the window's size when known.
    pub context: Option<(u64, Option<u64>)>,
    /// What fills the context, and where it compacts on its own.
    pub context_parts: Vec<dent_protocol::Part>,
    pub compacts_at: Option<u64>,
    /// Compacting or retrying, while it lasts.
    pub activity: Option<dent_protocol::Activity>,
    /// Side questions and their answers, kept out of the conversation.
    pub asides: Vec<dent_client_core::Aside>,
    /// The last event that finished a turn or asked something, and the
    /// event up to which it has been looked at.
    pub news: u64,
    pub seen: u64,
    /// Your messages that have earlier versions: (message, where in the
    /// timeline it sits, which version shows, how many there are).
    pub versions: Vec<(String, usize, usize, usize)>,
    /// Showing an earlier version: the rewind that left it behind.
    pub old: Option<u64>,
    /// Work the agent left running in the background, now.
    pub background: Vec<dent_protocol::Task>,
    /// When it carries on by itself as a used-up limit resets (Unix
    /// seconds).
    pub resume_at: Option<u64>,
}

impl Session {
    /// It finished or asked something since it was last looked at.
    pub fn unread(&self) -> bool {
        self.news > self.seen
    }

    /// The command that carries this conversation on in a terminal, in its
    /// folder, once the agent has said its id.
    pub fn resume_command(&self) -> Option<String> {
        let id = self.agent_session.as_deref()?;
        Some(format!(
            "cd {} && claude --resume {id}",
            crate::shell::shell_quote(&self.cwd)
        ))
    }

    /// A turn is under way: working, or waiting on you mid-turn. A turn that
    /// ended in an error also needs you, but nothing is running to stop.
    pub fn running(&self) -> bool {
        self.status == Status::Working || self.working_since.is_some()
    }

    /// Stands in for a session while it's being rebuilt.
    pub fn placeholder(id: &str) -> Self {
        Session {
            id: id.to_string(),
            title: String::new(),
            cwd: String::new(),
            project: String::new(),
            branch: None,
            root: None,
            machine: String::new(),
            agent: String::new(),
            model: None,
            permission: Permission::Ask,
            effort: None,
            status: Status::Working,
            detail: String::new(),
            updated: 0,
            working_since: None,
            closed: false,
            stopped: false,
            items: Rc::default(),
            arts: Vec::new(),
            diff: Rc::default(),
            context: None,
            context_parts: Vec::new(),
            compacts_at: None,
            activity: None,
            asides: Vec::new(),
            checkpoints: Default::default(),
            servers: Vec::new(),
            media: Vec::new(),
            made_sig: 0,
            todos: Vec::new(),
            agent_session: None,
            news: 0,
            seen: 0,
            background: Vec::new(),
            resume_at: None,
            versions: Vec::new(),
            old: None,
        }
    }
}

#[derive(Clone)]
pub struct FileDiff {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone)]
pub struct DiffLine {
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub kind: Line,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Context,
    Added,
    Removed,
    Hunk,
}

/// A unified diff, as git writes it, into lines with numbers.
pub fn parse_diff(path: &str, added: u32, removed: u32, diff: &str) -> FileDiff {
    let mut lines = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for l in diff.lines() {
        if let Some(rest) = l.strip_prefix("@@") {
            // @@ -a,b +c,d @@ context
            let mut nums = rest.split_whitespace();
            let start = |s: Option<&str>| {
                s.and_then(|s| s[1..].split(',').next()?.parse::<u32>().ok())
                    .unwrap_or(1)
            };
            old = start(nums.next());
            new = start(nums.next());
            in_hunk = true;
            lines.push(DiffLine {
                old: None,
                new: None,
                kind: Line::Hunk,
                text: l.to_string(),
            });
            continue;
        }
        if !in_hunk {
            continue;
        }
        let (kind, text) = match l.as_bytes().first() {
            Some(b'+') => (Line::Added, &l[1..]),
            Some(b'-') => (Line::Removed, &l[1..]),
            Some(b'\\') => continue,
            _ => (Line::Context, l.get(1..).unwrap_or("")),
        };
        let (o, n) = match kind {
            Line::Added => (None, Some(new)),
            Line::Removed => (Some(old), None),
            _ => (Some(old), Some(new)),
        };
        if kind != Line::Added {
            old += 1;
        }
        if kind != Line::Removed {
            new += 1;
        }
        lines.push(DiffLine {
            old: o,
            new: n,
            kind,
            text: text.to_string(),
        });
    }
    FileDiff {
        path: path.to_string(),
        added: added as usize,
        removed: removed as usize,
        lines,
    }
}

/// How long ago, in a word or two.
pub fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        86_400..2_592_000 => format!("{}d", s / 86_400),
        2_592_000..31_536_000 => format!("{}mo", s / 2_592_000),
        _ => format!("{}y", s / 31_536_000),
    }
}

/// Local calendar time for Unix seconds.
#[cfg(unix)]
pub fn local_tm(secs: i64) -> Option<libc::tm> {
    let t: libc::time_t = secs as libc::time_t;
    // SAFETY: an all-zero `tm` is a valid value to be overwritten.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call; localtime_r writes only
    // into `tm`.
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    ok.then_some(tm)
}

#[cfg(not(unix))]
pub fn local_tm(_secs: i64) -> Option<libc::tm> {
    None
}

/// A moment as a clock reads it: "3:42 PM" today, "Oct 12, 3:42 PM"
/// before, "Oct 12 2025, 3:42 PM" in another year. Local time.
pub fn clock(ms: u64) -> String {
    #[cfg(unix)]
    {
        let local = local_tm;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        if let (Some(at), Some(today)) = (local((ms / 1000) as i64), local(now)) {
            const MONTHS: [&str; 12] = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            let h = match at.tm_hour % 12 {
                0 => 12,
                h => h,
            };
            let time = format!(
                "{h}:{:02} {}",
                at.tm_min,
                if at.tm_hour < 12 { "AM" } else { "PM" }
            );
            let month = MONTHS[at.tm_mon.clamp(0, 11) as usize];
            return if at.tm_year == today.tm_year && at.tm_yday == today.tm_yday {
                time
            } else if at.tm_year == today.tm_year {
                format!("{month} {}, {time}", at.tm_mday)
            } else {
                format!("{month} {} {}, {time}", at.tm_mday, at.tm_year + 1900)
            };
        }
    }
    format!("{} ago", ago(ms))
}

/// A stretch of time ahead, in words: "40 minutes", "2h 10m", "3 days".
pub fn span(secs: u64) -> String {
    match secs {
        0..60 => "under a minute".into(),
        60..120 => "a minute".into(),
        120..3600 => format!("{} minutes", secs / 60),
        3600..86_400 => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
        _ if secs / 86_400 == 1 => "a day".into(),
        _ => format!("{} days", secs / 86_400),
    }
}

/// A machine as people say it: "this Mac" for the computer dent is on,
/// otherwise its name.
pub fn machine_shown(name: &str) -> String {
    static HERE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let here = HERE.get_or_init(|| {
        std::process::Command::new("hostname")
            .arg("-s")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    });
    if !name.is_empty() && name == here {
        if cfg!(target_os = "macos") {
            "this Mac".into()
        } else {
            "this computer".into()
        }
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_path_is_a_message_not_a_command() {
        use super::command_name;
        assert_eq!(command_name("/compact"), Some("compact"));
        assert_eq!(command_name("/model opus"), Some("model"));
        assert_eq!(command_name("/mcp__x__y"), Some("mcp__x__y"));
        assert_eq!(command_name("/plugin:cmd arg"), Some("plugin:cmd"));
        assert_eq!(
            command_name("/Users/me/app/crash.log why does this crash?"),
            None
        );
        assert_eq!(command_name("/notes.md"), None);
        assert_eq!(command_name("/ hello"), None);
        assert_eq!(command_name("/compact\nand more"), None);
        assert_eq!(command_name("hello /compact"), None);
    }

    #[test]
    fn resuming_in_a_terminal_starts_in_its_folder() {
        let mut s = super::Session::placeholder("x");
        s.cwd = "/Users/me/my app".into();
        assert_eq!(s.resume_command(), None);
        s.agent_session = Some("abc-1".into());
        assert_eq!(
            s.resume_command().as_deref(),
            Some("cd '/Users/me/my app' && claude --resume abc-1")
        );
    }

    #[test]
    fn opened_files_show_as_text_unless_binary() {
        use super::Kind;
        assert_eq!(Kind::opened("a.rs", b"fn main() {}\n"), Some(Kind::Text));
        assert_eq!(Kind::opened("a.md", b"# Hi"), Some(Kind::Doc));
        assert_eq!(Kind::opened("a.bin", b"\x00\x01\x02"), None);
        assert_eq!(Kind::of("a.rs"), None);
    }

    use super::*;

    #[test]
    fn spans_read_as_words() {
        assert_eq!(span(30), "under a minute");
        assert_eq!(span(40 * 60), "40 minutes");
        assert_eq!(span(2 * 3600 + 10 * 60), "2h 10m");
        assert_eq!(span(86_400 + 5), "a day");
        assert_eq!(span(3 * 86_400), "3 days");
    }

    #[test]
    fn diffs_number_their_lines() {
        let d = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3,3 +3,4 @@ fn\n a\n-b\n+c\n+d\n e\n";
        let f = parse_diff("x", 2, 1, d);
        let got: Vec<(Option<u32>, Option<u32>)> = f.lines.iter().map(|l| (l.old, l.new)).collect();
        assert_eq!(
            got,
            [
                (None, None),
                (Some(3), Some(3)),
                (Some(4), None),
                (None, Some(4)),
                (None, Some(5)),
                (Some(5), Some(6))
            ]
        );
    }

    #[test]
    fn every_fold_is_listed_for_its_item() {
        use super::{Fold, Item};
        let plan = Item::Plan {
            id: "p".into(),
            plan: "x".into(),
        };
        let keys: Vec<String> = Fold::of("s", 3, &plan).iter().map(Fold::key).collect();
        assert_eq!(keys, ["plan-open-p"]);
        assert_eq!(Fold::words(&plan).unwrap().key(), "plan-open-p");
        let queued = Item::Queued {
            id: "q".into(),
            text: "t".into(),
            attachments: Vec::new(),
        };
        assert_eq!(Fold::of("s", 0, &queued).len(), 1);
        // Each kind keeps its own key.
        let all = [
            Fold::Message("a"),
            Fold::Plan("a"),
            Fold::Report("a"),
            Fold::CallIn("s", "a"),
            Fold::CallOut("s", "a"),
            Fold::Command("a", ""),
            Fold::Sheet("s", 1, 2),
        ]
        .map(|f| f.key());
        let unique: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(unique.len(), all.len());
    }
}
