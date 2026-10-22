//! dent's own tools for the agent: a tiny MCP server, `dent-daemon
//! plan-tool`, that every Claude Code session gets. JSON-RPC over stdin and
//! stdout, a message a line.
//!
//! - `update_plan` takes the whole plan each time (`items`: `content` and
//!   `status`) and only says thanks; dent reads the plan from the call
//!   itself, as it streams by, and shows it above the composer.
//! - `show_visual` takes a page (`title`, `html`) and only says it showed;
//!   dent reads the page from the call and shows it beside the chat, so a
//!   chart or a mock-up needs no file in the project.
//! - `ask_secret` asks the person for a key or a password (`name`, `why`)
//!   through a private card. The daemon saves what they type where they
//!   choose and leaves an answer in `$DENT_ASKS/<name>.answer`; this server
//!   waits for it and tells the agent where the secret is, never what it is.

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

/// The tools' full names as the agent sees them, for allowing them up front.
pub const TOOL: &str = "mcp__dent__update_plan";
pub const VISUAL: &str = "mcp__dent__show_visual";
pub const SECRET: &str = "mcp__dent__ask_secret";
pub const LOOK: &str = "mcp__dent__preview_visual";

/// The `--mcp-config` that adds this server, run by `exe`; `asks` is the
/// folder its secret answers arrive in.
pub fn config(exe: &str, asks: &Path) -> String {
    json!({"mcpServers": {"dent": {
        "command": exe,
        "args": ["plan-tool"],
        "env": {"DENT_ASKS": asks.display().to_string()},
    }}})
    .to_string()
}

/// Serve until stdin closes. A secret's wait runs on its own thread, so
/// the agent's other calls (and pings) are answered meanwhile.
pub fn run() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let say = |out: &Mutex<std::io::Stdout>, reply: Value| {
        if let Ok(mut o) = out.lock() {
            let _ = writeln!(o, "{reply}");
            let _ = o.flush();
        }
    };
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if msg["method"] == "tools/call" && msg["params"]["name"] == "preview_visual" {
            let out = out.clone();
            std::thread::spawn(move || {
                let dir = std::env::var_os("DENT_ASKS").map(std::path::PathBuf::from);
                let result = look(dir.as_deref(), &msg["params"]["arguments"], LOOK_WAIT);
                say(
                    &out,
                    json!({"jsonrpc": "2.0", "id": msg["id"].clone(), "result": result}),
                );
            });
            continue;
        }
        if msg["method"] == "tools/call" && msg["params"]["name"] == "ask_secret" {
            let out = out.clone();
            std::thread::spawn(move || {
                let dir = std::env::var_os("DENT_ASKS").map(std::path::PathBuf::from);
                let text = ask_secret(dir.as_deref(), &msg["params"]["arguments"]);
                say(&out, called(msg["id"].clone(), &text));
            });
            continue;
        }
        if let Some(reply) = answer(&msg) {
            say(&out, reply);
        }
    }
    Ok(())
}

/// What `show_visual` tells the agent: what the page is for, where it shows
/// and how to make it sit well in the chat.
const VISUAL_DESCRIPTION: &str = "Show the person a finished visual inside the chat, above your reply: a chart, a table, a diagram, a mock-up, a comparison. Call it before you write that reply. They see the page, so your reply shouldn't announce it, say where it is or restate it: add only what the page doesn't say. Give one whole, self-contained HTML document (inline <style> and <script>; libraries may load from a CDN). Nothing is written to the project. Call it again with the same title to show a new version.\n\nHow it sits in the chat: on the chat's own background, about 700px wide, left-aligned with your reply, and exactly as tall as the page (the frame grows to fit, so the page never scrolls inside itself). So: no background on html or body, no outer card or banner; panels inside get at least 16px padding and var(--radius). Give charts a fixed pixel height. Never use viewport-height units (vh, svh, lvh, dvh) or height:100% on html or body: the frame follows the page's height, so a page that follows the frame grows without end, and it is refused.\n\nColours and type follow dent's light or dark theme through CSS variables, live: --background, --foreground, --muted, --muted-foreground, --card, --border, --primary, --primary-foreground, --accent, --destructive, --warning, --success, --info, --chart-1 to --chart-6, --radius, --font-sans, --font-serif, --font-mono. Use them rather than fixed colours. Links open in the person's browser.";

