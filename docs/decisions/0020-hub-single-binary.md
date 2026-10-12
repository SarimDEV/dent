# 0020. Hub: one Rust binary with SQLite and the iroh relay

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

The hub is one Rust binary with SQLite and an embedded iroh relay; the same build is hosted and self-hosted.

## Why

One codebase instead of a hosted variant plus a self-hosted one.

## Consequences

Must run in two regions with a status page before launch.
