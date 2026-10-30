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

/// A reply drawn a row per block, pushed at its edges: folded into its
/// finished turn (folding on, as it is by default), the session left and
/// come back to while it streams, zoomed mid-stream, read back mid-stream.
pub(super) fn rows() -> Vec<(f32, Act)> {
    let mut v = vec![(
        0.1,
        Call(|d, _, cx| {
            d.fold_turns = true;
            cx.notify();
        }),
    )];
    v.extend(new_session("show me colour output please"));
    v.extend([(
        0.1,
        Until(
            "the first session's turn ends",
            |d, _| done_saying(d, "Tests ran"),
            15.,
        ),
    )]);
    v.extend(new_session("stream bench 30"));
    v.extend([
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.8, Until("a few blocks in", |d, _| d.rows.len() > 8, 10.)),
        // Away and back while it streams.
        (0.1, Key("ctrl-tab")),
        (0.05, Call(let_go)),
        (0.3, Until("on the other session", |d, _| !streaming(d), 2.)),
        (0.1, Key("ctrl-tab")),
        (0.05, Call(let_go)),
        (
            0.3,
            Until(
                "back, it's still a row per block and still at its end",
                |d, _| streaming(d) && d.rows.len() > 8 && d.transcript.is_following_tail(),
                3.,
            ),
        ),
        // Larger text mid-stream: everything measured again.
        (0.1, Key("cmd-=")),
        (0.5, Key("cmd--")),
        (
            0.3,
            Until(
                "zoomed and back, it keeps following",
                |d, _| streaming(d) && d.transcript.is_following_tail(),
                3.,
            ),
        ),
        // Reading back while it streams: what you read stays put as
        // blocks land below.
        (0.1, Wheel("transcript", 400.)),
        (0.3, Call(save_scroll)),
        (
            1.0,
            Until(
                "reading back mid-stream, the chat stays still",
                |d, _| streaming(d) && !d.transcript.is_following_tail() && chat_still(d),
                0.5,
            ),
        ),
        (0.1, Call(to_end)),
        (
            0.1,
            Until(
                "the long reply finishes",
                |d, _| done_saying(d, "start with the first"),
                300.,
            ),
        ),
        // Reading partway down a reply, a row above it changing (here the
        // message that asked for it) leaves what you read where it is.
        (0.3, Call(read_mid_reply)),
        (0.3, Call(save_scroll)),
        (0.1, Call(toggle_first_message)),
        (
            0.5,
            Until(
                "a change above where you read doesn't move it",
                |d, _| chat_still(d),
                0.5,
            ),
        ),
        (0.1, Call(toggle_first_message)),
        (
            0.5,
            Until("nor does it changing back", |d, _| chat_still(d), 0.5),
        ),
        (0.1, Call(to_end)),
        // Narration of several blocks folds away with the turn's work.
        (0.2, Type(Field::Composer, "folded bench")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "the narrated turn ends folded",
                |d, _| {
                    done_saying(d, "start with the first")
                        && d.turn_folds
                            .iter()
                            .any(|f| matches!(f, crate::chat::TurnFold::Head { open: false, .. }))
                },
                30.,
            ),
        ),
        (
            0.1,
            Until(
                "its narration, three blocks, folds with the work",
                |d, _| narration_rows(d).is_some_and(|(n, hidden)| n == 3 && hidden),
                2.,
            ),
        ),
        // Find opens the fold to reach the narration's last block.
        (0.2, Key("cmd-f")),
        (0.1, Keys("narwhal marker")),
        (
            0.3,
            Until(
                "Find finds it in the folded narration",
                |d, _| find_at(d).is_some_and(|(_, n)| n == 1),
                2.,
            ),
        ),
        (
            0.3,
            Until(
                "and opens the turn to show it",
                |d, _| narration_rows(d).is_some_and(|(_, hidden)| !hidden),
                2.,
            ),
        ),
        (0.5, Call(look_shown)),
        (0.1, Until("the match is on screen", shown, 0.5)),
        (0.1, Key("escape")),
    ]);
    v
}

/// Read from partway into a block of the long reply.
fn read_mid_reply(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    if let Some(ix) = long_reply(d) {
        let row = d.rows.of(ix).start + 6;
        d.transcript.pause_following_tail();
        d.transcript.scroll_to(ListOffset {
            item_ix: row,
            offset_in_item: px(7.),
        });
        cx.notify();
    }
}

/// Open the session's first message's fold, or close it: its row changes.
fn toggle_first_message(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    let key = current(d).and_then(|s| match s.items.first() {
        Some(Item::User { id, .. }) => Some(crate::data::Fold::Message(id).key()),
        _ => None,
    });
    if let Some(key) = key {
        d.toggle_fold(&key);
        cx.notify();
    }
}

/// The narration's rows, and whether its turn's fold hides them.
fn narration_rows(d: &Dent) -> Option<(usize, bool)> {
    let s = current(d)?;
    let ix = s
        .items
        .iter()
        .position(|i| matches!(i, Item::Text(t) if t.contains("narwhal")))?;
    let fold = d.turn_folds.get(ix).copied()?;
    Some((
        d.rows.of(ix).len(),
        matches!(
            fold,
            crate::chat::TurnFold::Hidden | crate::chat::TurnFold::Head { open: false, .. }
        ),
    ))
}

/// The daemon killed while a reply streams: the app starts another, and
/// the reply finishes (as in `checks`, alone, for changes to how the
/// daemon starts).
pub(super) fn crash_mid_reply() -> Vec<(f32, Act)> {
    let mut v = new_session("a long answer please");
    v.extend([
        (
            0.1,
            Until("streams before the crash", |d, _| streaming(d), 10.),
        ),
        (0.1, Call(|d, _, _| d.drops = 0)),
        (0.1, Call(kill_daemon)),
        (
            0.1,
            Until("the app notices the daemon went", |d, _| d.drops > 0, 5.),
        ),
        (
            0.1,
            Until(
                "it starts a new one and the reply still finishes",
                |d, _| d.connection.is_ok() && done_saying(d, "That's the plan"),
                20.,
            ),
        ),
    ]);
    v
}
