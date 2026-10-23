//! The Claude Code driver ([decision 0008]): how to start `claude -p`, how
//! its stream becomes dent's [`Event`]s, and how dent's requests and
//! answers are put to it. Everything Claude Code calls things lives here
//! and below; the rest of the daemon knows it only as a [`Driver`].
//!
//! [decision 0008]: ../../../docs/decisions/0008-claude-code-stream-json.md

pub mod check;
pub mod commands;
pub mod moving;
pub mod sheets;
pub mod terminal;
pub mod usage;
pub mod wire;

use dent_protocol::{Account, Answer, Caps, Model, Permission, Window, runs_subagent};
use serde_json::{Value, json};

use crate::agent::{
    Asked, Catalog, Driver, Event, Imported, Launch, OneOff, Probe, Reply, Req, Start,
};
use crate::env::AgentEnv;
use wire::Inbound;

/// The agent's process, with dent's flags. `env` strips variables a parent
/// Claude Code session would leak into it, and sets the login shell's PATH
/// so its tools behave as in a terminal.
pub fn argv(claude: &str, path: &str, l: &Launch<'_>) -> Vec<String> {
    let Launch {
        model,
        permission,
        effort,
        start,
        asks,
    } = *l;
    let mut v: Vec<String> = [
        "/usr/bin/env",
        "-u",
        "CLAUDECODE",
        "-u",
        "CLAUDE_CODE_ENTRYPOINT",
        "-u",
        "CLAUDE_CODE_SSE_PORT",
    ]
    .map(String::from)
    .into();
    v.push(format!("PATH={path}"));
    v.push(claude.into());
    v.extend(
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            // Lets the person switch to full access mid-session; it does
            // nothing unless they do.
            "--allow-dangerously-skip-permissions",
            // dent shows the plan and each command's purpose rather than
            // the commands themselves, so ask for both.
            "--append-system-prompt",
            PLAN_NUDGE,
        ]
        .map(String::from),
    );
    // dent's own tools, from this same binary, allowed up front: showing
    // the plan or a visual never needs asking, and a secret is asked for by
    // its own card.
    if let Some(exe) = std::env::current_exe()
        .ok()
        .and_then(|e| e.to_str().map(str::to_owned))
    {
        v.extend([
            "--mcp-config".into(),
            crate::plan_tool::config(&exe, asks),
            "--allowedTools".into(),
            [
                crate::plan_tool::TOOL,
                crate::plan_tool::VISUAL,
                crate::plan_tool::LOOK,
                crate::plan_tool::SECRET,
            ]
            .join(","),
        ]);
    }
    if let Some(mode) = mode(permission) {
        v.extend(["--permission-mode".into(), mode.into()]);
    }
    if let Some(m) = model {
        v.extend(["--model".into(), m.into()]);
    }
    if let Some(e) = effort {
        v.extend(["--effort".into(), e.into()]);
    }
    match start {
        Start::Fresh(id) => v.extend(["--session-id".into(), id.clone()]),
        Start::Resume(id) => v.extend(["--resume".into(), id.clone()]),
        // A rewind: the conversation as it was at that message, as a new
        // session so the old one stays whole.
        Start::ResumeAt { id, at } => v.extend([
            "--resume".into(),
            id.clone(),
            "--resume-session-at".into(),
            at.clone(),
            "--fork-session".into(),
        ]),
    }
    v
}

/// Added to every session's system prompt.
const PLAN_NUDGE: &str = "Before you start any task with three or more steps, call the update_plan tool (from the dent server) with the whole plan, one short line per step. Call it again whenever a step starts or finishes, marking it in_progress or completed, so the plan always shows where you are, and mark every step completed before your final answer. Give every Bash command a short description of what it is for, starting with a verb. When you make a file for the person to look at (a video, a recording, a PDF), name its path in your reply. To show a chart, a table, a diagram or a mock-up, use the show_visual tool rather than writing a file; it appears in the chat above your reply, so don't describe it again. Check a page with preview_visual first when its layout matters. If you need a secret (an API key, a token, a password), ask with the ask_secret tool, never in chat.";