/// What `preview_visual` tells the agent.
const LOOK_DESCRIPTION: &str = "See a page as it will show in the chat before you show it with show_visual: dent draws it at the width you give (700 by default, the chat's column) in the person's theme and returns a picture, its height (contentHeight) and anything it reported going wrong in its console. Use it to check layout, overflow, wrapping and contrast, then fix and look again, or show it. It needs dent open on the person's computer; if it isn't, you'll be told.";

/// How long a look may take before the agent hears none came.
const LOOK_WAIT: Duration = Duration::from_secs(40);

/// Wait for a desktop's picture of the page (`look-<key>.json`, left by
/// the daemon), and give it to the agent as an image and a few words.
fn look(dir: Option<&Path>, args: &Value, wait: Duration) -> Value {
    let text = |t: &str| json!({"content": [{"type": "text", "text": t}], "isError": true});
    let Some(dir) = dir else {
        return text("dent can't draw pages here.");
    };
    let Some(html) = args["html"].as_str().filter(|h| !h.trim().is_empty()) else {
        return text("Give the page's whole HTML document as `html`.");
    };
    if let Some(why) = page_refused(Some(html)) {
        return text(&why);
    }
    let width = args["width"].as_u64().unwrap_or(700).clamp(240, 1600) as u32;
    let appearance = match args["appearance"].as_str() {
        Some(a @ ("light" | "dark")) => a,
        _ => "auto",
    };
    let key = dent_protocol::look_key(html, width, appearance);
    let file = dir.join(format!("look-{key}.json"));
    let until = std::time::Instant::now() + wait;
    loop {
        if let Ok(bytes) = std::fs::read(&file)
            && let Ok(l) = serde_json::from_slice::<dent_protocol::Look>(&bytes)
        {
            let _ = std::fs::remove_file(&file);
            return looked(&l);
        }
        if std::time::Instant::now() >= until {
            return text(
                "dent couldn't draw the page: no dent window answered (it may be closed, or this machine is reached only from a phone). Check the HTML by reading it, then show it with show_visual.",
            );
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// A look as the agent gets it: the picture, then what it measured.
fn looked(l: &dent_protocol::Look) -> Value {
    if let Some(why) = &l.failed {
        return json!({"content": [{"type": "text", "text": format!("dent couldn't draw the page: {why}")}], "isError": true});
    }
    let mut words = format!(
        "Drawn at {} px wide: contentHeight {} px.",
        l.width, l.content_height
    );
    if l.captured_height < l.content_height {
        words.push_str(&format!(
            " The picture shows the top {} px.",
            l.captured_height
        ));
    }
    if l.console.is_empty() {
        words.push_str(" Nothing went wrong in its console.");
    } else {
        words.push_str("\nIts console:\n");
        for line in &l.console {
            words.push_str("- ");
            words.push_str(line);
            words.push('\n');
        }
    }
    json!({"content": [
        {"type": "image", "data": l.png, "mimeType": "image/png"},
        {"type": "text", "text": words},
    ]})
}

/// What the agent hears once a page is shown.
const SHOWN: &str = "Shown to the person above your reply. Don't mention or describe the page; reply with only what it doesn't already say.";

/// Why a page can't be shown, if it can't: viewport-height units in its
/// styles make the fitted frame grow for ever.
fn page_refused(html: Option<&str>) -> Option<String> {
    let html = html?;
    let unit = viewport_height_unit(html)?;
    Some(format!(
        "The page uses a viewport height unit ({unit}). The frame grows to fit the page, so a size that follows the viewport makes it grow again and again. Let the content set the height, or use fixed pixel heights, and show it again."
    ))
}

/// The first viewport-height length (`100vh`, `50dvh`) in the page's styles:
/// `<style>` blocks and `style=` attributes, comments left out. Words and
/// data elsewhere (a base64 picture that happens to hold "1vh") don't count.
fn viewport_height_unit(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut css = String::new();
    // <style>…</style>
    let mut at = 0;
    while let Some(i) = lower[at..].find("<style") {
        let open = at + i;
        let Some(gt) = lower[open..].find('>') else {
            break;
        };
        let start = open + gt + 1;
        let end = lower[start..]
            .find("</style")
            .map_or(lower.len(), |e| start + e);
        css.push_str(&lower[start..end]);
        css.push('\n');
        at = end;
    }
    // style="…" and style='…'
    for quote in ['"', '\''] {
        let pat = format!("style={quote}");
        let mut at = 0;
        while let Some(i) = lower[at..].find(&pat) {
            let start = at + i + pat.len();
            let end = lower[start..]
                .find(quote)
                .map_or(lower.len(), |e| start + e);
            css.push_str(&lower[start..end]);
            css.push(';');
            at = end;
        }
    }
    // Without comments.
    let mut clean = String::with_capacity(css.len());
    let mut rest = css.as_str();
    while let Some(i) = rest.find("/*") {
        clean.push_str(&rest[..i]);
        rest = rest[i + 2..]
            .find("*/")
            .map_or("", |j| &rest[i + 2 + j + 2..]);
    }
    clean.push_str(rest);
    let b = clean.as_bytes();
    for unit in ["dvh", "svh", "lvh", "vh"] {
        let mut from = 0;
        while let Some(i) = clean[from..].find(unit) {
            let at = from + i;
            from = at + unit.len();
            // A number right before it, and no letter right after.
            let after = b.get(at + unit.len()).copied().unwrap_or(b' ');
            if after.is_ascii_alphanumeric() || after == b'-' || after == b'_' {
                continue;
            }
            let mut j = at;
            while j > 0 && (b[j - 1].is_ascii_digit() || b[j - 1] == b'.') {
                j -= 1;
            }
            let number = j < at;
            // What's before the number: a sign is fine (`-10vh`) unless
            // it's inside a name (`mt-2vh`).
            let before = if j > 0 { b[j - 1] } else { b' ' };
            let named = before.is_ascii_alphabetic()
                || before == b'_'
                || (before == b'-'
                    && j > 1
                    && (b[j - 2].is_ascii_alphanumeric() || b[j - 2] == b'_'));
            if number && !named {
                return Some(clean[j..at + unit.len()].to_string());
            }
        }
    }
    None
}

fn called(id: Value, text: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": text}]}})
}

/// The reply to one message; notifications get none.
fn answer(msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let result = match msg["method"].as_str().unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "dent", "version": env!("CARGO_PKG_VERSION")},
        }),
        "tools/list" => json!({"tools": [
            {
                "name": "update_plan",
                "description": "Show your plan for the current task to the person you're working with, as a checklist. Send the whole plan every time: one short line per step, each pending, in_progress or completed. Call it before you start a task with three or more steps, and again whenever a step starts or finishes.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"items": {"type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string", "description": "The step, in a few words."},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                        },
                        "required": ["content", "status"],
                    }}},
                    "required": ["items"],
                },
            },
            {
                "name": "show_visual",
                "description": VISUAL_DESCRIPTION,
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "A few words naming it."},
                        "html": {"type": "string", "description": "The whole HTML document."},
                    },
                    "required": ["title", "html"],
                },
            },
            {
                "name": "preview_visual",
                "description": LOOK_DESCRIPTION,
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "html": {"type": "string", "description": "The whole HTML document, as you'd give show_visual."},
                        "width": {"type": "integer", "minimum": 240, "maximum": 1600, "description": "The width to lay it out at, in CSS pixels. Defaults to 700, the chat's column; about 390 checks a phone."},
                        "appearance": {"type": "string", "enum": ["auto", "light", "dark"], "description": "dent's theme to draw it in. Defaults to auto, the person's own."},
                    },
                    "required": ["html"],
                },
            },
            {
                "name": "ask_secret",
                "description": "Ask the person for a secret (an API key, a token, a password) through a private card, and wait for them. You never see the value: it is saved where they choose, either the project's .env file or a file of this session's, under the name you give, and you are told which file. Load it in commands from there (for example `set -a; . <file>; set +a`) and never print, echo or log it. Never ask for secrets in chat, and never invent one.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "The environment variable it's saved as, like STRIPE_SECRET_KEY."},
                        "why": {"type": "string", "description": "What you need it for, in a few words."},
                    },
                    "required": ["name", "why"],
                },
            },
        ]}),
        "tools/call" => {
            if msg["params"]["name"] == "show_visual"
                && let Some(why) = page_refused(msg["params"]["arguments"]["html"].as_str())
            {
                return Some(json!({"jsonrpc": "2.0", "id": id, "result": {
                    "content": [{"type": "text", "text": why}],
                    "isError": true,
                }}));
            }
            let text = match msg["params"]["name"].as_str() {
                Some("show_visual") => SHOWN,
                // A secret is answered by `run`, on its own thread.
                Some("ask_secret") => "dent can't ask for secrets here.",
                _ => "Plan updated.",
            };
            return Some(called(id, text));
        }
        "ping" => json!({}),
        _ => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "method not found"},
            }));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// A secret's name as a file name: letters, digits, `_` and `-`, at most 64.
