//! The agent layer ([decision 0055]). Every agent dent drives sits behind
//! [`Driver`]: how to start its process, what each line it writes means
//! (an [`Event`]), and how to say what a session wants in its words.
//! Sessions, the daemon and clients only ever see dent's own types, so a
//! new agent is one new module and one line in [`DRIVERS`].
//!
//! Agents are told apart by id ([`Body::Created`]'s `agent`), which every
//! session keeps for good; a session made before ids were kept runs the
//! default agent.
//!
//! [decision 0055]: ../../../../docs/decisions/0055-agent-layer.md
//! [`Body::Created`]: dent_protocol::Body::Created

pub mod log;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use dent_protocol::{
    Account, Act, AgentInfo, Answer, Body, Caps, Command, CommandOutcome, Model, Part, Permission,
    RecentSession, Section, View, Window,
};
use serde::Deserialize;
use serde_json::Value;

use crate::env::AgentEnv;

/// Every agent this daemon can drive. The first is the default.
pub static DRIVERS: &[&dyn Driver] = &[&crate::claude::Claude];

/// The driver for agent `id`. An id this daemon doesn't know (a session
/// from a newer one) is an error, never another agent.
pub fn driver(id: &str) -> Option<&'static dyn Driver> {
    DRIVERS.iter().copied().find(|d| d.id() == id)
}

/// The agent a new session runs when none is asked for.
pub fn default() -> &'static dyn Driver {
    DRIVERS[0]
}

/// The driver for `id`, or the default when none is named.
pub fn named(id: Option<&str>) -> anyhow::Result<&'static dyn Driver> {
    match id.filter(|i| !i.is_empty()) {
        None => Ok(default()),
        Some(i) => driver(i).ok_or_else(|| anyhow::anyhow!("this machine has no agent called {i}")),
    }
}

/// What one agent can do and be asked. Implementations hold no state of
/// their own: what a session knows lives in the session, so a restarted
/// daemon rebuilds it from the log whichever agent it is.
pub trait Driver: Send + Sync {
    /// Stable id, kept with every session: `claude`.
    fn id(&self) -> &'static str;
    /// Full name, for sentences about the program: "Claude Code".
    fn name(&self) -> &'static str;
    /// Short name, for sentences about the model: "Claude".
    fn short(&self) -> &'static str;
    /// The CLI version this dent was tested against.
    fn tested(&self) -> &'static str;
    /// What it can do. Every use goes through [`Driver::caps`].
    fn own_caps(&self) -> Caps;
    /// What it can do, less what `DENT_CAPS_OFF` turns off: how tests and
    /// walkthroughs see dent with an agent that can do less, without a
    /// second agent ([`masked`]).
    fn caps(&self) -> Caps {
        masked(
            self.own_caps(),
            std::env::var("DENT_CAPS_OFF").ok().as_deref().unwrap_or(""),
        )
    }
    /// Where to read how to install it.
    fn install_url(&self) -> &'static str;
    /// What to run in a terminal to sign in, when it signs in that way.
    fn sign_in(&self) -> Option<String> {
        None
    }