/// One question to a small, quick model with no tools, answered in one
/// turn: titles, branch names, commit messages.
pub fn ask_argv(claude: &str, path: &str, prompt: &str) -> Vec<String> {
    let mut v: Vec<String> = [
        "/usr/bin/env",
        "-u",
        "CLAUDECODE",
        "-u",
        "CLAUDE_CODE_ENTRYPOINT",
        "-u",
        "CLAUDE_CODE_SSE_PORT",
    ]
    .map(String::from)
    .into();
    v.push(format!("PATH={path}"));
    v.push(claude.into());
    v.extend(
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "haiku",
            "--max-turns",
            "1",
            "--disallowedTools",
            "Bash,Write,Edit,MultiEdit,NotebookEdit,Read,Glob,Grep,WebFetch,WebSearch,Task,Agent",
        ]
        .map(String::from),
    );
    v.extend(["-p".into(), prompt.into()]);
    v
}

/// A one-off side question: the conversation `agent_session` so far, forked
/// so the original never sees it, answered without changing anything.
pub fn aside_argv(
    claude: &str,
    path: &str,
    model: Option<&str>,
    agent_session: &str,
    question: &str,
) -> Vec<String> {
    let mut v: Vec<String> = [
        "/usr/bin/env",
        "-u",
        "CLAUDECODE",
        "-u",
        "CLAUDE_CODE_ENTRYPOINT",
        "-u",
        "CLAUDE_CODE_SSE_PORT",
    ]
    .map(String::from)
    .into();
    v.push(format!("PATH={path}"));
    v.push(claude.into());
    v.extend(
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--resume",
            agent_session,
            "--fork-session",
            // Reading is fine; nothing that changes the project or runs.
            "--disallowedTools",
            "Bash,Write,Edit,MultiEdit,NotebookEdit",
            "--append-system-prompt",
            "This is a side question about the conversation so far. Answer it directly and briefly. Don't change any files.",
        ]
        .map(String::from),
    );
    if let Some(m) = model {
        v.extend(["--model".into(), m.into()]);
    }
    v.extend(["-p".into(), question.into()]);
    v
}

/// Claude Code's name for a permission mode. `None` is its default.
pub fn mode(p: Permission) -> Option<&'static str> {
    match p {
        Permission::Ask => None,
        Permission::Edits => Some("acceptEdits"),
        Permission::Plan => Some("plan"),
        Permission::Full => Some("bypassPermissions"),
    }
}

/// The mode name for `set_permission_mode`, which wants one for every mode.
pub fn mode_name(p: Permission) -> &'static str {
    mode(p).unwrap_or("default")
}

pub fn permission_from(mode: &str) -> Option<Permission> {
    Some(match mode {
        "default" | "manual" => Permission::Ask,
        "acceptEdits" => Permission::Edits,
        "plan" => Permission::Plan,
        "bypassPermissions" => Permission::Full,
        _ => return None,
    })
}

