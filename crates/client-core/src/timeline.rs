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
    /// Where the current turn's entries begin.
    turn_from: usize,
    /// The first entry that changed since [`SessionView::take_changes`].
    changed_from: Option<usize>,
    /// Where each tool call lives: entry index and position in it.
    calls: HashMap<String, (usize, usize)>,
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

    /// One plain line: what it did, to what.
    pub fn line(&self) -> String {
        let v = self.input();
        let short = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
        let what = match self.name.as_str() {
            "Bash" => v["command"].as_str().map(|c| first_line(c, 80)),
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
        let verb = match self.name.as_str() {
            "Bash" => match &self.result {
                None => "Running",
                Some((true, _)) => "Failed",
                Some((false, _)) => "Ran",
            },
            "Read" => "Read",
            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => "Changed",
            "Grep" => "Searched",
            "Glob" => "Searched files",
            "LS" => "Listed",
            "WebFetch" => "Fetched",
            "WebSearch" => "Searched the web for",
            "Task" | "Agent" => "Subagent:",
            "TodoWrite" => "Updated",
            other => other,
        };
        match what {
            Some(w) if self.name == "TodoWrite" => format!("{verb} {w}"),
            Some(w) => format!("{verb} {w}"),
            None => verb.to_string(),
        }
    }
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
            Body::Assistant { parent, blocks, .. } => {
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
            } => self.push(Entry::Ask {
                id: id.clone(),
                tool: tool.clone(),
                input: input.clone(),
                description: description.clone(),
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
                self.working_since = None;
                self.partial.clear();
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
                    Outcome::Interrupted { reason } => self.push(Entry::Note {
                        note: Note::Stopped,
                        text: reason.clone().unwrap_or_else(|| "Stopped".into()),
                    }),
                    Outcome::Error { message } => self.push(Entry::Note {
                        note: Note::Error,
                        text: message.clone(),
                    }),
                }
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
            Body::Background { tasks } => self.background = tasks.clone(),
            Body::Status { status } => {
                self.status = *status;
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
                    self.made.retain(|m| {
                        self.entries
                            .iter()
                            .any(|e| matches!(e, Entry::Made { path, .. } if *path == m.path))
                    });
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
            tool("c", "Bash", r#"{"command":"cargo test\nmore"}"#, None),
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
            Entry::Tools { calls } => assert_eq!(calls[0].line(), "Running cargo test…"),
            other => panic!("{other:?}"),
        }
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
