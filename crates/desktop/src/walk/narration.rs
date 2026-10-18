use super::*;

/// How many chat paragraphs contain `needle`, the one streaming included.
pub(super) fn times_said(d: &Dent, needle: &str) -> usize {
    current(d).map_or(0, |s| {
        s.items
            .iter()
            .filter(|i| matches!(i, Item::Text(t) if t.contains(needle)))
            .count()
    })
}

pub(super) fn detail_is(d: &Dent, text: &str) -> bool {
    current(d).is_some_and(|s| s.detail == text)
}

/// A turn that talks between calls: each paragraph shows once, the
/// next streams on its own, and the working line says what's going on
/// while a long file is written out.
pub(super) fn narration() -> Vec<(f32, Act)> {
    const FIRST: &str = "First I'll read the handler";
    const SECOND: &str = "Now I'll write up";
    let mut v = new_session("Narrate as you go");
    v.extend([
        (
            0.1,
            Until(
                "the first paragraph streams",
                |d, _| says(d, FIRST) && status_is(d, Status::Working),
                15.,
            ),
        ),
        (
            0.,
            Until(
                "the second streams on its own, and the first shows once",
                |d, _| {
                    current(d).is_some_and(
                        |s| matches!(s.items.last(), Some(Item::Text(t)) if t.starts_with(SECOND)),
                    ) && times_said(d, FIRST) == 1
                },
                15.,
            ),
        ),
        (
            0.,
            Until(
                "writing the file, the line names it",
                |d, _| detail_is(d, "Writing handler-notes.md"),
                10.,
            ),
        ),
        (0., Shot("narrate-writing")),
        (
            0.,
            Until(
                "nothing streamed shows twice meanwhile",
                |d, _| times_said(d, FIRST) == 1 && times_said(d, SECOND) == 1,
                0.1,
            ),
        ),
        (
            0.1,
            Until("it finishes", |d, _| done_saying(d, "All done"), 20.),
        ),
        (
            0.,
            Until(
                "every paragraph once, none run together",
                |d, _| {
                    times_said(d, FIRST) == 1
                        && times_said(d, SECOND) == 1
                        && times_said(d, "All done") == 1
                        && times_said(d, "arrive.Now") == 0
                },
                0.1,
            ),
        ),
    ]);
    v.extend(new_session("Research it in parallel"));
    v.extend([
        (
            0.,
            Until(
                "a background subagent stays at work after the turn ends",
                |d, _| {
                    status_is(d, Status::Waiting)
                        && has(d, |i| {
                            matches!(i, Item::Tools { summary, .. }
                                if summary.starts_with("Subagent working: Survey competitors"))
                        })
                },
                15.,
            ),
        ),
        (
            0.,
            Until(
                "and the line says what it's doing",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.detail == "Subagent: Survey competitors · Searching the web for Monarch free trial"
                    })
                },
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "once done, its report is what it found, not the launch receipt",
                |d, _| {
                    done_saying(d, "The research is in")
                        && has(d, |i| {
                            matches!(i, Item::Tools { summary, lines, .. }
                                if summary.starts_with("Subagent: Survey competitors")
                                    && lines.iter().any(|l| l.report.as_deref() == Some("Monarch starts everyone on a trial before linking."))
                                    && !lines.iter().any(|l| l.report.as_deref().is_some_and(|r| r.contains("Async"))))
                        })
                },
                20.,
            ),
        ),
    ]);
    v.extend(new_session("Use a subagent"));
    v.push((
        0.,
        Until(
            "a subagent's latest step shows beside its task",
            |d, _| {
                current(d)
                    .is_some_and(|s| s.detail.starts_with("Subagent: Survey the tests · Read"))
            },
            10.,
        ),
    ));
    v
}