pub fn models(init: &Value) -> Vec<Model> {
    init["models"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    Some(Model {
                        id: m["value"].as_str()?.into(),
                        name: m["displayName"].as_str().unwrap_or_default().into(),
                        description: m["description"].as_str().unwrap_or_default().into(),
                        resolved: m["resolvedModel"].as_str().unwrap_or_default().into(),
                        efforts: m["supportedEffortLevels"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|e| e.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn commands(init: &Value) -> Vec<dent_protocol::Command> {
    init["commands"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(dent_protocol::Command {
                        name: c["name"].as_str()?.into(),
                        description: c["description"].as_str().unwrap_or_default().into(),
                        hint: c["argumentHint"].as_str().unwrap_or_default().into(),
                        args: Default::default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Models to offer before the agent has told us its list: only until the
/// first usage reading, which asks it.
pub fn default_models() -> Vec<Model> {
    [
        ("default", "Default", "The account's recommended model"),
        ("opus", "Opus", "For complex work"),
        ("sonnet", "Sonnet", "Efficient for routine tasks"),
        ("haiku", "Haiku", "Fastest for quick answers"),
    ]
    .map(|(id, name, description)| Model {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        resolved: String::new(),
        efforts: Vec::new(),
    })
    .into()
}

/// What a call being written out works on, once its input has streamed
/// that far: the file, or a subagent's task.
pub fn target(tool: &str, head: &str) -> Option<String> {
    let key = match tool {
        "Write" | "Edit" | "MultiEdit" | "Read" => "file_path",
        "NotebookEdit" => "notebook_path",
        t if runs_subagent(t) => "description",
        _ => return None,
    };
    // `"key": "value"`, the value complete.
    let at = head.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = head[at..].trim_start().strip_prefix(':')?.trim_start();
    let body = rest.strip_prefix('"')?;
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return serde_json::from_str(&rest[..i + 2]).ok(),
            _ => {}
        }
    }
    None
}

pub fn limits(info: &wire::RateLimitInfo) -> Vec<Window> {
    info.unified_windows
        .iter()
        .map(|(name, w)| Window {
            name: name.clone(),
            used: w.utilization,
            resets_at: w.resets_at,
        })
        .collect()
}

/// The reply to a question tool: the agent's own input, plus answers keyed
/// by each question's text.
pub fn answers(input: &Value, picked: &[String]) -> Value {
    let mut out = input.clone();
    let mut map = serde_json::Map::new();
    if let Some(qs) = input["questions"].as_array() {
        for (q, a) in qs.iter().zip(picked) {
            if let Some(text) = q["question"].as_str() {
                map.insert(text.into(), Value::String(a.clone()));
            }
        }
    }
    out["answers"] = Value::Object(map);
    out
}

/// Output styles from `initialize`, with what each does when known.
pub fn styles(init: &Value) -> (Vec<(String, String)>, Option<String>) {
    let list = init["available_output_styles"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| Some((s.as_str()?.to_string(), String::new())))
                .collect()
        })
        .unwrap_or_default();
    (list, init["output_style"].as_str().map(str::to_string))
}

/// The questions Claude Code may put to the person through the host
/// (`request_user_dialog`), which dent shows as it shows the agent's own
/// questions.
pub const DIALOGS: [&str; 3] = [
    "refusal_fallback_prompt",
    "fable_overage_consent_prompt",
    "auto_mode_server_fallback",
];

/// A dialog as an agent question: the question tool's input, and for each
/// option its label and the answer Claude Code wants back.
pub fn dialog(kind: &str, p: &Value) -> Option<(Value, Vec<(String, String)>)> {
    let name = |k: &str| {
        p[k].as_str()
            .map(model_name)
            .unwrap_or_else(|| "another model".into())
    };
    let (header, question, options): (&str, String, Vec<(String, &str, &str)>) = match kind {
        "refusal_fallback_prompt" => {
            let fallback = name("fallbackModel");
            let mut q = format!("{} declined to answer this.", name("originalModel"));
            if let Some(g) = p["guidanceText"].as_str().filter(|g| !g.is_empty()) {
                q = format!("{q} {g}");
            }
            (
                "Declined",
                q,
                vec![
                    (
                        format!("Try with {fallback}"),
                        "retry_fallback",
                        "Send the same message to the other model",
                    ),
                    (
                        "Edit my message".into(),
                        "edit_prompt",
                        "Change what you asked and send it again",
                    ),
                    ("Cancel".into(), "cancelled", ""),
                ],
            )
        }
        "fable_overage_consent_prompt" => {
            let model = p["modelName"].as_str().unwrap_or("This model");
            let q = if p["overagesEnabled"] == true {
                format!("{model} uses usage credits once your plan's limit is reached.")
            } else {
                format!("{model} needs usage credits, which aren't turned on for this account.")
            };
            (
                "Usage credits",
                q,
                vec![
                    (
                        format!("Use {model}"),
                        "consent",
                        "Draw on usage credits for it",
                    ),
                    (
                        "Use the default model".into(),
                        "switch_default",
                        "Carry on without credits",
                    ),
                    ("Cancel".into(), "cancelled", ""),
                ],
            )
        }
        "auto_mode_server_fallback" => {
            let mut q = p["title"]
                .as_str()
                .unwrap_or("Auto mode changed")
                .to_string();
            if let Some(paras) = p["paragraphs"].as_array() {
                for para in paras.iter().filter_map(Value::as_str) {
                    q = format!("{q}\n\n{para}");
                }
            }
            (
                "Auto mode",
                q,
                vec![
                    ("Continue".into(), "continue", ""),
                    ("Stop".into(), "interrupt", "End this turn"),
                ],
            )
        }
        _ => return None,
    };
    // The answers ride along, so a daemon that restarts mid-question
    // still knows them.
    let input = serde_json::json!({ "dent_dialog": options.iter().map(|(l, v, _)| [l.as_str(), *v]).collect::<Vec<_>>(), "questions": [{
        "question": question,
        "header": header,
        "multiSelect": false,
        "options": options.iter().map(|(label, _, about)| serde_json::json!({
            "label": label, "description": about,
        })).collect::<Vec<_>>(),
    }]});
    let values = options
        .into_iter()
        .map(|(label, value, _)| (label, value.to_string()))
        .collect();
    Some((input, values))
}

/// A model id as people say it: `claude-opus-5-5` is "Opus 5.5".
pub fn model_name(id: &str) -> String {
    let id = id.trim_start_matches("claude-");
    let mut parts = id.split('-');
    let Some(family) = parts.next() else {
        return id.to_string();
    };
    let version: Vec<&str> = parts
        .take_while(|p| p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit()))
        .collect();
    let mut name: String = family
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + &family[c.len_utf8()..])
        .unwrap_or_default();
    if !version.is_empty() {
        name = format!("{name} {}", version.join("."));
    }
    name
}

/// What Claude Code says when the conversation it was asked to resume has
/// no transcript (it was stopped before writing one, or the file is gone).
const FORGOTTEN: &str = "No conversation found with session ID";

/// Claude Code, driven through `claude -p` and stream-json.
pub struct Claude;

impl Driver for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn name(&self) -> &'static str {
        "Claude Code"
    }

    fn short(&self) -> &'static str {
        "Claude"
    }

    fn tested(&self) -> &'static str {
        "2.1.301"
    }

    fn own_caps(&self) -> Caps {
        Caps {
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

    fn install_url(&self) -> &'static str {
        "https://code.claude.com/docs/en/setup"
    }

    fn sign_in(&self) -> Option<String> {
        Some(format!(
            "{} auth login",
            crate::agent::shell_word(&self.command())
        ))
    }

    fn command(&self) -> String {
        std::env::var("DENT_CLAUDE").unwrap_or_else(|_| "claude".into())
    }

    fn conversation(&self, agent_session: &str) -> Option<Vec<(std::path::PathBuf, String)>> {
        moving::conversation(agent_session)
    }

    fn place_conversation(
        &self,
        dir: &std::path::Path,
        from: &str,
        to: &str,
        cwd: &str,
    ) -> anyhow::Result<()> {
        moving::place(dir, from, to, cwd)
    }

    fn env_names(&self) -> &'static [&'static str] {
        // Claude Code's sign-ins and where it sends requests:
        // `claude setup-token`'s token, an API key, a gateway, or Bedrock
        // and Vertex.
        &[
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "AWS_PROFILE",
            "AWS_REGION",
            "CLOUD_ML_REGION",
            "ANTHROPIC_VERTEX_PROJECT_ID",
        ]
    }

    fn status(&self, env: &AgentEnv) -> (Option<String>, Account) {
        let run = |args: &[&str]| {
            let mut argv = vec![
                "/usr/bin/env".to_string(),
                format!("PATH={}", env.path),
                self.command(),
            ];
            argv.extend(args.iter().map(|a| a.to_string()));
            crate::agent::run_briefly_with(&argv, &env.vars, 10)
        };
        let Some(version) = run(&["--version"])
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        else {
            return (None, Account::Unknown);
        };
        let account = run(&["auth", "status", "--json"])
            .and_then(|out| serde_json::from_str::<Value>(&out).ok())
            .map(|v| account(&v))
            .unwrap_or_default();
        (Some(version), account)
    }

    fn launch(&self, env: &AgentEnv, l: &Launch<'_>) -> Vec<String> {
        argv(&self.command(), &env.path, l)
    }

    fn parse(&self, line: &[u8]) -> serde_json::Result<Event> {
        wire::parse(line).map(event)
    }

    fn message(&self, text: &str, steer: bool, attachments: &[String]) -> Value {
        wire::user_message(text, &crate::daemon::uuid(), steer, attachments)
    }

    fn request(&self, id: &str, req: &Req) -> Option<Value> {
        let (subtype, extra) = match req {
            Req::Initialize => ("initialize", json!({ "supportedDialogKinds": DIALOGS })),
            Req::Interrupt => ("interrupt", json!({})),
            Req::ContextUsage => ("get_context_usage", json!({})),
            Req::Settings => ("get_settings", json!({})),
            Req::Sheet(command) => (sheets::source(command)?, json!({})),
            Req::Act(act) => sheets::request(act)?,
            Req::Model(m) => ("set_model", json!({ "model": m })),
            Req::Permission(p) => ("set_permission_mode", json!({ "mode": mode_name(*p) })),
            Req::StopTask(task) => ("stop_task", json!({ "task_id": task })),
        };
        Some(wire::control_request(id, subtype, extra))
    }

    fn reply(&self, req: &Req, ok: bool, body: &Value, models: &[Model]) -> Reply {
        if !ok {
            return Reply::Refused(body.as_str().map(str::to_string).unwrap_or_else(
                || match req {
                    Req::Act(_) => "Claude Code couldn't do that".into(),
                    _ => "unknown request".into(),
                },
            ));
        }
        match req {
            Req::Initialize => {
                let (styles, style) = styles(body);
                Reply::Initialized {
                    models: self::models(body),
                    commands: commands(body),
                    styles,
                    style,
                }
            }
            Req::ContextUsage => match body["totalTokens"].as_u64() {
                Some(used) => Reply::Context {
                    used,
                    max: body["maxTokens"].as_u64(),
                    compacts_at: body["autoCompactThreshold"]
                        .as_u64()
                        .filter(|_| body["isAutoCompactEnabled"] != false),
                    parts: wire::parts(&body["categories"]),
                },
                None => Reply::Done,
            },
            Req::Settings => Reply::Settings {
                effective: body["effective"].clone(),
                applied: body["applied"].clone(),
            },
            Req::Sheet(command) => Reply::Sheet(sheets::sheet(command, body, models)),
            Req::Act(_) => Reply::Acted {
                url: body["authUrl"]
                    .as_str()
                    .filter(|_| body["requiresUserAction"] == true)
                    .map(str::to_string),
            },
            Req::Interrupt | Req::Model(_) | Req::Permission(_) | Req::StopTask(_) => Reply::Done,
        }
    }

    fn answer(&self, id: &str, asked: &Asked, answer: &Answer) -> anyhow::Result<Value> {
        Ok(match (answer, &asked.dialog) {
            (Answer::Choose { answers }, Some(values)) => {
                let picked = answers.first().map(String::as_str).unwrap_or_default();
                let value = values
                    .iter()
                    .find(|(label, _)| label == picked)
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("cancelled");
                wire::dialog_answer(id, Some(value))
            }
            (Answer::Deny { .. }, Some(_)) => wire::dialog_answer(id, None),
            (Answer::Allow { always, project }, _) => wire::allow(
                id,
                &asked.tool,
                &asked.input,
                *always,
                *project,
                &asked.suggestions,
            ),
            (Answer::Deny { message }, _) => wire::deny(id, message),
            (Answer::Choose { answers }, _) => wire::allow(
                id,
                &asked.tool,
                &self::answers(&asked.input, answers),
                false,
                false,
                &Value::Null,
            ),
            (Answer::Expired, _) => anyhow::bail!("can't answer with expired"),
            (Answer::Secret { .. }, _) => anyhow::bail!("only a secret's card takes a secret"),
        })
    }

    fn refuse(&self, id: &str, what: &str) -> Value {
        wire::refuse(id, what)
    }

    fn target(&self, tool: &str, head: &str) -> Option<String> {
        target(tool, head)
    }

    fn forgot(&self, text: &str) -> bool {
        text.contains(FORGOTTEN)
    }

    fn has_sheet(&self, name: &str) -> bool {
        sheets::source(name).is_some()
    }

    fn ask(&self, env: &AgentEnv, prompt: &str) -> Vec<String> {
        ask_argv(&self.command(), &env.path, prompt)
    }

    fn aside(
        &self,
        env: &AgentEnv,
        model: Option<&str>,
        agent_session: &str,
        question: &str,
    ) -> Vec<String> {
        aside_argv(&self.command(), &env.path, model, agent_session, question)
    }

    fn one_off(&self, line: &[u8]) -> OneOff {
        match wire::parse(line) {
            Ok(Inbound::Init { session_id, .. }) => OneOff::Session(session_id),
            Ok(Inbound::Assistant {
                parent: None,
                blocks,
                synthetic: false,
                ..
            }) => {
                let text: Vec<String> = blocks
                    .into_iter()
                    .filter_map(|b| match b {
                        wire::AssistantBlock::Text(t) if !t.trim().is_empty() => Some(t),
                        _ => None,
                    })
                    .collect();
                if text.is_empty() {
                    OneOff::Other
                } else {
                    OneOff::Text(text.join("\n\n"))
                }
            }
            Ok(Inbound::Result {
                is_error: true,
                text,
                ..
            }) => OneOff::Failed(text.unwrap_or_else(|| "The side question failed".into())),
            _ => OneOff::Other,
        }
    }

    fn default_models(&self) -> Vec<Model> {
        default_models()
    }

    fn probe(&self, env: &AgentEnv) -> Probe {
        usage::probe(env, &self.command())
    }

    fn typed(
        &self,
        raw: &[dent_protocol::Command],
        catalog: &Catalog,
    ) -> Vec<dent_protocol::Command> {
        commands::typed(raw, catalog)
    }

    fn recent(
        &self,
        owned: &std::collections::HashSet<String>,
        limit: usize,
    ) -> Vec<dent_protocol::RecentSession> {
        terminal::recent(owned, limit)
    }

    fn projects(&self) -> Vec<(String, u64)> {
        terminal::projects()
    }

    fn is_open(&self, agent_session: &str) -> bool {
        terminal::is_open(agent_session)
    }

    fn import(&self, agent_session: &str, media: &std::path::Path) -> Option<Imported> {
        terminal::import(&terminal::find(agent_session)?, media)
    }
}

