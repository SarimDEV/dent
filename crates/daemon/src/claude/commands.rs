//! What each slash command takes, so a client can offer it ([decision
//! 0046]). Claude Code describes a command's arguments in a short hint,
//! like `[auto|<tokens>]` or `<low|medium|high>`; that grammar is the same
//! for every command, so reading it once types them all. Placeholders the
//! agent can fill (`<model>`, `[style]`, `<server>`) take its own lists.
//!
//! [decision 0046]: ../../../../docs/decisions/0046-commands-by-type.md

use dent_protocol::{Args, Choice, Command, Model, Setting};
use serde_json::Value;

/// What the agent knows that fills a command's arguments in.
#[derive(Default, Clone, Debug)]
pub struct Catalog {
    pub models: Vec<Model>,
    /// Output styles, with what each does.
    pub styles: Vec<(String, String)>,
    pub style: Option<String>,
    /// What `/config` takes, from its own usage text.
    pub settings: Vec<Setting>,
    /// `get_settings`: what's in effect.
    pub effective: Value,
    pub applied: Value,
    pub servers: Vec<String>,
}

/// The agent's commands with what each takes.
pub fn typed(raw: &[Command], cat: &Catalog) -> Vec<Command> {
    raw.iter()
        .map(|c| Command {
            args: args(&c.name, &c.hint, cat),
            ..c.clone()
        })
        .collect()
}

/// What a command with this hint takes.
pub fn args(name: &str, hint: &str, cat: &Catalog) -> Args {
    let hint = hint.trim();
    if hint.is_empty() {
        return Args::None;
    }
    if hint.contains('=') && !hint.starts_with(['[', '<']) {
        return if cat.settings.is_empty() {
            text(hint, false)
        } else {
            Args::Settings {
                settings: cat
                    .settings
                    .iter()
                    .map(|s| Setting {
                        current: s.current.clone().or_else(|| current_setting(cat, &s.key)),
                        ..s.clone()
                    })
                    .collect(),
            }
        };
    }
    let (group, rest) = match hint.chars().next() {
        Some(open @ ('[' | '<')) => match close_of(hint, open) {
            Some(end) => (
                Group {
                    inner: &hint[1..end],
                    optional: open == '[',
                },
                hint[end + 1..].trim(),
            ),
            None => return text(hint, false),
        },
        // `consent | revoke`: alternatives with no brackets.
        _ if hint.contains('|') => (
            Group {
                inner: hint,
                optional: false,
            },
            "",
        ),
        _ => return text(hint, false),
    };
    let mut a = group.args(cat);
    mark_current(name, &mut a, cat);
    if !rest.is_empty() {
        // More follows the first argument (`[level] [--fix] [<pr#>]`):
        // after a choice, words of your own.
        let then = text(rest, true);
        match &mut a {
            Args::Choice { options, .. } => {
                for o in options {
                    if o.then.is_none() && o.placeholder.is_empty() {
                        o.then = then.clone();
                    }
                }
            }
            Args::Text { placeholder, .. } => {
                *placeholder = format!("{placeholder} {rest}");
            }
            _ => {}
        }
    }
    a
}

fn text(placeholder: &str, optional: bool) -> Args {
    let p = placeholder.trim();
    let (p, optional) = match p.strip_prefix('[').and_then(|p| p.strip_suffix(']')) {
        Some(inner) if close_of(p, '[') == Some(p.len() - 1) => (inner, true),
        _ => (p, optional),
    };
    let p = p
        .strip_prefix('<')
        .and_then(|p| p.strip_suffix('>'))
        .unwrap_or(p);
    Args::Text {
        placeholder: p.to_string(),
        optional,
    }
}

/// A bracketed run of alternatives, without its brackets.
struct Group<'a> {
    inner: &'a str,
    optional: bool,
}

