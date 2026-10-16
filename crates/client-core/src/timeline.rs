//! Events in, timelines out.

use std::collections::{BTreeMap, HashMap};

use dent_protocol::{
    Activity, Answer, Block, Body, CommandOutcome, Event, Out, Outcome, Part, Permission, Status,
    Task, View, Window,
};
use serde_json::Value;

/// Every session the client knows, kept current from the daemon's feed.
#[derive(Default, Debug, Clone)]
pub struct Sessions {
    /// The last event applied; subscribe from here after a reconnect.
    pub last_seq: u64,
    pub list: BTreeMap<String, SessionView>,
    /// The newest usage reading from any session, with when it arrived.
    pub limits: Option<(Vec<Window>, u64)>,
    /// Catch-up is over and what arrives is live.
    pub live: bool,
}

#[derive(Default, Debug, Clone)]
pub struct SessionView {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub agent: String,
    pub model: Option<String>,
    pub permission: Permission,
    /// The agent's effort level, when not its default.
    pub effort: Option<String>,
    pub status: Status,
    pub closed: bool,
    pub created: u64,
    pub updated: u64,
    /// When the current turn started, while one runs.
    pub working_since: Option<u64>,
    pub entries: Vec<Entry>,
    /// The reply streaming in, not yet an entry.
    pub partial: String,
    /// Tokens in the context window, and its size when known.
    pub context: Option<(u64, Option<u64>)>,
    /// What fills the context and where it compacts on its own, from the
    /// agent's last full count.
    pub context_parts: Vec<Part>,
    pub compacts_at: Option<u64>,
    /// What the agent is doing besides talking: compacting, retrying.
    pub activity: Option<Activity>,
    pub background: Vec<Task>,
    /// Files the agent wrote that can be shown, with every version.
    pub made: Vec<Made>,
    /// Messages with a snapshot of the files from just before them, so
    /// rewinding can bring the files back too.
    pub checkpoints: std::collections::HashSet<String>,
    /// Side questions, oldest first, kept apart from the conversation.
    pub asides: Vec<Aside>,
    /// The agent's own id for the conversation, as it last said: what
    /// `claude --resume` takes to carry on in a terminal.
    pub agent_session: Option<String>,
    /// Where the current turn's entries begin.
    turn_from: usize,
    /// The first entry that changed since [`SessionView::take_changes`].
    changed_from: Option<usize>,
    /// Where each tool call lives: entry index and position in it.
    calls: HashMap<String, (usize, usize)>,
    /// Subagents working in the background: the agent's id, and the call
    /// that started it, which finishes when the agent leaves `background`.
    agents: HashMap<String, String>,
}

/// A side question and, once it's in, its answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Aside {
    pub id: String,
    pub question: String,
    /// The answer, whether it's an error, and the forked agent conversation
    /// that holds it.
    pub answer: Option<(String, bool, Option<String>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    User {
        id: String,
        text: String,
        /// Waiting for the turn to end.
        queued: bool,
        steer: bool,
        /// Paths of files that went with it.
        attachments: Vec<String>,
        /// When it was sent, in Unix milliseconds.
        at: u64,
    },
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    /// Tool calls in a row, with what subagents did nested by `parent`.
    Tools {
        calls: Vec<Call>,
    },
    /// The agent asked something. `answer` is set once it's answered.
    Ask {
        id: String,
        tool: String,
        input: String,
        description: Option<String>,
        /// It can be allowed for good in this project.
        keeps: bool,
        answer: Option<Answer>,
    },
    /// The agent wrote a file worth showing.
    Made {
        path: String,
        version: usize,
    },
    Note {
        note: Note,
        text: String,
    },
    /// A turn ended having edited these files.
    Changed {
        paths: Vec<String>,
    },
    /// A turn finished: when, and how long it worked (both in milliseconds).
    TurnEnd {
        at: u64,
        took: u64,
    },
    /// The conversation was summarised to make room.
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
        outcome: CommandOutcome,
        view: Option<View>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    Stopped,
    Error,
    Info,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    /// The input as JSON.
    pub input: String,
    pub parent: Option<String>,
    pub result: Option<(bool, String)>,
    /// For a subagent: the last thing it said while working.
    pub said: Option<String>,
}

impl Call {
    pub fn input(&self) -> Value {
        serde_json::from_str(&self.input).unwrap_or(Value::Null)
    }

    /// The file a call works on, if any.
    pub fn path(&self) -> Option<String> {
        let v = self.input();
        v["file_path"]
            .as_str()
            .or(v["notebook_path"].as_str())
            .or(v["path"].as_str())
            .map(str::to_owned)
    }

    /// A Write that replaced a file already there, as its result says.
    pub fn overwrote(&self) -> bool {
        self.name == "Write"
            && matches!(&self.result, Some((false, out)) if out.contains("has been updated"))
    }

