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
///
/// 2: a message can carry its files (`Request::Send`'s `files`); a client
/// sends files that way only to a daemon of 2 or later.
///
/// 3: a project can be brought to the machine (`Request::Pack`,
/// `Request::ReadFiles`, and `Request::Create`'s `bring`); LAN access is
/// set from a client (`Request::SetLan`); the welcome says how to wake the
/// machine (`wake`).
///
/// 4: a session's files can be read from anywhere by what the session
/// wrote (`Request::Stat`, `Request::Read`; [decision 0068]).
///
/// [decision 0068]: ../../docs/decisions/0068-files-across-machines.md
pub const PROTOCOL: u32 = 4;
/// The oldest protocol this build still speaks. Two sides talk when their
/// ranges overlap ([decision 0060]).
///
/// [decision 0060]: ../../docs/decisions/0060-remote-wire.md
pub const PROTOCOL_MIN: u32 = 1;

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
        /// Given back by the daemon itself, not asked for (the project its
        /// session was to work in couldn't be brought here), with what it
        /// said: a client puts it back in the session's reply box.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        returned: Option<Returned>,
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
        /// Pictures that came back (a screenshot, an image read), saved on
        /// the session's machine, by path.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<String>,
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
        /// The rules the agent offered, as JSON, so "always allow" means
        /// the same after a daemon restart. Absent in older logs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        suggestions: Option<String>,
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
    /// The session's pull request, said when it's first seen and whenever
    /// its state, checks or review change; `None` when it no longer has one.
    PullRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pr: Option<PullRequest>,
    },
    /// Where the session works in Git terms, said when it starts and
    /// whenever its branch changes.
    Workspace {
        /// The branch checked out; `None` when detached or not a repository.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        /// For a session in a linked worktree, the project's main checkout:
        /// the folder its project is named after.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root: Option<String>,
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
    /// The session will carry on by itself at `at` (Unix seconds), when a
    /// used-up limit resets; `None` takes that back.
    Resume {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<u64>,
    },
    /// Someone looked at the session after event `upto`: what finished or
    /// asked before then is no longer new to them.
    Seen {
        upto: u64,
    },
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
    /// Writing out a call, which can take minutes for a long file: `tool`
    /// is its name, `what` its target (a path, a command, a subagent's
    /// task) once that much has streamed.
    Preparing {
        tool: String,
        #[serde(default)]
        what: Option<String>,
    },
    /// Thinking, and the last thing it thought, a sentence at most.
    Thinking { latest: String },
    /// Getting a new worktree ready before the first message goes, since
    /// Unix milliseconds `since`: `what` is the step ("Running .dent/setup").
    SettingUp { since: u64, what: String },
}

/// The command a one-line message runs (`/compact`,
/// `/model opus`, `/mcp__x__y`): its name. A message that only starts with
/// a slash, like a pasted path (`/Users/me/app/crash.log why…`), isn't one.
pub fn command_name(text: &str) -> Option<&str> {
    if text.contains('\n') {
        return None;
    }
    let rest = text.strip_prefix('/')?;
    let name = rest.split(char::is_whitespace).next()?;
    (!name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ':')))
    .then_some(name)
}

/// Whether a call (by its tool's name, as `Block::Tool` carries it) runs a
/// subagent, whose result is its report: the one rule for it, read by the
/// daemon (live and imported sessions alike), client-core and the desktop.
pub fn runs_subagent(tool: &str) -> bool {
    matches!(tool, "Task" | "Agent")
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
    /// A secret the agent asked for, and where to keep it. The value goes
    /// only to the daemon, which saves it; the log keeps it empty.
    Secret {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        value: String,
        place: SecretPlace,
    },
    /// The agent went away before anyone answered.
    Expired,
}

/// Where a secret the agent asked for is kept.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SecretPlace {
    /// The project's `.env`.
    Project,
    /// A file of the session's own, under the daemon's state.
    Session,
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

