# 0023. iOS core: UniFFI built by our own script

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

UniFFI pinned, built by our own script (not cargo-swift), with a hand-written Swift wrapper for cancellation and main-actor safety.

## Why

Control over the build and a safe Swift surface.

## Consequences

The client core stays free of I/O so it can be rebound.