impl Group<'_> {
    fn args(&self, cat: &Catalog) -> Args {
        let alts = split_top(self.inner, '|');
        // One word alone is a placeholder: `[style]`, `<model>`, `[name]`.
        if alts.len() == 1 {
            let alt = alts[0].trim();
            if alt.starts_with("--") && !alt.contains(' ') {
                return Args::Choice {
                    options: vec![word(alt, cat)],
                    optional: self.optional,
                };
            }
            let name = unwrap_angle(alt);
            if let Some(options) = filled(name, cat) {
                return Args::Choice {
                    options,
                    optional: self.optional,
                };
            }
            return Args::Text {
                placeholder: name.to_string(),
                optional: self.optional,
            };
        }
        let mut options = Vec::new();
        for alt in alts {
            let alt = alt.trim();
            if alt.is_empty() {
                continue;
            }
            if alt.starts_with('<') && close_of(alt, '<') == Some(alt.len() - 1) {
                let name = unwrap_angle(alt);
                match filled(name, cat) {
                    Some(more) => options.extend(more),
                    None => options.push(Choice {
                        label: name.to_string(),
                        placeholder: name.to_string(),
                        ..Default::default()
                    }),
                }
                continue;
            }
            // `ultracode [on|off]`: a word, then what may follow it.
            if let Some(i) = alt.find(['[', '<'])
                && i > 0
            {
                let head = alt[..i].trim();
                let open = alt[i..].chars().next().unwrap_or('[');
                let then = match close_of(&alt[i..], open) {
                    Some(end) => Group {
                        inner: &alt[i + 1..i + end],
                        optional: open == '[',
                    }
                    .args(cat),
                    None => Args::None,
                };
                options.push(Choice {
                    then,
                    ..word(head, cat)
                });
                continue;
            }
            if alt.contains(' ') {
                // Words, not a word: something of your own.
                options.push(Choice {
                    label: alt.to_string(),
                    placeholder: alt.to_string(),
                    ..Default::default()
                });
                continue;
            }
            options.push(word(alt, cat));
        }
        // Nothing but values of your own is just text.
        if options.iter().all(|o| !o.placeholder.is_empty()) {
            return Args::Text {
                placeholder: options
                    .iter()
                    .map(|o| o.placeholder.as_str())
                    .collect::<Vec<_>>()
                    .join(" or "),
                optional: self.optional,
            };
        }
        Args::Choice {
            options,
            optional: self.optional,
        }
    }
}

/// A literal word; one that names a model says which.
fn word(w: &str, cat: &Catalog) -> Choice {
    let about = cat
        .models
        .iter()
        .find(|m| m.id == w)
        .map(|m| m.name.clone())
        .unwrap_or_default();
    Choice {
        value: w.to_string(),
        about,
        ..Default::default()
    }
}

/// Placeholders the agent can fill with its own list.
fn filled(name: &str, cat: &Catalog) -> Option<Vec<Choice>> {
    let list: Vec<Choice> = match name.trim().to_lowercase().as_str() {
        "model" => cat
            .models
            .iter()
            .map(|m| Choice {
                value: m.id.clone(),
                label: m.name.clone(),
                about: m.description.clone(),
                ..Default::default()
            })
            .collect(),
        "style" => cat
            .styles
            .iter()
            .map(|(s, about)| Choice {
                value: s.clone(),
                about: about.clone(),
                ..Default::default()
            })
            .collect(),
        "server" => cat
            .servers
            .iter()
            .map(|s| Choice {
                value: s.clone(),
                ..Default::default()
            })
            .collect(),
        _ => return None,
    };
    (!list.is_empty()).then_some(list)
}

