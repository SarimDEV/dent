# 0041. Diagram labels measured with rustybuzz

Status: accepted (2026-10-13) · Spec: [chat-rendering.md](../chat-rendering.md)

## Decision

rustybuzz (pinned `=0.20.1`) shapes each Mermaid label in the bundled IBM Plex Sans to measure its width for merman's layout. Widths are cached per label.

## Why

merman sizes boxes from the text widths it is given. Measuring with the face resvg then draws in keeps labels inside their boxes ([spike 04](../spikes/04-markdown-math-mermaid.md)). rustybuzz is pure Rust and is already in the tree through resvg.

## Consequences

Labels are measured and drawn in Plex Sans only. A theme that changes the interface face still draws diagrams in Plex Sans until the diagram fonts follow the theme.
