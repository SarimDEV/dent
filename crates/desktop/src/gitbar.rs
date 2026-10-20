//! The session header's Git button: the one next step for its branch
//! (commit, push, open a pull request, pull), from where the branch stands.

use dent_protocol::{GitAction, GitStatus};

/// What the button does when pressed.
#[derive(Debug, Clone, PartialEq)]
pub enum Press {
    Run(GitAction),
    /// Open the branch's pull request.
    Open(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quick {
    pub label: String,
    pub icon: &'static str,
    /// `None`: shown, but nothing to do now; the tip says why.
    pub press: Option<Press>,
    pub tip: String,
}

/// The next step for a branch; `None` when there's nothing to show.
pub fn quick(st: &GitStatus) -> Option<Quick> {
    use GitAction as A;
    let branch = st.branch.as_deref()?;
    let on_default = st.default_branch.as_deref() == Some(branch);
    let pr_open = st.pr.is_some();
    // A pull request for a branch that isn't the default, when one can be
    // made and there isn't one already.
    let can_pr = st.gh && st.remote && !on_default && !pr_open;
    let run = |label: &str, icon, a: GitAction, tip: String| {
        Some(Quick {
            label: label.into(),
            icon,
            press: Some(Press::Run(a)),
            tip,
        })
    };
    if st.changes {
        return if !st.remote {
            run(
                "Commit",
                "git-commit-horizontal",
                A::Commit,
                "Commit everything changed, with a message written from the diff".into(),
            )
        } else if can_pr {
            run(
                "Commit, push & PR",
                "git-pull-request-create",
                A::CommitPushPr,
                format!("Commit everything, push {branch} and open a pull request"),
            )
        } else {
            run(
                "Commit & push",
                "git-commit-horizontal",
                A::CommitPush,
                format!("Commit everything and push {branch}"),
            )
        };
    }
    if st.ahead > 0 && st.behind > 0 {
        return Some(Quick {
            label: "Diverged".into(),
            icon: "git-compare-arrows",
            press: None,
            tip: format!(
                "{branch} and the remote each have commits the other hasn't; ask the agent to rebase or merge"
            ),
        });
    }
    if st.behind > 0 {
        return run(
            "Pull",
            "arrow-down-to-line",
            A::Pull,
            format!(
                "Bring in {} commit{} from the remote",
                st.behind,
                plural(st.behind)
            ),
        );
    }
    // Commits no remote has yet (for a branch never pushed, every one).
    if st.remote && st.ahead > 0 {
        return if can_pr {
            run(
                "Push & PR",
                "git-pull-request-create",
                A::Pr,
                format!("Push {branch} and open a pull request"),
            )
        } else {
            run(
                "Push",
                "arrow-up-from-line",
                A::Push,
                format!("Push {} commit{} on {branch}", st.ahead, plural(st.ahead)),
            )
        };
    }
    st.pr.as_ref().map(|pr| Quick {
        label: format!("PR #{}", pr.number),
        icon: "git-pull-request",
        press: Some(Press::Open(pr.url.clone())),
        tip: "Open the pull request".into(),
    })
}

/// The button's words while an action runs.
pub fn busy(a: GitAction) -> &'static str {
    match a {
        GitAction::Commit => "Committing…",
        GitAction::CommitPush | GitAction::Push => "Pushing…",
        GitAction::CommitPushPr | GitAction::Pr => "Opening PR…",
        GitAction::Pull => "Pulling…",
    }
}

/// The button's words when a push would go to the default branch.
pub fn confirm(st: &GitStatus) -> String {
    format!("Push to {}?", st.branch.as_deref().unwrap_or("it"))
}

fn plural(n: u32) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dent_protocol::PullRequest;

    fn st() -> GitStatus {
        GitStatus {
            branch: Some("dent/fix-login".into()),
            default_branch: Some("main".into()),
            remote: true,
            gh: true,
            ..Default::default()
        }
    }

    fn label(s: &GitStatus) -> Option<String> {
        quick(s).map(|q| q.label)
    }

    #[test]
    fn changes_go_as_far_as_they_can() {
        let mut s = st();
        s.changes = true;
        assert_eq!(label(&s).as_deref(), Some("Commit, push & PR"));
        s.gh = false;
        assert_eq!(label(&s).as_deref(), Some("Commit & push"));
        s.remote = false;
        assert_eq!(label(&s).as_deref(), Some("Commit"));
        // On the default branch there's no pull request to open.
        let mut s = st();
        s.changes = true;
        s.branch = Some("main".into());
        assert_eq!(label(&s).as_deref(), Some("Commit & push"));
    }

    #[test]
    fn committed_work_is_pushed_then_shown() {
        let mut s = st();
        assert_eq!(label(&s), None);
        s.ahead = 2;
        assert_eq!(label(&s).as_deref(), Some("Push & PR"));
        s.pr = Some(PullRequest {
            number: 7,
            url: "https://example.com/7".into(),
        });
        assert_eq!(label(&s).as_deref(), Some("Push"));
        s.ahead = 0;
        s.upstream = true;
        let q = quick(&s).unwrap();
        assert_eq!(q.label, "PR #7");
        assert_eq!(q.press, Some(Press::Open("https://example.com/7".into())));
    }

    #[test]
    fn behind_pulls_and_diverged_says_so() {
        let mut s = st();
        s.upstream = true;
        s.behind = 1;
        assert_eq!(label(&s).as_deref(), Some("Pull"));
        s.ahead = 1;
        assert_eq!(quick(&s).unwrap().press, None);
    }

    #[test]
    fn detached_shows_nothing() {
        let mut s = st();
        s.branch = None;
        s.changes = true;
        assert_eq!(quick(&s), None);
    }
}
