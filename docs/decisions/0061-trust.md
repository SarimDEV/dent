# 0061. Trust: a pinned key from pairing, or a strict live Tailscale check; only authenticated removal

Status: proposed (2026-10-20) · confirmed by [R-B1–B9](../remote-tests.md#b-trust-and-identity) · Builds on [0033](0033-direct-websocket-hub-on-hold.md), [spike 05](../spikes/05-direct-links.md) · Plan: [remote-plan.md](../remote-plan.md)

## Decision

A device is trusted by a machine in one of two ways. Nothing else grants trust: not discovery, an address, a name or a probe.

### 1. Paired

The 6-digit code and SPAKE2 of 0033, bound to the Noise channel. Both sides pin each other's key, and paired trust holds on every route. Pairing fixes:
- **Only a real wrong guess counts.** The version range and the responder's key are checked before the exchange counts as a guess, and a dropped connection mid-exchange doesn't burn one. Only a completed exchange with a wrong code counts.
- **A retry after an interruption resumes the same pending pairing,** never adds a second.
- **Each outcome has its own code:** wrong code, expired, closed, paused (with the time left), version mismatch (with both versions), different machine, already paired. The machine showing the code also shows its dent version.
- **A too-old machine** that speaks the maintenance subset ([0060](0060-remote-wire.md)) can be paired, then updated.

### 2. Same Tailscale account

Checked live, on every connection, and never saved as trust. All of these must hold:
- The connection arrived on the Tailscale interface, and its source address is one Tailscale lists as a peer. Never loopback: Serve, Funnel and SSH forwards arrive from 127.0.0.1.
- `whois` for that address succeeds through Tailscale's **LocalAPI**:
  - on Linux, the tailscaled socket;
  - on macOS, the standalone app's LocalAPI.

  The CLI is never spawned on a connection path. Where the LocalAPI isn't reachable (for example, the App Store build's sandbox), account trust is unavailable and pairing is offered.
- The peer is owned by a user, not by tags. A tagged node has no user, and two tagged nodes must never compare as the same account.
- That user is the same user that owns the daemon's own node.

The answer is cached per peer address for the life of the connection, and re-checked when Tailscale's network map changes and every 5 minutes. A peer that leaves the account is dropped at the next check.

Over any other route, a device trusted only this way gets `trusted_on { tailscale }`. The client demotes that route for that machine and offers "Pair with a code to use the LAN too". It never marks the machine untrusted.

### Removing trust

- **Removal is only real when authenticated.** "You were removed" counts only inside a Noise session that proved the machine's pinned key. The client then forgets the key and stops every route. A refusal or failure from anything unauthenticated is backed off, never acted on.
- **Uncertainty never deletes trust.** Not an offline launch, not a timeout, not a refusal on one route.
- **Revocation closes live sessions** within a second, and is written (atomic rename and fsync) before it's acknowledged.

### Limits

As spike 05: at most 8 connections per address, and no large messages before trust. Also: a global cap on handshakes in progress, a 5 s limit for a connection to finish its handshake, and per-key limits after it.

### iPhone

- **Keys:**
  - The device key lives in the keychain under a constant service name and access group, never one derived from the app's display name.
  - The notification key uses `kSecAttrAccessibleAfterFirstUnlock` in the shared app-group keychain, so the extension can read it while the phone is locked.
  - Face ID gates interactive use only.
- **Transport:** Network.framework. URLSession's App Transport Security blocks unencrypted WebSockets to Tailscale's 100.x addresses, and Noise already encrypts.

## Why

Spike 05 found and fixed several trust bugs. One was Tailscale trust being saved like pairing, so it outlived leaving the account and worked over the LAN. The study of other tools added more:
- Running the Tailscale CLI on connection paths: slow (about 2.5 s on some machines), and once it relaunched itself as the GUI and opened dozens of login pages.
- Tagged and loopback connections passing naive account checks.
- 100.64.0.0/10 shared with other VPNs.
- A refusal on one route treated as machine-wide, and retried forever.
- An offline launch deleting saved machines.

## Consequences

- The daemon reads Tailscale state through a small LocalAPI client. That is a new dependency, needing a decision record if it's a crate rather than our own few calls.
- R-B3 needs the owner to create a tag once.
