# dent-desktop

The dent desktop app (GPUI): the session inbox, the chat and the viewer, drawn from what this machine's daemon reports. It starts `dent-daemon` (installed next to it) when none is running.

```sh
scripts/bundle.sh [--install]                   # dent.app with the daemon inside; --install puts it in ~/Applications
cargo run -p dent-desktop                       # uses ~/Library/Application Support/dent
scripts/walk.sh checks                          # walkthrough against the fake agent, in a scratch home
WINID=… scripts/walk.sh stills --shots DIR      # stills of the same, window only
```

- `live.rs`: the socket to the daemon, with reconnects and resends.
- `bridge.rs`: the client core's timelines into chat items, inbox rows and artifacts.
- `app.rs`: window state and actions; `chat.rs`, `sidebar.rs`, `viewer.rs` draw it.
- `walk/`: scripted walkthroughs through the real event path: the runner and scene table in `mod.rs`, one file per scene, shared helpers in `common.rs`.
