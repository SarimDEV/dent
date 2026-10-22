use super::*;

/// A session with some history, then a long mixed reply streamed at the
/// real agent's pace: run with DENT_FRAME_TIMES=1 on a release build to
/// time drawing while a reply streams.
pub(super) fn bench() -> Vec<(f32, Act)> {
    let mut v = new_session("show me colour output please");
    v.extend([
        (
            0.1,
            Until(
                "the first turn ends",
                |d, _| done_saying(d, "Tests ran"),
                15.,
            ),
        ),
        (
            0.2,
            Type(Field::Composer, "the math, diagram and big table"),
        ),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "the second turn ends",
                |d, _| done_saying(d, "all sixty"),
                20.,
            ),
        ),
        (0.5, Type(Field::Composer, "stream bench 30")),
        (0.1, Key("enter")),
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (
            0.1,
            Until(
                "the long reply finishes",
                |d, _| done_saying(d, "start with the first"),
                300.,
            ),
        ),
        (1.0, Until("rests", |_, _| true, 1.)),
        (
            0.1,
            Until(
                "a long reply is drawn a row per block",
                |d, _| current(d).is_some_and(|s| d.rows.len() > s.items.len() + 100),
                2.,
            ),
        ),
        // Selected whole, it copies as one message: every block once, in
        // order, paragraphs apart.
        (
            0.1,
            Call(|d, w, _| {
                if let Some(s) = current(d) {
                    crate::select::select_all(crate::select::area(&format!("chat/{}", s.id)), w);
                }
            }),
        ),
        (
            0.1,
            Until(
                "the reply copies whole, each block once and in order",
                |_, _| {
                    let c = crate::select::copied();
                    let at = |n: usize| c.find(&format!("Part {n}: the stage_{n} path"));
                    (1..=30)
                        .all(|n| c.matches(&format!("Part {n}: the stage_{n} path")).count() == 1)
                        && (1..30).all(|n| at(n) < at(n + 1))
                        && c.contains("stage_1 path\n\nThe 1th stage")
                },
                2.,
            ),
        ),
        (0.1, Key("escape")),
        // Find reaches a block far above, in a row of its own.
        (0.2, Key("cmd-f")),
        (0.1, Keys("the newest is checked first")),
        (
            0.1,
            Until(
                "Find counts a match in each block",
                |d, _| find_at(d).is_some_and(|(_, n)| n == 30),
                2.,
            ),
        ),
        (0.1, Key("cmd-a")),
        (0.1, Keys("Part 2:")),
        (
            0.3,
            Until(
                "one match, far above",
                |d, _| find_at(d).is_some_and(|(_, n)| n == 1),
                2.,
            ),
        ),
        (0.5, Call(look_shown)),
        (
            0.1,
            Until("is brought into view, in its block's row", shown, 0.5),
        ),
        (0.1, Key("escape")),
        // Once it's an older turn, its actions show while the pointer is on
        // any of its blocks, not only the last.
        (0.2, Type(Field::Composer, "and after that?")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "another turn ends",
                |d, _| done_saying(d, "Want me to start"),
                20.,
            ),
        ),
        (0.2, Call(to_long_reply)),
        (0.3, Hover("reply", 0.5, 0.5)),
        (
            0.2,
            Until(
                "pointing at its first block counts as pointing at the reply",
                |d, _| {
                    let long = long_reply(d);
                    long.is_some() && d.hover_row.map(|r| d.rows.item_of(r)) == long
                },
                2.,
            ),
        ),
    ]);
    v
}

/// Where the long reply is.
fn long_reply(d: &Dent) -> Option<usize> {
    current(d)?
        .items
        .iter()
        .position(|i| matches!(i, Item::Text(t) if t.contains("Part 30")))
}

/// Scroll so the long reply starts at the top.
fn to_long_reply(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    if let Some(ix) = long_reply(d) {
        d.scroll_to_item(ix, cx);
    }
}
