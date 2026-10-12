# 0015. Diagrams: merman

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

merman, a headless Rust port of Mermaid producing SVG.

## Why

Mermaid.js needs a browser to measure text.

## Consequences

Single-maintainer dependency; fallback is an offline web view running mermaid.js. Confirmed in Phase 1.

Confirmed in [spike 04](../spikes/04-markdown-math-mermaid.md): all 22 diagram types render headless in Field Notes, SVG in about 1 ms and a 2x raster in about 3 ms.
