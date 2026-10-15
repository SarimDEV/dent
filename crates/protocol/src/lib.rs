//! Wire messages, event types and protocol versioning shared by every dent
//! client and daemon.
//!
//! Everything a session does is an [`Event`] in the daemon's append-only log.
//! A client says [`Request::Subscribe`] with the last sequence number it has,
//! gets everything after it in batches, then [`Out::CaughtUp`], then live
//! events. Reconnecting is the same call.
//!
//! The format is one JSON object per line. Fields are only ever added, and a
//! client must ignore fields and event kinds it doesn't know.

use serde::{Deserialize, Serialize};

/// Bumped when a change would break an older client. Adding fields or event
/// kinds doesn't count.
pub const PROTOCOL: u32 = 1;

/// Catch-up batches stop growing past this many bytes of events.
pub const BATCH_BYTES: usize = 64 * 1024;

pub type SessionId = String;

/// One entry in the log.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Event {
    /// Monotonic per daemon, starting at 1.
    pub seq: u64,
    pub session: SessionId,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    #[serde(flatten)]
    pub body: Body,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    /// A new session. Always its first event.
    Created {
        agent: String,
        /// The directory the agent works in.
        cwd: String,
        model: Option<String>,
        permission: Permission,
        title: String,
    },
    /// The person changed a setting, or the agent reported one.
    Settings {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission: Option<Permission>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// How hard the agent thinks: low, medium, high, xhigh or max.
        /// Empty means the agent's default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    /// Something the person sent. `id` is the client's, so a resend after a
    /// reconnect is recognised and never runs twice.
    User {
        id: String,
        text: String,
        /// Sent while the agent was working: held until the turn ends, then
        /// [`Body::Delivered`], unless [`Body::Unqueued`] first.
        #[serde(default)]
        queued: bool,
        /// Sent into the running turn rather than after it.
        #[serde(default)]
        steer: bool,
        /// Files that went with it, as paths on the daemon's machine.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
    },
    /// A held message went to the agent.
    Delivered {
        id: String,
    },
    /// A held message was taken back before it went.
    Unqueued {
        id: String,
    },
    /// A held message moved in the queue: to just before held message
    /// `before`, or to the end.
    Requeued {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
    },
    /// A complete assistant message. Text streams first as [`Out::Delta`];
    /// this is what history keeps.
    Assistant {
        /// Set when a subagent said it, naming the tool call that started it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        blocks: Vec<Block>,
        /// The agent's id for the message, to rewind the conversation to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uuid: Option<String>,
    },
    /// The working tree as it was just before a message went to the agent,
    /// kept as a hidden commit.
    Checkpoint {
        /// The message it was taken for.
        id: String,
        commit: String,
    },
    /// This session began as a copy of another; its agent picks up that
    /// conversation, up to the assistant message `upto`, as a new session.
    /// (Body fields share the event's namespace: `at`, `seq` and `session`
    /// are taken.)
    Forked {
        from: SessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        upto: Option<String>,
    },
    /// The session went back to just before message `to`: what came after
    /// is no longer part of the conversation, and with `files` the working
    /// tree is as it was then.
    Rewound {
        to: String,
        files: bool,
    },
    ToolResult {
        tool: String,
        #[serde(default)]
        error: bool,
        /// The start of the output, cut to a few kilobytes.
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    /// The agent is blocked until the person answers.
    Asked {
        id: String,
        tool: String,
        /// The tool's input as JSON.
        input: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// The agent has a rule it could save for this kind of call, so
        /// it can be allowed for good in this project.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        keeps: bool,
    },
    Answered {
        id: String,
        answer: Answer,
    },
    TurnStarted,
    TurnEnded {
        outcome: Outcome,
    },
    /// How full the context window is, after each assistant message.
    Context {
        used: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<u64>,
        /// Where the agent compacts on its own; absent when it won't.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compacts_at: Option<u64>,
        /// What fills it, when the agent said.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        parts: Vec<Part>,
    },
    /// The agent summarised the conversation to make room.
    Compacted {
        /// It did so on its own as the context filled, not when asked.
        #[serde(default)]
        auto: bool,
        /// Tokens in context before, and after when the agent said.
        before: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<u64>,
    },
    /// A command the agent ran itself, with no model turn: its answer.
    Command {
        /// Without its `/`.
        name: String,
        #[serde(default)]
        args: String,
        /// What it said, as Markdown.
        text: String,
        #[serde(default)]
        outcome: CommandOutcome,
        /// The same answer as data, when there's a better way to show it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view: Option<View>,
    },
    /// Account usage, as the agent last reported it.
    Limits {
        windows: Vec<Window>,
    },
    /// Work the agent started that outlives its turn.
    Background {
        tasks: Vec<Task>,
    },
    Status {
        status: Status,
    },
    /// A quiet line in the chat: resumed, the agent restarted, and so on.
    Notice {
        text: String,
    },
    /// What the agent said about itself when it started.
    Agent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_session: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    Closed,
    Reopened,
    /// A side question: asked with the conversation's context, kept out of
    /// the conversation itself.
    Aside {
        id: String,
        question: String,
    },
    /// The answer to side question `id`. `agent_session` is the forked agent
    /// conversation that holds it, for making it a real fork.
    AsideReply {
        id: String,
        text: String,
        #[serde(default)]
        error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_session: Option<String>,
    },
}

/// How a command the agent ran itself went.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandOutcome {
    #[default]
    Done,
    /// It works only in the agent's own terminal app.
    TerminalOnly,
    /// No such command; `suggestion` is the agent's guess, without `/`.
    Unknown {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        suggestion: Option<String>,
    },
    Failed,
}