    /// The command that runs it: an override from the environment
    /// (`DENT_CLAUDE`), or its usual name on the PATH.
    fn command(&self) -> String;
    /// What it needs from the login shell to be signed in on a machine
    /// with no keychain (a service; see `crate::env`): its credential
    /// variables.
    fn env_names(&self) -> &'static [&'static str] {
        &[]
    }
    /// Its version and account, from its CLI. Blocking, and bounded: a CLI
    /// that doesn't answer in a few seconds counts as not there.
    fn status(&self, env: &AgentEnv) -> (Option<String>, Account);

    // ---- a session's process ----

    /// The process for a session, from `/usr/bin/env` on.
    fn launch(&self, env: &AgentEnv, l: &Launch<'_>) -> Vec<String>;
    /// One line of the agent's output, without its line break.
    fn parse(&self, line: &[u8]) -> serde_json::Result<Event>;
    /// A message from the person; `steer` sends it into the running turn.
    fn message(&self, text: &str, steer: bool, attachments: &[String]) -> Value;
    /// A request in the agent's words, under id `id`; `None` when this
    /// agent has no such thing.
    fn request(&self, id: &str, req: &Req) -> Option<Value>;
    /// What the answer to `req` says.
    fn reply(&self, req: &Req, ok: bool, body: &Value, models: &[Model]) -> Reply;
    /// The person's answer to what the agent asked.
    fn answer(&self, id: &str, asked: &Asked, answer: &Answer) -> anyhow::Result<Value>;
    /// Turn down something it asked that dent can't show, so it stops
    /// waiting.
    fn refuse(&self, id: &str, what: &str) -> Value;
    /// What a call whose input is still streaming works on, once its input
    /// has streamed that far: a file, a subagent's task.
    fn target(&self, tool: &str, head: &str) -> Option<String>;
    /// `text` (an error, or the end of stderr) says the conversation it
    /// was asked to resume is gone.
    fn forgot(&self, text: &str) -> bool;
    /// Whether command `name`'s answer is a sheet, read with a request.
    fn has_sheet(&self, name: &str) -> bool;

    // ---- one-off runs ----

    /// One question to a small, quick model with no tools: titles, branch
    /// names, commit messages.
    fn ask(&self, env: &AgentEnv, prompt: &str) -> Vec<String>;
    /// A side question about a conversation, answered from a copy of it.
    fn aside(
        &self,
        env: &AgentEnv,
        model: Option<&str>,
        agent_session: &str,
        question: &str,
    ) -> Vec<String>;
    /// A line of a one-off run's output.
    fn one_off(&self, line: &[u8]) -> OneOff;

    // ---- what the account and the CLI offer ----

    /// Models to offer before it has said which it has.
    fn default_models(&self) -> Vec<Model>;
    /// Usage, models and commands, read through the CLI with no model
    /// call. Blocking.
    fn probe(&self, env: &AgentEnv) -> Probe;
    /// Commands with what each takes, from what it knows.
    fn typed(&self, raw: &[Command], catalog: &Catalog) -> Vec<Command>;

    // ---- sessions started in its own terminal app ----

    /// Folders it has worked in from its own terminal app, each with when
    /// it last did (Unix milliseconds; 0 if not known).
    fn projects(&self) -> Vec<(String, u64)> {
        Vec::new()
    }

    fn recent(&self, _owned: &HashSet<String>, _limit: usize) -> Vec<RecentSession> {
        Vec::new()
    }
    /// It's still running in a terminal: picking it up would run it twice.
    fn is_open(&self, _agent_session: &str) -> bool {
        false
    }
    /// A terminal session's history as dent events. Pictures it holds are
    /// saved under `media`.
    fn import(&self, _agent_session: &str, _media: &Path) -> Option<Imported> {
        None
    }

    /// The files a conversation lives in, for moving it to another machine
    /// (decision 0064): each, and its name in the move, the transcript
    /// first as `transcript.jsonl`. `None` when this agent's conversations
    /// can't move; none at all when its conversation can't be found (it
    /// moves without, and starts there from what was said, as text).
    fn conversation(&self, _agent_session: &str) -> Option<Vec<(PathBuf, String)>> {
        None
    }

    /// Put a conversation moved here in place, as `to` (its new id), for
    /// work in `cwd`, from the move's files in `dir` (named as
    /// [`Driver::conversation`] named them), so it resumes from `cwd`.
    fn place_conversation(&self, _dir: &Path, _from: &str, _to: &str, _cwd: &str) -> Result<()> {
        anyhow::bail!(
            "{}'s conversations can't move between machines yet",
            self.name()
        )
    }
}

/// How a session's agent process starts.
pub struct Launch<'a> {
    pub model: Option<&'a str>,
    pub permission: Permission,
    pub effort: Option<&'a str>,
    pub start: &'a Start,
    /// Where dent's own tools wait for a secret's answer.
    pub asks: &'a Path,
}

/// How an agent process picks up its conversation.
pub enum Start {
    /// A new conversation, under an id dent chose.
    Fresh(String),
    Resume(String),
    /// Resume up to the assistant message `at`, as a new conversation so
    /// the old one stays whole.
    ResumeAt {
        id: String,
        at: String,
    },
}

