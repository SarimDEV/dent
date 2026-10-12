# 0024. Web core: a separate wasm-bindgen facade

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

The web client uses a separate wasm-bindgen facade crate over the same I/O-free client core. Transport may be a WebSocket through the hub.

## Why

Same sync rules and protocol in one place.

## Consequences

The web UI is scoped to chat, approvals, docs and diffs.

[Spike 07](../spikes/07-client-core.md): the facade is about 150 lines. Notes cross as one JSON string per animation frame, and bytes as `Uint8Array`. The release build is 919 KB of wasm, 290 KB gzipped. The page's JavaScript host is shared with a Node test that runs the same scenarios.
