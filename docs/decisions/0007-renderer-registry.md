# 0007. Chat rendering through a renderer registry

Status: accepted (2026-10-11) · Spec: [chat-rendering.md](../chat-rendering.md)

## Decision

Messages parse into a typed block tree in the client core. Each block type has one self-contained renderer registered in one place, implementing the renderer contract (match, streaming state, final render, fallback, header, plain text, fixtures, budget).

## Why

Adding a content type touches one place, and testing (streaming prefixes, fuzz, goldens, perf) applies automatically to every registered renderer.

## Consequences

A block never disappears: any failure falls back to a code block. iPhone and web render the same tree with their own renderers.