fn default_agent() -> String {
    DEFAULT_AGENT.into()
}

/// A file a message brings with it ([`Request::Send`]'s `files`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    /// Its name where it came from; only the last part is kept.
    pub name: String,
    pub bytes_b64: String,
}

/// A file a session names, as it was written: absolute, `~/…`,
/// `file://…` or relative. The daemon resolves it, with its own home, against
/// `base` (the folder of the document it was written in) or else the
/// session's folder ([decision 0068]).
///
/// [decision 0068]: ../../docs/decisions/0068-files-across-machines.md
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileRef {
    pub session: SessionId,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

/// One version of a file: equal stamps are the same bytes, as far as the
/// machine can tell.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Stamp {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    /// Modification time, in nanoseconds since the epoch.
    pub mtime_ns: i64,
}

/// How a file a session names stands on its machine. Every answer but
/// `File` carries the path that was checked, to say where it looked.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileStat {
    File {
        /// The full path it resolved to.
        path: String,
        stamp: Stamp,
        /// A picture's size, from its header, so a place can be kept for it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
    },
    Dir {
        path: String,
    },
    Missing {
        checked: String,
    },
    /// The machine won't share it with this device.
    Refused {
        checked: String,
    },
    /// There, but not readable as a file: no permission, a pipe, a device.
    Unreadable {
        checked: String,
        error: String,
    },
}

/// The most one [`Request::Read`] returns, so a reply never holds up live
/// events for long.
pub const READ_MAX: u32 = 256 * 1024;

/// A network card a sleeping machine can be woken through: a magic packet
/// to `broadcast` (its subnet's) naming `mac`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Wake {
    /// `aa:bb:cc:dd:ee:ff`.
    pub mac: String,
    pub broadcast: String,
}

/// A message given back ([`Body::Unqueued`]'s `returned`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Returned {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
}

/// What another machine needs to make its own checkout of a project
/// ([`Request::Pack`]).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Pack {
    /// The checkout's folder name.
    pub name: String,
    /// Its Git remotes another machine can clone from, the one to clone
    /// first (`origin` when there is one) first. Remotes that are paths on
    /// this machine are left out.
    #[serde(default)]
    pub remotes: Vec<GitRemote>,
    /// The branch checked out here.
    #[serde(default)]
    pub branch: Option<String>,
    /// Files Git ignores, which a clone won't have (`.env`), as paths
    /// relative to the checkout. Ignored folders (`node_modules/`) aren't
    /// listed.
    #[serde(default)]
    pub ignored: Vec<Ignored>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct GitRemote {
    pub name: String,
    pub url: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Ignored {
    pub path: String,
    pub size: u64,
    /// Offered ticked: an environment file (`.env`, `.env.local`).
    #[serde(default)]
    pub carry: bool,
}

/// A project to bring to the machine a new session starts on
/// ([`Request::Create`]'s `bring`): cloned from its remotes into the
/// machine's projects folder, on its branch, with the ignored files it
/// carries (each [`Carried::name`] a path relative to the checkout).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Bring {
    pub name: String,
    pub remotes: Vec<GitRemote>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<Carried>,
}

/// The most a message's files may come to, so the message stays well
/// inside what a link takes once base64 has grown it by a third.
pub const CARRIED_MAX: usize = 40 << 20;

/// The agent a session runs when nothing says otherwise, and the one every
/// session made before agents had names ran.
pub const DEFAULT_AGENT: &str = "claude";

