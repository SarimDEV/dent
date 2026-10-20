# 0057. One swipe moves one axis, in every scroller

Status: accepted (2026-10-20) · Spec: [desktop.md](../desktop.md#diffs-and-staging), [chat-rendering.md](../chat-rendering.md#sizes-and-overflow)

## Decision

A trackpad swipe locks to the axis it starts along and only unlocks once it clearly turns; a mouse wheel moves whichever way it moved most. This holds everywhere something scrolls:
- **A plain scroller** (`overflow_*_scroll`) also sets `restrict_scroll_to_axis`, GPUI's own lock. A unit test (`swipe::scrollers_lock_to_one_axis`) fails on one without it. A scroller that means to turn a wheel sideways, as the tab strip does, says why in a `swipe:` note.
- **Nested scrollers** (the diff and text views, wide blocks and tall tables in the chat) lay `swipe::lock` under them. It takes the wheel first and moves them itself: sideways movement goes to the sideways scroller and nowhere else, and up-and-down movement goes to the inner list until it reaches an end, then on to whatever scrolls around it. It replaces the chat table's own handoff.

## Why

The owner (2026-10-20): scrolling the diff with a trackpad drifted right on the way down and left on the way up. GPUI gives a wheel event to every scroller under the pointer, so where scrollers nest, each took its own part of a diagonal swipe. The same cause turned up elsewhere: a sideways swipe over a code block nudged the chat, a tall table's sideways scroll barely moved, a mouse wheel over the diff scrolled it sideways, and a sideways swipe in a doc or menu scrolled it up or down. Locking to the starting axis is how macOS scroll views behave. One rule in one place, plus a test, stops each new scroller from bringing the bug back.
