//! A project's scripts: the files in its `.dent/actions/`, each a shell
//! script you run from the session's header into your terminal ("test",
//! "dev"), beside the `.dent/setup` a new worktree runs.

use std::path::Path;

/// Where a project keeps them.
pub const DIR: &str = ".dent/actions";

/// A project's scripts in `dir`: each name, and what it does as its first
/// comment says (`# Run the tests`), A to Z.
pub fn list(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir.join(DIR)) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Hidden files and editors' leftovers aren't scripts.
            if name.starts_with('.') || name.ends_with('~') {
                return None;
            }
            // Only its start: the description is its first comment.
            let mut head = Vec::new();
            if let Ok(f) = std::fs::File::open(e.path()) {
                use std::io::Read;
                let _ = f.take(HEAD).read_to_end(&mut head);
            }
            Some((name, about(&String::from_utf8_lossy(&head))))
        })
        .collect();
    out.sort();
    out
}

/// A script's first comment, past any `#!` line.
fn about(text: &str) -> String {
    text.lines()
        .filter(|l| !l.starts_with("#!"))
        .map(str::trim)
        .find(|l| !l.is_empty())
        .and_then(|l| l.strip_prefix('#'))
        .map(|l| l.trim().to_string())
        .unwrap_or_default()
}

/// How much of a script is read for its description.
const HEAD: u64 = 1024;

/// The line typed into your terminal to run one. In the session's folder
/// it's just the script; anywhere else it goes there first in a subshell,
/// leaving the terminal where it was. `here` is where the terminal is.
pub fn command(name: &str, cwd: &str, here: Option<&str>) -> String {
    let script = format!("sh {}", crate::shell::shell_quote(&format!("{DIR}/{name}")));
    if here == Some(cwd) {
        return script;
    }
    // `~` left bare so the shell expands it; the rest quoted.
    let shown = crate::picker::tilde(cwd);
    let dir = match shown.strip_prefix("~/") {
        Some(rest) => format!("~/{}", crate::shell::shell_quote(rest)),
        None => crate::shell::shell_quote(cwd),
    };
    format!("(cd {dir} && {script})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_listed_with_what_they_do() {
        let d = std::env::temp_dir().join(format!("dent-actions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(DIR)).unwrap();
        std::fs::write(
            d.join(DIR).join("test"),
            "#!/bin/sh\n# Run the tests\ncargo test\n",
        )
        .unwrap();
        std::fs::write(d.join(DIR).join("dev"), "npm run dev\n").unwrap();
        std::fs::write(d.join(DIR).join(".DS_Store"), "").unwrap();
        assert_eq!(
            list(&d),
            vec![
                ("dev".to_string(), String::new()),
                ("test".to_string(), "Run the tests".to_string())
            ]
        );
        assert_eq!(
            command("my script", "/p/my app", Some("/p/my app")),
            "sh '.dent/actions/my script'"
        );
        assert_eq!(
            command("test", "/p/my app", Some("/elsewhere")),
            "(cd '/p/my app' && sh .dent/actions/test)"
        );
        assert!(list(&d.join("nope")).is_empty());
    }
}