/// What an agent can do, so clients offer only what works with it. Every
/// flag is false for an agent that doesn't say, which hides the feature
/// rather than offering something that fails.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Caps {
    /// A message can go into the running turn (Send now, ⌘↩).
    pub steer: bool,
    /// The conversation can go back to before one of your messages (Edit
    /// from here, Try again).
    pub rewind: bool,
    /// A new session can carry on from this one's conversation.
    pub fork: bool,
    /// Side questions answered from a copy of the conversation.
    pub aside: bool,
    /// The permission modes it takes, in the order to offer them.
    pub permissions: Vec<Permission>,
    /// It takes an effort level (the model's own list says which).
    pub effort: bool,
    /// Its model can change mid-session.
    pub set_model: bool,
    /// A denial carries a reason the agent reads.
    pub deny_reason: bool,
    /// It can save "always allow" as a rule of the project's.
    pub project_rules: bool,
    /// Slash commands, with `/` completion.
    pub commands: bool,
    /// Commands whose answer is a sheet of rows to act on (`/mcp`…).
    pub sheets: bool,
    /// Background work it lists, each stoppable on its own.
    pub stop_task: bool,
    /// It reports how full its context is.
    pub context: bool,
    /// It can be asked to compact its context.
    pub compact: bool,
    /// dent's own tools (the plan, visuals, secrets) reach it.
    pub dent_tools: bool,
    /// Sessions started in its own terminal app can be picked up.
    pub adopt: bool,
    /// It reports the account's usage windows.
    pub usage: bool,
    /// It can start again on the same conversation to load new skills,
    /// plugins and settings.
    pub restart: bool,
}

impl Caps {
    /// Everything on: what a daemon from before agents said what they can
    /// do always offered.
    pub fn everything() -> Self {
        Self {
            steer: true,
            rewind: true,
            fork: true,
            aside: true,
            permissions: Permission::ALL.to_vec(),
            effort: true,
            set_model: true,
            deny_reason: true,
            project_rules: true,
            commands: true,
            sheets: true,
            stop_task: true,
            context: true,
            compact: true,
            dent_tools: true,
            adopt: true,
            usage: true,
            restart: true,
        }
    }
}

/// How an agent's CLI is signed in.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Account {
    /// Signed in to a plan; usage windows apply.
    SignedIn {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        /// The plan, as the agent names it ("max", "pro").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<String>,
    },
    /// Billed by API key: no plan limits.
    ApiKey,
    SignedOut,
    /// It couldn't be asked (not installed, or it didn't answer).
    #[default]
    Unknown,
}

/// One agent this machine's daemon can drive, and how it stands.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AgentInfo {
    /// Stable id, as in [`Body::Created`]: `claude`.
    pub id: String,
    /// Its full name: "Claude Code".
    pub name: String,
    /// What to call it in a sentence about the model ("Claude is
    /// compacting"): "Claude".
    pub short: String,
    /// Its CLI was found and answered.
    pub installed: bool,
    /// The command dent runs.
    pub command: String,
    /// The version it reports, when installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The version this dent was tested against: another may still work,
    /// and clients can say it's untested.
    pub tested: String,
    #[serde(default)]
    pub account: Account,
    pub caps: Caps,
    /// Where to read how to install it.
    #[serde(default)]
    pub install_url: String,
    /// What to run in a terminal to sign in, when it signs in that way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<String>,
}

/// How this machine is set up for what dent does besides agents: Git for
/// worktrees and commits, the GitHub CLI for pull requests, and starting
/// at login so sessions resume after a restart.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Machine {
    /// `git --version`'s answer; `None` when there's no Git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// Who commits are by ("Ada <ada@example.com>"); `None` when Git has
    /// no name or email to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_user: Option<String>,
    pub gh: Gh,
    /// The daemon starts at login.
    pub login_item: bool,
    /// Why it can't be made to, when it can't (a build run from a folder
    /// macOS guards from background programs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_item_blocked: Option<String>,
    /// Devices on its local network may connect without Tailscale (to pair
    /// with a code); paired ones always can.
    #[serde(default)]
    pub lan: bool,
}

/// The GitHub CLI, as pull requests need it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Gh {
    #[default]
    Missing,
    SignedOut,
    SignedIn {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
    },
}

impl AgentInfo {
    /// A version other than the one this dent was tested against.
    pub fn untested(&self) -> bool {
        self.version
            .as_deref()
            .is_some_and(|v| version_number(v) != version_number(&self.tested))
    }
}

