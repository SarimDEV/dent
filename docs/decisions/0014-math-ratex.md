# 0014. Math: RaTeX (KaTeX on web)

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

RaTeX, pure Rust, renders TeX on desktop and iOS; KaTeX on the web.

## Why

No script engine or web view needed in the normal case.

## Consequences

Single-maintainer dependency; fallback is web-view KaTeX. Confirmed in Phase 1.

Confirmed in [spike 04](../spikes/04-markdown-math-mermaid.md): every fixture typesets in about 70 µs and the display list is drawn natively in GPUI.
