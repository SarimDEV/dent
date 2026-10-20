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

thread_local! {
    /// Where the diff stood before the sideways swipe.
    static SWIPE_TOP: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
}

/// A diagonal swipe over a wide diff moves it one way only: mostly down
/// scrolls down and not sideways, mostly sideways scrolls sideways and not
/// down.
pub(super) fn diff_swipe() -> Vec<(f32, Act)> {
    let mut v = new_session("hello");
    v.extend([
        (
            0.1,
            Until("the turn ends", |d, _| status_is(d, Status::Done), 10.),
        ),
        (
            0.2,
            Call(|d, _, cx| {
                let wide: String = (1..=200)
                    .map(|i| format!("{i} {}\n", "word ".repeat(60)))
                    .collect();
                let _ = std::fs::write(in_project("wide.txt"), &wide);
                git_in_project(&["add", "wide.txt"]);
                git_in_project(&["commit", "-qm", "wide"]);
                let _ = std::fs::write(in_project("wide.txt"), wide.replace("word", "WORD"));
                let sid = d.selected.clone().unwrap_or_default();
                d.review_turn(&sid, None, Some("wide.txt".into()), cx);
            }),
        ),
        (
            0.1,
            Until(
                "the wide diff is open",
                |d, _| {
                    diff_paths(d).contains(&"wide.txt".to_string()) && drawn(d, "diff").is_some()
                },
                5.,
            ),
        ),
        (0.3, Shot("swipe-0")),
        (0.3, Wheel("diff", -200.)),
        (
            0.3,
            Until(
                "a mouse wheel scrolls down, not sideways",
                |d, _| {
                    d.diff_list.scroll_px_offset_for_scrollbar().y < px(0.)
                        && d.diff_side.offset().x == px(0.)
                },
                1.,
            ),
        ),
        (0.3, Swipe("diff", -40., -400.)),
        (0.3, Shot("swipe-1")),
        (
            0.3,
            Until(
                "a swipe mostly down scrolls down and not sideways",
                |d, _| {
                    d.diff_list.logical_scroll_top().item_ix > 0 && d.diff_side.offset().x == px(0.)
                },
                1.,
            ),
        ),
        (
            0.1,
            Call(|d, _, _| {
                SWIPE_TOP.set(Some(
                    d.diff_list.scroll_px_offset_for_scrollbar().y.as_f32(),
                ))
            }),
        ),
        (0.3, Swipe("diff", -400., 40.)),
        (0.3, Shot("swipe-2")),
        (
            0.3,
            Until(
                "a swipe mostly sideways scrolls sideways and not up",
                |d, _| {
                    d.diff_side.offset().x < px(0.)
                        && SWIPE_TOP.get()
                            == Some(d.diff_list.scroll_px_offset_for_scrollbar().y.as_f32())
                },
                1.,
            ),
        ),
    ]);
    v
}