/// The dotted number in an agent's version line: `2.1.301` from
/// `2.1.301 (Claude Code)`.
pub fn version_number(v: &str) -> &str {
    v.split_whitespace()
        .map(|w| w.trim_start_matches('v'))
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or(v)
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
    /// What kind of work: `local_bash` for a shell, `local_agent` for a
    /// subagent, and so on.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The tool call that started it, when the agent said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
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
    /// Which agent ran it ([`AgentInfo::id`]).
    #[serde(default = "default_agent")]
    pub agent: String,
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

/// Where a new session's worktree starts.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct NewWorktree {
    /// The branch it starts from; `None` is the one checked out now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

/// A repository as a new session sees it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Repo {
    /// Its main checkout, even when asked about one of its worktrees.
    pub root: String,
    /// The branch checked out in the folder asked about.
    #[serde(default)]
    pub current: Option<String>,
    /// Local branches, the most recently committed to first.
    #[serde(default)]
    pub branches: Vec<String>,
    /// Its linked worktrees, newest first; the main checkout isn't one.
    #[serde(default)]
    pub worktrees: Vec<Worktree>,
    /// It has a `.dent/setup` script, run in each new worktree.
    #[serde(default)]
    pub setup: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Worktree {
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
}

/// Where a session's branch stands against its remote.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct GitStatus {
    /// `None` when detached.
    pub branch: Option<String>,
    /// The remote's default branch (`main`), when known.
    #[serde(default)]
    pub default_branch: Option<String>,
    /// Changes not yet committed.
    pub changes: bool,
    /// The repository has an `origin` to push to.
    pub remote: bool,
    /// The branch tracks one on the remote.
    pub upstream: bool,
    pub ahead: u32,
    pub behind: u32,
    /// The GitHub CLI is there and signed in, so pull requests can be made.
    pub gh: bool,
    /// The branch's latest pull request, open or not, when there is one.
    #[serde(default)]
    pub pr: Option<PullRequest>,
}

/// A branch's pull request as GitHub has it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    #[serde(default)]
    pub state: PrState,
    #[serde(default)]
    pub checks: Checks,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<Review>,
}

impl PullRequest {
    pub fn open(&self) -> bool {
        self.state == PrState::Open
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    #[default]
    Open,
    Merged,
    Closed,
}

/// Its CI checks, counted.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Checks {
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
}

/// What its reviewers decided, when they have.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Review {
    Approved,
    ChangesRequested,
}

