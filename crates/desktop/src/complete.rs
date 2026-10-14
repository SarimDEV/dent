//! Completing what's typed in a prompt: a `/command` that opens the message,
//! or an `@file` anywhere in it. Pure text work, so the composer and the new
//! session field share it and it can be tested without a window.

use dent_protocol::Command;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Slash,
    Mention,
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
    })
}

/// The text with the token replaced by `with`, and the caret's new place.
/// A finished word gets a space after it (or the caret steps over the one
/// already there); a folder doesn't, so its files can follow.
pub fn apply(text: &str, t: &Token, with: &str, finished: bool) -> (String, usize) {
    let rest = &text[t.end..];
    let has_space = rest.starts_with(char::is_whitespace);
    let space = if finished && !has_space { " " } else { "" };
    let head = format!("{}{with}{space}", &text[..t.start]);
    let caret = head.len() + usize::from(finished && has_space);
    (format!("{head}{rest}"), caret)
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
pub const OWN: [(&str, &str, bool); 4] = [
    ("model", "Choose the model", false),
    ("effort", "Choose how hard it thinks", false),
    ("rename", "Rename this session", true),
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
                hint: String::new(),
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
            description: String::new(),
            hint: String::new(),
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
            hint: String::new(),
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
        // Past the word, on a space, nothing is being typed.
        assert!(token("/compact ", 9).is_none());
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
            ["model", "effort", "rename", "clear", "compact"]
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