/// A command's answer as data.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum View {
    /// Something filling up, in parts: the context window.
    Meter {
        used: u64,
        max: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compacts_at: Option<u64>,
        parts: Vec<Part>,
    },
    /// Usage against the account's limits.
    Limits { bars: Vec<Bar> },
    /// A screen of facts in sections: each row says how something stands
    /// and what you can do about it. `/status`, `/mcp`, `/skills`,
    /// `/memory`, `/hooks` and `/permissions` answer with one.
    Sheet { sections: Vec<Section> },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Section {
    pub title: String,
    /// A line under the title that's true of every row.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub rows: Vec<Row>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub label: String,
    /// How it stands, in a few words.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    /// More, for those who want it: a description, where it's from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<Mark>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    /// Which part is something typed (a rule, a path, a command), to set
    /// in the code face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Field>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Label,
    Value,
    Detail,
}

/// How a row stands, at a glance.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    /// Working: connected, on.
    Good,
    /// Turned off, or not there.
    Off,
    /// On its way: starting, connecting.
    Waiting,
    /// Needs you: sign in, approve.
    Attention,
    /// Broken.
    Bad,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Action {
    pub label: String,
    pub act: Act,
}

/// Something a sheet's row offers. The client sends it back with
/// [`Request::Act`], except [`Act::Open`], which it does itself.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Act {
    /// Turn an MCP server on or off for this session.
    McpEnable {
        server: String,
        enabled: bool,
    },
    McpReconnect {
        server: String,
    },
    /// Start signing in to an MCP server: [`Out::Acted`] carries the page
    /// to open.
    McpSignIn {
        server: String,
    },
    /// A file or folder on the daemon's machine.
    Open {
        path: String,
    },
}

/// One limit and how much of it is used.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Bar {
    /// Like "Current session" or "This week (Fable)".
    pub label: String,
    /// 0 to 1.
    pub used: f64,
    /// Unix seconds, or 0 when not known.
    #[serde(default)]
    pub resets_at: i64,
}

/// A share of a meter.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    pub tokens: u64,
    /// Loaded only when used, so not counted against the window.
    #[serde(default)]
    pub deferred: bool,
}