/// What a session asks its agent, besides messages.
#[derive(Clone, Debug, PartialEq)]
pub enum Req {
    /// The first thing a new process is asked: models, commands, styles.
    Initialize,
    Interrupt,
    /// How full the context is, by the agent's own count.
    ContextUsage,
    /// What's in effect, to say which choice is current.
    Settings,
    /// Command `name`'s sheet.
    Sheet(String),
    /// Do what a sheet's row offers.
    Act(Act),
    Model(String),
    Permission(Permission),
    StopTask(String),
}

/// What the answer to a [`Req`] says.
pub enum Reply {
    Initialized {
        models: Vec<Model>,
        commands: Vec<Command>,
        styles: Vec<(String, String)>,
        style: Option<String>,
    },
    Context {
        used: u64,
        max: Option<u64>,
        compacts_at: Option<u64>,
        parts: Vec<Part>,
    },
    Settings {
        effective: Value,
        applied: Value,
    },
    /// A sheet's rows; `None` when the answer couldn't be read as one.
    Sheet(Option<Vec<Section>>),
    /// Done; `url` is a page the person finishes it on (a sign-in).
    Acted {
        url: Option<String>,
    },
    Done,
    /// Turned down, in the agent's words.
    Refused(String),
}

/// What the agent asked the person and is waiting on.
#[derive(Clone, Debug)]
pub struct Asked {
    pub tool: String,
    pub input: Value,
    /// What the agent offered to keep allowing, as it sent it; `null` when
    /// not known (a question from an older log).
    pub suggestions: Value,
    /// A question from the agent program itself rather than the model:
    /// each option's label and the answer it gives.
    pub dialog: Option<Vec<(String, String)>>,
}

/// A question's labels and answers, from the input it was logged with
/// (they ride along, so a daemon that restarts mid-question still knows
/// them).
pub fn dialog_values(input: &Value) -> Option<Vec<(String, String)>> {
    input["dent_dialog"].as_array().map(|a| {
        a.iter()
            .filter_map(|p| Some((p[0].as_str()?.to_string(), p[1].as_str()?.to_string())))
            .collect()
    })
}

/// One line of an agent's output, in dent's terms.
#[derive(Debug)]
pub enum Event {
    /// A turn starts, including one the agent starts itself after its
    /// background work finishes.
    TurnStarted {
        agent_session: String,
        version: String,
        permission: Option<Permission>,
        /// Each MCP server and how it stands: connected, failed,
        /// needs-auth, pending, disabled.
        mcp: Vec<(String, String)>,
    },
    /// The full current set of background work.
    BackgroundTasks(Vec<BackgroundTask>),
    /// Background work and the tool call that started it.
    TaskStarted {
        task: String,
        call: Option<String>,
    },
    /// A new assistant message is starting to stream.
    MessageStart {
        parent: Option<String>,
    },
    TextDelta {
        parent: Option<String>,
        text: String,
    },
    ThinkingDelta {
        parent: Option<String>,
        text: String,
    },
    /// A tool call starts streaming its input.
    ToolStart {
        parent: Option<String>,
        name: String,
    },
    /// More of that call's input, as raw JSON text.
    InputDelta {
        parent: Option<String>,
        json: String,
    },
    /// A complete assistant message.
    Assistant {
        parent: Option<String>,
        /// The agent's id for it, to rewind the conversation to.
        uuid: Option<String>,
        blocks: Vec<AssistantBlock>,
        /// How full the context is.
        context: Option<u64>,
        /// Written by the agent program, not the model: a usage limit or
        /// an API error, which the turn's end also reports.
        synthetic: bool,
        /// Results of tools that ran on the model's side, which come with
        /// their calls.
        results: Vec<ToolResult>,
    },
    ToolResults {
        parent: Option<String>,
        results: Vec<ToolResult>,
    },
    /// Usage windows as the agent last saw them.
    Limits(Vec<Window>),
    /// A tool call waiting for the person's approval.
    Approval {
        id: String,
        tool: String,
        input: Value,
        description: Option<String>,
        /// It offered a rule it could save for this kind of call.
        keeps: bool,
        /// What it offered, handed back with an "always" answer.
        suggestions: Value,
    },
    /// A question from the agent program itself, shown as the agent's own
    /// questions are: the question tool's input, and each option's label
    /// with the answer it gives.
    Question {
        id: String,
        input: Value,
        options: Vec<(String, String)>,
    },
    /// The answer to a [`Req`] sent under `id`.
    Response {
        id: String,
        ok: bool,
        body: Value,
    },
    /// Something it waits on that dent can't show: refused with
    /// [`Driver::refuse`].
    Unanswerable {
        id: String,
        what: String,
        /// The MCP server asking, when one is.
        server: Option<String>,
    },
    /// It no longer needs an answer to `id`.
    Cancelled {
        id: String,
    },
    Compacted {
        auto: bool,
        before: u64,
        after: Option<u64>,
    },
    Compacting {
        on: bool,
        failed: Option<String>,
    },
    Retrying {
        attempt: u32,
        max: u32,
        delay_ms: u64,
        error: String,
    },
    /// The whole new list of commands.
    CommandsChanged(Vec<Command>),
    /// A command the agent ran itself, with no model turn.
    LocalCommand {
        name: Option<String>,
        args: String,
        text: String,
        outcome: CommandOutcome,
        view: Option<View>,
    },
    /// Something to say in the chat: a warning, a model falling back.
    Notice(String),
    /// The turn is over.
    TurnEnded {
        /// Stopped by an interrupt (or a steer cutting it short).
        aborted: bool,
        error: bool,
        /// The reply, or on error what went wrong.
        text: Option<String>,
    },
    Other,
}

