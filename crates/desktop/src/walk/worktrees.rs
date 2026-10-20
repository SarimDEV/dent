use super::*;

/// The session's folder and the project's, as the disk has them, so a
/// `/var` and a `/private/var` spelling compare equal.
fn real(p: &str) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.into())
}

fn last_commit(dir: &str) -> String {
    std::process::Command::new("git")
        .args(["-C", dir, "log", "-1", "--format=%s"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// A session in a new worktree (decision 0053): chosen under the field,
/// made with the setup script run and the branch named from the task, its
/// work committed from the header, then closed, its worktree removed and
/// brought back by reopening.
pub(super) fn worktrees() -> Vec<(f32, Act)> {
    vec![
        // A setup script the repository keeps, committed so worktrees have it.
        (
            0.1,
            Call(|_, _, _| {
                let _ = std::fs::create_dir_all(in_project(".dent"));
                let _ = std::fs::write(in_project(".dent/setup"), "echo ready > setup-ran.txt\n");
                git_in_project(&["add", ".dent/setup"]);
                git_in_project(&["commit", "-qm", "Add a setup script"]);
            }),
        ),
        (0.3, Key("cmd-n")),
        (0.2, Call(use_project)),
        (
            0.1,
            Until(
                "a repository offers where to work",
                |d, _| drawn(d, "chip-workspace").is_some(),
                5.,
            ),
        ),
        (0.3, Click("chip-workspace")),
        (
            0.1,
            Until(
                "the menu offers the checkout and a new worktree",
                |d, _| drawn(d, "m-here").is_some() && drawn(d, "m-worktree").is_some(),
                2.,
            ),
        ),
        (0.3, Shot("worktree-menu")),
        (0.3, Click("m-worktree")),
        (
            0.1,
            Until(
                "a new worktree shows the branch it starts from",
                |d, _| d.defaults.worktree && drawn(d, "chip-base").is_some(),
                2.,
            ),
        ),
        (0.2, Type(Field::Starter, "write wtnote.md please")),
        (0.3, Shot("worktree-chips")),
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
                "it works in a worktree of the project, kept under ~/dent/worktrees",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.cwd.contains("/dent/worktrees/proj/")
                            && s.root.as_deref().map(real)
                                == Some(real(&in_project("").display().to_string()))
                            && s.project == "proj"
                    })
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "the setup script ran in it before the task",
                |d, _| {
                    current(d).is_some_and(|s| {
                        std::path::Path::new(&s.cwd).join("setup-ran.txt").exists()
                    }) && has(
                        d,
                        |i| matches!(i, Item::Note { text, .. } if text.contains("Set up the worktree")),
                    )
                },
                15.,
            ),
        ),
        (
            0.1,
            Until(
                "its branch is named from the task",
                |d, _| current(d).is_some_and(|s| s.branch.as_deref() == Some("dent/walk-task")),
                15.,
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
                "it writes in the worktree, not the main checkout",
                |d, _| {
                    status_is(d, Status::Done)
                        && current(d).is_some_and(|s| {
                            std::path::Path::new(&s.cwd).join("wtnote.md").exists()
                        })
                        && !in_project("wtnote.md").exists()
                },
                15.,
            ),
        ),
        (
            0.1,
            Until(
                "its branch shows under the reply box, and the next Git step is a commit",
                |d, _| {
                    drawn(d, "branch-strip").is_some()
                        && drawn(d, "git-action").is_some()
                        && current(d).is_some_and(|s| {
                            d.git
                                .get(&s.id)
                                .and_then(crate::gitbar::quick)
                                .is_some_and(|q| q.label == "Commit")
                        })
                },
                10.,
            ),
        ),
        (0.3, Shot("worktree-session")),
        (0.2, Click("git-action")),
        (
            0.1,
            Until(
                "it commits on the worktree's branch, with a message written for it",
                |d, _| {
                    current(d).is_some_and(|s| last_commit(&s.cwd) == "Update the notes")
                        && d.git_busy.is_empty()
                        && d.toast
                            .as_ref()
                            .is_some_and(|t| t.text.contains("Committed"))
                },
                15.,
            ),
        ),
        (0.3, Shot("committed")),
        (0.3, Click("close")),
        (
            0.1,
            Until("it closes", |d, _| d.sessions.iter().any(|s| s.closed), 5.),
        ),
        (
            0.2,
            Call(|d, w, cx| {
                let id = d
                    .sessions
                    .iter()
                    .find(|s| s.closed && s.root.is_some())
                    .map(|s| s.id.clone());
                d.select(id, w, cx);
            }),
        ),
        (
            0.1,
            Until(
                "a closed session offers to remove its worktree",
                |d, _| drawn(d, "remove-worktree").is_some(),
                5.,
            ),
        ),
        (0.3, Shot("closed-worktree")),
        (0.2, Click("remove-worktree")),
        (
            0.1,
            Until(
                "removing takes the folder away and keeps the branch",
                |d, _| {
                    current(d).is_some_and(|s| !std::path::Path::new(&s.cwd).exists())
                        && std::process::Command::new("git")
                            .args([
                                "-C",
                                &in_project("").display().to_string(),
                                "rev-parse",
                                "--verify",
                                "-q",
                                "refs/heads/dent/walk-task",
                            ])
                            .output()
                            .is_ok_and(|o| o.status.success())
                },
                10.,
            ),
        ),
        (0.3, Click("reopen")),
        (
            0.1,
            Until(
                "reopening brings the worktree back on its branch",
                |d, _| {
                    current(d).is_some_and(|s| {
                        !s.closed
                            && std::path::Path::new(&s.cwd).join("wtnote.md").exists()
                            && last_commit(&s.cwd) == "Update the notes"
                    })
                },
                10.,
            ),
        ),
    ]
}
