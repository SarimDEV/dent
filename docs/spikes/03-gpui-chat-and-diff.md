# Spike 03: GPUI chat list and diff view at 100,000 events

Date: 2026-10-11 · gpui-kit / gpui-component 0.7.1 on GPUI snapshot `gpui-pre =0.3.8` · Apple Silicon, debug build (dependencies at opt-level 2, GPUI at 3) · Code: [spikes/gpui-chat](../../spikes/gpui-chat)

Question: can GPUI on pinned gpui-component hold the frame budget for a 100,000-event chat, a 100,000-line diff and a 1,000-session sidebar, in the Field Notes design?

**Answer: yes, with lots of room.** Every scripted interaction stayed under the 120 Hz budget (8.3 ms) at p99, with zero frames over 8.3 ms in the measured phases. All data is generated locally, so no agent is called.

![Paper, chat at the bottom of a 100k-event session](img/03-chat-bottom-paper.jpg)

![Carbon, chat scrolled up](img/03-chat-scrolled-carbon.jpg)

## Results

Frame cost is GPUI's own `Window::draw` time from its frame trace (`gpui::profiler`). Every phase redraws the whole window every frame.

| Phase | Frames | p50 | p99 | Max | Over 8.3 ms |
| --- | --- | --- | --- | --- | --- |
| Scroll chat (wheel, 60 px/frame, ~250 rows) | 600 | 4.5 ms | 5.4 ms | 5.6 ms | 0 |
| Jump to random unmeasured rows | 120 | 5.3 ms | 5.8 ms | 5.8 ms | 0 |
| Stream into the last message (a word per frame) | 600 | 4.7 ms | 5.3 ms | 6.6 ms | 0 |
| Scroll diff (100k lines, 80 px/frame) | 600 | 4.6 ms | 5.2 ms | 7.5 ms | 0 |
| Scroll sidebar (1,000 sessions) | 300 | 4.6 ms | 5.3 ms | 5.6 ms | 0 |

| Budget | Target (spec) | Measured |
| --- | --- | --- |
| Open a 100k-event session (fresh list state to drawn frame) | < 50 ms | **18 ms** |
| Cold start to first frame | < 300 ms | **169 ms** with tiny data; 430 ms with 100k fake rows, of which 237 ms is generating them, so ~190 ms |
| Memory | flat per session | 99 MB baseline RSS; +74 MB holding 100k events and 100k diff lines as strings |

Paper and Carbon measure the same. One frame in warm-up (~22 ms) is the first draw with font and glyph atlas setup.

## What we learned