/// How `claude auth status --json` says it's signed in.
fn account(v: &Value) -> Account {
    if v["loggedIn"] != true {
        return Account::SignedOut;
    }
    let method = v["authMethod"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if method.contains("api") || method.contains("key") {
        return Account::ApiKey;
    }
    let s = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    Account::SignedIn {
        email: s("email"),
        plan: s("subscriptionType"),
    }
}

/// A line of the stream in dent's terms.
fn event(i: Inbound) -> Event {
    match i {
        Inbound::Init {
            session_id,
            version,
            permission_mode,
            mcp,
        } => Event::TurnStarted {
            agent_session: session_id,
            version,
            permission: permission_from(&permission_mode),
            // claude.ai's connectors you haven't set up want signing in:
            // they're yours to add, not trouble.
            mcp: mcp
                .into_iter()
                .filter(|(name, status)| {
                    !(status == "needs-auth" && name.starts_with("claude.ai "))
                })
                .collect(),
        },
        Inbound::BackgroundTasks(t) => Event::BackgroundTasks(t),
        Inbound::TaskStarted {
            task_id,
            tool_use_id,
        } => Event::TaskStarted {
            task: task_id,
            call: tool_use_id,
        },
        Inbound::MessageStart { parent } => Event::MessageStart { parent },
        Inbound::TextDelta { parent, text } => Event::TextDelta { parent, text },
        Inbound::ThinkingDelta { parent, text } => Event::ThinkingDelta { parent, text },
        Inbound::ToolStart { parent, name } => Event::ToolStart { parent, name },
        Inbound::InputDelta { parent, json } => Event::InputDelta { parent, json },
        Inbound::Assistant {
            parent,
            uuid,
            blocks,
            context,
            synthetic,
            results,
        } => Event::Assistant {
            parent,
            uuid,
            blocks,
            context,
            synthetic,
            results,
        },
        Inbound::ToolResults { parent, results } => Event::ToolResults { parent, results },
        Inbound::RateLimit(info) => Event::Limits(limits(&info)),
        Inbound::CanUseTool {
            request_id,
            tool_name,
            input,
            description,
            suggestions,
        } => Event::Approval {
            id: request_id,
            tool: tool_name,
            input,
            description,
            keeps: wire::keeps(&suggestions),
            suggestions,
        },
        Inbound::ControlResponse {
            request_id,
            ok,
            body,
        } => Event::Response {
            id: request_id,
            ok,
            body,
        },
        Inbound::Request {
            request_id,
            subtype,
            server,
        } => Event::Unanswerable {
            id: request_id,
            what: subtype,
            server,
        },
        Inbound::CancelRequest { request_id } => Event::Cancelled { id: request_id },
        Inbound::Compacted {
            auto,
            before,
            after,
        } => Event::Compacted {
            auto,
            before,
            after,
        },
        Inbound::Compacting { on, failed } => Event::Compacting { on, failed },
        Inbound::Retrying {
            attempt,
            max,
            delay_ms,
            error,
        } => Event::Retrying {
            attempt,
            max,
            delay_ms,
            error,
        },
        Inbound::CommandsChanged(v) => Event::CommandsChanged(commands(&v)),
        Inbound::LocalCommand {
            name,
            args,
            text,
            outcome,
            view,
        } => Event::LocalCommand {
            name,
            args,
            text,
            outcome,
            view,
        },
        Inbound::Notice(t) => Event::Notice(t),
        // A kind dent didn't declare is never answered: Claude Code takes
        // its default.
        Inbound::Dialog {
            request_id,
            kind,
            payload,
        } => match dialog(&kind, &payload) {
            Some((input, options)) => Event::Question {
                id: request_id,
                input,
                options,
            },
            None => Event::Other,
        },
        Inbound::Result {
            terminal_reason,
            is_error,
            text,
        } => Event::TurnEnded {
            aborted: matches!(
                terminal_reason.as_deref(),
                Some("aborted_streaming" | "aborted_tools")
            ),
            error: is_error,
            text,
        },
        Inbound::Other => Event::Other,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn auth_status_says_how_it_is_signed_in() {
        use dent_protocol::Account;
        let v = serde_json::json!({"loggedIn": true, "authMethod": "claude.ai", "email": "a@b.c", "subscriptionType": "max"});
        assert_eq!(
            super::account(&v),
            Account::SignedIn {
                email: Some("a@b.c".into()),
                plan: Some("max".into())
            }
        );
        let v = serde_json::json!({"loggedIn": true, "authMethod": "api_key"});
        assert_eq!(super::account(&v), Account::ApiKey);
        let v = serde_json::json!({"loggedIn": false});
        assert_eq!(super::account(&v), Account::SignedOut);
    }

    #[test]
    fn model_ids_read_as_names() {
        assert_eq!(super::model_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(super::model_name("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(super::model_name("opus"), "Opus");
    }

    #[test]
    fn a_call_names_its_target_once_streamed() {
        use super::target;
        assert_eq!(target("Write", r#"{"file_pa"#), None);
        assert_eq!(target("Write", r#"{"file_path": "/x/no"#), None);
        assert_eq!(
            target("Write", r#"{"file_path": "/x/a \"b\".md", "content": "hi"#).as_deref(),
            Some(r#"/x/a "b".md"#)
        );
        assert_eq!(
            target("Agent", r#"{"description":"Survey the paywall","prompt":"#).as_deref(),
            Some("Survey the paywall")
        );
        assert_eq!(target("Bash", r#"{"command":"ls"}"#), None);
    }
}
