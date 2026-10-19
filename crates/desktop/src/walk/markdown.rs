use super::*;

pub(super) type Saved = (Option<ListOffset>, Option<Option<ClipboardItem>>);

thread_local! {
    /// The conversation's scroll position before a swipe, and the clipboard
    /// before a copy, to compare against and put back.
    pub(super) static SAVED: std::cell::RefCell<Saved> = Default::default();
}

pub(super) fn save_scroll(d: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
    let top = d.transcript.logical_scroll_top();
    SAVED.with(|s| s.borrow_mut().0 = Some(top));
}

pub(super) fn chat_still(d: &Dent) -> bool {
    let top = d.transcript.logical_scroll_top();
    SAVED.with(|s| {
        s.borrow().0.is_some_and(|was| {
            was.item_ix == top.item_ix && (was.offset_in_item - top.offset_in_item).abs() < px(0.5)
        })
    })
}

pub(super) fn table_top(d: &Dent) -> f32 {
    d.mdv.table_scroll().map_or(0., |(top, _)| top)
}

/// Maths, a diagram and a tall table in a reply: drawn, scrolled, copied.
pub(super) fn rich_reply() -> Vec<(f32, Act)> {
    let mut v = new_session("show me some math, a diagram and a big table");
    v.extend([
        (
            0.1,
            Until(
                "a reply with maths, a diagram and a big table finishes",
                |d, _| done_saying(d, "all sixty"),
                20.,
            ),
        ),
        (0.3, Call(to_reply)),
        (
            0.1,
            Until(
                "display and inline maths are drawn",
                |d, _| drawn(d, "md-math").is_some() && drawn(d, "md-inline-math").is_some(),
                5.,
            ),
        ),
        (
            0.1,
            Until(
                "a diagram is drawn off the UI thread",
                |d, _| drawn(d, "md-diagram").is_some_and(|b| b.size.width > px(100.)),
                10.,
            ),
        ),
        (0.4, Shot("maths")),
        (0.2, Call(to_end)),
        (
            0.3,
            Until(
                "a tall table shows its header and 10½ rows",
                |d, _| {
                    drawn(d, "md-table-body").is_some_and(|b| {
                        (b.size.height.as_f32() - 10.5 * crate::markdown::ROW_H).abs() < 1.
                    }) && d.mdv.table_scroll().is_some_and(|(_, max)| max > 100.)
                },
                5.,
            ),
        ),
        (0.3, Shot("table")),
        (0.1, Call(save_scroll)),
        (0.1, Wheel("md-table-body", -90.)),
        (0.1, Wheel("md-table-body", -90.)),
        (
            0.2,
            Until(
                "the wheel scrolls the table, not the chat",
                |d, _| table_top(d) > 100. && chat_still(d),
                3.,
            ),
        ),
        (0.1, Call(save_scroll)),
        (0.1, Wheel("md-table-body", 60.)),
        (
            0.2,
            Until(
                "scrolling back up moves the table before the chat",
                |d, _| (table_top(d) - 120.).abs() < 1. && chat_still(d),
                3.,
            ),
        ),
        (0.1, Wheel("md-table-body", 300.)),
        (0.1, Call(save_scroll)),
        (0.1, Wheel("md-table-body", 60.)),
        (
            0.2,
            Until(
                "at the table's top, the wheel carries on to the chat",
                |d, _| table_top(d) < 0.5 && !chat_still(d),
                3.,
            ),
        ),
        (0.2, Call(to_end)),
        (
            0.3,
            Call(|_, _, cx| {
                let was = cx.read_from_clipboard();
                SAVED.with(|s| s.borrow_mut().1 = Some(was));
            }),
        ),
        (0.1, Click("md-copy-csv")),
        (
            0.1,
            Until(
                "Copy CSV puts the whole table on the clipboard as CSV",
                |_, cx| {
                    let t = clipboard_text(cx);
                    t.starts_with("#,Event,Duration\n1,event number 1,1.1 ms\n")
                        && t.lines().count() == 61
                        && t.ends_with("60,event number 60,6.4 ms")
                },
                3.,
            ),
        ),
        (0.2, Shot("table-copied")),
        (0.1, Click("md-copy-md")),
        (
            0.1,
            Until(
                "Copy puts it on the clipboard as Markdown",
                |_, cx| {
                    let t = clipboard_text(cx);
                    t.starts_with("| # | Event | Duration |\n| ---: | --- | ---: |\n| 1 |")
                        && t.lines().count() == 62
                },
                3.,
            ),
        ),
        (
            0.1,
            Call(|_, _, cx| {
                // Put back whatever was on the clipboard before.
                if let Some(was) = SAVED.with(|s| s.borrow_mut().1.take()) {
                    cx.write_to_clipboard(
                        was.unwrap_or_else(|| ClipboardItem::new_string(String::new())),
                    );
                }
            }),
        ),
    ]);
    v
}

/// Only the rich reply, for working on how Markdown renders.
pub(super) fn markdown() -> Vec<(f32, Act)> {
    let mut v: Vec<(f32, Act)> = vec![(
        0.,
        Until(
            "connects to the daemon, starting it",
            |d, _| d.connection.is_ok(),
            15.,
        ),
    )];
    v.extend(rich_reply());
    v
}
