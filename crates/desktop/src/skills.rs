//! A message kept as a Claude Code skill: `<skills>/<name>/SKILL.md`, its
//! name made from the message's first words, its description its first
//! line. Claude Code lists it under `/<name>` from then on.

use std::path::Path;

/// Words in a skill's name at most.
const NAME_WORDS: usize = 4;

/// Write `words` as a new skill under `skills`; its name, made unique.
pub fn save(skills: &Path, words: &str) -> std::io::Result<String> {
    let base = name(words);
    let mut name = base.clone();
    let mut n = 2;
    while skills.join(&name).exists() {
        name = format!("{base}-{n}");
        n += 1;
    }
    let dir = skills.join(&name);
    std::fs::create_dir_all(&dir)?;
    let first = words.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut about: String = first.trim().chars().take(120).collect();
    if first.trim().chars().count() > 120 {
        about.push('…');
    }
    let about = about.replace('"', "'");
    std::fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: \"{about}\"\n---\n\n{}\n",
            words.trim()
        ),
    )?;
    Ok(name)
}

/// A skill name from a message: its first few words, lowercase, joined by
/// hyphens, letters and digits only.
pub fn name(words: &str) -> String {
    let n: Vec<String> = words
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .take(NAME_WORDS)
        .collect();
    if n.is_empty() {
        "saved-prompt".into()
    } else {
        n.join("-")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_message_becomes_a_skill_named_from_its_first_words() {
        assert_eq!(
            super::name("Review this branch: tests, docs!"),
            "review-this-branch-tests"
        );
        assert_eq!(super::name("  ✨  "), "saved-prompt");
        let dir = std::env::temp_dir().join(format!("dent-skill-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = super::save(&dir, "Write the PR description").unwrap();
        let b = super::save(&dir, "Write the PR description").unwrap();
        assert_eq!(
            (a.as_str(), b.as_str()),
            ("write-the-pr-description", "write-the-pr-description-2")
        );
        let skill = std::fs::read_to_string(dir.join(&a).join("SKILL.md")).unwrap();
        assert!(skill.starts_with("---\nname: write-the-pr-description\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
