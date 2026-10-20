//! Completing what's typed in a prompt: a `/command` that opens the message,
//! or an `@file` anywhere in it. Pure text work, so the composer and the new
//! session field share it and it can be tested without a window.

use dent_protocol::{Args, Command};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Slash,
    Mention,
    /// A word after a command's name: what the command takes.
    Arg,
}

/// The word the caret is in, when it's one to complete.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Token {
    pub kind: Kind,
    /// Where the word starts (its `/` or `@`) and ends, past the caret too.
    pub start: usize,
    pub end: usize,
    /// What's typed after the sign, up to the caret.
    pub query: String,
    /// For an argument: the command, and the words already given to it.
    pub command: String,
    pub before: Vec<String>,
}

/// One row of the prompt's menu.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Row {
    /// What picking it writes (a command's name, a path, a value).
    pub name: String,
    /// How it's shown, when not its name.
    pub label: String,
    pub about: String,
    /// What's in effect now.
    pub current: bool,
    /// A value of your own: picking it leaves you typing.
    pub open: bool,
    /// More follows it: picking it keeps the menu open for the next word.
    pub more: bool,
    /// Words of your own may follow it.
    pub then_text: bool,
    /// Another session, picked to go with the message as context.
    pub session: Option<String>,
}

impl From<Command> for Row {
    fn from(c: Command) -> Self {
        Row {
            // A command that takes a choice opens it once picked.
            more: matches!(c.args, Args::Choice { .. } | Args::Settings { .. }),
            about: if c.description.is_empty() {
                c.hint
            } else {
                c.description
            },
            name: c.name,
            ..Default::default()
        }
    }
}

/// The token around `caret`. A command only opens the message, since the
/// agent reads one nowhere else; a mention can be anywhere.
pub fn token(text: &str, caret: usize) -> Option<Token> {
    let mut caret = caret.min(text.len());
    while !text.is_char_boundary(caret) {
        caret -= 1;
    }
    let start = text[..caret]
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let end = text[caret..]
        .find(char::is_whitespace)
        .map(|i| caret + i)
        .unwrap_or(text.len());
    let word = &text[start..caret];
    // Past a command's name: its arguments.
    if start > 0
        && !word.starts_with('@')
        && let Some(rest) = text.strip_prefix('/')
    {
        // Only after a command's name, by the one rule for that: a path
        // like `/notes.md` or `/Users/me` takes no arguments.
        let first = rest.lines().next().unwrap_or_default();
        let name = crate::data::command_name(&format!("/{first}")).map(str::to_string)?;
        let before: Vec<String> = text[1 + name.len()..start]
            .split_whitespace()
            .map(str::to_string)
            .collect();
        return Some(Token {
            kind: Kind::Arg,
            start,
            end,
            query: word.to_string(),
            command: name.to_string(),
            before,
        });
    }
    let (kind, query) = if let Some(q) = word.strip_prefix('/') {
        // `/Users/me` is a path, not a command.
        if start != 0 || text[start + 1..end].contains('/') {
            return None;
        }
        (Kind::Slash, q)
    } else {
        (Kind::Mention, word.strip_prefix('@')?)
    };
    Some(Token {
        kind,
        start,
        end,
        query: query.to_string(),
        command: String::new(),
        before: Vec::new(),
    })
}