/// Which setting a command changes, where it says the choice in effect:
/// the one piece of per-command knowledge, kept to this table.
fn mark_current(name: &str, a: &mut Args, cat: &Catalog) {
    let now = match name {
        "output-style" => cat.style.clone(),
        "advisor" => cat.applied.is_object().then(|| {
            cat.applied["advisor"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("off")
                .to_string()
        }),
        "model" => cat.applied["model"].as_str().map(str::to_string),
        "effort" => cat.applied["effort"].as_str().map(str::to_string),
        _ => None,
    };
    if let (Some(now), Args::Choice { options, .. }) = (now, a) {
        for o in options {
            o.current = o.value.eq_ignore_ascii_case(&now);
        }
    }
}

/// A `/config` key's value in effect, when the settings use its name.
fn current_setting(cat: &Catalog, key: &str) -> Option<String> {
    match &cat.effective[key] {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `/config`'s usage text: `  key=a|b|c` or `  key=<value>` per line.
pub fn settings_from_usage(text: &str) -> Vec<Setting> {
    text.lines()
        .filter(|l| l.starts_with("  "))
        .filter_map(|l| {
            let (key, values) = l.trim().split_once('=')?;
            if key.is_empty() || key.contains(' ') {
                return None;
            }
            let values = if values.starts_with('<') {
                Vec::new()
            } else {
                values.split('|').map(str::to_string).collect()
            };
            Some(Setting {
                key: key.to_string(),
                values,
                current: None,
            })
        })
        .collect()
}

fn unwrap_angle(s: &str) -> &str {
    s.strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(s)
        .trim()
}

/// Where the bracket opening `s` closes, counting nested ones.
fn close_of(s: &str, open: char) -> Option<usize> {
    let close = if open == '[' { ']' } else { '>' };
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        if c == '[' || c == '<' {
            depth += 1;
        } else if c == ']' || c == '>' {
            depth -= 1;
            if depth == 0 {
                return (c == close).then_some(i);
            }
        }
    }
    None
}

/// `s` split at `sep` outside brackets.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut from) = (0i32, 0);
    for (i, c) in s.char_indices() {
        match c {
            '[' | '<' => depth += 1,
            ']' | '>' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[from..i]);
                from = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[from..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(a: &Args) -> Vec<String> {
        match a {
            Args::Choice { options, .. } => options
                .iter()
                .map(|o| {
                    if o.placeholder.is_empty() {
                        o.value.clone()
                    } else {
                        format!("<{}>", o.placeholder)
                    }
                })
                .collect(),
            other => panic!("not a choice: {other:?}"),
        }
    }

    fn cat() -> Catalog {
        Catalog {
            models: vec![Model {
                id: "opus".into(),
                name: "Opus 5.5".into(),
                description: "For complex work".into(),
                resolved: String::new(),
                efforts: Vec::new(),
            }],
            styles: vec![
                ("default".into(), String::new()),
                ("Concise".into(), "Terse".into()),
            ],
            style: Some("Concise".into()),
            settings: settings_from_usage(
                "Usage: /config key=value\n  autoCompact=true|false\n  language=<value>\n",
            ),
            effective: serde_json::json!({ "autoCompact": false }),
            applied: serde_json::json!({ "advisor": null }),
            servers: vec!["github".into()],
        }
    }

    #[test]
    fn reads_every_shape_claude_code_uses() {
        let c = cat();
        assert_eq!(args("x", "", &c), Args::None);
        assert_eq!(
            values(&args("autocompact", "[auto|<tokens>]", &c)),
            ["auto", "<tokens>"]
        );
        // A word with what may follow it.
        let effort = args("effort", "<low|high|ultracode [on|off]>", &c);
        let Args::Choice { options, optional } = &effort else {
            panic!()
        };
        assert!(!optional);
        assert_eq!(values(&effort), ["low", "high", "ultracode"]);
        assert_eq!(values(&options[2].then), ["on", "off"]);
        // Placeholders the agent fills.
        assert_eq!(
            values(&args("output-style", "[style]", &c)),
            ["default", "Concise"]
        );
        assert_eq!(values(&args("model", "<model>", &c)), ["opus"]);
        // A model's alias says which model it is.
        let Args::Choice { options, .. } = args("advisor", "[fable|opus|sonnet|off]", &c) else {
            panic!()
        };
        assert_eq!(options[1].about, "Opus 5.5");
        // Which is in effect.
        assert!(options[3].current, "{options:?}");
        let Args::Choice { options, .. } = args("output-style", "[style]", &c) else {
            panic!()
        };
        assert!(options[1].current && !options[0].current);
        // Words of your own.
        assert_eq!(
            args(
                "compact",
                "<optional custom summarization instructions>",
                &c
            ),
            Args::Text {
                placeholder: "optional custom summarization instructions".into(),
                optional: false
            }
        );
        assert_eq!(
            args("rename", "[name]", &c),
            Args::Text {
                placeholder: "name".into(),
                optional: true
            }
        );
        assert!(matches!(
            args("x", "[<target>]", &c),
            Args::Text { optional: true, .. }
        ));
        assert!(matches!(
            args("loop", "[interval] [prompt]", &c),
            Args::Text { .. }
        ));
        // Bare alternatives, and a flag.
        assert_eq!(
            values(&args("design", "consent | revoke", &c)),
            ["consent", "revoke"]
        );
        assert_eq!(
            values(&args("reload-plugins", "[--force]", &c)),
            ["--force"]
        );
        // A choice, then more of your own.
        let review = args("code-review", "[low|high|ultra] [--fix] [<pr#>]", &c);
        let Args::Choice { options, .. } = &review else {
            panic!()
        };
        assert!(matches!(options[0].then, Args::Text { optional: true, .. }));
        // Nested alternatives with a placeholder the agent fills.
        let mcp = args("mcp", "[reconnect|enable|disable [<server>|all]]", &c);
        let Args::Choice { options, .. } = &mcp else {
            panic!()
        };
        assert_eq!(values(&options[2].then), ["github", "all"]);
    }

    #[test]
    fn settings_come_from_config_usage() {
        let c = cat();
        let Args::Settings { settings } = args("config", "key=value", &c) else {
            panic!()
        };
        assert_eq!(settings[0].key, "autoCompact");
        assert_eq!(settings[0].values, ["true", "false"]);
        assert_eq!(settings[0].current.as_deref(), Some("false"));
        assert!(settings[1].values.is_empty());
        // Without the list, it's text.
        let bare = Catalog::default();
        assert!(matches!(
            args("config", "key=value", &bare),
            Args::Text { .. }
        ));
    }
}
