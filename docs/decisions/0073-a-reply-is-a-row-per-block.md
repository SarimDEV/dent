# 0073. The chat's list draws a reply a row per block

Status: accepted (2026-10-22, owner: "build it out in a sustainable and extendable way") · Spec: [chat-rendering.md](../chat-rendering.md#streaming)

## Decision

- The chat's list rows are derived from the session's items in one place (`rows.rs`): an item each, except a reply, which is a row per top-level block. The list skips what's off screen and remembers row heights, so a long reply costs only what's on screen, and as it streams only its last block is measured again.
- Everything else still speaks of items: selection, copying, Find, folds, a turn's footer. Only the list and what scrolls it speak of rows, through `Rows` (`of_item`, `of_block`, `item_of`, and `Dent::item_row`, `Dent::top_item`).
- A reply's blocks keep their place in its selection (`select::part`), so it selects and copies as one message whether or not all its blocks are drawn. Find counts a reply's matches block by block, so a block drawn alone numbers its matches on from those before it, and a match is brought into view in its block's row.
- A block row's signature is the block's own (`markdown::block_sig`) and whether it ends the reply, never the whole reply's text, so earlier blocks aren't measured again as later ones arrive.

## Why

Measured on the release build with the `bench` walk (a 30-part, 60 KB reply streamed at the real agent's pace): per-frame work grew with the reply, from 5.8 ms to 11.3 ms p50 (p99 15 ms), because the reply was one row, laid out and built in full each frame, off-screen blocks included. A row per block holds it at 5.2–5.7 ms p50, p99 about 7 ms, however long the reply gets.

Drawing finished rows from a GPUI cached view was considered and not done: a cached view is reused only while its bounds stay put, and while a reply streams the chat follows its end, so every row moves every frame and the cache would never be hit. It would also have needed every row's look to be invalidated by hand, a standing rule for every future change. Teaching the reply row to skip its own off-screen blocks wasn't done either: GPUI can't measure a row's contents during layout, so its height would trail by a frame each time a block arrived.

## Consequences

- A new kind of item drawn whole needs nothing. Another that can grow long (a long message of yours, a tool's output) can be split the same way by giving it parts in `Rows::derive` and drawing a part in the row processor.
- What's drawn below an item (the gap to the next, a turn's footer) goes on its last row; what's drawn above it (the working line, a turn's line) on its first. `Row::first` and `Row::last` say which.
- An older turn's actions show while the pointer is on any block of its last reply (`Dent::hover_row`), since GPUI's hover groups don't reach across rows.
- Opening a session parses each of its replies once to know its blocks, where before only those on screen were parsed: about 14 µs per KB on the release build (a session with 2 MB of replies, under 30 ms, once; parsed replies are cached and the last turn's are parsed ahead off the UI thread).
- `DENT_WHOLE_REPLIES=1` draws each reply as one row again, to compare stills and timings against.
- The `bench` walk keeps the numbers honest and checks copying and Find across a reply's rows.
