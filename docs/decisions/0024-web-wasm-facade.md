# 0024. Web core: a separate wasm-bindgen facade

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

The web client uses a separate wasm-bindgen facade crate over the same I/O-free client core. Transport may be a WebSocket through the hub.

## Why

Same sync rules and protocol in one place.

## Consequences

The web UI is scoped to chat, approvals, docs and diffs.
