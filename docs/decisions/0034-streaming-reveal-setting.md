# 0034. How streaming text appears is a setting; word by word by default

Status: accepted (2026-10-12) · Spec: [chat-rendering.md](../chat-rendering.md)

## Decision

How a streaming reply appears is a reading preference with three choices:

- **Word by word** (the default): text appears as it arrives.
- **Word by word, smoothed:** whole words at a steady pace, never more than about 0.25 s behind, with new words fading in over 180 ms.
- **Block by block:** each paragraph, list, table or code block appears whole once the next one has started, fading in over 300 ms.

The setting changes only when text becomes visible. Parsing, committing blocks and the event log are the same for all three.

## Why

The owner watched the three side by side in spike 04 (`--compare`). Word by word felt fastest. Block by block was the easiest to read. Smoothed was pleasant but adds a little latency. Most people will judge speed first, so word by word is the default, and the others are a setting away.

Cost is not a reason to choose. Per reply, word by word redraws once per burst (107 for the test reply). Block by block redraws once per block, plus its fade frames. Smoothed redraws every frame while revealing (831 at 120 Hz). Even the most expensive keeps a fraction of one core busy, and only while streaming.

## Consequences

- The choice is per device and travels with the user's other preferences. The transcript never depends on it.
- Reduced motion drops the fades.
- In-app search and copy see all text that has arrived, whatever is visible.
