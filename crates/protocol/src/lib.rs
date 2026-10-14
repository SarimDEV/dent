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
}

/// A slash command or skill the agent offers in a session's folder.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Command {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// What goes after the name, like `<file>`.
    #[serde(default)]
    pub hint: String,
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
    /// Take back a message that's still waiting.
    Unqueue {
        session: SessionId,
        id: String,
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
    Projects,
    Models,
    /// The account's usage windows, read from the provider.
    Usage,
    /// The agent's slash commands: the session's, or the last ones seen.
    Commands {
        #[serde(default)]
        session: Option<SessionId>,
    },
    /// What the session's directory has changed, against its last commit.
    Changes {
        session: SessionId,
    },
    /// A file in the session's directory, for the viewer.
    ReadFile {
        session: SessionId,
        path: String,
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
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Welcome {
        proto: u32,
        version: String,
        machine: String,
        home: String,
    },
    Ok,
    Created {
        session: SessionId,
    },
    Projects {
        projects: Vec<Project>,
    },
    Models {
        models: Vec<Model>,
    },
    Commands {
        commands: Vec<Command>,
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
    File {
        bytes_b64: String,
    },
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
