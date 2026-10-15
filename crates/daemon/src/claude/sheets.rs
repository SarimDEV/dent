//! Commands whose answer is a screen ([decision 0046]). Claude Code's
//! `/status`, `/mcp`, `/skills`, `/memory`, `/hooks` and `/permissions` are
//! dialogs in its terminal app and refuse to run headless, but each has a
//! control request that returns the dialog's data. Each becomes the same
//! kind of thing, a [`View::Sheet`]: sections of rows that say how
//! something stands and what you can do about it.
//!
//! [decision 0046]: ../../../../docs/decisions/0046-commands-by-type.md

use dent_protocol::{Act, Action, Field, Mark, Model, Row, Section, View};
use serde_json::{Value, json};

/// The control request that answers a command with a sheet.
pub fn source(command: &str) -> Option<&'static str> {
    Some(match command {
        "status" => "get_status",
        "mcp" => "mcp_status",
        "skills" => "get_skills_dialog",
        "memory" => "get_memory_dialog",
        "hooks" => "get_hooks_listing",
        "permissions" => "list_permission_rules",
        _ => return None,
    })
}

/// The control request that does what a row offers; `None` for what the
/// client does itself.
pub fn request(act: &Act) -> Option<(&'static str, Value)> {
    Some(match act {
        Act::McpEnable { server, enabled } => (
            "mcp_toggle",
            json!({ "serverName": server, "enabled": enabled }),
        ),
        Act::McpReconnect { server } => ("mcp_reconnect", json!({ "serverName": server })),
        Act::McpSignIn { server } => ("mcp_authenticate", json!({ "serverName": server })),
        Act::Open { .. } => return None,
    })
}

/// A command's sheet, from its control request's answer. `models` names
/// the models the agent reports by id.
pub fn sheet(command: &str, body: &Value, models: &[Model]) -> Option<Vec<Section>> {
    let sections = match command {
        "status" => status(body, models),
        "mcp" => mcp(body),
        "skills" => skills(body),
        "memory" => memory(body),
        "hooks" => hooks(body),
        "permissions" => permissions(body),
        _ => return None,
    };
    (!sections.is_empty()).then_some(sections)
}

/// A sheet in plain words, for clients that don't draw one.
pub fn text(sections: &[Section]) -> String {
    let mut out = String::new();
    for s in sections {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&s.title);
        out.push('\n');
        if !s.note.is_empty() {
            out.push_str(&format!("  {}\n", s.note));
        }
        for r in &s.rows {
            match r.value.is_empty() {
                true => out.push_str(&format!("  {}\n", r.label)),
                false => out.push_str(&format!("  {}: {}\n", r.label, r.value)),
            }
        }
    }
    out
}

pub fn view(sections: Vec<Section>) -> View {
    View::Sheet { sections }
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_default()
}

