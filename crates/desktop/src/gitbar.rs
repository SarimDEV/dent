//! The session header's Git button: the one next step for its branch
//! (commit, push, open a pull request, pull), from where the branch stands.

use dent_protocol::{GitAction, GitStatus, PrBrief, PrState, PullRequest, Review};

/// What the button does when pressed.
#[derive(Debug, Clone, PartialEq)]
pub enum Press {
    Run(GitAction),
    /// Send the agent what it needs about the pull request.
    Brief(PrBrief),
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
    let open_pr = st.pr.as_ref().filter(|p| p.open());
    let pr_open = open_pr.is_some();
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
    // Nothing of yours to send: what the pull request needs, if anything.
    let pr = open_pr?;
    if pr.checks.failed > 0 {
        return Some(Quick {
            label: "Fix checks".into(),
            icon: "wrench",
            press: Some(Press::Brief(PrBrief::Checks)),
            tip: format!(
                "Send the agent the failing check{} on #{} and the end of the log",
                plural(pr.checks.failed),
                pr.number
            ),
        });
    }
    (pr.review == Some(Review::ChangesRequested)).then(|| Quick {
        label: "Address review".into(),
        icon: "message-square",
        press: Some(Press::Brief(PrBrief::Review)),
        tip: format!(
            "Send the agent the review on #{} and its comments",
            pr.number
        ),
    })
}

/// A pull request's state in a few words, the most pressing first.
pub fn pr_words(pr: &PullRequest) -> &'static str {
    match pr.state {
        PrState::Merged => "Merged",
        PrState::Closed => "Closed",
        PrState::Open if pr.checks.failed > 0 => "Checks failing",
        PrState::Open if pr.review == Some(Review::ChangesRequested) => "Changes requested",
        PrState::Open if pr.checks.pending > 0 => "Checks running",
        PrState::Open if pr.review == Some(Review::Approved) => "Approved",
        PrState::Open if pr.checks.passed > 0 => "Checks passed",
        PrState::Open => "Open",
    }
}

/// How a pull request's state is marked: needs you, fine, running, or done.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tone {
    Trouble,
    Good,
    Running,
    Merged,
    Closed,
}

pub fn pr_tone(pr: &PullRequest) -> Tone {
    match pr.state {
        PrState::Merged => Tone::Merged,
        PrState::Closed => Tone::Closed,
        _ if pr.checks.failed > 0 || pr.review == Some(Review::ChangesRequested) => Tone::Trouble,
        _ if pr.checks.pending > 0 => Tone::Running,
        _ => Tone::Good,
    }
}

/// The button's words while an action runs.
pub fn busy(a: GitAction) -> &'static str {
    match a {
        GitAction::Commit => "Committing…",
        GitAction::Push => "Pushing…",
        GitAction::CommitPush => "Committing & pushing…",
        GitAction::Pr => "Opening PR…",
        GitAction::CommitPushPr => "Committing & opening PR…",
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
            state: PrState::Open,
            checks: Default::default(),
            review: None,
        });
        assert_eq!(label(&s).as_deref(), Some("Push"));
        s.ahead = 0;
        s.upstream = true;
        // Pushed, its pull request fine: nothing to do here.
        assert_eq!(quick(&s), None);
        // Failing checks, then a review asking for changes: the agent hears.
        s.pr.as_mut().unwrap().checks.failed = 1;
        assert_eq!(
            quick(&s).unwrap().press,
            Some(Press::Brief(PrBrief::Checks))
        );
        s.pr.as_mut().unwrap().checks.failed = 0;
        s.pr.as_mut().unwrap().review = Some(Review::ChangesRequested);
        assert_eq!(label(&s).as_deref(), Some("Address review"));
        // Merged: a new commit goes up for a new pull request.
        let pr = s.pr.as_mut().unwrap();
        pr.state = PrState::Merged;
        pr.review = None;
        s.ahead = 1;
        assert_eq!(label(&s).as_deref(), Some("Push & PR"));
        assert_eq!(pr_words(s.pr.as_ref().unwrap()), "Merged");
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
