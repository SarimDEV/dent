//! One swipe, one axis.
//!
//! GPUI hands a wheel event to every scroller under the pointer, so where
//! scrollers nest, each took its own share of a diagonal trackpad swipe: the
//! diff's lines drifted sideways as they scrolled down, a sideways swipe over
//! a code block nudged the chat, and a tall table's own scrolling kept its
//! sideways scroll from moving at all. As in macOS scroll views, a swipe here
//! locks to the axis it starts along and only unlocks once it clearly turns;
//! a mouse wheel moves whichever way it moved most.
//!
//! Every scroller follows that one rule. A plain scrolling `div` does it with
//! `restrict_scroll_to_axis` (GPUI's own lock, the same algorithm as here;
//! `scrollers_lock_to_one_axis` checks none is left without it). Where
//! scrollers nest, `lock` laid under them takes the wheel first and moves
//! them itself.

use std::cell::RefCell;

use gpui_kit::*;

thread_local! {
    /// The swipe in progress. Filtering an event twice gives the same answer,
    /// so every `lock` under the pointer can ask.
    static SWIPE: RefCell<OngoingScroll> = RefCell::default();
}

/// This wheel event's movement, held to the axis its swipe is on.
fn delta(e: &ScrollWheelEvent, window: &Window) -> Point<Pixels> {
    let mut d = e.delta.pixel_delta(window.line_height());
    if e.delta.precise() {
        SWIPE.with(|s| s.borrow_mut().filter(&mut d, e.touch_phase));
    } else if d.x.abs() > d.y.abs() {
        d.y = px(0.);
    } else {
        d.x = px(0.);
    }
    d
}

/// Takes the wheel over its bounds for the scrollers it names: `down`, a
/// list scrolling up and down, and `side`, a scroll handle moving sideways.
/// Sideways movement goes to `side` and nowhere else. Up-and-down movement
/// goes to `down` until it reaches an end, then carries on to whatever
/// scrolls around it (the chat, past a tall table); with no `down` it always
/// carries on. Lay it under what it covers, before the scrollers, so its
/// capture handler runs ahead of theirs; the `div` behind `side` still needs
/// `restrict_scroll_to_axis` for the movement that carries on.
pub fn lock(down: Option<ListState>, side: Option<ScrollHandle>) -> impl IntoElement {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || !hitbox.should_handle_scroll(window) {
                    return;
                }
                let d = delta(e, window);
                let mut kept = false;
                if let Some(side) = side.as_ref().filter(|_| d.x != px(0.)) {
                    let (o, max) = (side.offset(), side.max_offset());
                    side.set_offset(point((o.x + d.x).clamp(-max.x, px(0.)), o.y));
                    kept = true;
                }
                if let Some(down) = &down {
                    let top = -down.scroll_px_offset_for_scrollbar().y;
                    let max = down.max_offset_for_scrollbar().y;
                    if (d.y < px(0.) && top < max - px(0.5)) || (d.y > px(0.) && top > px(0.5)) {
                        down.scroll_by(-d.y);
                        kept = true;
                    }
                    // GPUI's list merges a frame's wheel deltas by sign, and
                    // 0.0 counts as positive: the zero-movement event that
                    // ends a swipe would cancel that frame's scrolling.
                    if d == Point::default() {
                        kept = true;
                    }
                }
                if kept {
                    cx.stop_propagation();
                    window.refresh();
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    /// Every scrolling `div` locks a swipe to one axis, or says why not.
    /// GPUI's default turns a wheel's one axis into the other where only one
    /// scrolls, and lets nested scrollers share a swipe.
    #[test]
    fn scrollers_lock_to_one_axis() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut bad = Vec::new();
        let mut files = vec![dir];
        while let Some(f) = files.pop() {
            if f.is_dir() {
                files.extend(std::fs::read_dir(&f).unwrap().map(|e| e.unwrap().path()));
                continue;
            }
            if f.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&f).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for (i, l) in lines.iter().enumerate() {
                let t = l.trim();
                if !(t.starts_with(".overflow_") && t.ends_with("scroll()")) {
                    continue;
                }
                let near = &lines[i.saturating_sub(3)..(i + 5).min(lines.len())];
                if !near
                    .iter()
                    .any(|n| n.contains(".restrict_scroll_to_axis()") || n.contains("swipe: "))
                {
                    bad.push(format!("{}:{}", f.display(), i + 1));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "scrollers without .restrict_scroll_to_axis() (or a `swipe: ` note saying why): {bad:#?}"
        );
    }
}