fn list(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The first sentence of a description, short enough for a row.
fn gist(s: &str) -> String {
    let s = s.trim();
    let end = s
        .char_indices()
        .find(|&(i, c)| c == '.' && s[i + 1..].starts_with([' ', '\n']))
        .map_or(s.len(), |(i, _)| i + 1);
    let first = &s[..end];
    if first.chars().count() > 140 {
        let cut: String = first.chars().take(139).collect();
        format!("{}…", cut.trim_end())
    } else {
        first.to_string()
    }
}

/// `PreToolUse` as words: `Pre tool use`.
fn words_of(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// `User settings (~/.claude/settings.json)` reads as `User settings`.
fn short_source(s: &str) -> String {
    s.split(" (").next().unwrap_or(s).to_string()
}

/// Where a rule or a skill comes from, as people say it.
fn source_label(s: &str) -> String {
    match s {
        "userSettings" | "user" => "User settings".into(),
        "projectSettings" | "project" => "Project settings".into(),
        "localSettings" | "local" => "Your project settings".into(),
        "policySettings" | "managed" | "enterprise" => "Your organization".into(),
        "flagSettings" | "cliArg" => "Command line".into(),
        "session" | "command" => "This session".into(),
        other => short_source(&capital(other)),
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// `/status`: already sections of label and value. Models get their
/// names, paths start from `~`, and paths and ids are set as code.
fn status(body: &Value, models: &[Model]) -> Vec<Section> {
    let home = std::env::var("HOME").unwrap_or_default();
    list(&body["sections"])
        .iter()
        .map(|s| Section {
            title: str_of(s, "title").to_string(),
            rows: list(&s["rows"])
                .iter()
                .map(|r| {
                    let mut value = str_of(r, "value").to_string();
                    if let Some(m) = models.iter().find(|m| m.id == value || m.resolved == value) {
                        value = m.name.clone();
                    }
                    if !home.is_empty()
                        && let Some(rest) = value.strip_prefix(&home)
                    {
                        value = format!("~{rest}");
                    }
                    let literal = value.starts_with(['/', '~'])
                        || (value.len() == 36 && value.matches('-').count() == 4);
                    Row {
                        label: str_of(r, "label").to_string(),
                        value,
                        code: literal.then_some(Field::Value),
                        ..Default::default()
                    }
                })
                .collect(),
            ..Default::default()
        })
        .filter(|s| !s.rows.is_empty())
        .collect()
}

/// Where a server's set up, as a heading.
fn scope_title(scope: &str) -> String {
    match scope {
        "user" => "Yours".into(),
        "project" => "This project".into(),
        "local" => "This project, just you".into(),
        "claudeai" => "claude.ai connectors".into(),
        "plugin" => "From plugins".into(),
        "enterprise" | "managed" => "Set by your organization".into(),
        "dynamic" | "sdk" | "session" => "This session".into(),
        other => capital(other),
    }
}

/// `/mcp`: each server, how it's doing, and what can be done about it.
/// Its configuration stays out: a command's arguments can hold a key.
fn mcp(body: &Value) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    for s in list(&body["mcpServers"]) {
        let name = str_of(s, "name").to_string();
        let tools = list(&s["tools"]).len();
        let enable = |enabled: bool| Action {
            label: if enabled { "Turn on" } else { "Turn off" }.into(),
            act: Act::McpEnable {
                server: name.clone(),
                enabled,
            },
        };
        let reconnect = Action {
            label: "Reconnect".into(),
            act: Act::McpReconnect {
                server: name.clone(),
            },
        };
        let (value, mark, actions) = match str_of(s, "status") {
            "connected" => (
                match tools {
                    0 => "Connected".to_string(),
                    1 => "Connected · 1 tool".to_string(),
                    n => format!("Connected · {n} tools"),
                },
                Mark::Good,
                // A working server needs no reconnecting.
                vec![enable(false)],
            ),
            "pending" => ("Connecting…".into(), Mark::Waiting, vec![enable(false)]),
            "needs-auth" => (
                "Needs you to sign in".into(),
                Mark::Attention,
                vec![
                    Action {
                        label: "Sign in".into(),
                        act: Act::McpSignIn {
                            server: name.clone(),
                        },
                    },
                    enable(false),
                ],
            ),
            "failed" => (
                "Couldn't connect".into(),
                Mark::Bad,
                vec![reconnect, enable(false)],
            ),
            "disabled" => ("Off".into(), Mark::Off, vec![enable(true)]),
            other => (capital(other), Mark::Waiting, Vec::new()),
        };
        let scope = str_of(s, "scope");
        let label = match scope {
            "claudeai" => name.strip_prefix("claude.ai ").unwrap_or(&name).to_string(),
            _ => name.clone(),
        };
        let detail = match s["error"].as_str() {
            Some(e) if !e.is_empty() => e.to_string(),
            _ => gist(str_of(&s["serverInfo"], "description")),
        };
        let row = Row {
            label,
            value,
            detail,
            mark: Some(mark),
            actions,
            code: None,
        };
        let title = scope_title(scope);
        match sections.iter_mut().find(|x| x.title == title) {
            Some(x) => x.rows.push(row),
            None => sections.push(Section {
                title,
                rows: vec![row],
                ..Default::default()
            }),
        }
    }
    if sections.is_empty() {
        sections.push(Section {
            title: "MCP servers".into(),
            note: "None set up. In a terminal, claude mcp add adds one.".into(),
            rows: Vec::new(),
        });
    }
    sections
}

/// `/skills`: what Claude can load, grouped by where it's from.
fn skills(body: &Value) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    for s in list(&body["skills"]) {
        let on = str_of(s, "state") == "on";
        let tokens = s["tokens"].as_u64().unwrap_or(0);
        let mut detail = gist(str_of(s, "description"));
        if let Some(by) = s["locked_by"].as_str() {
            detail = format!("{detail} Managed by its {by}.").trim().to_string();
        }
        let row = Row {
            label: str_of(s, "display_name").to_string(),
            value: if on {
                format!("{tokens} tokens")
            } else {
                "Off".into()
            },
            detail,
            mark: Some(if on { Mark::Good } else { Mark::Off }),
            actions: Vec::new(),
            code: None,
        };
        let title = match str_of(s, "source") {
            "claude.ai sync" => "From claude.ai".to_string(),
            "plugin" => "From plugins".into(),
            "user" | "userSettings" => "Yours".into(),
            "project" | "projectSettings" => "This project".into(),
            "builtin" | "bundled" => "Built in".into(),
            other => capital(other),
        };
        match sections.iter_mut().find(|x| x.title == title) {
            Some(x) => x.rows.push(row),
            None => sections.push(Section {
                title,
                rows: vec![row],
                ..Default::default()
            }),
        }
    }
    if let Some(first) = sections.first_mut() {
        first.note =
            "Claude sees each one's name and description; the rest loads when used.".into();
    }
    if sections.is_empty() {
        sections.push(Section {
            title: "Skills".into(),
            note: "None installed.".into(),
            rows: Vec::new(),
        });
    }
    sections
}

fn open(path: &str, label: &str) -> Vec<Action> {
    if path.is_empty() {
        return Vec::new();
    }
    vec![Action {
        label: label.into(),
        act: Act::Open { path: path.into() },
    }]
}

/// `/memory`: the instruction files Claude reads, and what it remembers.
fn memory(body: &Value) -> Vec<Section> {
    let files: Vec<Row> = list(&body["files"])
        .iter()
        .map(|f| {
            let exists = f["exists"] != false;
            Row {
                label: str_of(f, "label").to_string(),
                value: if exists {
                    str_of(f, "description").to_string()
                } else {
                    "Not written yet".into()
                },
                detail: if exists {
                    String::new()
                } else {
                    str_of(f, "description").to_string()
                },
                mark: Some(if exists { Mark::Good } else { Mark::Off }),
                actions: if exists {
                    open(str_of(f, "path"), "Open")
                } else {
                    Vec::new()
                },
                code: None,
            }
        })
        .collect();
    let mut remembered: Vec<Row> = Vec::new();
    let auto = &body["auto_memory"];
    if auto.is_object() {
        let on = auto["enabled"] == true;
        let folder = list(&body["folders"])
            .iter()
            .find(|f| str_of(f, "kind") == "auto")
            .map(|f| str_of(f, "path"))
            .unwrap_or_default();
        remembered.push(Row {
            label: "Auto-memory".into(),
            value: if on { "On" } else { "Off" }.into(),
            detail: "Claude keeps notes about you and this project between conversations.".into(),
            mark: Some(if on { Mark::Good } else { Mark::Off }),
            actions: open(folder, "Open folder"),
            code: None,
        });
    }
    for m in list(&body["memories"]) {
        let path = str_of(m, "path");
        let label = [str_of(m, "label"), str_of(m, "name"), str_of(m, "title")]
            .into_iter()
            .find(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_string());
        remembered.push(Row {
            label,
            detail: gist(str_of(m, "description")),
            actions: open(path, "Open"),
            ..Default::default()
        });
    }
    let dream = &body["auto_dream"];
    if dream["shown"] == true {
        remembered.push(Row {
            label: "Dreaming".into(),
            value: capital(str_of(dream, "status")),
            detail: str_of(dream, "detail").to_string(),
            mark: Some(if dream["enabled"] == true {
                Mark::Good
            } else {
                Mark::Off
            }),
            ..Default::default()
        });
    }
    [("Instructions", files), ("Remembered", remembered)]
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(title, rows)| Section {
            title: title.into(),
            rows,
            ..Default::default()
        })
        .collect()
}

/// `/hooks`: what runs when, by event, in the order things happen.
fn hooks(body: &Value) -> Vec<Section> {
    let all = list(&body["hooks"]);
    let policy = &body["policy"];
    let off = policy["allDisabled"] == true || policy["disabledByPolicy"] == true;
    let mut sections: Vec<Section> = list(&body["events"])
        .iter()
        .filter_map(|e| {
            let name = str_of(e, "name");
            let rows: Vec<Row> = all
                .iter()
                .filter(|h| str_of(h, "event") == name)
                .map(|h| {
                    let matcher = str_of(h, "matcher");
                    let label = match matcher {
                        "" | "*" | ".*" if e["supportsMatcher"] == true => "Every time".into(),
                        "" | "*" | ".*" => "Always".into(),
                        m => m.to_string(),
                    };
                    Row {
                        label,
                        value: short_source(str_of(h, "sourceLabel")),
                        detail: [str_of(h, "displayText"), str_of(h, "commandText")]
                            .into_iter()
                            .find(|s| !s.is_empty())
                            .unwrap_or_default()
                            .to_string(),
                        mark: off.then_some(Mark::Off),
                        actions: Vec::new(),
                        code: Some(Field::Detail),
                    }
                })
                .collect();
            // The event's name, in words (`PreToolUse` reads "Pre tool
            // use"), with what it means beneath.
            (!rows.is_empty()).then(|| Section {
                title: words_of(name),
                note: str_of(e, "summary").to_string(),
                rows,
            })
        })
        .collect();
    if sections.is_empty() {
        sections.push(Section {
            title: "Hooks".into(),
            note: "None set up.".into(),
            rows: Vec::new(),
        });
    }
    if off && let Some(first) = sections.first_mut() {
        first.note = if policy["disabledByPolicy"] == true {
            "Your organization has turned hooks off.".into()
        } else {
            "Hooks are turned off.".into()
        };
    }
    sections
}

/// `/permissions`: what Claude may do without asking, must ask about, and
/// may never do.
fn permissions(body: &Value) -> Vec<Section> {
    let state = &body["state"];
    let rules = list(&state["rules"]);
    let mut sections: Vec<Section> = [
        ("allow", "Allowed"),
        ("ask", "Asks first"),
        ("deny", "Never allowed"),
    ]
    .into_iter()
    .filter_map(|(behavior, title)| {
        let rows: Vec<Row> = rules
            .iter()
            .filter(|r| str_of(r, "behavior") == behavior)
            .map(|r| {
                let off =
                    r["notInEffect"].as_bool().unwrap_or(false) || r["notInEffect"].is_string();
                Row {
                    label: str_of(r, "rule").to_string(),
                    value: if off {
                        "Not in effect".into()
                    } else {
                        source_label(str_of(r, "source"))
                    },
                    detail: str_of(r, "description").to_string(),
                    mark: off.then_some(Mark::Off),
                    actions: Vec::new(),
                    code: Some(Field::Label),
                }
            })
            .collect();
        (!rows.is_empty()).then(|| Section {
            title: title.into(),
            rows,
            ..Default::default()
        })
    })
    .collect();
    if sections.is_empty() {
        sections.push(Section {
            title: "Rules".into(),
            note: "None yet. Claude asks before anything your permission mode doesn't allow."
                .into(),
            rows: Vec::new(),
        });
    }
    let dirs: Vec<Row> = list(&state["workspaceDirectories"])
        .iter()
        .filter_map(|d| d.as_str().or_else(|| d["path"].as_str()))
        .map(|d| Row {
            label: d.to_string(),
            code: Some(Field::Label),
            ..Default::default()
        })
        .collect();
    if !dirs.is_empty() {
        sections.push(Section {
            title: "Other folders Claude can use".into(),
            rows: dirs,
            ..Default::default()
        });
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_says_how_each_server_stands_and_keeps_config_out() {
        let body = json!({ "mcpServers": [
            { "name": "docs", "status": "connected", "scope": "user",
              "config": { "type": "stdio", "command": "npx", "args": ["--api-key", "secret"] },
              "serverInfo": { "description": "Up-to-date docs. And more." },
              "tools": [{ "name": "a" }, { "name": "b" }] },
            { "name": "claude.ai Tickets", "status": "needs-auth", "scope": "claudeai" },
            { "name": "old", "status": "disabled", "scope": "user" },
            { "name": "broken", "status": "failed", "scope": "project", "error": "spawn ENOENT" },
        ]});
        let s = sheet("mcp", &body, &[]).unwrap();
        assert_eq!(
            s.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(),
            ["Yours", "claude.ai connectors", "This project"]
        );
        let docs = &s[0].rows[0];
        assert_eq!(docs.value, "Connected · 2 tools");
        assert_eq!(docs.detail, "Up-to-date docs.");
        assert_eq!(docs.mark, Some(Mark::Good));
        assert!(!serde_json::to_string(&s).unwrap().contains("secret"));
        let tickets_row = &s[1].rows[0];
        assert_eq!(tickets_row.label, "Tickets");
        assert_eq!(tickets_row.mark, Some(Mark::Attention));
        assert_eq!(
            tickets_row.actions[0].act,
            Act::McpSignIn {
                server: "claude.ai Tickets".into()
            }
        );
        assert_eq!(s[0].rows[1].actions[0].label, "Turn on");
        assert_eq!(s[2].rows[0].detail, "spawn ENOENT");
        assert_eq!(
            request(&tickets_row.actions[0].act).unwrap(),
            (
                "mcp_authenticate",
                json!({ "serverName": "claude.ai Tickets" })
            )
        );
    }

    #[test]
    fn every_sheet_reads_its_dialog() {
        let status = json!({ "sections": [{ "title": "Session", "rows": [
            { "label": "Version", "value": "2.1.300" }] }] });
        assert_eq!(
            sheet("status", &status, &[]).unwrap()[0].rows[0].value,
            "2.1.300"
        );

        let memory = json!({
            "files": [
                { "kind": "user", "path": "/u/CLAUDE.md", "label": "User instructions",
                  "description": "Saved in ~/.claude/CLAUDE.md", "exists": true },
                { "kind": "project", "path": "/p/CLAUDE.md", "label": "Project instructions",
                  "description": "Saved in ./CLAUDE.md", "exists": false }],
            "folders": [{ "kind": "auto", "path": "/m/", "label": "Open auto-memory folder" }],
            "memories": [],
            "auto_memory": { "enabled": true },
            "auto_dream": { "shown": false } });
        let m = sheet("memory", &memory, &[]).unwrap();
        assert_eq!(
            m[0].rows[0].actions[0].act,
            Act::Open {
                path: "/u/CLAUDE.md".into()
            }
        );
        assert!(m[0].rows[1].actions.is_empty());
        assert_eq!(m[0].rows[1].value, "Not written yet");
        assert_eq!(m[1].rows[0].actions[0].label, "Open folder");

        let hooks = json!({
            "events": [{ "name": "PreToolUse", "summary": "Before tool execution", "supportsMatcher": true },
                       { "name": "Stop", "summary": "Right before Claude concludes", "supportsMatcher": false }],
            "hooks": [{ "event": "Stop", "matcher": "", "sourceLabel": "User settings (~/.claude/settings.json)",
                        "displayText": "say done" }],
            "policy": { "allDisabled": false } });
        let h = sheet("hooks", &hooks, &[]).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].title, "Stop");
        assert_eq!(words_of("PreToolUse"), "Pre tool use");
        assert_eq!(h[0].rows[0].label, "Always");
        assert_eq!(h[0].rows[0].value, "User settings");
        assert_eq!(h[0].rows[0].detail, "say done");

        let perms = json!({ "state": { "rules": [
            { "behavior": "deny", "rule": "Bash(rm -rf:*)", "source": "userSettings" },
            { "behavior": "allow", "rule": "Read", "source": "session" }],
            "workspaceDirectories": [] } });
        let p = sheet("permissions", &perms, &[]).unwrap();
        assert_eq!(p[0].title, "Allowed");
        assert_eq!(p[0].rows[0].value, "This session");
        assert_eq!(p[1].rows[0].value, "User settings");
        assert_eq!(p[1].title, "Never allowed");
        let empty = sheet("permissions", &json!({ "state": { "rules": [] } }), &[]).unwrap();
        assert!(empty[0].note.starts_with("None yet"));

        let skills = json!({ "skills": [
            { "display_name": "dataviz", "description": "Charts. More.", "source": "plugin",
              "tokens": 120, "state": "on", "locked_by": "plugin" }] });
        let k = sheet("skills", &skills, &[]).unwrap();
        assert_eq!(k[0].rows[0].value, "120 tokens");
        assert_eq!(k[0].rows[0].detail, "Charts. Managed by its plugin.");

        assert!(text(&k).contains("dataviz: 120 tokens"));
    }
}
