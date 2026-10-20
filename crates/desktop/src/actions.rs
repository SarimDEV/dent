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
            let text = std::fs::read_to_string(e.path()).unwrap_or_default();
            Some((name, about(&text)))
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

/// The line typed into your terminal to run one.
pub fn command(name: &str) -> String {
    format!("sh {}", crate::shell::shell_quote(&format!("{DIR}/{name}")))
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
        assert_eq!(command("my script"), "sh '.dent/actions/my script'");
        assert!(list(&d.join("nope")).is_empty());
    }
}
