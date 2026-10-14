# 0039. Diagrams rasterise with resvg

Status: accepted (2026-10-13) · Spec: [chat-rendering.md](../chat-rendering.md)

## Decision

resvg (pinned `=0.46.0`, only its `text` feature) turns the SVG merman makes into a picture, off the UI thread, at twice its size. The picture becomes a GPUI `RenderImage`. resvg draws labels from a font database holding only the bundled IBM Plex Sans.

## Why

GPUI already uses resvg to draw SVG icons, so this adds no new kind of code, only a newer version of it. It is pure Rust, renders every diagram type merman produces ([spike 04](../spikes/04-markdown-math-mermaid.md)), and takes about 3 ms for a diagram at 2x.

## Consequences

Two resvg versions sit in the tree until GPUI moves to 0.46. A diagram is a bitmap, so zooming past 2x softens it. It is rendered again when the theme changes.