/// What of a pull request to brief the agent on.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrBrief {
    /// The failing checks, with the end of their logs.
    Checks,
    /// The review's requested changes and comments on lines.
    Review,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GitAction {
    Commit,
    CommitPush,
    CommitPushPr,
    Push,
    /// Push if needed, then open a pull request.
    Pr,
    Pull,
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

/// A device this machine trusts.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// Its key's fingerprint.
    pub id: String,
    pub name: String,
    /// "paired" (with a code). Devices trusted through the Tailscale
    /// account aren't stored, so never listed.
    pub how: String,
    /// "full" in v1.
    pub access: String,
    /// When it was added, in ms since the epoch.
    pub added: u64,
    /// When it last connected, in ms since the epoch; 0 for never.
    #[serde(default)]
    pub seen: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Hello {
        proto: u32,
        client: String,
    },
    /// Everything after `after`, then live. `epoch` is the log the client's
    /// `after` counts in; a different one means the daemon's log was reset,
    /// so it answers [`Out::Resync`] and starts from the beginning.
    Subscribe {
        after: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        epoch: Option<String>,
    },
    /// Show a pairing code on this machine, for a device to pair with
    /// ([decision 0061](../../docs/decisions/0061-trust.md)). Answered with
    /// [`Reply::Pairing`].
    OpenPairing,
    /// The devices this machine trusts.
    Devices,
    /// Stop trusting a device, and cut it off if it's connected.
    RemoveDevice {
        id: String,
    },
    Create {
        /// Where it works; empty for the machine's scratch folder.
        cwd: String,
        /// Which agent runs it ([`AgentInfo::id`]); none means the default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        /// Work in a new worktree of `cwd`'s repository instead of `cwd`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree: Option<NewWorktree>,
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
        /// As for [`Request::Send`].
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<Carried>,
        /// Clone this project here first and work in it (`cwd` is then
        /// empty). The session exists at once; its first message waits
        /// until the clone's done. A daemon restarted meanwhile starts the
        /// clone over.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bring: Option<Bring>,
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
        /// Files it carries itself, from a client on another machine whose
        /// paths mean nothing here: written to this machine first, then
        /// attached after `attachments`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<Carried>,
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
        /// Fork from an earlier version: everything before event `until`
        /// (the rewind that left it behind).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<u64>,
        /// The client's id for this fork, so a resend after a reconnect
        /// gets the same session rather than a second one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
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
        /// What asked: "the Stop button", "⌘.". The daemon logs it, so a
        /// stop no one remembers can be traced.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
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
    /// Start the agent again on the same conversation, so skills, plugins,
    /// MCP servers and settings changed on disk load: with the next message,
    /// or once the turn and its background work are done.
    RestartAgent {
        session: SessionId,
    },
    Close {
        session: SessionId,
    },
    Reopen {
        session: SessionId,
    },
    /// Carry on by itself at `at` (Unix seconds), as a limit resets, or
    /// not (`None`).
    Resume {
        session: SessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<u64>,
    },
    /// Stop one piece of background work.
    StopTask {
        session: SessionId,
        task: String,
    },
    /// Keep the machine awake while any agent works (on by default).
    KeepAwake {
        on: bool,
    },
    /// The session was looked at, up to event `upto`.
    Seen {
        session: SessionId,
        upto: u64,
    },
    /// Title the session again from what it's about now; the new title
    /// arrives as a `settings` event.
    Retitle {
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
    /// The agents this machine can drive, and how each stands. With
    /// `fresh`, asked again now rather than from the last minute's answer.
    Agents {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fresh: bool,
    },
    /// How this machine stands for Git, GitHub and starting at login.
    Machine,
    /// Let devices on the local network connect, or not; replies with the
    /// machine.
    SetLan {
        on: bool,
    },
    /// Start the daemon at login, or stop; replies with the machine.
    SetLoginItem {
        on: bool,
    },
    /// An agent's models (none: the default agent's).
    Models {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// The account's usage windows, read from the provider (for an agent;
    /// none: the default agent).
    Usage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// Sessions started with an agent in a terminal, newest first, from
    /// every agent that can be picked up.
    Recent,
    /// Carry on a terminal session in dent, with its history.
    Adopt {
        agent_session: String,
        /// Whose session it is; none means the default agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// The agent's slash commands: the session's, or the last ones seen.
    Commands {
        #[serde(default)]
        session: Option<SessionId>,
    },
    /// Exit so a newer daemon can take over. Agents keep running in the
    /// session holder, and the client starts the new daemon.
    Restart,
    /// What the session's directory has changed, against its last commit;
    /// with `turn` (one of your messages), only what that turn changed.
    Changes {
        session: SessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn: Option<String>,
    },
    /// Put a changed file back as it was (or, with `hunk`, one hunk of it,
    /// counted from 0): as at the last commit, or with `turn` as before
    /// that turn. Only the working tree as it is now can be put back.
    Discard {
        session: SessionId,
        path: String,
        #[serde(default)]
        hunk: Option<u32>,
        #[serde(default)]
        turn: Option<String>,
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
    /// What another machine needs to bring the project at `dir` there.
    /// Answered with [`Reply::Pack`].
    Pack {
        dir: String,
    },
    /// Ignored files of the checkout at `dir`, read for another machine
    /// ([`Pack::ignored`] paths), up to [`CARRIED_MAX`] in all. Answered
    /// with [`Reply::Carried`].
    ReadFiles {
        dir: String,
        paths: Vec<String>,
    },
    /// A folder's repository, for the new session's worktree and branch
    /// choices.
    Repo {
        dir: String,
    },
    /// Take away the worktree a closed session worked in. Without `force`,
    /// one with changes not committed is kept and the error says so. Its
    /// branch, and the commits on it, stay.
    RemoveWorktree {
        session: SessionId,
        #[serde(default)]
        force: bool,
    },
    /// Where the session's branch stands, for the Git actions.
    GitStatus {
        session: SessionId,
    },
    /// What the agent needs to fix a pull request's failing checks or
    /// answer its review, as a message to send it.
    PrBrief {
        session: SessionId,
        about: PrBrief,
    },
    /// Commit, push, open a pull request or pull, in the session's folder.
    /// A commit with no `message` gets one written from the diff.
    Git {
        session: SessionId,
        action: GitAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// A file in the session's directory, for the viewer.
    ReadFile {
        session: SessionId,
        path: String,
    },
    /// How each file stands, in order. Answered with [`Reply::Stats`].
    Stat {
        files: Vec<FileRef>,
    },
    /// Up to `len` bytes (at most [`READ_MAX`]) from `offset`. With
    /// `expect`, only from that version: a file rewritten since answers
    /// [`Reply::FileGone`] with `changed`. Answered with [`Reply::Bytes`].
    Read {
        file: FileRef,
        offset: u64,
        len: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<Stamp>,
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
    /// The log was reset (a new database or a restore): drop what was
    /// cached from this machine; events from the start follow.
    Resync {
        epoch: String,
    },
    Error {
        message: String,
    },
    Events {
        events: Vec<Event>,
    },
    /// An agent's model list changed, say after it updated.
    Models {
        models: Vec<Model>,
        /// Whose; empty from a daemon older than agent ids (the default).
        #[serde(default)]
        agent: String,
    },
    /// Catch-up is over; what follows is live.
    CaughtUp {
        seq: u64,
    },
    /// Streaming text not yet in an [`Body::Assistant`] event. `text` is
    /// appended to the session's partial reply; `reset` clears it first.
    /// The message's own event replaces it: clients clear the partial
    /// reply on any top-level `Assistant` event.
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
        /// The agents it can drive, their names and what each can do, the
        /// default first. How each stands on the machine (installed,
        /// signed in) isn't asked here, which takes seconds: see
        /// [`Request::Agents`].
        #[serde(default)]
        agents: Vec<AgentInfo>,
        /// This machine's id: its key's fingerprint
        /// ([decision 0059](../../docs/decisions/0059-machine-identity.md)).
        /// Empty from a daemon older than remote access.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        id: String,
        /// The event log's epoch, made with the database; `Subscribe`
        /// carries it back.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        epoch: String,
        /// Addresses other devices can reach this machine at, as
        /// `host:port`, best first. Clients learn routes from them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        addresses: Vec<String>,
        /// How to wake it from its own network while it sleeps.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        wake: Vec<Wake>,
    },
    Ok,
    Pairing {
        code: String,
        /// When the code stops working, in ms since the epoch.
        expires: u64,
    },
    Devices {
        devices: Vec<Device>,
    },
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
    Agents {
        agents: Vec<AgentInfo>,
        /// The agent a new session gets when none is asked for.
        default: String,
    },
    Machine {
        machine: Machine,
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
    Stats {
        stats: Vec<FileStat>,
    },
    Bytes {
        stamp: Stamp,
        offset: u64,
        /// The file's whole size.
        total: u64,
        bytes_b64: String,
    },
    /// A read that couldn't be done, and why; `changed` when the file is
    /// there but isn't the version expected.
    FileGone {
        stat: FileStat,
        #[serde(default)]
        changed: bool,
    },
    /// `None` when the folder isn't in a Git repository.
    Repo {
        repo: Option<Repo>,
    },
    /// `None` when the folder isn't in a Git repository.
    Pack {
        pack: Option<Pack>,
    },
    Carried {
        files: Vec<Carried>,
    },
    /// `None` when the session's folder isn't in a Git repository.
    GitStatus {
        status: Option<GitStatus>,
    },
    PrBrief {
        text: String,
    },
    /// A Git action done: what happened, in a line, and the pull request's
    /// address when there is one.
    GitDone {
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
}

impl Request {
    /// Every session id this request names. A client connected to several
    /// machines qualifies ids with their machine and routes a request by
    /// them ([decision 0058](../../docs/decisions/0058-independent-machines.md)).
    /// No catch-all arm: a new request must say whether it names a session.
    pub fn sessions_mut(&mut self) -> Vec<&mut SessionId> {
        use Request as R;
        match self {
            R::Send { session, .. }
            | R::Rewind { session, .. }
            | R::Fork { session, .. }
            | R::Unqueue { session, .. }
            | R::Requeue { session, .. }
            | R::Interrupt { session, .. }
            | R::Answer { session, .. }
            | R::SetModel { session, .. }
            | R::SetPermission { session, .. }
            | R::SetEffort { session, .. }
            | R::Rename { session, .. }
            | R::RestartAgent { session }
            | R::Close { session }
            | R::Reopen { session }
            | R::Resume { session, .. }
            | R::StopTask { session, .. }
            | R::Seen { session, .. }
            | R::Retitle { session }
            | R::Aside { session, .. }
            | R::PromoteAside { session, .. }
            | R::Changes { session, .. }
            | R::Discard { session, .. }
            | R::Files { session }
            | R::RemoveWorktree { session, .. }
            | R::GitStatus { session }
            | R::PrBrief { session, .. }
            | R::Git { session, .. }
            | R::ReadFile { session, .. }
            | R::Act { session, .. } => vec![session],
            R::Stat { files } => files.iter_mut().map(|f| &mut f.session).collect(),
            R::Read { file, .. } => vec![&mut file.session],
            R::Commands { session } => session.iter_mut().collect(),
            R::Hello { .. }
            | R::Subscribe { .. }
            | R::OpenPairing
            | R::Devices
            | R::RemoveDevice { .. }
            | R::Create { .. }
            | R::KeepAwake { .. }
            | R::Projects
            | R::Agents { .. }
            | R::Machine
            | R::SetLan { .. }
            | R::SetLoginItem { .. }
            | R::Models { .. }
            | R::Usage { .. }
            | R::Recent
            | R::Adopt { .. }
            | R::Restart
            | R::FilesIn { .. }
            | R::Folders { .. }
            | R::AddProject { .. }
            | R::HideProject { .. }
            | R::NewProject { .. }
            | R::CloneProject { .. }
            | R::Pack { .. }
            | R::ReadFiles { .. }
            | R::Repo { .. } => Vec::new(),
        }
    }
}

impl Out {
    /// Every session id this message names, as [`Request::sessions_mut`].
    pub fn sessions_mut(&mut self) -> Vec<&mut SessionId> {
        match self {
            Out::Events { events } => {
                let mut v = Vec::new();
                for e in events {
                    v.push(&mut e.session);
                    if let Body::Forked { from, .. } = &mut e.body {
                        v.push(from);
                    }
                }
                v
            }
            Out::Delta { session, .. }
            | Out::Activity { session, .. }
            | Out::Commands { session, .. }
            | Out::Sheet { session, .. }
            | Out::Acted { session, .. } => vec![session],
            Out::Reply { reply } => match reply {
                Reply::Created { session } => vec![session],
                Reply::Welcome { .. }
                | Reply::Ok
                | Reply::Pairing { .. }
                | Reply::Devices { .. }
                | Reply::Projects { .. }
                | Reply::Folders { .. }
                | Reply::ProjectMade { .. }
                | Reply::Models { .. }
                | Reply::Agents { .. }
                | Reply::Machine { .. }
                | Reply::Commands { .. }
                | Reply::Recent { .. }
                | Reply::Usage { .. }
                | Reply::Changes { .. }
                | Reply::Files { .. }
                | Reply::File { .. }
                | Reply::Stats { .. }
                | Reply::Bytes { .. }
                | Reply::FileGone { .. }
                | Reply::Repo { .. }
                | Reply::Pack { .. }
                | Reply::Carried { .. }
                | Reply::GitStatus { .. }
                | Reply::PrBrief { .. }
                | Reply::GitDone { .. } => Vec::new(),
            },
            Out::Resync { .. } | Out::Error { .. } | Out::Models { .. } | Out::CaughtUp { .. } => {
                Vec::new()
            }
        }
    }
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
    fn requests_from_before_agent_ids_still_parse() {
        let r = |s: &str| serde_json::from_str::<Request>(s).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r(r#"{"op":"models"}"#), Request::Models { agent: None });
        assert_eq!(r(r#"{"op":"agents"}"#), Request::Agents { fresh: false });
        assert_eq!(
            serde_json::to_string(&Request::Agents { fresh: false }).unwrap(),
            r#"{"op":"agents"}"#
        );
        assert_eq!(r(r#"{"op":"usage"}"#), Request::Usage { agent: None });
        assert!(matches!(
            r(r#"{"op":"adopt","agent_session":"a"}"#),
            Request::Adopt { agent: None, .. }
        ));
        let recent: RecentSession =
            serde_json::from_str(r#"{"agent_session":"a","cwd":"/","title":"t","updated":1}"#)
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(recent.agent, DEFAULT_AGENT);
    }

    #[test]
    fn a_version_other_than_the_tested_one_says_so() {
        let mut a = AgentInfo {
            id: "claude".into(),
            name: "Claude Code".into(),
            short: "Claude".into(),
            installed: true,
            command: "claude".into(),
            version: Some("2.1.301 (Claude Code)".into()),
            tested: "2.1.301".into(),
            account: Account::Unknown,
            caps: Caps::default(),
            install_url: String::new(),
            sign_in: None,
        };
        assert!(!a.untested());
        a.version = Some("v2.2.0".into());
        assert!(a.untested());
        a.version = None;
        assert!(!a.untested());
    }

    #[test]
    fn a_stop_from_an_older_client_still_parses() {
        let old: Request = serde_json::from_str(r#"{"op":"interrupt","session":"s1"}"#)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(old, Request::Interrupt { by: None, .. }));
    }

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
                suggestions: None,
            },
        };
        let line = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&line).unwrap(), e);
        let newer = line.replace("\"seq\":7", "\"seq\":7,\"future\":true");
        assert_eq!(serde_json::from_str::<Event>(&newer).unwrap(), e);
    }

    #[test]
    fn requests_carry_their_id() {
        let line = encode(
            3,
            &Request::Subscribe {
                after: 9,
                epoch: None,
            },
        );
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
        assert_eq!(
            env.body,
            Request::Subscribe {
                after: 9,
                epoch: None
            }
        );
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

    /// Each byte's value in [`ABC`], or 255 for one not in it.
    const VALUE: [u8; 256] = {
        let mut t = [255u8; 256];
        let mut i = 0;
        while i < 64 {
            t[ABC[i] as usize] = i as u8;
            i += 1;
        }
        t
    };

    pub fn decode(s: &str) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(s.len() / 4 * 3);
        let (mut acc, mut bits) = (0u32, 0);
        // Padding and line breaks (some encoders wrap at 76) carry nothing.
        for b in s.bytes().filter(|b| *b != b'=' && !b.is_ascii_whitespace()) {
            let v = match VALUE[b as usize] {
                255 => return None,
                v => u32::from(v),
            };
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
            assert_eq!(super::decode("Zm9v\nYmFy\r\n").unwrap(), b"foobar");
        }
    }
}