    /// One plain line: what it did, to what.
    pub fn line(&self) -> String {
        let v = self.input();
        let short = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
        let what = match self.name.as_str() {
            // What the command is for, as the agent put it; the command
            // itself waits inside, for when the line is opened.
            "Bash" => v["description"]
                .as_str()
                .filter(|d| !d.trim().is_empty())
                .map(|d| first_line(d, 80)),
            "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
                self.path().map(|p| short(&p))
            }
            "Grep" | "Glob" => v["pattern"].as_str().map(|p| first_line(p, 60)),
            "LS" => v["path"].as_str().map(|p| first_line(p, 60)),
            "WebFetch" => v["url"].as_str().map(|u| first_line(u, 80)),
            "WebSearch" => v["query"].as_str().map(|q| first_line(q, 80)),
            "Task" | "Agent" => v["description"].as_str().map(|d| first_line(d, 80)),
            "TodoWrite" => Some("updated the plan".into()),
            _ => None,
        };
        // Running, a call says what it's doing; done, what it did.
        let running = self.result.is_none();
        let tense = |doing: &'static str, did: &'static str| if running { doing } else { did };
        let verb = match self.name.as_str() {
            "Bash" => match &self.result {
                None => "Running",
                Some((true, _)) => "Failed",
                Some((false, _)) => "Ran",
            },
            "Read" => tense("Reading", "Read"),
            // Write also replaces a file that was there; its result says which.
            "Write" if running => "Writing",
            "Write" if self.overwrote() => "Rewrote",
            "Write" => "Created",
            "Edit" | "MultiEdit" | "NotebookEdit" => tense("Editing", "Edited"),
            "Grep" => tense("Searching for", "Searched for"),
            "Glob" => tense("Looking for files", "Looked for files"),
            "LS" => tense("Listing", "Listed"),
            "WebFetch" => tense("Fetching", "Fetched"),
            "WebSearch" => tense("Searching the web for", "Searched the web for"),
            "Task" | "Agent" => "Subagent:",
            "TodoWrite" => "Updated",
            other => other,
        };
        if self.name == "Bash" {
            return match (&self.result, what) {
                (Some((true, _)), Some(w)) => format!("Failed to {}", lower_first(&w)),
                (Some((true, _)), None) => "Failed: a command".into(),
                (_, Some(w)) => tensed(&w, running),
                (None, None) => "Running a command".into(),
                (Some(_), None) => "Ran a command".into(),
            };
        }
        match what {
            Some(w) => format!("{verb} {w}"),
            None => verb.to_string(),
        }
    }
}

/// An agent's description of a command ("Run the tests") in the tense of
/// the moment: "Running the tests" while it runs, "Ran the tests" after. A
/// description that doesn't start with a verb we know reads as written.
pub fn tensed(desc: &str, running: bool) -> String {
    let tense = |word: &str| {
        TENSES
            .iter()
            .find(|(base, ..)| base.eq_ignore_ascii_case(word))
            .map(|&(_, doing, did)| if running { doing } else { did })
    };
    let mut words: Vec<String> = desc.split(' ').map(str::to_owned).collect();
    let Some(first) = words.first().and_then(|w| tense(w)) else {
        return desc.to_string();
    };
    words[0] = first.to_string();
    // A verb joined on by "and" or "then" ("… and summarize it", "…, then
    // rerun") takes the same tense, so the line reads as one sentence. After
    // a bare comma it may be a noun in a list ("config, test results").
    for i in 1..words.len() {
        let joined = matches!(words[i - 1].as_str(), "and" | "then");
        if joined && let Some(v) = tense(&words[i]) {
            words[i] = v.to_lowercase();
        }
    }
    words.join(" ")
}

/// The verbs command descriptions start with: base, doing, done.
const TENSES: &[(&str, &str, &str)] = &[
    ("Add", "Adding", "Added"),
    ("Append", "Appending", "Appended"),
    ("Capture", "Capturing", "Captured"),
    ("Diff", "Diffing", "Diffed"),
    ("Edit", "Editing", "Edited"),
    ("Extract", "Extracting", "Extracted"),
    ("Filter", "Filtering", "Filtered"),
    ("Generate", "Generating", "Generated"),
    ("Locate", "Locating", "Located"),
    ("Measure", "Measuring", "Measured"),
    ("Parse", "Parsing", "Parsed"),
    ("Patch", "Patching", "Patched"),
    ("Record", "Recording", "Recorded"),
    ("Report", "Reporting", "Reported"),
    ("Revert", "Reverting", "Reverted"),
    ("Review", "Reviewing", "Reviewed"),
    ("Sort", "Sorting", "Sorted"),
    ("Summarise", "Summarising", "Summarised"),
    ("Summarize", "Summarizing", "Summarized"),
    ("Trace", "Tracing", "Traced"),
    ("Validate", "Validating", "Validated"),
    ("Apply", "Applying", "Applied"),
    ("Build", "Building", "Built"),
    ("Check", "Checking", "Checked"),
    ("Clean", "Cleaning", "Cleaned"),
    ("Clone", "Cloning", "Cloned"),
    ("Commit", "Committing", "Committed"),
    ("Compare", "Comparing", "Compared"),
    ("Compile", "Compiling", "Compiled"),
    ("Confirm", "Confirming", "Confirmed"),
    ("Copy", "Copying", "Copied"),
    ("Count", "Counting", "Counted"),
    ("Create", "Creating", "Created"),
    ("Delete", "Deleting", "Deleted"),
    ("Download", "Downloading", "Downloaded"),
    ("Execute", "Executing", "Executed"),
    ("Fetch", "Fetching", "Fetched"),
    ("Find", "Finding", "Found"),
    ("Fix", "Fixing", "Fixed"),
    ("Format", "Formatting", "Formatted"),
    ("Get", "Getting", "Got"),
    ("Grep", "Searching", "Searched"),
    ("Init", "Initializing", "Initialized"),
    ("Initialize", "Initializing", "Initialized"),
    ("Inspect", "Inspecting", "Inspected"),
    ("Install", "Installing", "Installed"),
    ("Lint", "Linting", "Linted"),
    ("List", "Listing", "Listed"),
    ("Look", "Looking", "Looked"),
    ("Make", "Making", "Made"),
    ("Move", "Moving", "Moved"),
    ("Open", "Opening", "Opened"),
    ("Print", "Printing", "Printed"),
    ("Pull", "Pulling", "Pulled"),
    ("Push", "Pushing", "Pushed"),
    ("Read", "Reading", "Read"),
    ("Rebuild", "Rebuilding", "Rebuilt"),
    ("Remove", "Removing", "Removed"),
    ("Rename", "Renaming", "Renamed"),
    ("Rerun", "Rerunning", "Reran"),
    ("Re-run", "Re-running", "Re-ran"),
    ("Reset", "Resetting", "Reset"),
    ("Restore", "Restoring", "Restored"),
    ("Retry", "Retrying", "Retried"),
    ("Run", "Running", "Ran"),
    ("Save", "Saving", "Saved"),
    ("Search", "Searching", "Searched"),
    ("Set", "Setting", "Set"),
    ("Show", "Showing", "Showed"),
    ("Stage", "Staging", "Staged"),
    ("Start", "Starting", "Started"),
    ("Stash", "Stashing", "Stashed"),
    ("Stop", "Stopping", "Stopped"),
    ("Test", "Testing", "Tested"),
    ("Update", "Updating", "Updated"),
    ("Verify", "Verifying", "Verified"),
    ("View", "Viewing", "Viewed"),
    ("Write", "Writing", "Wrote"),
];

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        // An acronym or a name ("PR", "Cargo") keeps its capital.
        Some(f) if c.clone().next().is_some_and(|n| n.is_lowercase()) => {
            f.to_lowercase().chain(c).collect()
        }
        Some(f) => std::iter::once(f).chain(c).collect(),
        None => String::new(),
    }
}

