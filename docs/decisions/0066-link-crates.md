# 0066. The link's dependencies: snow, spake2 and tokio-tungstenite

Status: accepted (2026-10-20) · Builds on [0033](0033-direct-websocket-hub-on-hold.md), [0060](0060-remote-wire.md), [0061](0061-trust.md) · Spec: [daemon.md](../daemon.md#remote-devices)

## Decision

A new crate, `dent-link` (`crates/link`), holds the handshake, framing and pairing with no I/O, so the daemon and the client core share one implementation. It depends on, exact-pinned as in spikes 05 and 07:
- `snow` 0.9.6 (Noise XX, `Noise_XX_25519_ChaChaPoly_BLAKE2s`);
- `spake2` 0.4.0 (pairing with a code);
- `hmac` 0.12.1 and `sha2` 0.10.9 (binding pairing to the channel; key fingerprints);
- `getrandom` 0.2.17 (codes; the `js` feature on wasm);
- `hex` 0.4.3.

The daemon adds `tokio-tungstenite` 0.26.2 and `futures-util` 0.3.34 for the WebSocket it listens on, plus `hex`.

Tailscale's LocalAPI is reached with a few lines of our own over its Unix socket or loopback port, not an HTTP client crate.

## Why

These are the versions spikes 05, 07 and 12 ran on real links, natively, in wasm and on iOS. They are small, widely used and do one thing each. Keeping the handshake I/O-free lets the iPhone and web clients use it unchanged ([spike 07](../spikes/07-client-core.md)).

## Consequences

- `client-core` will depend on `dent-link` when it gains connections (the next stage-1 PR). That stays within its rule of no I/O and no runtime.
- Upgrades follow [0029](0029-pin-everything.md).
