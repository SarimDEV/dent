//! How the desktop shows an agent: its mark, and what's said about how it
//! stands on this machine (for Settings and onboarding). Which agents
//! there are, and what each can do, is the daemon's to say (decision 0055).

use dent_protocol::{Account, AgentInfo};
use gpui_kit::Hsla;

/// The icon for agent `id`, and its colour (`None`: the text's own).
pub fn mark(id: &str) -> (&'static str, Option<Hsla>) {
    match id {
        "claude" => ("claude", Some(crate::theme::CLAUDE)),
        _ => ("bot", None),
    }
}

/// How an agent stands, as Settings and onboarding say it.
#[derive(Debug, Clone, PartialEq)]
pub enum Standing {
    /// Its CLI wasn't found.
    Missing,
    /// Installed, but signed out: it can't work until you sign in.
    SignedOut,
    /// Ready to work.
    Ready,
}

pub fn standing(a: &AgentInfo) -> Standing {
    match (&a.installed, &a.account) {
        (false, _) => Standing::Missing,
        (true, Account::SignedOut) => Standing::SignedOut,
        _ => Standing::Ready,
    }
}

/// Its version, and whether it's the one dent was tested with.
pub fn version_line(a: &AgentInfo) -> String {
    match &a.version {
        None => "Not installed".into(),
        Some(v) if a.untested() => format!(
            "{} · dent was tested with {}",
            dent_protocol::version_number(v),
            a.tested
        ),
        Some(v) => dent_protocol::version_number(v).to_string(),
    }
}

/// How it's signed in, in a few words.
pub fn account_line(a: &AgentInfo) -> String {
    match &a.account {
        Account::SignedIn { email, plan } => match (email, plan) {
            (Some(e), Some(p)) => format!("Signed in as {e} · {} plan", title_case(p)),
            (Some(e), None) => format!("Signed in as {e}"),
            (None, Some(p)) => format!("Signed in · {} plan", title_case(p)),
            (None, None) => "Signed in".into(),
        },
        Account::ApiKey => "Billed by API key · no plan limits".into(),
        Account::SignedOut => "Signed out".into(),
        Account::Unknown if a.installed => "Couldn't tell how it's signed in".into(),
        Account::Unknown => String::new(),
    }
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude() -> AgentInfo {
        let mut a = crate::bridge::agent_info(&[], "claude");
        a.installed = true;
        a.version = Some("2.1.301 (Claude Code)".into());
        a.tested = "2.1.301".into();
        a
    }

    #[test]
    fn an_agent_says_how_it_stands() {
        let mut a = claude();
        a.account = Account::SignedIn {
            email: Some("a@b.c".into()),
            plan: Some("max".into()),
        };
        assert_eq!(standing(&a), Standing::Ready);
        assert_eq!(version_line(&a), "2.1.301");
        assert_eq!(account_line(&a), "Signed in as a@b.c · Max plan");
        a.version = Some("2.2.0 (Claude Code)".into());
        assert_eq!(version_line(&a), "2.2.0 · dent was tested with 2.1.301");
        a.account = Account::SignedOut;
        assert_eq!(standing(&a), Standing::SignedOut);
        a.installed = false;
        assert_eq!(standing(&a), Standing::Missing);
    }

    #[test]
    fn every_agent_mark_is_an_icon_there_is() {
        use gpui_kit::AssetSource;
        for id in ["claude", "anything-else"] {
            let (name, _) = mark(id);
            let path = format!("icons/{name}.svg");
            assert!(
                matches!(crate::assets::Assets.load(&path), Ok(Some(_))),
                "{path} is missing"
            );
        }
    }
}