/// What a command takes at this point, given the words before: `None`
/// when it takes words of its own here, or nothing.
pub fn arg_rows(args: &Args, before: &[String], query: &str) -> Option<Vec<Row>> {
    let mut at = args;
    for w in before {
        match at {
            Args::Choice { options, .. } => {
                let o = options
                    .iter()
                    .find(|o| !o.value.is_empty() && o.value == *w)?;
                at = &o.then;
            }
            // Settings are one word each: `key=value`.
            Args::Settings { .. } => {}
            _ => return None,
        }
    }
    let q = query.to_lowercase();
    let rows: Vec<Row> = match at {
        Args::Choice { options, .. } => options
            .iter()
            .filter(|o| {
                q.is_empty()
                    || o.value.to_lowercase().starts_with(&q)
                    || o.label.to_lowercase().contains(&q)
            })
            .map(|o| Row {
                name: o.value.clone(),
                label: if o.label.is_empty() {
                    o.value.clone()
                } else {
                    o.label.clone()
                },
                about: o.about.clone(),
                current: o.current,
                open: o.value.is_empty(),
                more: matches!(o.then, Args::Choice { .. }),
                then_text: matches!(o.then, Args::Text { .. }),
                session: None,
            })
            .collect(),
        Args::Settings { settings } => match query.split_once('=') {
            // A setting's values, once its key is typed.
            Some((key, v)) => {
                let s = settings.iter().find(|s| s.key == key)?;
                let v = v.to_lowercase();
                s.values
                    .iter()
                    .filter(|x| x.to_lowercase().starts_with(&v))
                    .map(|x| Row {
                        name: format!("{key}={x}"),
                        label: x.clone(),
                        current: s.current.as_deref() == Some(x.as_str()),
                        ..Default::default()
                    })
                    .collect()
            }
            None => settings
                .iter()
                .filter(|s| q.is_empty() || s.key.to_lowercase().contains(&q))
                .map(|s| Row {
                    name: format!("{}=", s.key),
                    label: s.key.clone(),
                    about: match (&s.current, s.values.is_empty()) {
                        (Some(c), _) => c.clone(),
                        (None, true) => "your own value".into(),
                        (None, false) => s.values.join(" · "),
                    },
                    more: !s.values.is_empty(),
                    open: s.values.is_empty(),
                    ..Default::default()
                })
                .collect(),
        },
        _ => return None,
    };
    // Typing past every choice is a value of your own.
    (!rows.is_empty()).then_some(rows)
}

/// The text with the token replaced by `with`, and the caret's new place.
/// A finished word gets a space after it (or the caret steps over the one
/// already there); a folder doesn't, so its files can follow.
#[cfg(test)]
pub fn apply(text: &str, t: &Token, with: &str, finished: bool) -> (String, usize) {
    let (put, caret) = apply_at(text, t, with, finished);
    (
        format!("{}{put}{}", &text[..t.start], &text[t.end..]),
        caret,
    )
}

/// What [`apply`] puts in place of the word (`t.start..t.end`), and where
/// the caret goes then: the edit alone, so the rest of the box is left as
/// it is.
pub fn apply_at(text: &str, t: &Token, with: &str, finished: bool) -> (String, usize) {
    let has_space = text[t.end..].starts_with(char::is_whitespace);
    let space = if finished && !has_space { " " } else { "" };
    let put = format!("{with}{space}");
    let caret = t.start + put.len() + usize::from(finished && has_space);
    (put, caret)
}

/// How a picked path is written: `@path`, quoted when it has a space, the
/// way the agent reads it.
pub fn mention(path: &str) -> String {
    if path.contains(char::is_whitespace) {
        format!("@\"{path}\"")
    } else {
        format!("@{path}")
    }
}

/// Commands dent does itself, because it keeps what they'd change: the
/// agent switching its model behind dent's back would leave the chips wrong.
/// Name, what it does, and whether it needs an open session.
pub const OWN: [(&str, &str, bool); 5] = [
    ("model", "Choose the model", false),
    ("effort", "Choose how hard it thinks", false),
    ("rename", "Rename this session", true),
    (
        "restart",
        "Start the agent again, to load new skills, plugins and settings",
        true,
    ),
    ("clear", "Start a new session in this project", true),
];

/// Agent commands that can't work here: they drive the terminal app's own
/// screen, or the agent says they aren't available to apps like dent.
const HIDDEN: [&str; 12] = [
    "color",
    "focus",
    "fast",
    "heapdump",
    "extra-usage",
    "agents",
    "exit",
    "vim",
    "terminal-setup",
    "ide",
    "theme",
    "statusline",
];

/// What `/` offers: dent's own commands, then the agent's that work here.
pub fn offered(agent: &[Command], in_session: bool) -> Vec<Command> {
    let own =
        OWN.iter()
            .filter(|(_, _, needs)| in_session || !needs)
            .map(|(name, description, _)| Command {
                name: (*name).into(),
                description: (*description).into(),
                ..Default::default()
            });
    let theirs = agent.iter().filter(|c| {
        // Internal ones start with `__`; ours replace theirs.
        !c.name.starts_with("__")
            && !c.name.starts_with("workflow-launch")
            && !HIDDEN.contains(&c.name.as_str())
            && !OWN.iter().any(|(n, ..)| *n == c.name)
    });
    own.chain(theirs.cloned()).collect()
}

