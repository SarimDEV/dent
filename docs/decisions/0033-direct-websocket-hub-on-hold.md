# 0033. Connectivity for now: WebSocket over the tailnet or LAN; hub on hold

Status: accepted (2026-10-11) · Supersedes [0002](0002-iroh-with-n0-relays.md) · Spec: [hub.md](../hub.md)

## Decision

Clients connect straight to each daemon with one WebSocket over TCP, at its Tailscale or LAN address. There is no hub and no relay for now. Tailscale provides reachability and transport encryption; dent adds device pairing on top, so a new device gets a token from a short code the daemon shows and the daemon rejects anything without one.

The hub records (0020, 0021, 0025, 0026) stand but are deferred. Their work moves out of the near phases.

## Why

A hub decides accounts, sign-in and relays before dent is useful to its first user, and working around it would slow everything else. Tailscale already joins the machines that matter. A WebSocket is the simplest transport that every client can open, including the browser natively, and the protocol stays transport-agnostic so a hub or QUIC can come back later.

## Consequences

- The daemon listens on loopback and its Tailscale address by default, never on every interface. A LAN address is an explicit setting.
- Spike 05 measures this path: connecting, pairing, reconnecting after sleep or a network change, and replay throughput.
- How LAN-only use (no Tailscale) is encrypted is settled in spike 05.
- Machines outside the tailnet, iPhone push and the browser away from home need the hub or another transport later.
