# 0030. Core runtime dependencies: tokio, serde, anyhow

Status: accepted (2026-10-11) · Spec: [building-with-agents.md](../building-with-agents.md)

## Decision

tokio for async processes and I/O, serde and serde_json for the wire formats, anyhow for errors in binaries (thiserror in libraries when needed). All exact-pinned in `[workspace.dependencies]`.

## Why

The standard, well-maintained choices; iroh and most of the ecosystem already use tokio.

## Consequences

`client-core` must not use tokio's I/O directly (it stays I/O-free behind traits); it may use runtime-agnostic types.
