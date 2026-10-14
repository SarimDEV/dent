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
}

#[derive(Clone)]
pub enum Item {
    /// Something you sent: its id, for rewinding to it, and its text.
    User {
        id: String,
        text: String,
    },
    /// A reply, in Markdown.
    Text(String),
    /// Tool calls folded to one line, expandable.
    Tools {
        summary: String,
        lines: Vec<String>,
        open: bool,
    },
    /// The agent wrote a viewable file: a card that opens it.
    Made {
        art: &'static str,
        version: usize,
    },
    Approval {
        id: String,
        title: String,
        command: String,
        why: String,
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
}

#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub project: String,
    pub machine: String,
    /// "Claude · Opus", for the composer.
    pub agent: String,
    pub model: Option<String>,
    pub permission: Permission,
    pub status: Status,
    /// What it is doing, or what it wants, in plain words.
    pub detail: String,
    /// Unix milliseconds of its last event.
    pub updated: u64,
    pub working_since: Option<Instant>,
    pub closed: bool,
    pub stopped: bool,
    pub items: Vec<Item>,
    pub arts: Vec<&'static str>,
    pub diff: Rc<Vec<FileDiff>>,
    pub terminal: Vec<(Tone, String)>,
    /// The agent's plan, as its latest todo list: (state, what, doing-form).
    pub todos: Vec<(String, String, String)>,
    /// Messages with a snapshot of the files from just before them.
    pub checkpoints: std::collections::HashSet<String>,
    /// Tokens in context, and the window's size when known.
    pub context: Option<(u64, Option<u64>)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Quiet,
    Good,
    Command,
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
        _ => format!("{}d", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