/// What the agent is doing that isn't a message: short-lived, never logged.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Activity {
    /// Summarising the conversation, since Unix milliseconds `since`.
    Compacting { since: u64 },
    /// A request failed and will be tried again at Unix milliseconds `at`.
    Retrying {
        attempt: u32,
        max: u32,
        at: u64,
        /// What went wrong, in the agent's words.
        error: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    Tool {
        id: String,
        name: String,
        input: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    Allow {
        /// Allow this kind of call for the rest of the session.
        #[serde(default)]
        always: bool,
        /// Allow it from now on in this project: the agent saves its rule
        /// to the project's own settings, where it lists and enforces it.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        project: bool,
    },
    Deny {
        /// Reaches the agent verbatim.
        message: String,
    },
    /// Answers to an agent's questions, in order.
    Choose { answers: Vec<String> },
    /// The agent went away before anyone answered.
    Expired,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outcome {
    Done,
    /// Stopped by the person, or cut off by a crash or restart.
    Interrupted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Error {
        message: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Ask before edits and commands.
    #[default]
    Ask,
    /// Edit files without asking; ask before commands.
    Edits,
    /// Read and plan only.
    Plan,
    /// Never ask.
    Full,
}

impl Permission {
    pub const ALL: [Permission; 4] = [Self::Ask, Self::Edits, Self::Plan, Self::Full];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "Supervised",
            Self::Edits => "Auto-accept edits",
            Self::Plan => "Plan",
            Self::Full => "Full access",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Created, nothing run yet.
    #[default]
    New,
    Working,
    /// The turn ended, but work it started is still running.
    Background,
    /// Blocked on an answer.
    NeedsYou,
    /// The turn finished; waiting for the next prompt.
    Done,
    /// Stopped before the turn finished.
    Interrupted,
    Error,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Window {
    /// How the agent names it, like `five_hour`.
    pub name: String,
    /// 0 to 1.
    pub used: f64,
    /// Unix seconds.
    pub resets_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Task {
    pub id: String,
    pub description: String,
}

/// A model the agent offers.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The exact model this choice runs, like `claude-opus-5-5`; empty when
    /// the agent didn't say.
    #[serde(default)]
    pub resolved: String,
    /// The effort levels it takes; empty when the agent didn't say.
    #[serde(default)]
    pub efforts: Vec<String>,
}

/// A slash command or skill the agent offers in a session's folder.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Command {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// What goes after the name, like `<file>`.
    #[serde(default)]
    pub hint: String,
    /// What it takes, read from the hint and filled in from what the agent
    /// knows (its models, styles, settings), so a client can offer it.
    #[serde(default, skip_serializing_if = "Args::is_none")]
    pub args: Args,
}

/// What a command takes after its name.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Args {
    /// Nothing: it just runs.
    #[default]
    None,
    /// Words of your own, like instructions or a name.
    Text {
        /// What to write, like `custom summarization instructions`.
        placeholder: String,
        #[serde(default)]
        optional: bool,
    },
    /// One of a set.
    Choice {
        options: Vec<Choice>,
        #[serde(default)]
        optional: bool,
    },
    /// `key=value` settings.
    Settings { settings: Vec<Setting> },
}

impl Args {
    pub fn is_none(&self) -> bool {
        matches!(self, Args::None)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Choice {
    /// What's written: a word, or empty for a value of your own (see
    /// `placeholder`).
    pub value: String,
    /// How it's shown, when not the value itself.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
    /// A value of your own, like `tokens`: picking it leaves you typing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub placeholder: String,
    /// What's in effect now.
    #[serde(default)]
    pub current: bool,
    /// What may follow it.
    #[serde(default, skip_serializing_if = "Args::is_none")]
    pub then: Args,
}

/// A setting the agent's `/config` takes.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Setting {
    pub key: String,
    /// The values it takes; empty for any text.
    pub values: Vec<String>,
    /// Its value now, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
}

/// A session someone started with the agent in a terminal.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RecentSession {
    /// The agent's own session id.
    pub agent_session: String,
    pub cwd: String,
    /// Its name in Claude Code (yours, or the one it gave it), else the
    /// latest thing asked.
    pub title: String,
    /// The latest thing asked.
    #[serde(default)]
    pub last: String,
    /// Unix milliseconds of the last thing said in it.
    pub updated: u64,
    /// Its transcript is still being written: most likely it's open in a
    /// terminal, and picking it up would run it twice.
    #[serde(default)]
    pub open: bool,
}

/// A directory sessions run in.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Project {
    pub path: String,
    pub name: String,
    /// Unix milliseconds of the last session there, if any.
    #[serde(default)]
    pub last_used: u64,
}

/// A changed file, with its diff in unified format.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FileChange {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    /// `git diff` output for this file; empty for binary files.
    pub diff: String,
}