pub fn secret_key(name: &str) -> String {
    let key: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .take(64)
        .collect();
    if key.is_empty() { "SECRET".into() } else { key }
}

/// Wait for the person's answer to `ask_secret` and say what came of it.
fn ask_secret(dir: Option<&Path>, args: &Value) -> String {
    let Some(dir) = dir else {
        return "dent can't ask for secrets here.".into();
    };
    let name = secret_key(args["name"].as_str().unwrap_or_default());
    let file = dir.join(format!("{name}.answer"));
    let _ = std::fs::remove_file(&file);
    loop {
        if let Ok(s) = std::fs::read_to_string(&file)
            && let Ok(v) = serde_json::from_str::<Value>(&s)
        {
            let _ = std::fs::remove_file(&file);
            return said(&name, &v);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// What the agent hears of an answer: where the secret is, or that none
/// came.
pub fn said(name: &str, answer: &Value) -> String {
    match answer["saved"].as_str() {
        Some(path) => {
            let mut s = format!(
                "The person saved {name} in {path}. Load it in commands from there (`set -a; . '{path}'; set +a`) or let the project's tools read it; never print, echo or log its value."
            );
            if answer["ignored"] == true {
                s.push_str(" dent added .env to .gitignore, so it's never committed.");
            }
            s
        }
        None => format!(
            "The person chose not to give {name}. Don't ask for it again this turn: carry on without it, or say what it blocks."
        ),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_look_waits_for_its_picture_and_gives_up_in_time() {
        let dir = std::env::temp_dir().join(format!("dent-look-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let args = serde_json::json!({"html": "<p>hi</p>", "width": 390});
        // No window answers: told so, quickly.
        let none = super::look(Some(&dir), &args, std::time::Duration::from_millis(300));
        assert_eq!(none["isError"], true);
        assert!(
            none["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("no dent window")
        );
        // One does, under the same key the desktop works out.
        let key = dent_protocol::look_key("<p>hi</p>", 390, "auto");
        let l = dent_protocol::Look {
            png: "AAAA".into(),
            width: 390,
            content_height: 5000,
            captured_height: 4000,
            console: vec!["TypeError: x is undefined".into()],
            failed: None,
        };
        std::fs::write(
            dir.join(format!("look-{key}.json")),
            serde_json::to_vec(&l).unwrap(),
        )
        .unwrap();
        let got = super::look(Some(&dir), &args, std::time::Duration::from_secs(2));
        assert_eq!(got["content"][0]["type"], "image");
        let words = got["content"][1]["text"].as_str().unwrap();
        assert!(words.contains("contentHeight 5000"), "{words}");
        assert!(words.contains("top 4000"), "{words}");
        assert!(words.contains("TypeError"), "{words}");
        // Taken once: the file is gone.
        assert!(!dir.join(format!("look-{key}.json")).exists());
        // A page sized by the viewport is refused before anyone looks.
        let vh = serde_json::json!({"html": "<style>body{height:100vh}</style>"});
        assert_eq!(
            super::look(Some(&dir), &vh, std::time::Duration::ZERO)["isError"],
            true
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_page_sized_by_the_viewport_is_refused() {
        use super::viewport_height_unit as vh;
        assert_eq!(
            vh("<style>body{min-height:100vh}</style>").as_deref(),
            Some("100vh")
        );
        assert_eq!(
            vh("<div style='height: 50dvh'>x</div>").as_deref(),
            Some("50dvh")
        );
        assert_eq!(
            vh("<style>.a{margin:-10vh 0}</style>").as_deref(),
            Some("10vh")
        );
        assert_eq!(
            vh("<STYLE>main{height:calc(100svh - 2rem)}</STYLE>").as_deref(),
            Some("100svh")
        );
        // Not in its styles, in a comment, or part of a name: fine.
        assert_eq!(vh("<p>Use 100vh with care</p>"), None);
        assert_eq!(vh("<style>/* not 100vh */ body{margin:0}</style>"), None);
        assert_eq!(vh("<style>.mt-2vh{margin:2px}</style>"), None);
        assert_eq!(vh("<img src='data:image/png;base64,AAA1vhZZ'>"), None);
        assert_eq!(
            vh("<style>.x{height:240px}</style><div style=\"width:50vw\"></div>"),
            None
        );
        assert_eq!(vh(""), None);
        // An unclosed style still counts to its end.
        assert_eq!(vh("<style>body{height:100vh").as_deref(), Some("100vh"));
    }

    use super::*;

    #[test]
    fn it_lists_its_tools_and_answers_calls() {
        let init = answer(
            &json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
        );
        assert_eq!(
            init.as_ref().map(|r| &r["result"]["protocolVersion"]),
            Some(&json!("2025-06-18"))
        );
        // Notifications get no reply.
        assert!(answer(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        let list = answer(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(
            names,
            ["update_plan", "show_visual", "preview_visual", "ask_secret"]
        );
        let call = answer(
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"update_plan","arguments":{"items":[]}}}),
        );
        assert_eq!(call.as_ref().map(|r| &r["id"]), Some(&json!(2)));
        let other = answer(&json!({"jsonrpc":"2.0","id":3,"method":"resources/list"}));
        assert!(other.is_some_and(|r| r.get("error").is_some()));
    }

    #[test]
    fn a_secret_waits_for_its_answer_and_never_says_it() {
        let dir = std::env::temp_dir().join(format!("dent-asks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.clone();
        let waiting = std::thread::spawn(move || {
            ask_secret(Some(&d), &json!({"name": "API KEY!", "why": "tests"}))
        });
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(
            dir.join("APIKEY.answer"),
            json!({"saved": "/p/.env", "ignored": true}).to_string(),
        )
        .unwrap();
        let said = waiting.join().unwrap();
        assert!(said.contains("saved APIKEY in /p/.env"), "{said}");
        assert!(said.contains(".gitignore"), "{said}");
        assert!(!dir.join("APIKEY.answer").exists());
        assert!(super::said("K", &json!({"declined": true})).contains("chose not to give K"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
