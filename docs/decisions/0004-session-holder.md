# 0004. A session holder owns agent processes and terminals

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

A tiny, rarely changed `session-holder` process owns agent child processes and PTYs. The daemon connects to it and reconnects after an upgrade.

## Why

Service managers kill child processes when a service restarts, so a daemon upgrade would otherwise interrupt every session.

## Consequences

Two processes per machine. The holder's interface must stay small and stable. If the holder dies, sessions resume from the agent's own transcript.

[Spike 06](../spikes/06-session-holder.md) settled the shape:

- The daemon starts the holder on demand. It leaves the daemon's process group; on Linux it runs in its own systemd unit.
- Output is written to disk and streamed from any byte offset, and each event records the offset it ended at, so the daemon resumes exactly.
- Writes are applied whole or not at all.
- Protocol operations are only ever added. A newer holder takes over only once the old one is idle.
- The holder exits by itself once it has had no processes and no daemon for 60 s. With no daemon for 10 minutes it closes its agents' stdin so they finish, and once all have finished it drops their uncollected output and exits ([0038](0038-local-core.md)).
