# 0033. Connectivity for now: WebSocket over the tailnet or LAN; hub on hold

Status: accepted (2026-10-11) · Supersedes [0002](0002-iroh-with-n0-relays.md) · Spec: [hub.md](../hub.md)

## Decision

Clients connect straight to each daemon with one WebSocket over TCP, at its Tailscale or LAN address. There is no hub and no relay for now. Every connection is encrypted and authenticated by dent itself: Noise XX with each device's own key, on Tailscale too ([spike 05](../spikes/05-direct-links.md)). A LAN machine is paired once with a 6-digit code it shows, through SPAKE2 bound to the channel. A machine on the same Tailscale account is trusted with no code.

The hub records (0020, 0021, 0025, 0026) stand but are deferred. Their work moves out of the near phases.

## Why

A hub decides accounts, sign-in and relays before dent is useful to its first user, and working around it would slow everything else. Tailscale already joins the machines that matter. A WebSocket is the simplest transport that every client can open, including the browser natively, and the protocol stays transport-agnostic so a hub or QUIC can come back later.

## Consequences

- The daemon listens on loopback and its Tailscale address by default, never on every interface. A LAN address is an explicit setting.
- Spike 05 measures this path: connecting, pairing, reconnecting after sleep or a network change, and replay throughput.
- LAN machines are found with Bonjour; Tailscale machines through the local Tailscale app.
- Three ways to connect: LAN, Tailscale, and later a relay (free for up to 3 devices, paid beyond).
- Machines outside the tailnet, iPhone push and the browser away from home need the hub or another transport later.
