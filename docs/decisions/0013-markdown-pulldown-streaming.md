# 0013. Markdown: pulldown-cmark plus our own streaming layer

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

pulldown-cmark for parsing, with a stable-prefix streaming layer and a terminator for the unfinished tail, owned by dent.

## Why

Finished blocks must never move while streaming; the layer is small enough to own.

## Consequences

Covered by every-prefix streaming tests.

Confirmed in [spike 04](../spikes/04-markdown-math-mermaid.md): 6.66 million every-byte steps over real transcripts with no committed block changing; the terminator brings flicker from 32% of messages to 0.25%.