/// A folder's files with the folders they're in, each folder once and
/// ending in `/`, so a mention can name a whole folder.
pub fn with_folders(paths: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut folders = Vec::new();
    for p in &paths {
        let mut at = 0;
        while let Some(i) = p[at..].find('/') {
            let dir = &p[..at + i + 1];
            if seen.insert(dir.to_string()) {
                folders.push(dir.to_string());
            }
            at += i + 1;
        }
    }
    folders.extend(paths);
    folders
}

/// Whether `q`'s letters appear in `s` in order.
fn in_order(s: &str, q: &str) -> bool {
    let mut it = s.chars();
    q.chars().all(|c| it.any(|x| x == c))
}

/// How well a command matches, lower is better: its name exactly, at its
/// start, at a word inside it, anywhere in it, then its description, then
/// its letters in order.
fn command_rank(q: &str, c: &Command) -> Option<u8> {
    if q.is_empty() {
        return Some(0);
    }
    let name = c.name.to_lowercase();
    if name == q {
        return Some(0);
    }
    if name.starts_with(q) {
        return Some(1);
    }
    if let Some(i) = name.find(q) {
        let word = name[..i].ends_with(['-', '_', ':', '.', '/', ' ']);
        return Some(if word { 2 } else { 3 });
    }
    if c.description.to_lowercase().contains(q) {
        return Some(5);
    }
    in_order(&name, q).then_some(8)
}

/// Commands matching `query`, best first; ties keep the agent's order.
pub fn commands(list: &[Command], query: &str) -> Vec<Command> {
    let q = query.to_lowercase();
    let mut hits: Vec<(u8, usize, &Command)> = list
        .iter()
        .enumerate()
        .filter_map(|(i, c)| Some((command_rank(&q, c)?, i, c)))
        .collect();
    hits.sort_by_key(|(rank, i, _)| (*rank, *i));
    hits.into_iter().map(|(_, _, c)| c.clone()).collect()
}

/// The most files a mention offers: past this, typing narrows faster than
/// scrolling.
const FILES_SHOWN: usize = 50;

/// The last part of a path, a folder's without its `/`.
pub fn base(path: &str) -> &str {
    let p = path.strip_suffix('/').unwrap_or(path);
    p.rsplit('/').next().unwrap_or(p)
}

