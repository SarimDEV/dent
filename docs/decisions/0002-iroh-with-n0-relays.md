# 0002. Connectivity: iroh, on n0's free public relays for now

Status: superseded by [0033](0033-direct-websocket-hub-on-hold.md) (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

All client–daemon traffic is one protocol over QUIC via iroh 1.0, a stream per subscription. Use n0's free public relays while building and sharing with a few people; move to own relays on Hetzner (or n0's paid tier) before public launch.

## Why

Stable wire format and API, NAT traversal, Swift bindings, open-source relay. Public relays cost nothing during development.

## Consequences

Relay addresses are a setting, never hard-coded. Tailscale/LAN paths are used automatically when reachable. The browser client may use a hub WebSocket because iroh in the browser is relay-only.
