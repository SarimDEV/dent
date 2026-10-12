# 0023. iOS core: UniFFI built by our own script

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

UniFFI pinned, built by our own script (not cargo-swift), with a hand-written Swift wrapper for cancellation and main-actor safety.

## Why

Control over the build and a safe Swift surface.

## Consequences

The client core stays free of I/O so it can be rebound.

[Spike 07](../spikes/07-client-core.md) confirmed it. UniFFI 0.32.2 proc macros, with the core's records and enums derived behind a feature, and a bindgen binary in the facade crate that reads the built library. A build script makes the XCFramework, and xcodegen makes the project. Because the core is synchronous with no I/O, no async crosses FFI. The Swift host owns the sockets and the timer on one serial queue, which gives the cancellation and main-actor safety asked for here without UniFFI's async support.
