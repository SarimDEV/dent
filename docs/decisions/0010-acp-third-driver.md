# 0010. ACP as the third driver

Status: accepted (2026-10-11) · Spec: [daemon.md](../daemon.md)

## Decision

One Agent Client Protocol v1 client, crate pinned, brings in every other agent.

## Why

ACP has session resume, usage updates and a 1.0 Rust SDK.

## Consequences

Support v1 and v2 side by side when v2 lands.
