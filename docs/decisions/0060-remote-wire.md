# 0060. The remote wire: framed Noise over a WebSocket, with a frozen maintenance subset

Status: accepted (2026-10-20, owner's call after stage 0) · Evidence: [spikes 12](../spikes/12-link-under-real-networks.md), [14](../spikes/14-routes-identity-and-boot.md) and [15](../spikes/15-sleep-restart-and-serve.md) · Before release: [R-A2 and R-A7–A12](../remote-tests.md#a-links-liveness-and-routes) against the real daemon · Builds on [0033](0033-direct-websocket-hub-on-hold.md), [0038](0038-local-core.md) · Plan: [remote-plan.md](../remote-plan.md)

## Decision

The transport stays as 0033 chose: one WebSocket over TCP to each machine, carrying Noise XX between device keys. The JSON lines of [0038](0038-local-core.md) ride inside it, and unknown fields and event kinds are ignored. This record fixes what's hard to change once clients ship.

### Handshake

- **Abort before revealing yourself.** The client checks the responder's static key (Noise message 2) against the pinned key for that machine, and aborts before message 3 if it differs. That way it never reveals its own key to a stranger at a known address.
- **Versions travel in the handshake payloads:** message 2 carries the daemon's protocol range, message 3 the client's. With no overlap, the daemon sends one small `incompatible { daemon_range, client_range }` and closes, before any other work. The client then knows which side to update.
- **The Bonjour record carries only:** the key fingerprint, the protocol range and the name. Never the OS, agents or user.

### Frames

Inside Noise, every message is a frame with a small header: stream id, lane (control or data), and a last-chunk flag.
- **Size:** frames are at most 64 KiB. Larger messages are split into chunks.
- **The control lane goes first.** Ping, pong, approvals, answers, stop and goodbye are sent between data chunks, never behind a whole payload.
- **One bad message fails its stream, not the connection.** A message that can't be decoded fails only its stream, which resumes by sequence.
- **Compression:** per-message compression inside Noise is reserved as a capability. Compression outside Noise is never used.

### The hello (`Welcome`) grows

New fields are added, never changed:
- the machine's capability set, separate from each agent's `Caps`;
- the log epoch ([0059](0059-machine-identity.md));
- the daemon's current addresses (LAN, Tailscale IP and name, port), also pushed when they change;
- the setting keys it stores ([0058](0058-independent-machines.md));
- the last stall: how long, when, load, and the cause if known;
- the last update: `{ id, from, to, outcome }`, so a reconnect can tell an update from a rollback;
- why it last restarted, and how long it was down.

### Resume

`Subscribe` carries `{ epoch, after }`. A different epoch, or `after` beyond the daemon's head, means the client drops its copy of that machine's log and resyncs. The daemon attaches the live stream before replaying up to the head captured at the request, so nothing published during the replay is lost.

### Refusal codes

Each code says whether retrying can help:
- `incompatible`: blocked until a version changes.
- `not_trusted`: blocked; pairing is needed.
- `trusted_on { routes }`: this route can't be used (for example, Tailscale-only trust over the LAN); other routes still can.
- `removed`: sent only inside an authenticated session. The client forgets the machine.
- `starting`: the daemon is still loading; transient.
- `busy` and `rate_limited`: transient, with a retry-after.

The daemon loads its trust store before it listens, so a trusted device is never refused while the daemon starts.

### Idempotency and jobs

- **Every request that changes something carries a client-generated id:** prompt, stop, approve, deny, file write, setting write, clone, move. The daemon remembers ids for 7 days and answers a repeat with the original result.
- **Nothing is replayed automatically after a reconnect.** The client core resends only its own outbox, by id ([0063](0063-queued-prompts.md)).
- **Long work is a job with an id:** updates, clones, moves, large transfers. Clients attach to a job by id and get its progress replayed. A disconnect never cancels a job.
- **Attaching never creates.** Attaching to something that has since stopped reports it as stopped (terminals later).

### Announcements

The daemon sends `going_to_sleep`, `quitting`, `restarting { expected_down }` and `awake`. A machine that wakes also re-announces on Bonjour, so clients redial at once.

### The frozen maintenance subset

Six messages never change shape and are answered whatever the main protocol range: `hello`, `version`, `update { to }`, `update_progress`, `update_result` and `goodbye`. Any dent client can update any older dent daemon it trusts, even when they share no main protocol version.

## Why

Each piece answers a failure seen in shipped tools:
- An exact version check locked new clients out of old servers, with no way to update them.
- A large message delayed pongs, so healthy links were dropped.
- A full subscription buffer stopped the reader and hid pongs.
- One poison message looped reconnects.
- Clients that couldn't tell "needs update" from "network blip" retried forever.
- Cancelling an update because the phone's socket dropped.
- Reconnecting resent actions and created duplicates.

Clients in the field can't be changed afterwards, so these must be in v1.

## Consequences

- `PROTOCOL` becomes a range. The one-JSON-object-per-line format stays inside frames, so the local Unix socket of [0038](0038-local-core.md) speaks the same messages without Noise or framing.
- The spike's link code is reworked to frames and the new hello before it moves into `client-core`.
- Catch-up batches of about 64 KB ([risks](../risks.md)) fit the frame size.
