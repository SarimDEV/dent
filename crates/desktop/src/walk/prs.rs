use super::*;

/// The stand-in GitHub CLI's state for the walk's pull request.
fn gh_state(json: &str) {
    if let Ok(path) = std::env::var("DENT_WALK_GH_STATE") {
        let _ = std::fs::write(path, json);
    }
}

/// What the header's Git button says now, as it draws it.
fn quick_label(d: &Dent) -> Option<String> {
    let s = current(d)?;
    let st = d.git.get(&s.id)?;
    let st = dent_protocol::GitStatus {
        pr: s.pr.clone().or_else(|| st.pr.clone()),
        ..st.clone()
    };
    crate::gitbar::quick(&st).map(|q| q.label)
}

fn sent(d: &Dent, needle: &'static str) -> bool {
    current(d).is_some_and(|s| {
        s.items
            .iter()
            .any(|i| matches!(i, Item::User { text, .. } if text.contains(needle)))
    })
}

const FAILING: &str = r#"{"number":12,"url":"https://github.com/walk/proj/pull/12","state":"OPEN","statusCheckRollup":[{"name":"test","status":"COMPLETED","conclusion":"FAILURE","detailsUrl":"https://github.com/walk/proj/actions/runs/123/job/4"}]}"#;
const CHANGES: &str = r#"{"number":12,"url":"https://github.com/walk/proj/pull/12","state":"OPEN","reviewDecision":"CHANGES_REQUESTED","reviews":[{"author":{"login":"reviewer"},"body":"Close, one thing to change."}],"statusCheckRollup":[{"name":"test","status":"COMPLETED","conclusion":"SUCCESS"}]}"#;
const MERGED: &str = r#"{"number":12,"url":"https://github.com/walk/proj/pull/12","state":"MERGED","statusCheckRollup":[{"name":"test","status":"COMPLETED","conclusion":"SUCCESS"}]}"#;

/// A session's pull request in the chat (decision 0054): opened from the
/// header, its checks failing and the agent briefed on them, a review asking
/// for changes and the agent briefed on that, then merged: marked, and left
/// in the list for you to close.
pub(super) fn prs() -> Vec<(f32, Act)> {
    let mut v = vec![(
        0.1,
        Call(|_, _, _| {
            let proj = in_project("");
            let origin = proj.join("../origin.git");
            let _ = std::process::Command::new("git")
                .args(["init", "-q", "--bare", "-b", "main"])
                .arg(&origin)
                .output();
            git_in_project(&["remote", "add", "origin", &origin.display().to_string()]);
            git_in_project(&["push", "-q", "-u", "origin", "main"]);
            git_in_project(&["remote", "set-head", "origin", "main"]);
        }),
    )];
    v.extend([
        (0.3, Key("cmd-n")),
        (0.2, Call(use_project)),
        (
            0.2,
            Call(|d, _, cx| {
                d.defaults.worktree = true;
                cx.notify();
            }),
        ),
        (0.2, Type(Field::Starter, "write prnote.md please")),
        (0.2, Click("start")),
        (
            0.1,
            Until(
                "a new session opens",
                |d, _| current(d).is_some() && !d.creating,
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "asks to write",
                |d, _| has(d, |i| matches!(i, Item::Approval { .. })),
                15.,
            ),
        ),
        (0.2, Click("allow")),
        (
            0.1,
            Until(
                "with a remote and GitHub, the next step opens a pull request",
                |d, _| {
                    status_is(d, Status::Done)
                        && quick_label(d).as_deref() == Some("Commit, push & PR")
                },
                15.,
            ),
        ),
        (
            0.2,
            Call(|d, _, cx| {
                d.panel_open = false;
                cx.notify();
            }),
        ),
        (0.3, Click("git-action")),
        (
            0.1,
            Until(
                "the pull request shows in the header, its checks running",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.pr.as_ref()
                            .is_some_and(|p| p.number == 12 && p.checks.pending == 1)
                    }) && drawn(d, "pr-badge").is_some()
                },
                15.,
            ),
        ),
        (0.3, Shot("pr-running")),
        (0.1, Call(|_, _, _| gh_state(FAILING))),
        (
            0.1,
            Until(
                "failing checks show, and the next step is to brief the agent",
                |d, _| {
                    current(d).is_some_and(|s| s.pr.as_ref().is_some_and(|p| p.checks.failed == 1))
                        && quick_label(d).as_deref() == Some("Fix checks")
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "failing checks put the session in Needs you, saying why in the list",
                |d, _| {
                    status_is(d, Status::Needs)
                        && current(d).is_some_and(|s| s.detail == "PR #12 · Checks failing")
                },
                5.,
            ),
        ),
        (0.3, Shot("pr-failing")),
        (0.2, Click("git-action")),
        (
            0.1,
            Until(
                "Fix checks sends the agent which failed and the end of the log",
                |d, _| {
                    sent(d, "The CI checks on pull request #12 failed")
                        && sent(d, "assertion failed")
                },
                10.,
            ),
        ),
        // The stand-in agent takes "failed" for a cue to fail its turn:
        // whatever came of it, the turn ends.
        (
            0.1,
            Until(
                "the agent's turn ends",
                |d, _| current(d).is_some_and(|s| !s.running()),
                15.,
            ),
        ),
        (0.1, Call(|_, _, _| gh_state(CHANGES))),
        (
            0.1,
            Until(
                "a review asking for changes is the next thing to send",
                |d, _| quick_label(d).as_deref() == Some("Address review"),
                10.,
            ),
        ),
        (0.2, Click("git-action")),
        (
            0.1,
            Until(
                "Address review sends the review and its comments on lines",
                |d, _| sent(d, "has review comments") && sent(d, "Use the theme colour here"),
                10.,
            ),
        ),
        // …and takes the review for a cue to ask a question: skipped.
        (
            0.1,
            Until(
                "the agent asks something",
                |d, _| drawn(d, "skip-question").is_some(),
                15.,
            ),
        ),
        (0.2, Click("skip-question")),
        (
            0.1,
            // Its turn ends; the review still asks for changes, so it needs you.
            Until(
                "and finishes, still needing you while the review stands",
                |d, _| current(d).is_some_and(|s| !s.running()) && status_is(d, Status::Needs),
                15.,
            ),
        ),
        (0.1, Call(|_, _, _| gh_state(MERGED))),
        (
            0.1,
            Until(
                "merged: marked in the header and the list, and the session stays open",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.pr.as_ref()
                            .is_some_and(|p| p.state == dent_protocol::PrState::Merged)
                            && s.detail == "Merged · PR #12"
                            && !s.closed
                    })
                },
                10.,
            ),
        ),
        (0.4, Shot("pr-merged")),
    ]);
    v
}