/// The agent's id, when a subagent's call came back saying it went to the
/// background.
fn launched(output: &str) -> Option<String> {
    if !output.starts_with("Async agent launched") {
        return None;
    }
    let id = output.split("agentId:").nth(1)?.split_whitespace().next()?;
    Some(id.to_string())
}

/// Every verb a call's line can start with, longest first so "Searching
/// the web for" wins over "Searching for".
const VERBS: &[&str] = &[
    "Searching the web for",
    "Searched the web for",
    "Looking for files",
    "Looked for files",
    "Waiting for you:",
    "Searching for",
    "Searched for",
    "Subagent:",
    "Creating",
    "Created",
    "Writing",
    "Rewrote",
    "Editing",
    "Edited",
    "Reading",
    "Running",
    "Fetching",
    "Fetched",
    "Listing",
    "Listed",
    "Updated",
    "Failed",
    "Read",
    "Ran",
];

/// How much of a call's line is its verb, so the verb can sit quieter
/// than what it worked on: 0 when the line starts with no known verb.
pub fn verb_len(line: &str) -> usize {
    let starts =
        |v: &str| line.starts_with(v) && line[v.len()..].chars().next().is_none_or(|c| c == ' ');
    VERBS
        .iter()
        .copied()
        .chain(TENSES.iter().flat_map(|(_, doing, did)| [*doing, *did]))
        .filter(|v| starts(v))
        .map(str::len)
        .max()
        .unwrap_or(0)
}

fn first_line(s: &str, max: usize) -> String {
    let l = s.lines().next().unwrap_or_default();
    let mut out: String = l.chars().take(max).collect();
    if l.chars().count() > max || s.lines().count() > 1 {
        out.push('…');
    }
    out
}

/// A file the agent wrote, with each version it wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct Made {
    pub path: String,
    pub versions: Vec<Version>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    /// The whole file, when the call told us; an edit to a file whose
    /// earlier text we don't have leaves this empty until the host reads it.
    pub content: Option<String>,
}

/// Files the viewer can show.
pub fn viewable(path: &str) -> bool {
    let ext = path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "html" | "htm" | "md" | "markdown" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg"
    )
}

