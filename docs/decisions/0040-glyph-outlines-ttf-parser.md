# 0040. KaTeX Size glyphs drawn from outlines with ttf-parser

Status: accepted (2026-10-13) · Spec: [chat-rendering.md](../chat-rendering.md)

## Decision

ttf-parser (pinned `=0.25.1`) reads glyph outlines from the KaTeX fonts that have no `m` glyph (KaTeX_Size1 to Size4), and the desktop draws those glyphs as paths. Every other KaTeX font loads into GPUI and is drawn as text.

## Why

GPUI skips any font without an `m` glyph, so big operators and stretchy delimiters fell back to a system face ([spike 04](../spikes/04-markdown-math-mermaid.md)). ttf-parser is small, has no dependencies of its own, and is already in the tree through GPUI's font stack.

## Consequences

Outlines are cached per glyph. The outline path only runs for fonts without an `m`, so if a later GPUI loads these fonts nothing needs to change.
