# 0038. The local core: a Unix socket for clients on the same machine, SQLite through rusqlite

Status: accepted (2026-10-13) · Spec: [daemon.md](../daemon.md), [architecture.md](../architecture.md)

## Decision

- **Clients on the daemon's own machine connect over a Unix socket** in the daemon's state directory (0700, socket 0600), speaking the same protocol, one JSON object per line. The WebSocket with Noise from [0033](0033-direct-websocket-hub-on-hold.md) carries the same lines between machines when Phase 4 adds them. A loopback TCP port would be open to every local process, including web pages; a socket only this user can open needs no pairing.
- **The app starts the daemon** when it isn't running, and the daemon keeps going after the app quits. Installing it as a launchd service, so it also starts at boot, comes with the installer ([0022](0022-install-service-manager-cargo-dist.md)).
- **SQLite through rusqlite 0.37.0 with the `bundled` feature,** so every machine runs the same SQLite with WAL and FTS5, whatever the system ships.
- **The desktop app** reads the daemon's feed on a thread and hands it to the window through a `futures` channel (0.3.34). It keeps spike 08's `image` and `objc2` crates for page snapshots.
- **A fake `claude`** (`crates/fake-claude`) speaks stream-json with the shapes recorded in [spike 01](../spikes/01-claude-code-stream-json.md). The daemon's tests and demos run against it, so nothing touches a real account.

## How the Claude Code driver behaves

- dent names the agent's session up front (`--session-id`) and resumes it with `--resume` once it has run, so a session can be resumed even if its first process dies early.
- A message sent while the agent works waits in the daemon and goes when the turn ends; it can be taken back until then. **Send now** steers it into the running turn instead (`priority: "now"`). The CLI reports the cut-short request as `aborted_streaming`, which isn't the turn ending.
- **Stop** asks the agent to interrupt and stops its process 2 seconds later if the turn hasn't ended, since an interrupt doesn't always produce a result. Waiting messages come back to the person; open approvals expire, as do any the agent cancels (`control_cancel_request`).
- **Always allow** sends the agent's own suggested rules back with their destination set to the session.
- An agent with nothing running stops after 5 minutes and resumes from its transcript on the next message: fast replies while you work, no idle processes overnight.
- Results with `is_error` count as errors even when their subtype is `success`; internal `[ede_diagnostic]` lines are never shown.

## Consequences

- Phone and other-machine clients need the WebSocket path; the protocol is already transport-agnostic.
- Until the installer exists, sessions survive the app quitting and daemon restarts, but not a reboot until the app is opened again (then they resume from their transcripts).