/// A line of a one-off run's output.
pub enum OneOff {
    /// The conversation it ran in, which a side question carries on from.
    Session(String),
    /// Some of the answer.
    Text(String),
    Failed(String),
    Other,
}

/// What the CLI said when asked with no model call; each part empty when
/// it couldn't say.
#[derive(Default)]
pub struct Probe {
    /// `None` when it can't say: signed out, an API key, or it failed.
    pub windows: Option<Vec<Window>>,
    pub models: Vec<Model>,
    /// The commands any session starts with.
    pub commands: Vec<Command>,
    pub catalog: Catalog,
}

/// What the agent knows that fills a command's arguments in.
#[derive(Default, Clone, Debug)]
pub struct Catalog {
    pub models: Vec<Model>,
    /// Output styles, with what each does.
    pub styles: Vec<(String, String)>,
    pub style: Option<String>,
    /// What its settings command takes, from its own usage text.
    pub settings: Vec<dent_protocol::Setting>,
    /// What's in effect.
    pub effective: Value,
    pub applied: Value,
    pub servers: Vec<String>,
}

/// A terminal session's history, ready to become a dent session.
pub struct Imported {
    pub cwd: String,
    pub title: String,
    pub bodies: Vec<Body>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackgroundTask {
    pub task_id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub task_type: String,
}

#[derive(Debug)]
pub enum AssistantBlock {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub is_error: bool,
    pub output: String,
    /// Pictures in the result, as their `source` objects.
    pub images: Vec<Value>,
}

/// `word` as one word for a shell: as it is when that's safe, else in
/// single quotes.
pub fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+:@%=".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// `caps` less the ones named in `off`, a comma-separated list of field
/// names (`steer,rewind`) and permission modes (`mode:full`). A name that
/// isn't one is passed over.
pub fn masked(caps: Caps, off: &str) -> Caps {
    let Ok(Value::Object(mut v)) = serde_json::to_value(&caps) else {
        return caps;
    };
    for name in off.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        if let Some(mode) = name.strip_prefix("mode:") {
            if let Some(Value::Array(modes)) = v.get_mut("permissions") {
                modes.retain(|m| m.as_str() != Some(mode));
            }
        } else if let Some(b @ Value::Bool(_)) = v.get_mut(name) {
            *b = Value::Bool(false);
        }
    }
    serde_json::from_value(Value::Object(v)).unwrap_or(caps)
}