// ---- client to daemon ----

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Envelope<T> {
    /// Echoed on the reply. 0 for messages nobody asked for.
    #[serde(default)]
    pub req: u64,
    #[serde(flatten)]
    pub body: T,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Hello {
        proto: u32,
        client: String,
    },
    /// Everything after `after`, then live.
    Subscribe {
        after: u64,
    },
    Create {
        cwd: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        permission: Permission,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        /// The first message; its id, as for [`Request::Send`].
        id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
    },
    /// While the agent works, the message waits for the turn to end,
    /// unless `steer` sends it into the running turn.
    Send {
        session: SessionId,
        id: String,
        text: String,
        #[serde(default)]
        steer: bool,
        /// Paths of files that go with it. Images reach the agent as
        /// images; every file is also named by its path.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
    },
    /// Go back to just before one of your messages. With `files`, the
    /// working tree goes back too.
    Rewind {
        session: SessionId,
        to: String,
        files: bool,
    },
    /// A new session carrying on from this one: everything so far, or up
    /// to just before message `before`.
    Fork {
        session: SessionId,
        #[serde(default)]
        before: Option<String>,
    },
    /// Take back a message that's still waiting.
    Unqueue {
        session: SessionId,
        id: String,
    },
    /// Move a waiting message to just before another waiting one, or (with
    /// no `before`) to the end of the queue.
    Requeue {
        session: SessionId,
        id: String,
        #[serde(default)]
        before: Option<String>,
    },
    /// Stop the current turn. The session stays and takes the next message.
    Interrupt {
        session: SessionId,
    },
    Answer {
        session: SessionId,
        id: String,
        answer: Answer,
    },
    SetModel {
        session: SessionId,
        model: String,
    },
    SetPermission {
        session: SessionId,
        permission: Permission,
    },
    /// low, medium, high, xhigh or max; empty for the agent's default.
    SetEffort {
        session: SessionId,
        effort: String,
    },
    Rename {
        session: SessionId,
        title: String,
    },
    Close {
        session: SessionId,
    },
    Reopen {
        session: SessionId,
    },
    /// Ask a side question; the answer arrives as an `aside_reply` event.
    Aside {
        session: SessionId,
        id: String,
        text: String,
    },
    /// Make a side question and its answer into a new session that carries
    /// on from there.
    PromoteAside {
        session: SessionId,
        id: String,
    },
    Projects,
    Models,
    /// The account's usage windows, read from the provider.
    Usage,
    /// Sessions started with the agent in a terminal, newest first.
    Recent,
    /// Carry on a terminal session in dent, with its history.
    Adopt {
        agent_session: String,
    },
    /// The agent's slash commands: the session's, or the last ones seen.
    Commands {
        #[serde(default)]
        session: Option<SessionId>,
    },
    /// Exit so a newer daemon can take over. Agents keep running in the
    /// session holder, and the client starts the new daemon.
    Restart,
    /// What the session's directory has changed, against its last commit.
    Changes {
        session: SessionId,
    },
    /// Files in the session's directory, for @ mentions: tracked and
    /// untracked, ignoring what git ignores, at most a few thousand.
    Files {
        session: SessionId,
    },
    /// The same for a folder no session has yet: a new session's project,
    /// for @ mentions before it starts.
    FilesIn {
        dir: String,
    },
    /// Folders matching a typed path (`~/code/de`): the folder it's in and
    /// its subfolders starting with the last part. Hidden ones only when
    /// that part starts with a dot.
    Folders {
        partial: String,
    },
    /// Keep a folder in the project list, making it first if `create`.
    /// Replies with the list.
    AddProject {
        path: String,
        create: bool,
    },
    /// Take a project off the list, or put it back; its files stay.
    /// Replies with the list.
    HideProject {
        path: String,
        hidden: bool,
    },
    /// A new project from a name: a folder under `~/dent/projects`, a Git
    /// repository with a README and a first commit.
    NewProject {
        name: String,
    },
    /// Clone a Git URL into `~/dent/projects`.
    CloneProject {
        url: String,
    },
    /// A file in the session's directory, for the viewer.
    ReadFile {
        session: SessionId,
        path: String,
    },
    /// Do what a sheet's row offers. `command` names the sheet, which
    /// arrives again as [`Out::Sheet`] once it's done.
    Act {
        session: SessionId,
        command: String,
        act: Act,
    },
}

