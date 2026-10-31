//! Secrets the person gives the agent through its private card, saved as
//! `NAME=value` lines in an env file: the project's `.env`, or one of the
//! session's own under the daemon's state. A new file is readable by you
//! alone.

use std::path::Path;

use anyhow::Result;

/// Save `name` as `value` in the env file at `file`: its line replaced if
/// it's there (`NAME=` or `export NAME=`), else added at the end.
pub fn save(file: &Path, name: &str, value: &str) -> Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let old = std::fs::read_to_string(file).unwrap_or_default();
    let line = format!("{name}={}", quoted(value));
    let mut found = false;
    let mut lines: Vec<String> = old
        .lines()
        .map(|l| {
            let bare = l
                .trim_start()
                .strip_prefix("export ")
                .unwrap_or(l.trim_start());
            if !found && bare.strip_prefix(name).is_some_and(|r| r.starts_with('=')) {
                found = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        lines.push(line);
    }
    let text = lines.join("\n") + "\n";
    let new = !file.exists();
    std::fs::write(file, text)?;
    #[cfg(unix)]
    if new {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// A value as an env file reads it back: bare when it's plain, else in
/// double quotes with `\` and `"` escaped.
fn quoted(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:+=@,%".contains(c));
    if plain {
        return value.to_string();
    }
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' | '\\' | '$' | '`' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether git would track `file` in the repository at `cwd`: it's in a
/// repository and not ignored there.
pub fn tracked(cwd: &Path, file: &Path) -> bool {
    let repo = crate::git::cmd()
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .is_ok_and(|o| o.status.success());
    repo && crate::git::cmd()
        .arg("-C")
        .arg(cwd)
        .args(["check-ignore", "-q"])
        .arg(file)
        .status()
        .is_ok_and(|s| s.code() == Some(1))
}

/// Add `name` to the `.gitignore` at `cwd`, on a line of its own.
pub fn ignore(cwd: &Path, name: &str) -> Result<()> {
    let file = cwd.join(".gitignore");
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    if text.lines().any(|l| l.trim() == name) {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(name);
    text.push('\n');
    std::fs::write(file, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_replaces_its_line_or_joins_the_end() {
        let dir = std::env::temp_dir().join(format!("dent-secrets-{}", std::process::id()));
        let file = dir.join(".env");
        let _ = std::fs::remove_dir_all(&dir);
        save(&file, "A", "one").unwrap();
        save(&file, "B", "two words \"quoted\"").unwrap();
        std::fs::write(
            &file,
            std::fs::read_to_string(&file).unwrap() + "export AB=x\n",
        )
        .unwrap();
        save(&file, "A", "uno").unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "A=uno\nB=\"two words \\\"quoted\\\"\"\nexport AB=x\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
