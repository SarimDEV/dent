# dent-daemon

The per-machine dent daemon: it owns every agent session, keeps their events in SQLite, and serves clients over a Unix socket.

```sh
cargo run -p dent-daemon -- --state /tmp/dent-state        # foreground
DENT_CLAUDE=target/debug/dent-fake-claude cargo run -p dent-daemon   # with the fake agent
cargo test -p dent-daemon                                   # end to end, with the fake agent
```

- `session.rs`: one task per session; agent output becomes events, commands become agent input.
- `claude/`: the Claude Code driver (flags, stream-json parsing).
- `store.rs`: the event log and the sessions projection.
- `holder.rs`: the client for the session holder, which owns the agent processes.
- `server.rs`: the socket, catch-up and live streaming.

State lives in `~/Library/Application Support/dent` on macOS (`--state` or `DENT_STATE` to change it). Agents that sit idle for 5 minutes stop and resume on the next message (`DENT_IDLE_MS`).