// ---- daemon to client ----

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Out {
    Reply {
        #[serde(flatten)]
        reply: Reply,
    },
    Error {
        message: String,
    },
    Events {
        events: Vec<Event>,
    },
    /// The agent's model list changed, say after it updated.
    Models {
        models: Vec<Model>,
    },
    /// Catch-up is over; what follows is live.
    CaughtUp {
        seq: u64,
    },
    /// Streaming text not yet in an [`Body::Assistant`] event. `text` is
    /// appended to the session's partial reply; `reset` clears it first.
    Delta {
        session: SessionId,
        text: String,
        #[serde(default)]
        reset: bool,
    },
    /// What the session's agent is doing besides talking, or `None` once
    /// it's done.
    Activity {
        session: SessionId,
        #[serde(default)]
        activity: Option<Activity>,
    },
    /// The session's commands changed (a plugin or skill reloaded).
    Commands {
        session: SessionId,
        commands: Vec<Command>,
    },
    /// A command's sheet as it is now, after something on it changed. It
    /// replaces the sheet of the session's latest answer to `command`.
    Sheet {
        session: SessionId,
        command: String,
        sections: Vec<Section>,
    },
    /// How an [`Request::Act`] went. `url` is a page for the client that
    /// asked to open, like a sign-in.
    Acted {
        session: SessionId,
        command: String,
        act: Act,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Welcome {
        proto: u32,
        version: String,
        machine: String,
        home: String,
        /// Identifies the exact daemon build: the running binary's size and
        /// modification time. A client beside a newer one asks it to restart.
        #[serde(default)]
        build: String,
    },
    Ok,
    Created {
        session: SessionId,
    },
    Projects {
        projects: Vec<Project>,
    },
    Folders {
        /// The folder listed, as typed (`~` kept).
        parent: String,
        /// Subfolders, full paths as typed.
        folders: Vec<String>,
    },
    /// A project made or cloned, and what went wrong short of failing (a
    /// first commit with no Git identity).
    ProjectMade {
        path: String,
        warning: Option<String>,
    },
    Models {
        models: Vec<Model>,
    },
    Commands {
        commands: Vec<Command>,
    },
    Recent {
        sessions: Vec<RecentSession>,
    },
    /// Empty when the agent can't say (signed out, or an API key).
    Usage {
        windows: Vec<Window>,
        /// Unix milliseconds of the reading.
        at: u64,
    },
    Changes {
        files: Vec<FileChange>,
    },
    /// Paths relative to the session's folder.
    Files {
        paths: Vec<String>,
    },
    File {
        bytes_b64: String,
    },
}

/// The socket a daemon with state directory `state` listens on. Usually
/// inside it; a state directory with a path too long for a socket address
/// (about 104 bytes on macOS) gets one in a private folder under /tmp,
/// named after the state path.
pub fn daemon_socket(state: &std::path::Path) -> std::path::PathBuf {
    let inside = state.join("daemon.sock");
    if inside.as_os_str().len() < 100 {
        return inside;
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in state.as_os_str().as_encoded_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    short_socket_dir(state).join(format!("{h:016x}.sock"))
}

/// `/tmp/dent-<uid>`, made private to this user. If someone else made it
/// first, sockets go nowhere useful rather than somewhere they control.
fn short_socket_dir(state: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let uid = std::fs::metadata(state)
        .or_else(|_| std::fs::metadata(state.parent().unwrap_or(state)))
        .map(|m| m.uid())
        .unwrap_or(u32::MAX);
    let dir = std::path::PathBuf::from(format!("/tmp/dent-{uid}"));
    let _ = std::fs::create_dir(&dir);
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() && m.uid() == uid && m.mode() & 0o077 == 0 => dir,
        _ => state.join("daemon.sock"),
    }
}

/// A binary's build stamp: its size and modification time, so a replaced
/// binary reads differently from the one that's running.
pub fn build_stamp(path: &std::path::Path) -> String {
    std::fs::metadata(path)
        .ok()
        .map(|m| {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("{}-{t}", m.len())
        })
        .unwrap_or_default()
}

/// One line of the wire format.
pub fn encode<T: Serialize>(req: u64, body: &T) -> Vec<u8> {
    #[derive(Serialize)]
    struct Line<'a, T> {
        req: u64,
        #[serde(flatten)]
        body: &'a T,
    }
    // Serializing these types can't fail: no maps with non-string keys.
    let mut v = serde_json::to_vec(&Line { req, body }).unwrap_or_default();
    v.push(b'\n');
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_and_tolerate_new_fields() {
        let e = Event {
            seq: 7,
            session: "s1".into(),
            at: 1,
            body: Body::Asked {
                id: "r".into(),
                tool: "Bash".into(),
                input: "{}".into(),
                description: None,
                keeps: false,
            },
        };
        let line = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&line).unwrap(), e);
        let newer = line.replace("\"seq\":7", "\"seq\":7,\"future\":true");
        assert_eq!(serde_json::from_str::<Event>(&newer).unwrap(), e);
    }

    #[test]
    fn requests_carry_their_id() {
        let line = encode(3, &Request::Subscribe { after: 9 });
        // A body may not reuse the event's own field names.
        let e = Event {
            seq: 1,
            session: "s".into(),
            at: 5,
            body: Body::Forked {
                from: "p".into(),
                upto: Some("u".into()),
            },
        };
        let back: Event = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
        let env: Envelope<Request> = serde_json::from_slice(&line).unwrap();
        assert_eq!(env.req, 3);
        assert_eq!(env.body, Request::Subscribe { after: 9 });
    }
}

/// Standard base64, for file bytes in [`Reply::File`].
pub mod b64 {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for c in bytes.chunks(3) {
            let n = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            for i in 0..4 {
                if i <= c.len() {
                    out.push(ABC[(n >> (18 - 6 * i)) as usize & 63] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    pub fn decode(s: &str) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(s.len() / 4 * 3);
        let (mut acc, mut bits) = (0u32, 0);
        for b in s.bytes().filter(|b| *b != b'=') {
            let v = ABC.iter().position(|c| *c == b)? as u32;
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        Some(out)
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn round_trips() {
            for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
                let e = super::encode(s.as_bytes());
                assert_eq!(super::decode(&e).unwrap(), s.as_bytes());
            }
            assert_eq!(super::encode(b"foobar"), "Zm9vYmFy");
            assert_eq!(super::encode(b"fo"), "Zm8=");
        }
    }
}
