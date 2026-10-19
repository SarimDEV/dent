use super::*;

pub(super) fn diff_paths(d: &Dent) -> Vec<String> {
    current(d).map_or(Vec::new(), |s| {
        s.diff.iter().map(|f| f.path.clone()).collect()
    })
}

pub(super) fn git_in_project(args: &[&str]) {
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(in_project(""))
        .args(["-c", "user.name=walk", "-c", "user.email=walk@example.com"])
        .args(args)
        .output();
}

pub(super) const TWELVE: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n";

/// One turn's changes on their own, and putting back a file or a hunk.
pub(super) fn turn_changes() -> Vec<(f32, Act)> {
    let mut v = new_session("write turnone.md please");
    v.extend([
        (0.1, Until("asks to write", |d, _| has(d, |i| matches!(i, Item::Approval { .. })), 10.)),
        (0.2, Click("allow")),
        (0.1, Until("the first turn writes", |d, _| status_is(d, Status::Done) && in_project("turnone.md").exists(), 10.)),
        (0.2, Type(Field::Composer, "write turntwo.md please")),
        (0.1, Key("enter")),
        (0.1, Until("asks again", |d, _| has(d, |i| matches!(i, Item::Approval { .. })), 10.)),
        (0.2, Click("allow")),
        (0.1, Until("the second turn writes", |d, _| status_is(d, Status::Done) && in_project("turntwo.md").exists(), 10.)),
        (0.2, Call(|d, _, cx| review(d, 1, "turntwo.md", cx))),
        (
            0.1,
            Until(
                "a turn's Review shows only what that turn changed",
                |d, _| d.changes_turn.len() == 1 && diff_paths(d) == ["turntwo.md"] && drawn(d, "all-changes").is_some(),
                5.,
            ),
        ),
        (0.3, Shot("turn-changes")),
        (0.8, Click("discard-file")),
        (0.1, Until("Discard asks first", |d, _| drawn(d, "discard-file-yes").is_some(), 1.)),
        (0.3, Shot("discard-ask")),
        (0.8, Click("discard-file-yes")),
        (
            0.1,
            Until(
                "discarding puts it back, and with the turn's changes gone Changes shows everything",
                |d, _| {
                    !in_project("turntwo.md").exists()
                        && in_project("turnone.md").exists()
                        && d.changes_turn.is_empty()
                        && diff_paths(d).contains(&"turnone.md".to_string())
                },
                5.,
            ),
        ),
        (0.2, Call(|d, _, cx| review(d, 0, "turnone.md", cx))),
        (
            0.1,
            Until(
                "an older turn's changes can't be discarded from its own view",
                |d, _| d.changes_turn.len() == 1 && drawn(d, "discard-file").is_some(),
                5.,
            ),
        ),
        (0.2, Click("discard-file")),
        (
            0.3,
            Until(
                "and pressing it does nothing",
                |d, _| d.discarding.is_none() && in_project("turnone.md").exists(),
                1.,
            ),
        ),
        // A committed file with two changes far apart: one hunk goes back.
        (
            0.2,
            Call(|_, _, _| {
                let _ = std::fs::write(in_project("twelve.txt"), TWELVE);
                git_in_project(&["add", "twelve.txt"]);
                git_in_project(&["commit", "-qm", "twelve"]);
                let _ = std::fs::write(
                    in_project("twelve.txt"),
                    TWELVE.replace("2\n3", "TWO\n3").replace("11\n", "ELEVEN\n"),
                );
            }),
        ),
        (0.2, Call(|d, _, cx| {
            let sid = d.selected.clone().unwrap_or_default();
            d.review_turn(&sid, None, Some("twelve.txt".into()), cx);
        })),
        (
            0.1,
            Until(
                "all changes show the file's two hunks",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.diff.iter().any(|f| {
                            f.path == "twelve.txt"
                                && f.lines.iter().filter(|l| l.kind == crate::data::Line::Hunk).count() == 2
                        })
                    })
                },
                5.,
            ),
        ),
        (
            0.2,
            Call(|d, _, cx| {
                let sid = d.selected.clone().unwrap_or_default();
                d.discarding = Some((sid, "twelve.txt".into(), Some(1)));
                cx.notify();
            }),
        ),
        (0.2, Click("discard-hunk-yes-1")),
        (
            0.1,
            Until(
                "discarding one hunk puts back just that change",
                |_, _| {
                    std::fs::read_to_string(in_project("twelve.txt"))
                        .is_ok_and(|s| s.contains("TWO") && !s.contains("ELEVEN") && s.contains("11\n"))
                },
                5.,
            ),
        ),
    ]);
    v
}