- **Pinning:** gpui-component pins GPUI itself (`gpui-pre =0.3.8`, a snapshot of Zed's GPUI crates), so dent pins `gpui-kit =0.7.1` and inherits that exact snapshot. GPUI must be built at `opt-level = 3` even in dev builds; set in the workspace `Cargo.toml`.
- **Builds:** the first GPUI build is 2 min 50 s; after that a spike rebuild is a few seconds. That's fine for the inner loop.
- **Chat list:** GPUI's `list` with `ListState` (variable heights, bottom-aligned) only measures what it shows, so opening 100k rows is constant-time. Streaming uses `splice(last..last+1, 1)` to re-measure just the growing message, and finished rows never move.
- **Gotcha:** `ListState::scroll_by` does nothing on a bottom-aligned list that is following its tail (the position is re-derived from the end each frame). Real wheel input works. The bench drives scrolling with synthetic `ScrollWheelEvent`s through `Window::dispatch_event`, which also proves the automation-socket idea: input can be scripted precisely.
- **Diff view:** a plain `uniform_list` with fixed 20 px rows handles 100k lines trivially. Editing the modified side (gpui-component's editor) is not tested yet; that belongs to the Review phase.
- **Fonts:** fonts load from bytes with `text_system().add_fonts`, so the app bundles them (`assets/fonts/`, SIL OFL). The spike later compared five pairings with `--fonts` and `--font-dir`; Book won (decision 0032). Variable fonts render only their default instance, so bundle static weights.
- **Status squares:** distinct patterns work at 12 px: working = 2 px dither, needs you = solid, done = four dots, waiting and idle = outlines.
- **Screenshots:** `screencapture -R` on a fixed window rectangle is enough for the spike. Real screenshot tests should render offscreen.

## Motion and the design system (added after the first pass)

The spike was rebuilt on the tokens in [docs/design-system.md](../design-system.md): one 44 px header height across all panels, five type sizes, a radius scale with nesting, ink-alpha state colours, and the motion vocabulary below. Every motion is opacity, offset, colour or flat quads.

- **Arrive:** new rows fade in and rise 6 px over 180 ms.
- **Stream:** each new word fades in over 180 ms; a block cursor in the working colour breathes; "Thinking" shimmers before the first word.
- **Resolve:** approval cards arrive through the 8×8 Bayer dither (240 ms, 3 px cells).
- **Fold:** an approved card fades its content and collapses to one line, which then shows the tool call.
- **Confirm:** a finished tool's done square pops from 60% on a spring (0.3 s, bounce 0.2).
- **Progress:** a running tool shows a hairline fill towards 90% of its expected time and a counting timer.
- **Tick:** event, usage and blocked counts roll digit by digit.
- **Compact:** the serif title springs from 28 to 20 px while you read history and a live pill slides in. The chat uses GPUI's `FollowMode::Tail`.
- **Theme switch:** every colour tweens from Paper to Carbon (or back) over 200 ms, and the grain crossfades.
- **Drift:** the working square's dither ripples outward, stepped at 8 fps.

Frame draw times per scripted scene (`--demo NAME` prints them; debug build, 1360 × 780 window):

| Scene | Frames | p50 | p95 | Over 8.3 ms |
| --- | --- | --- | --- | --- |
| Stream (thinking, words, progress, tick) | 668 | 4.5 ms | 4.8 ms | 1 (first frame) |
| Approval (resolve, press, fold) | 468 | 4.6 ms | 5.0 ms | 3 |
| Scroll (flick, compact, live) | 432 | 4.5 ms | 5.0 ms | 1 (first frame) |
| Theme switch, twice | 323 | 4.3 ms | 4.7 ms | 4: first frame, plus three ~8.7 ms frames on the first switch only |

- **The dither does not scale to a full window.** As the theme-switch effect it was 30,000 quads a frame at 6 px cells and up to 55 ms per frame. Merging runs of covered cells helped but stayed over budget below 12 px cells. It is now kept for small surfaces, and the theme switch is a colour tween.
- **The first theme switch costs about 4 ms extra for three frames,** and later switches don't. Removing the grain made no difference, so it looks like a one-time glyph cost for the other text colour. Under the 60 Hz budget; left alone.
- **No transforms on divs in this GPUI:** presses inset a background layer by 1 px instead of scaling, and rises use relative offsets, so neighbours never move. GPUI does provide analytic springs, which the title and Confirm use.
- **Rows in motion are remeasured each frame** (`ListState::splice` on just those rows); settled rows stay cached.

`--demo stream|approval|scroll|theme|rest [--record FILE.mov]` plays a scripted scene. `scripts/record-demos.sh` records every scene with ffmpeg (pointer hidden with one synthetic keypress, since this macOS ignores ffmpeg's hide-cursor flag) and writes X-ready MP4s, posters and stills to the gitignored `media/` folder. The window stays open 1.5 s after the recorder stops, so a clip can never capture what is behind the window.

## Not covered

- Markdown rendering inside messages (spike 04); the editable diff; Linux and Windows; a release build (debug was already well inside budget).

## Changes to the spec

- Decision 0001 now records the exact pins (`gpui-kit =0.7.1` → `gpui-pre =0.3.8`).
- New decision 0032: bundle the Field Notes fonts.
- docs/desktop.md: the frame budget is met with headroom on the spike's synthetic load; the scroll gotcha is recorded for the automation socket.

## How to run

```
just dev spike-gpui-chat               # Paper
just dev spike-gpui-chat --dark        # Carbon
just dev spike-gpui-chat --bench --shots /tmp/shots   # scripted run, JSON report, screenshots
```