/// Files and folders matching `query`: the name exactly, at its start,
/// inside it, then the path, then the letters in order anywhere; shorter
/// paths first. Typed with a `/`, it looks inside that folder.
pub fn files(paths: &[String], query: &str) -> Vec<Command> {
    let q = query.to_lowercase();
    let mut hits: Vec<(u8, usize, &String)> = paths
        .iter()
        .filter_map(|p| {
            let lower = p.to_lowercase();
            if lower == q {
                return None;
            }
            let name = base(&lower);
            let rank = if q.is_empty() {
                4
            } else if name == q || lower.starts_with(&q) && q.ends_with('/') {
                0
            } else if name.starts_with(&q) {
                1
            } else if name.contains(&q) {
                2
            } else if lower.contains(&q) {
                3
            } else if in_order(&lower, &q) {
                4
            } else {
                return None;
            };
            Some((rank, p.len(), p))
        })
        .collect();
    hits.sort();
    hits.into_iter()
        .take(FILES_SHOWN)
        .map(|(_, _, p)| Command {
            name: p.clone(),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(name: &str, description: &str) -> Command {
        Command {
            name: name.into(),
            description: description.into(),
            ..Default::default()
        }
    }

    #[test]
    fn finds_the_token_at_the_caret() {
        let t = token("/com", 4).unwrap();
        assert_eq!(
            (t.kind, t.query.as_str(), t.start, t.end),
            (Kind::Slash, "com", 0, 4)
        );
        // Caret mid-word: the query is what's before it, the word all of it.
        let t = token("/compact", 3).unwrap();
        assert_eq!((t.query.as_str(), t.end), ("co", 8));
        // A command only opens the message; a mention can be anywhere.
        assert!(token("see /com", 8).is_none());
        let t = token("look at @src/ma please", 15).unwrap();
        assert_eq!(
            (t.kind, t.query.as_str(), t.start, t.end),
            (Kind::Mention, "src/ma", 8, 15)
        );
        // Paths, emails and plain words aren't tokens.
        assert!(token("/Users/me", 3).is_none());
        assert!(token("me@example.com", 5).is_none());
        assert!(token("hello", 5).is_none());
        // Past a command's name: what it takes.
        let t = token("/effort ultracode o", 19).unwrap();
        assert_eq!(
            (
                t.kind,
                t.command.as_str(),
                t.before.as_slice(),
                t.query.as_str()
            ),
            (Kind::Arg, "effort", &["ultracode".to_string()][..], "o")
        );
        assert_eq!(token("/compact ", 9).unwrap().kind, Kind::Arg);
        // A mention there is still a mention.
        assert_eq!(token("/review @sr", 11).unwrap().kind, Kind::Mention);
        // Past a plain word, on a space, nothing is being typed.
        assert!(token("hello ", 6).is_none());
        // Multibyte text before the caret is fine.
        let t = token("café @ré", 10).unwrap();
        assert_eq!(t.query, "ré");
    }

    #[test]
    fn applies_in_place() {
        let t = token("/co", 3).unwrap();
        assert_eq!(apply("/co", &t, "/compact", true), ("/compact ".into(), 9));
        // Mid-text: the rest stays, and its space isn't doubled.
        let text = "fix @ma now";
        let t = token(text, 7).unwrap();
        assert_eq!(
            apply(text, &t, "@src/main.rs", true),
            ("fix @src/main.rs now".into(), 17)
        );
        // A folder leaves the caret in the word, to go on into it.
        let t = token("@sr", 3).unwrap();
        assert_eq!(apply("@sr", &t, "@src/", false), ("@src/".into(), 5));
        assert_eq!(token("@src/", 5).unwrap().query, "src/");
        assert_eq!(mention("my notes.md"), "@\"my notes.md\"");
    }

    #[test]
    fn ranks_commands() {
        let list = [
            cmd("review", "Review a pull request"),
            cmd("pr-comments", "Get comments from a GitHub pull request"),
            cmd("compact", "Clear history but keep a summary"),
            cmd("context", "Show context usage"),
        ];
        let names =
            |q: &str| -> Vec<String> { commands(&list, q).into_iter().map(|c| c.name).collect() };
        assert_eq!(names(""), ["review", "pr-comments", "compact", "context"]);
        assert_eq!(names("co"), ["compact", "context", "pr-comments"]);
        assert_eq!(names("comments"), ["pr-comments"]);
        // The description finds it when the name doesn't.
        assert_eq!(names("summary"), ["compact"]);
        assert!(names("zzz").is_empty());
    }

    #[test]
    fn offers_what_works_here() {
        let agent = [
            cmd("model", "theirs"),
            cmd("compact", ""),
            cmd("color", ""),
            cmd("__x", ""),
        ];
        let names = |s| -> Vec<String> { offered(&agent, s).into_iter().map(|c| c.name).collect() };
        assert_eq!(
            names(true),
            ["model", "effort", "rename", "restart", "clear", "compact"]
        );
        assert_eq!(names(false), ["model", "effort", "compact"]);
    }

    #[test]
    fn folders_and_inside_them() {
        let all = with_folders(vec!["src/main.rs".into(), "src/app/mod.rs".into()]);
        assert_eq!(all, ["src/", "src/app/", "src/main.rs", "src/app/mod.rs"]);
        let names =
            |q: &str| -> Vec<String> { files(&all, q).into_iter().map(|c| c.name).collect() };
        assert_eq!(names("sr")[0], "src/");
        // Inside a folder: everything under it, not the folder again.
        assert_eq!(names("src/"), ["src/app/", "src/main.rs", "src/app/mod.rs"]);
        assert_eq!(names("src/app/"), ["src/app/mod.rs"]);
    }

    #[test]
    fn ranks_files() {
        let paths: Vec<String> = [
            "src/main.rs",
            "docs/maintain.md",
            "main.rs",
            "src/app/domain.rs",
        ]
        .map(String::from)
        .into();
        let names: Vec<String> = files(&paths, "main").into_iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            [
                "main.rs",
                "src/main.rs",
                "docs/maintain.md",
                "src/app/domain.rs"
            ]
        );
    }
}
