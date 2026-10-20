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

/// A branch name long enough to need truncating anywhere it shows.
const LONG: &str = "feature/improve-the-onboarding-flow-for-teams-with-many-repositories";

fn git_in(dir: &std::path::Path, args: &[&str]) {
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=walk", "-c", "user.email=walk@example.com"])
        .args(args)
        .output();
}

/// The worst cases, for stills at every window size: a remote, seventy
/// branches (some very long), nine worktrees already made, a setup script
/// that's slow and fails, a long branch under the reply box, files with
/// long names in the viewer's tabs, and the states the Git button and a
/// closed session go through.
pub(super) fn worktree_edges() -> Vec<(f32, Act)> {
    vec![
        (
            0.1,
            Call(|_, _, _| {
                let proj = in_project("");
                let origin = proj.join("../origin.git");
                let _ = std::process::Command::new("git")
                    .args(["init", "-q", "--bare", "-b", "main"])
                    .arg(&origin)
                    .output();
                let _ = std::fs::create_dir_all(in_project(".dent"));
                let _ = std::fs::write(
                    in_project(".dent/setup"),
                    "echo installing\nsleep 4\necho 'npm ERR! missing script: install' >&2\nexit 1\n",
                );
                git_in(&proj, &["add", ".dent/setup"]);
                git_in(&proj, &["commit", "-qm", "Add a setup script"]);
                git_in(
                    &proj,
                    &["remote", "add", "origin", &origin.display().to_string()],
                );
                git_in(&proj, &["push", "-q", "-u", "origin", "main"]);
                git_in(&proj, &["remote", "set-head", "origin", "main"]);
                for i in 0..60 {
                    git_in(&proj, &["branch", &format!("topic/branch-{i:02}")]);
                }
                git_in(&proj, &["branch", LONG]);
                let home = std::env::var("HOME").unwrap_or_default();
                for i in 0..9 {
                    let b = format!("fix/a-long-running-piece-of-work-number-{i}-with-a-long-name");
                    let dir = format!("{home}/dent/worktrees/proj/existing-{i}");
                    git_in(&proj, &["worktree", "add", "-q", "-b", &b, &dir, "main"]);
                }
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
        (0.3, Shot("edge-chips-current")),
        (0.3, Click("chip-workspace")),
        (
            0.1,
            Until(
                "the menu lists eight of the nine worktrees, newest first",
                |d, _| {
                    d.menu == Some(crate::app::Menu::Workspace)
                        && d.workspace_entries()
                            .iter()
                            .filter(|e| e.key.starts_with("m-wt-"))
                            .count()
                            == 8
                },
                3.,
            ),
        ),
        (0.3, Shot("edge-workspace-menu")),
        (0.3, Click("m-worktree")),
        (0.3, Click("chip-base")),
        (
            0.1,
            Until(
                "the branch menu opens with its search",
                |d, _| drawn(d, "m-base-main").is_some(),
                3.,
            ),
        ),
        (0.3, Shot("edge-base-menu")),
        (0.2, Type(Field::Branches, "onboarding")),
        (
            0.1,
            Until(
                "typing narrows the branches to the one that matches",
                |d, cx| d.base_entries(cx).len() == 1,
                3.,
            ),
        ),
        (0.3, Shot("edge-base-search")),
        (0.2, Key("enter")),
        (
            0.1,
            Until(
                "Return picks it, and the chip says so",
                |d, _| d.defaults.base.as_deref() == Some(LONG) && d.menu.is_none(),
                3.,
            ),
        ),
        (
            0.2,
            Type(
                Field::Starter,
                "write a-really-long-file-name-for-checking-the-viewer-tabs.md please",
            ),
        ),
        (0.3, Shot("edge-chips-long-base")),
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
                "while setup runs, the session shows it working on it",
                |d, _| {
                    current(d).is_some_and(|s| {
                        matches!(s.activity, Some(dent_protocol::Activity::SettingUp { .. }))
                            && s.status == Status::Working
                    })
                },
                5.,
            ),
        ),
        (1.5, Shot("edge-setting-up")),
        (
            0.1,
            Until(
                "a failed setup says how, and the task still goes",
                |d, _| {
                    has(
                        d,
                        |i| matches!(i, Item::Note { text, .. } if text.contains("missing script")),
                    ) && has(d, |i| matches!(i, Item::Approval { .. }))
                },
                15.,
            ),
        ),
        (0.3, Shot("edge-setup-failed")),
        (0.2, Click("allow")),
        (
            0.1,
            Until("it writes", |d, _| status_is(d, Status::Done), 15.),
        ),
        // The agent moves to a branch with a long name: it shows at the turn's end.
        (
            0.2,
            Call(|d, _, _| {
                if let Some(s) = current(d) {
                    git_in(
                        std::path::Path::new(&s.cwd),
                        &[
                            "checkout",
                            "-q",
                            "-b",
                            &format!("{LONG}-and-then-some-more"),
                        ],
                    );
                }
            }),
        ),
        (
            0.2,
            Type(
                Field::Composer,
                "write second-file-with-a-long-name-too.md please",
            ),
        ),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "asks again",
                |d, _| has(d, |i| matches!(i, Item::Approval { .. })),
                15.,
            ),
        ),
        (0.2, Click("allow")),
        (
            0.1,
            Until(
                "the long branch shows under the reply box, and the button offers a pull request",
                |d, _| {
                    status_is(d, Status::Done)
                        && current(d).is_some_and(|s| {
                            s.branch
                                .as_deref()
                                .is_some_and(|b| b.ends_with("then-some-more"))
                                && d.git.get(&s.id).and_then(crate::gitbar::quick).is_some_and(
                                    |q| {
                                        q.label.starts_with("Commit, push")
                                            || q.label == "Commit & push"
                                    },
                                )
                        })
                        && drawn(d, "branch-strip").is_some()
                },
                15.,
            ),
        ),
        (0.3, Shot("edge-session-panel")),
        (
            0.2,
            Call(|d, _, cx| {
                d.panel_open = false;
                cx.notify();
            }),
        ),
        (0.6, Shot("edge-session-wide")),
        // A session in the main checkout, on the default branch: pushing asks first.
        (0.3, Key("cmd-n")),
        (0.2, Call(use_project)),
        (0.3, Click("chip-workspace")),
        (0.3, Click("m-here")),
        (0.2, Type(Field::Starter, "write onmain.md please")),
        (0.2, Click("start")),
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
                "on main, the next step is to commit and push",
                |d, _| {
                    status_is(d, Status::Done)
                        && current(d).is_some_and(|s| {
                            s.root.is_none()
                                && d.git
                                    .get(&s.id)
                                    .and_then(crate::gitbar::quick)
                                    .is_some_and(|q| q.label == "Commit & push")
                        })
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
                "a push to main asks for a second press",
                |d, _| current(d).is_some_and(|s| d.git_confirming(&s.id).is_some()),
                2.,
            ),
        ),
        (0.3, Shot("edge-push-to-main")),
        (
            5.5,
            Until(
                "unanswered, the question goes away",
                |d, _| current(d).is_some_and(|s| d.git_confirming(&s.id).is_none()),
                2.,
            ),
        ),
        // Back to the worktree session, closed with its changes not committed.
        (
            0.2,
            Call(|d, w, cx| {
                let id = d
                    .sessions
                    .iter()
                    .find(|s| s.root.is_some())
                    .map(|s| s.id.clone());
                d.select(id, w, cx);
            }),
        ),
        (0.3, Click("close")),
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
        (0.3, Click("remove-worktree")),
        (
            0.1,
            Until(
                "a worktree with changes isn't removed; the button asks again",
                |d, _| {
                    current(d).is_some_and(|s| {
                        std::path::Path::new(&s.cwd).exists() && d.worktree_force.contains(&s.id)
                    })
                },
                10.,
            ),
        ),
        (0.3, Shot("edge-remove-anyway")),
    ]
}