/// What agent `d` is, without asking its CLI how it stands.
pub fn brief(d: &dyn Driver) -> AgentInfo {
    AgentInfo {
        id: d.id().into(),
        name: d.name().into(),
        short: d.short().into(),
        installed: false,
        command: d.command(),
        version: None,
        tested: d.tested().into(),
        account: Account::Unknown,
        caps: d.caps(),
        install_url: d.install_url().into(),
        sign_in: d.sign_in(),
    }
}

/// How agent `d` stands on this machine, asked of its CLI. Blocking.
pub fn info(d: &dyn Driver, env: &AgentEnv) -> AgentInfo {
    let (version, account) = d.status(env);
    AgentInfo {
        installed: version.is_some(),
        version,
        account,
        ..brief(d)
    }
}

/// Run `argv` with stdin closed and give its stdout, or `None` if it
/// failed or took longer than `secs`.
pub fn run_briefly(argv: &[String], secs: u64) -> Option<String> {
    run_briefly_with(argv, &[], secs)
}

/// As [`run_briefly`], with `env` added to its environment.
pub fn run_briefly_with(argv: &[String], env: &[(String, String)], secs: u64) -> Option<String> {
    use std::io::Read;
    let (bin, args) = argv.split_first()?;
    let mut child = std::process::Command::new(bin)
        .args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(std::env::temp_dir())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        let _ = tx.send(s);
    });
    let got = rx.recv_timeout(std::time::Duration::from_secs(secs)).ok();
    let _ = child.kill();
    let ok = child.wait().is_ok_and(|s| s.success());
    got.filter(|_| ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_has_its_own_id_and_the_first_is_the_default() {
        let ids: Vec<&str> = DRIVERS.iter().map(|d| d.id()).collect();
        let unique: HashSet<&&str> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "{ids:?}");
        assert_eq!(default().id(), dent_protocol::DEFAULT_AGENT);
        assert!(named(None).is_ok());
        assert!(named(Some("")).is_ok());
        assert!(named(Some("no-such-agent")).is_err());
    }

    /// Nothing above a driver knows which agent it talks to: outside an
    /// agent's own module, only the list of drivers names one.
    #[test]
    fn only_drivers_name_their_agent() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut bad = Vec::new();
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                // Each driver's own folder is its business.
                if p.is_dir() {
                    if !DRIVERS.iter().any(|d| p == src.join(d.id())) {
                        stack.push(p);
                    }
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                for (n, line) in text.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or_default();
                    if code.contains("crate::claude") && !code.contains("&crate::claude::Claude") {
                        bad.push(format!("{}:{}", p.display(), n + 1));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "reaches into a driver: {bad:?}");
    }

    #[test]
    fn a_shell_word_survives_spaces_and_quotes() {
        assert_eq!(shell_word("claude"), "claude");
        assert_eq!(shell_word("/a b/claude"), "'/a b/claude'");
        assert_eq!(shell_word("it's"), "'it'\\''s'");
    }

    #[test]
    fn caps_can_be_turned_off_for_tests() {
        let all = Caps::everything();
        assert_eq!(masked(all.clone(), ""), all);
        let less = masked(all.clone(), "steer, rewind,mode:full,no_such_thing");
        assert!(!less.steer && !less.rewind && less.fork);
        assert_eq!(
            less.permissions,
            vec![Permission::Ask, Permission::Edits, Permission::Plan]
        );
    }

    #[test]
    fn a_brief_run_gives_its_output_or_nothing() {
        let echo = ["/bin/echo".to_string(), "2.1.301".into()];
        assert_eq!(run_briefly(&echo, 5).as_deref(), Some("2.1.301\n"));
        assert_eq!(run_briefly(&["/usr/bin/false".into()], 5), None);
        assert_eq!(run_briefly(&["/no/such/thing".into()], 5), None);
        let slow = ["/bin/sleep".to_string(), "5".into()];
        let t = std::time::Instant::now();
        assert_eq!(run_briefly(&slow, 1), None);
        assert!(t.elapsed() < std::time::Duration::from_secs(4));
    }
}