impl Sessions {
    /// Apply one message from the daemon. Returns the sessions it changed.
    pub fn apply(&mut self, out: &Out) -> Vec<String> {
        match out {
            Out::Events { events } => {
                let mut changed: Vec<String> = Vec::new();
                for e in events {
                    if e.seq <= self.last_seq {
                        continue;
                    }
                    self.last_seq = e.seq;
                    self.event(e);
                    if !changed.contains(&e.session) {
                        changed.push(e.session.clone());
                    }
                }
                changed
            }
            Out::Delta {
                session,
                text,
                reset,
            } => match self.list.get_mut(session) {
                Some(s) => {
                    if *reset {
                        s.partial.clear();
                    }
                    s.partial.push_str(text);
                    vec![session.clone()]
                }
                None => Vec::new(),
            },
            Out::Activity { session, activity } => match self.list.get_mut(session) {
                Some(s) => {
                    s.activity = activity.clone();
                    vec![session.clone()]
                }
                None => Vec::new(),
            },
            Out::Sheet {
                session,
                command,
                sections,
            } => match self.list.get_mut(session) {
                Some(s) => {
                    s.sheet(command, sections);
                    vec![session.clone()]
                }
                None => Vec::new(),
            },
            Out::CaughtUp { .. } => {
                self.live = true;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn event(&mut self, e: &Event) {
        if let Body::Limits { windows } = &e.body
            && self.limits.as_ref().is_none_or(|(_, at)| *at <= e.at)
        {
            self.limits = Some((windows.clone(), e.at));
        }
        let s = self
            .list
            .entry(e.session.clone())
            .or_insert_with(|| SessionView {
                id: e.session.clone(),
                created: e.at,
                ..SessionView::default()
            });
        s.updated = e.at;
        s.event(e);
    }
}

impl SessionView {
    /// The first entry that changed since the last call, and forget it.
    /// `None` when no entry changed.
    pub fn take_changes(&mut self) -> Option<usize> {
        self.changed_from.take()
    }

    /// A command's sheet as it is now: it replaces the one in the latest
    /// answer to that command.
    fn sheet(&mut self, command: &str, sections: &[dent_protocol::Section]) {
        let Some(ix) = self
            .entries
            .iter()
            .rposition(|e| matches!(e, Entry::Command { name, .. } if name == command))
        else {
            return;
        };
        if let Entry::Command { view, .. } = &mut self.entries[ix] {
            *view = Some(dent_protocol::View::Sheet {
                sections: sections.to_vec(),
            });
        }
        self.touch(ix);
    }

    fn touch(&mut self, ix: usize) {
        self.changed_from = Some(self.changed_from.map_or(ix, |c| c.min(ix)));
    }

    fn push(&mut self, e: Entry) {
        self.touch(self.entries.len());
        self.entries.push(e);
    }

    fn event(&mut self, e: &Event) {
        match &e.body {
            Body::Created {
                agent,
                cwd,
                model,
                permission,
                title,
            } => {
                self.agent = agent.clone();
                self.cwd = cwd.clone();
                self.model = model.clone();
                self.permission = *permission;
                self.title = title.clone();
                self.created = e.at;
            }
            Body::Settings {
                model,
                permission,
                title,
                effort,
            } => {
                if let Some(e) = effort {
                    self.effort = (!e.is_empty()).then(|| e.clone());
                }
                if let Some(m) = model {
                    self.model = Some(m.clone());
                }
                if let Some(p) = permission {
                    self.permission = *p;
                }
                if let Some(t) = title {
                    self.title = t.clone();
                }
            }
            Body::User {
                id,
                text,
                queued,
                steer,
                attachments,
            } => {
                if !queued && self.working_since.is_none() {
                    self.working_since = Some(e.at);
                }
                self.push(Entry::User {
                    id: id.clone(),
                    text: text.clone(),
                    queued: *queued,
                    steer: *steer,
                    attachments: attachments.clone(),
                    at: e.at,
                });
            }
            Body::Delivered { id } => {
                // It goes to the agent now, so it moves to here.
                if let Some(ix) = self.user_ix(id) {
                    let mut entry = self.entries.remove(ix);
                    if let Entry::User { queued, .. } = &mut entry {
                        *queued = false;
                    }
                    self.reindex(ix);
                    self.push(entry);
                }
                self.working_since = Some(e.at);
            }
            Body::Unqueued { id } => {
                if let Some(ix) = self.user_ix(id) {
                    self.entries.remove(ix);
                    self.reindex(ix);
                }
            }
            // Waiting messages show in the order they sit; moving one moves
            // its entry to just before the other, or after everything.
            Body::Requeued { id, before } => {
                if let Some(ix) = self.user_ix(id) {
                    let entry = self.entries.remove(ix);
                    let to = before
                        .as_deref()
                        .and_then(|b| self.user_ix(b))
                        .unwrap_or(self.entries.len());
                    self.entries.insert(to, entry);
                    self.reindex(ix.min(to));
                }
            }
            Body::Assistant { parent, blocks, .. } => {
                // The finished message takes the place of what streamed;
                // the next one streams from empty.
                if parent.is_none() {
                    self.partial.clear();
                }
                for b in blocks {
                    match (b, parent) {
                        (Block::Text { text }, None) if !text.trim().is_empty() => {
                            self.push(Entry::Text { text: text.clone() })
                        }
                        (Block::Thinking { text }, None) if !text.trim().is_empty() => {
                            self.push(Entry::Thinking { text: text.clone() })
                        }
                        // A subagent's words belong to the call that started it.
                        (Block::Text { text }, Some(p)) if !text.trim().is_empty() => {
                            if let Some(&(ix, at)) = self.calls.get(p) {
                                if let Some(Entry::Tools { calls }) = self.entries.get_mut(ix)
                                    && let Some(c) = calls.get_mut(at)
                                {
                                    c.said = Some(text.clone());
                                }
                                self.touch(ix);
                            }
                        }
                        (Block::Tool { id, name, input }, _) => {
                            self.call(Call {
                                id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                                parent: parent.clone(),
                                result: None,
                                said: None,
                            });
                        }
                        _ => {}
                    }
                }
            }
            Body::ToolResult {
                tool,
                error,
                output,
                ..
            } => {
                // A subagent sent to the background: its call goes on until
                // the agent finishes, and the receipt isn't its report.
                if let Some(agent) = launched(output)
                    && let Some(&(ix, _)) = self.calls.get(tool)
                {
                    self.agents.insert(agent, tool.clone());
                    self.touch(ix);
                    return;
                }
                if let Some(&(ix, at)) = self.calls.get(tool) {
                    let mut made = None;
                    if let Some(Entry::Tools { calls }) = self.entries.get_mut(ix)
                        && let Some(c) = calls.get_mut(at)
                    {
                        c.result = Some((*error, output.clone()));
                        if !error {
                            made = Some(c.clone());
                        }
                    }
                    self.touch(ix);
                    if let Some(c) = made {
                        self.made(&c);
                    }
                }
            }
            Body::Asked {
                id,
                tool,
                input,
                description,
                keeps,
            } => self.push(Entry::Ask {
                id: id.clone(),
                tool: tool.clone(),
                input: input.clone(),
                description: description.clone(),
                keeps: *keeps,
                answer: None,
            }),
            Body::Answered { id, answer } => {
                if let Some(ix) = self
                    .entries
                    .iter()
                    .rposition(|x| matches!(x, Entry::Ask { id: i, .. } if i == id))
                {
                    if let Entry::Ask { answer: a, .. } = &mut self.entries[ix] {
                        *a = Some(answer.clone());
                    }
                    self.touch(ix);
                }
            }
            Body::TurnStarted => {
                if self.working_since.is_none() {
                    self.working_since = Some(e.at);
                }
                self.turn_from = self.entries.len();
            }
            Body::TurnEnded { outcome } => {
                self.activity = None;
                let took = self.working_since.map_or(0, |s| e.at.saturating_sub(s));
                self.working_since = None;
                self.partial.clear();
                self.stop_unfinished();
                // What the turn edited, to review in one place.
                let mut paths: Vec<String> = Vec::new();
                for e in self.entries.get(self.turn_from..).unwrap_or_default() {
                    if let Entry::Tools { calls } = e {
                        for c in calls {
                            let edits = matches!(
                                c.name.as_str(),
                                "Write" | "Edit" | "MultiEdit" | "NotebookEdit"
                            );
                            if edits
                                && matches!(c.result, Some((false, _)))
                                && let Some(p) = c.path()
                                && !p.contains("/.claude/")
                                && !paths.contains(&p)
                            {
                                paths.push(p);
                            }
                        }
                    }
                }
                if !paths.is_empty() {
                    self.push(Entry::Changed { paths });
                }
                match outcome {
                    Outcome::Done => {}
                    // You stopped it.
                    Outcome::Interrupted { reason: None } => self.push(Entry::Note {
                        note: Note::Stopped,
                        text: "Stopped".into(),
                    }),
                    // Something else did (the agent crashed, dent restarted
                    // under it): that went wrong, and can be tried again.
                    Outcome::Interrupted { reason: Some(r) } => self.push(Entry::Note {
                        note: Note::Error,
                        text: r.clone(),
                    }),
                    Outcome::Error { message } => self.push(Entry::Note {
                        note: Note::Error,
                        text: message.clone(),
                    }),
                }
                self.push(Entry::TurnEnd { at: e.at, took });
            }
            Body::Context {
                used,
                max,
                compacts_at,
                parts,
            } => {
                let max = max.or(self.context.and_then(|c| c.1));
                self.context = Some((*used, max));
                // A count from a reply alone says nothing about its parts.
                if !parts.is_empty() {
                    self.context_parts = parts.clone();
                    self.compacts_at = *compacts_at;
                }
            }
            Body::Compacted {
                auto,
                before,
                after,
            } => {
                self.activity = None;
                self.push(Entry::Compacted {
                    auto: *auto,
                    before: *before,
                    after: *after,
                });
            }
            Body::Command {
                name,
                args,
                text,
                outcome,
                view,
            } => self.push(Entry::Command {
                name: name.clone(),
                args: args.clone(),
                text: text.clone(),
                outcome: outcome.clone(),
                view: view.clone(),
            }),
            Body::Background { tasks } => {
                self.background = tasks.clone();
                self.finish_agents(|agent| tasks.iter().any(|t| t.id == agent));
            }
            Body::Status { status } => {
                self.status = *status;
                // Nothing runs any more: no subagent is still at work.
                if !matches!(
                    status,
                    Status::Working | Status::NeedsYou | Status::Background
                ) {
                    self.finish_agents(|_| false);
                    self.stop_unfinished();
                }
                if !matches!(status, Status::Working | Status::NeedsYou) {
                    self.working_since = None;
                }
            }
            Body::Notice { text } => self.push(Entry::Note {
                note: Note::Info,
                text: text.clone(),
            }),
            Body::Checkpoint { id, .. } => {
                self.checkpoints.insert(id.clone());
            }
            // Everything from that message on leaves the conversation.
            Body::Rewound { to, .. } => {
                if let Some(ix) = self.user_ix(to) {
                    self.entries.truncate(ix);
                    self.reindex(ix);
                    // A file keeps only the versions written before that
                    // message, so its stepper forgets the abandoned ones.
                    for m in &mut self.made {
                        let kept = self
                            .entries
                            .iter()
                            .filter_map(|e| match e {
                                Entry::Made { path, version } if *path == m.path => Some(*version),
                                _ => None,
                            })
                            .max()
                            .unwrap_or(0);
                        m.versions.truncate(kept);
                    }
                    self.made.retain(|m| !m.versions.is_empty());
                }
                self.partial.clear();
                self.working_since = None;
            }
            Body::Closed => self.closed = true,
            Body::Reopened => self.closed = false,
            Body::Aside { id, question } => self.asides.push(Aside {
                id: id.clone(),
                question: question.clone(),
                answer: None,
            }),
            Body::AsideReply {
                id,
                text,
                error,
                agent_session,
            } => {
                if let Some(a) = self.asides.iter_mut().find(|a| a.id == *id) {
                    a.answer = Some((text.clone(), *error, agent_session.clone()));
                }
            }
            Body::Agent {
                agent_session: Some(a),
                ..
            } => self.agent_session = Some(a.clone()),
            Body::Limits { .. } | Body::Agent { .. } | Body::Forked { .. } => {}
        }
    }

    fn user_ix(&self, id: &str) -> Option<usize> {
        self.entries
            .iter()
            .rposition(|x| matches!(x, Entry::User { id: i, .. } if i == id))
    }

    /// Entries from `from` moved: rebuild where calls live.
    fn reindex(&mut self, from: usize) {
        self.touch(from);
        self.calls.clear();
        for (ix, e) in self.entries.iter().enumerate() {
            if let Entry::Tools { calls } = e {
                for (at, c) in calls.iter().enumerate() {
                    self.calls.insert(c.id.clone(), (ix, at));
                }
            }
        }
    }

    /// Calls the turn ended under (the agent stopped or crashed mid-call,
    /// or dent restarted under it) won't finish now: they stop reading as
    /// running, a subagent working in the foreground too. A subagent sent
    /// to the background, and what it does, works on past the turn.
    fn stop_unfinished(&mut self) {
        let from = self.turn_from.min(self.entries.len());
        let live: Vec<&String> = self.agents.values().collect();
        let mut touched = None;
        for (ix, e) in self.entries.iter_mut().enumerate().skip(from) {
            let Entry::Tools { calls } = e else { continue };
            // Live while it, or a call it sits under, is a background
            // subagent still at work.
            let under_live = |c: &Call| {
                let mut id = Some(&c.id);
                for _ in 0..16 {
                    let Some(i) = id else { return false };
                    if live.contains(&i) {
                        return true;
                    }
                    id = calls
                        .iter()
                        .find(|x| &x.id == i)
                        .and_then(|x| x.parent.as_ref());
                }
                false
            };
            let stop: Vec<usize> = calls
                .iter()
                .enumerate()
                .filter(|(_, c)| c.result.is_none() && !under_live(c))
                .map(|(i, _)| i)
                .collect();
            for i in stop {
                calls[i].result = Some((true, "Stopped before it finished".into()));
                touched.get_or_insert(ix);
            }
        }
        if let Some(ix) = touched {
            self.touch(ix);
        }
    }

    /// Background subagents that have stopped: each one's report is the
    /// last thing it said.
    fn finish_agents(&mut self, running: impl Fn(&str) -> bool) {
        let done: Vec<String> = self
            .agents
            .keys()
            .filter(|a| !running(a))
            .cloned()
            .collect();
        for agent in done {
            let Some(tool) = self.agents.remove(&agent) else {
                continue;
            };
            if let Some(&(ix, at)) = self.calls.get(&tool) {
                if let Some(Entry::Tools { calls }) = self.entries.get_mut(ix)
                    && let Some(c) = calls.get_mut(at)
                    && c.result.is_none()
                {
                    c.result = Some((false, c.said.clone().unwrap_or_default()));
                }
                self.touch(ix);
            }
        }
    }

    fn call(&mut self, c: Call) {
        // A subagent's calls join the group holding the call that started it.
        let group = match &c.parent {
            Some(p) => self.calls.get(p).map(|(ix, _)| *ix),
            None => match self.entries.last() {
                Some(Entry::Tools { .. }) => Some(self.entries.len() - 1),
                _ => None,
            },
        };
        let ix = match group {
            Some(ix) => ix,
            None => {
                self.push(Entry::Tools { calls: Vec::new() });
                self.entries.len() - 1
            }
        };
        if let Some(Entry::Tools { calls }) = self.entries.get_mut(ix) {
            self.calls.insert(c.id.clone(), (ix, calls.len()));
            calls.push(c);
        }
        self.touch(ix);
    }

    /// A write to a viewable file adds a version and a card.
    fn made(&mut self, c: &Call) {
        if !matches!(c.name.as_str(), "Write" | "Edit" | "MultiEdit") {
            return;
        }
        let Some(path) = c.path() else { return };
        // The agent's own bookkeeping (plans it saves under ~/.claude) isn't
        // something it made for you; the plan has its own card.
        if !viewable(&path) || path.contains("/.claude/") {
            return;
        }
        let v = c.input();
        let prev = self
            .made
            .iter()
            .find(|m| m.path == path)
            .and_then(|m| m.versions.last())
            .and_then(|v| v.content.clone());
        let content = match c.name.as_str() {
            "Write" => v["content"].as_str().map(str::to_owned),
            "Edit" => prev.and_then(|p| {
                apply_edit(
                    &p,
                    v["old_string"].as_str()?,
                    v["new_string"].as_str()?,
                    v["replace_all"].as_bool().unwrap_or(false),
                )
            }),
            _ => v["edits"].as_array().and_then(|edits| {
                edits.iter().try_fold(prev?, |text, e| {
                    apply_edit(
                        &text,
                        e["old_string"].as_str()?,
                        e["new_string"].as_str()?,
                        e["replace_all"].as_bool().unwrap_or(false),
                    )
                })
            }),
        };
        let version = match self.made.iter_mut().find(|m| m.path == path) {
            Some(m) => {
                m.versions.push(Version { content });
                m.versions.len()
            }
            None => {
                self.made.push(Made {
                    path: path.clone(),
                    versions: vec![Version { content }],
                });
                1
            }
        };
        self.push(Entry::Made { path, version });
    }

    /// The question waiting for an answer, if any.
    pub fn open_ask(&self) -> Option<&Entry> {
        self.entries
            .iter()
            .rev()
            .find(|e| matches!(e, Entry::Ask { answer: None, .. }))
    }
}

fn apply_edit(text: &str, old: &str, new: &str, all: bool) -> Option<String> {
    if old.is_empty() || !text.contains(old) {
        return None;
    }
    Some(if all {
        text.replace(old, new)
    } else {
        text.replacen(old, new, 1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(seq: u64, body: Body) -> Event {
        Event {
            seq,
            session: "s".into(),
            at: seq,
            body,
        }
    }

    fn tool(id: &str, name: &str, input: &str, parent: Option<&str>) -> Body {
        Body::Assistant {
            parent: parent.map(str::to_owned),
            blocks: vec![Block::Tool {
                id: id.into(),
                name: name.into(),
                input: input.into(),
            }],
            uuid: None,
        }
    }

    fn result(id: &str) -> Body {
        Body::ToolResult {
            tool: id.into(),
            error: false,
            output: "ok".into(),
            parent: None,
        }
    }

    #[test]
    fn the_agents_own_id_is_kept_for_resuming() {
        let s = feed(vec![
            Body::Agent {
                agent_session: Some("abc-1".into()),
                version: None,
            },
            Body::Agent {
                agent_session: None,
                version: Some("2.1".into()),
            },
        ]);
        assert_eq!(s.list["s"].agent_session.as_deref(), Some("abc-1"));
    }

    fn feed(bodies: Vec<Body>) -> Sessions {
        let mut s = Sessions::default();
        let events = bodies
            .into_iter()
            .enumerate()
            .map(|(i, b)| ev(i as u64 + 1, b))
            .collect();
        s.apply(&Out::Events { events });
        s
    }

    #[test]
    fn tool_calls_group_and_subagents_nest() {
        let s = feed(vec![
            tool("a", "Read", r#"{"file_path":"/x/src/lib.rs"}"#, None),
            result("a"),
            tool("t", "Task", r#"{"description":"Survey"}"#, None),
            tool("b", "Read", r#"{"file_path":"tests/db.rs"}"#, Some("t")),
            Body::Assistant {
                parent: None,
                blocks: vec![Block::Text {
                    text: "Done.".into(),
                }],
                uuid: None,
            },
            tool(
                "c",
                "Bash",
                r#"{"command":"cargo test\nmore","description":"Run the tests"}"#,
                None,
            ),
            tool("d", "Bash", r#"{"command":"ls | wc -l"}"#, None),
        ]);
        let v = &s.list["s"];
        assert_eq!(v.entries.len(), 3);
        match &v.entries[0] {
            Entry::Tools { calls } => {
                assert_eq!(calls.len(), 3);
                assert_eq!(calls[0].line(), "Read lib.rs");
                assert_eq!(calls[0].result, Some((false, "ok".into())));
                assert_eq!(calls[2].parent.as_deref(), Some("t"));
            }
            other => panic!("{other:?}"),
        }
        match &v.entries[2] {
            // A command reads as what it's for; with no description, as a
            // command. The shell itself waits inside.
            Entry::Tools { calls } => {
                assert_eq!(calls[0].line(), "Running the tests");
                assert_eq!(calls[1].line(), "Running a command");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn command_descriptions_take_the_tense_of_the_moment() {
        assert_eq!(tensed("Run the tests", true), "Running the tests");
        assert_eq!(
            tensed("Stage all changes and commit", false),
            "Staged all changes and committed"
        );
        assert_eq!(
            tensed("Run audit tool and summarize its output", false),
            "Ran audit tool and summarized its output"
        );
        assert_eq!(
            tensed("Fix the parser, then rerun tests", true),
            "Fixing the parser, then rerunning tests"
        );
        // A verb-like word that isn't joined on stays as it is.
        assert_eq!(tensed("Show test results", false), "Showed test results");
        assert_eq!(
            tensed("Print config, test results", false),
            "Printed config, test results"
        );
        assert_eq!(
            tensed("Rerun tests after the fix", false),
            "Reran tests after the fix"
        );
        // Not a verb we know: as written.
        assert_eq!(tensed("Baseline test run", true), "Baseline test run");
        assert_eq!(verb_len("Listed repo files"), "Listed".len());
        assert_eq!(lower_first("List repo files"), "list repo files");
        assert_eq!(lower_first("PR checks"), "PR checks");
    }

    /// A subagent sent to the background works on past its launch receipt
    /// and the turn's end; it finishes when it leaves the background list,
    /// with what it last said as its report.
    #[test]
    fn background_subagents_run_until_they_leave() {
        let task = |id: &str| dent_protocol::Task {
            id: id.into(),
            description: "Research".into(),
        };
        let receipt = Body::ToolResult {
            tool: "t".into(),
            error: false,
            output: "Async agent launched successfully. (internal)\nagentId: a6094 (internal ID)\nThe agent is working in the background.".into(),
            parent: None,
        };
        let mut s = feed(vec![
            tool("t", "Agent", r#"{"description":"Research"}"#, None),
            Body::Background {
                tasks: vec![task("a6094")],
            },
            receipt,
            tool("w", "WebSearch", r#"{"query":"plaid"}"#, Some("t")),
            Body::TurnEnded {
                outcome: Outcome::Done,
            },
            Body::Status {
                status: Status::Background,
            },
        ]);
        let agent = |s: &Sessions| match &s.list["s"].entries[0] {
            Entry::Tools { calls } => calls[0].clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(agent(&s).result, None, "still at work");
        let step = |s: &Sessions| match &s.list["s"].entries[0] {
            Entry::Tools { calls } => calls[1].clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(step(&s).result, None, "its steps work on too");
        s.apply(&Out::Events {
            events: vec![
                ev(
                    20,
                    Body::Assistant {
                        parent: Some("t".into()),
                        blocks: vec![Block::Text {
                            text: "Plaid charges per Item.".into(),
                        }],
                        uuid: None,
                    },
                ),
                ev(21, Body::Background { tasks: vec![] }),
            ],
        });
        assert_eq!(
            agent(&s).result,
            Some((false, "Plaid charges per Item.".into()))
        );
    }

    /// A subagent working in the foreground when the turn ends (stopped,
    /// crashed, restarted under) won't report now: it and its steps read
    /// as stopped, never as working.
    #[test]
    fn a_foreground_subagent_stops_with_its_turn() {
        let s = feed(vec![
            Body::TurnStarted,
            tool("t", "Task", r#"{"description":"Survey"}"#, None),
            tool("b", "Read", r#"{"file_path":"x"}"#, Some("t")),
            Body::TurnEnded {
                outcome: Outcome::Interrupted { reason: None },
            },
        ]);
        let Entry::Tools { calls } = &s.list["s"].entries[0] else {
            panic!()
        };
        assert!(calls.iter().all(|c| matches!(c.result, Some((true, _)))));
        // Nothing running any more says the same without a turn's end.
        let s = feed(vec![
            Body::TurnStarted,
            tool("t", "Agent", r#"{"description":"Survey"}"#, None),
            Body::Status {
                status: Status::Interrupted,
            },
        ]);
        let Entry::Tools { calls } = &s.list["s"].entries[0] else {
            panic!()
        };
        assert!(calls[0].result.is_some());
    }

    /// Rewinding forgets the versions written after the message.
    #[test]
    fn rewinding_drops_later_versions() {
        let write = |id: &str, content: &str| {
            tool(
                id,
                "Write",
                &format!(r#"{{"file_path":"a.md","content":"{content}"}}"#),
                None,
            )
        };
        let user = |id: &str| Body::User {
            id: id.into(),
            text: id.into(),
            queued: false,
            steer: false,
            attachments: Vec::new(),
        };
        let s = feed(vec![
            user("u1"),
            write("w1", "one"),
            result("w1"),
            user("u2"),
            write("w2", "two"),
            result("w2"),
            write("w3", "three"),
            result("w3"),
            Body::Rewound {
                to: "u2".into(),
                files: false,
            },
        ]);
        let v = &s.list["s"];
        assert_eq!(v.made.len(), 1);
        assert_eq!(v.made[0].versions.len(), 1);
        assert_eq!(v.made[0].versions[0].content.as_deref(), Some("one"));
    }

    /// Each message streams from empty: what streamed before its final
    /// message never shows twice, nor runs into the next message.
    #[test]
    fn a_final_message_replaces_its_stream() {
        let mut s = feed(vec![Body::TurnStarted]);
        let delta = |text: &str| Out::Delta {
            session: "s".into(),
            text: text.into(),
            reset: false,
        };
        let said = |seq, text: &str| Out::Events {
            events: vec![ev(
                seq,
                Body::Assistant {
                    parent: None,
                    blocks: vec![Block::Text { text: text.into() }],
                    uuid: None,
                },
            )],
        };
        s.apply(&delta("First part."));
        assert_eq!(s.list["s"].partial, "First part.");
        s.apply(&said(10, "First part."));
        assert_eq!(s.list["s"].partial, "");
        s.apply(&delta("Second."));
        assert_eq!(s.list["s"].partial, "Second.");
        s.apply(&said(11, "Second."));
        let v = &s.list["s"];
        assert_eq!(v.partial, "");
        assert_eq!(
            v.entries
                .iter()
                .filter(|e| matches!(e, Entry::Text { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn writes_and_edits_make_versions() {
        let s = feed(vec![
            tool(
                "w",
                "Write",
                r#"{"file_path":"site/a.html","content":"<h1>Hi</h1>"}"#,
                None,
            ),
            result("w"),
            tool(
                "e",
                "Edit",
                r#"{"file_path":"site/a.html","old_string":"Hi","new_string":"Hello"}"#,
                None,
            ),
            result("e"),
            tool(
                "x",
                "Write",
                r#"{"file_path":"src/main.rs","content":"fn main(){}"}"#,
                None,
            ),
            result("x"),
        ]);
        let v = &s.list["s"];
        assert_eq!(v.made.len(), 1);
        assert_eq!(
            v.made[0].versions[1].content.as_deref(),
            Some("<h1>Hello</h1>")
        );
        let cards: Vec<_> = v
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Made { .. }))
            .collect();
        assert_eq!(cards.len(), 2);
    }

    #[test]
    fn queued_messages_move_when_delivered_and_vanish_when_taken_back() {
        let user = |id: &str, queued| Body::User {
            id: id.into(),
            text: id.into(),
            queued,
            steer: false,
            attachments: Vec::new(),
        };
        let s = feed(vec![
            user("a", false),
            user("b", true),
            user("c", true),
            Body::Assistant {
                parent: None,
                blocks: vec![Block::Text {
                    text: "reply".into(),
                }],
                uuid: None,
            },
            Body::Unqueued { id: "c".into() },
            Body::Delivered { id: "b".into() },
        ]);
        let v = &s.list["s"];
        let ids: Vec<String> = v
            .entries
            .iter()
            .map(|e| match e {
                Entry::User { id, queued, .. } => format!("{id}{}", if *queued { "?" } else { "" }),
                Entry::Text { .. } => "text".into(),
                _ => "other".into(),
            })
            .collect();
        assert_eq!(ids, ["a", "text", "b"]);
    }

    #[test]
    fn waiting_messages_move_in_the_queue() {
        let user = |id: &str, queued| Body::User {
            id: id.into(),
            text: id.into(),
            queued,
            steer: false,
            attachments: Vec::new(),
        };
        let order = |s: &Sessions| -> Vec<String> {
            s.list["s"]
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::User { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect()
        };
        let s = feed(vec![
            user("a", false),
            user("b", true),
            user("c", true),
            user("d", true),
            Body::Requeued {
                id: "d".into(),
                before: Some("b".into()),
            },
        ]);
        assert_eq!(order(&s), ["a", "d", "b", "c"]);
        let s = feed(vec![
            user("b", true),
            user("c", true),
            Body::Requeued {
                id: "b".into(),
                before: None,
            },
        ]);
        assert_eq!(order(&s), ["c", "b"]);
    }

    #[test]
    fn changes_report_the_first_touched_entry() {
        let mut s = feed(vec![
            tool("a", "Read", r#"{"file_path":"x"}"#, None),
            Body::Assistant {
                parent: None,
                blocks: vec![Block::Text { text: "t".into() }],
                uuid: None,
            },
        ]);
        assert_eq!(s.list.get_mut("s").unwrap().take_changes(), Some(0));
        assert_eq!(s.list.get_mut("s").unwrap().take_changes(), None);
        s.apply(&Out::Events {
            events: vec![ev(3, result("a"))],
        });
        assert_eq!(s.list.get_mut("s").unwrap().take_changes(), Some(0));
        // Old events are ignored.
        assert!(
            s.apply(&Out::Events {
                events: vec![ev(2, result("a"))]
            })
            .is_empty()
        );
    }
}
