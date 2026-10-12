# 0009. Codex: `codex app-server` with generated types

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

Drive `codex app-server` over stdio (JSON-RPC), v2 methods only, no experimental API. Rust types are generated from its JSON schema dump at a pinned Codex version.

## Why

It is Codex's own rich-client interface, including approvals and rate-limit notifications. Generated types make protocol drift visible.

## Consequences

Every Codex bump produces a schema diff for review.
