use super::*;

/// The real agent at work, without the real agent: the fake replays one
/// turn recorded from it (narration, a plan, reads, an edit, a failed run
/// and the fix, an approval, the answer), stilled in both themes at each
/// moment. For seeing a visual change in seconds; `compare` runs the real
/// agent, once, before a batch lands.
pub(super) fn look() -> Vec<(f32, Act)> {
    let mut v = new_session("replay a real turn");
    v.push((
        0.1,
        Until(
            "the plan shows while it works",
            |d, _| current(d).is_some_and(|s| !s.todos.is_empty()) && plan_above_composer(d),
            20.,
        ),
    ));
    v.push((0.4, Call(to_end)));
    both_themes(&mut v, "look-1-working");
    v.push((
        0.1,
        Until(
            "its test run waits for approval",
            |d, _| has(d, |i| matches!(i, Item::Approval { .. })),
            30.,
        ),
    ));
    v.push((0.2, Call(to_end)));
    both_themes(&mut v, "look-2-approval");
    v.extend([
        (0.2, Click("allow")),
        (
            0.1,
            Until(
                "the turn ends with its answer",
                |d, _| done_saying(d, "now counts words correctly"),
                30.,
            ),
        ),
        (0.3, Call(to_reply)),
    ]);
    both_themes(&mut v, "look-3-answer");
    v.push((
        0.2,
        Call(|d, _, cx| {
            d.transcript.scroll_to(ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
            cx.notify();
        }),
    ));
    both_themes(&mut v, "look-4-top");
    v
}

/// A finished turn's work folds into one "Worked for" line above its
/// answer, and opens in place.
pub(super) fn fold() -> Vec<(f32, Act)> {
    let mut v = vec![(
        0.1,
        Call(|d, _, cx| {
            d.fold_turns = true;
            cx.notify();
        }),
    )];
    v.extend(new_session("replay a real turn"));
    v.extend([
        (
            0.1,
            Until(
                "while it works, nothing folds",
                |d, _| drawn(d, "tool-row").is_some() && drawn(d, "worked").is_none(),
                20.,
            ),
        ),
        (
            0.1,
            Until(
                "its test run waits for approval",
                |d, _| has(d, |i| matches!(i, Item::Approval { .. })),
                30.,
            ),
        ),
        (0.2, Click("allow")),
        (
            0.1,
            Until(
                "done, its work folds into one line above the answer",
                |d, _| {
                    done_saying(d, "now counts words correctly")
                        && drawn(d, "worked").is_some()
                        && drawn(d, "tool-row").is_none()
                },
                30.,
            ),
        ),
        (0.3, Call(to_end)),
    ]);
    both_themes(&mut v, "fold-closed");
    v.extend([
        (0.2, Click("worked")),
        (
            0.1,
            Until(
                "a click opens the work in place",
                |d, _| drawn(d, "tool-row").is_some() && drawn(d, "worked").is_some(),
                3.,
            ),
        ),
        (0.4, Shot("fold-open")),
        (0.2, Click("worked")),
        (
            0.1,
            Until(
                "and another folds it again",
                |d, _| drawn(d, "tool-row").is_none(),
                3.,
            ),
        ),
    ]);
    v
}

fn live(d: &Dent) -> usize {
    d.inline.borrow().live()
}

/// Pages in the chat at their edges: more than can be live, scrolling,
/// menus over them, another session, a tall one, a theme change.
pub(super) fn pages() -> Vec<(f32, Act)> {
    let mut v = new_session("show five visuals for the edges");
    v.extend([
        (
            0.1,
            Until("all five are shown", |d, _| says(d, "Five pages"), 30.),
        ),
        (
            0.1,
            Until(
                "pages in the chat open no empty viewer beside it",
                |d, _| current(d).is_some_and(|s| !d.shows_panel(s)),
                2.,
            ),
        ),
        (0.3, Call(to_end)),
        (
            0.1,
            Until(
                "the pages on screen go live, never more than three",
                |d, _| (1..=3).contains(&live(d)),
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "the tall one says its full height",
                |d, _| d.inline.borrow().heights.values().any(|h| *h > 1200.),
                10.,
            ),
        ),
        (0.2, Wheel("transcript", 300.)),
        (
            0.,
            Until(
                "scrolling, they step aside for their pictures",
                |d, _| live(d) == 0,
                1.,
            ),
        ),
        (
            0.1,
            Until(
                "still again, the ones in view go live",
                |d, _| live(d) >= 1,
                3.,
            ),
        ),
        (0.2, Key("cmd-k")),
        (
            0.,
            Until(
                "the palette over the chat sends them to pictures",
                |d, _| live(d) == 0,
                1.,
            ),
        ),
        (0.2, Key("escape")),
        (
            0.1,
            Until("closed, they come back", |d, _| live(d) >= 1, 3.),
        ),
        (0.3, Call(to_end)),
    ]);
    both_themes(&mut v, "pages-end");
    v.extend(new_session("first notes"));
    v.extend([(
        0.1,
        Until(
            "another session shows none of them",
            |d, _| live(d) == 0,
            3.,
        ),
    )]);
    v
}
