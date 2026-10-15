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
}

impl Kind {
    pub fn of(path: &str) -> Option<Kind> {
        let ext = path.rsplit('.').next()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "html" | "htm" => Kind::Html,
            "md" | "markdown" => Kind::Doc,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => Kind::Image,
            _ => return None,
        })
    }
    /// Shown live in the web view.
    pub fn is_page(self) -> bool {
        matches!(self, Kind::Html | Kind::Server)
    }
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Html => "app-window",
            Kind::Doc => "file-text",
            Kind::Image => "image",
            Kind::Server => "globe",
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
}

#[derive(Clone)]
pub enum Item {
    /// Something you sent: its id, for rewinding to it, and its text.
    User {
        id: String,
        text: String,
        /// When you sent it, in Unix milliseconds.
        at: u64,
    },
    /// A reply, in Markdown.
    Text(String),
    /// What the agent thought before acting, folded to one line.
    Thinking(String),
    /// Tool calls folded to one line, expandable.
    Tools {
        summary: String,
        lines: Vec<ToolLine>,
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
    },
    /// The agent's plan, waiting for a go-ahead.
    Plan {
        id: String,
        plan: String,
    },
    /// Questions with choices; the reply is one click each.
    Question {
        id: String,
        questions: Vec<Question>,
    },
    /// One quiet line: Allowed, Stopped, an error…
    Note {
        icon: &'static str,
        text: String,
    },
    Queued {
        id: String,
        text: String,
    },
    /// Files a turn edited; opens Changes.
    Changed {
        paths: Vec<String>,
    },
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

impl Item {
    /// What it reads as in plain text, for copying when it isn't drawn:
    /// your messages and the replies, the plan and notes. Folded work
    /// (thinking, tools) has none.
    pub fn plain(&self) -> Option<String> {
        match self {
            Item::User { text, .. } => Some(
                text.lines()
                    .filter(|l| !l.starts_with("› "))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
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
}

/// What a tool call did in full: what it was given, what came back, and
/// the file it worked on.
#[derive(Debug, PartialEq)]
pub struct CallDetail {
    /// The command, the edit as removed and added lines, or the input.
    pub input: String,
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
    pub project: String,
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
}

impl Session {
    /// Stands in for a session while it's being rebuilt.
    pub fn placeholder(id: &str) -> Self {
        Session {
            id: id.to_string(),
            title: String::new(),
            cwd: String::new(),
            project: String::new(),
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
            made_sig: 0,
            todos: Vec::new(),
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

/// A moment as a clock reads it: "3:42 PM" today, "Oct 12, 3:42 PM"
/// before, "Oct 12 2025, 3:42 PM" in another year. Local time.
pub fn clock(ms: u64) -> String {
    #[cfg(unix)]
    {
        let local = |secs: i64| {
            let t: libc::time_t = secs as libc::time_t;
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            // SAFETY: both pointers are valid for the call; localtime_r
            // writes only into `tm`.
            let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
            ok.then_some(tm)
        };
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
}