/// Each project brings back what its last new session started with; one
/// never started in keeps what's chosen.
pub(super) fn project_defaults() -> Vec<(f32, Act)> {
    vec![
        (0.3, Key("cmd-n")),
        (0.2, Call(use_project)),
        (
            0.2,
            Call(|d, _, cx| {
                d.defaults.model = Some("haiku".into());
                d.defaults.worktree = false;
                cx.notify();
            }),
        ),
        (0.2, Type(Field::Starter, "hello")),
        (0.2, Click("start")),
        (
            0.1,
            Until(
                "a new session opens",
                |d, _| current(d).is_some() && !d.creating,
                10.,
            ),
        ),
        (0.3, Key("cmd-n")),
        // Another project, chosen with other settings…
        (
            0.2,
            Call(|d, _, cx| {
                d.set_project(None);
                d.defaults.model = Some("opus".into());
                cx.notify();
            }),
        ),
        // …and back: the project's own come back.
        (
            0.2,
            Call(|d, _, cx| {
                d.set_project(std::env::var("DENT_WALK_PROJECT").ok());
                cx.notify();
            }),
        ),
        (
            0.1,
            Until(
                "back in the project, its last session's model is chosen again",
                |d, _| d.defaults.model.as_deref() == Some("haiku"),
                2.,
            ),
        ),
        (0.3, Shot("project-defaults")),
    ]
}

/// A project's scripts in `.dent/actions`, run from the header into your
/// terminal.
pub(super) fn scripts() -> Vec<(f32, Act)> {
    vec![
        (
            0.1,
            Call(|_, _, _| {
                let _ = std::fs::create_dir_all(in_project(".dent/actions"));
                let _ = std::fs::write(
                    in_project(".dent/actions/test"),
                    "# Run the tests\necho tests-ran > ran.txt\n",
                );
                git_in_project(&["add", ".dent/actions"]);
                git_in_project(&["commit", "-qm", "Add a test script"]);
            }),
        ),
        (0.3, Key("cmd-n")),
        (0.2, Call(use_project)),
        (
            0.2,
            Call(|d, _, cx| {
                d.defaults.worktree = false;
                cx.notify();
            }),
        ),
        (0.2, Type(Field::Starter, "hello")),
        (0.2, Click("start")),
        (
            0.1,
            Until(
                "a new session opens",
                |d, _| current(d).is_some() && !d.creating,
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "the header offers the project's scripts",
                |d, _| drawn(d, "chip-actions").is_some(),
                10.,
            ),
        ),
        (0.3, Click("chip-actions")),
        (
            0.1,
            Until(
                "the menu lists them, with what each does",
                |d, _| drawn(d, "m-action-test").is_some(),
                3.,
            ),
        ),
        (0.3, Shot("scripts-menu")),
        (0.2, Click("m-action-test")),
        (
            0.1,
            Until(
                "choosing one runs it in your terminal, in the session's folder",
                |_, _| in_project("ran.txt").exists(),
                15.,
            ),
        ),
        (0.6, Shot("scripts-ran")),
        // Something running in the terminal: a script isn't typed into it.
        (
            0.2,
            Call(|d, _, _| {
                if let Some(sh) = current(d).and_then(|s| d.shells.get(&s.id)) {
                    sh.type_bytes(b"sleep 30\r".to_vec());
                }
            }),
        ),
        (1.0, Click("chip-actions")),
        (0.3, Click("m-action-test")),
        (
            0.1,
            Until(
                "with something running there, it says so instead of typing into it",
                |d, _| {
                    d.toast
                        .as_ref()
                        .is_some_and(|t| t.text.contains("running in your terminal"))
                },
                5.,
            ),
        ),
        (0.3, Shot("scripts-busy")),
    ]
}
