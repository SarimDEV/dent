# Desktop concepts

The desktop design running in GPUI with fake agents: the session inbox, starting a session by typing, and the viewer (live pages with versions, widths, pointing and error fixing; docs; images; changes with review comments; the terminal). Findings: [docs/spikes/08-desktop-concepts.md](../../docs/spikes/08-desktop-concepts.md).

```
cargo run -p spike-desktop-concepts                      # Paper, 1440×860
cargo run -p spike-desktop-concepts -- --dark --size 1100x720
cargo run -p spike-desktop-concepts -- --demo tour   --shots /tmp/shots   # walkthrough + 15 checks
cargo run -p spike-desktop-concepts -- --demo checks --pace 0.5           # 17 edge cases
cargo run -p spike-desktop-concepts -- --demo tour --record /tmp/tour.mov  # needs ffmpeg
```

Stills capture the window by number when `WINID` points at a build of `tools/winid.swift` (`swiftc -O tools/winid.swift -o /tmp/winid`).

Nothing here talks to an agent or the network. `fixtures/` holds what the fake agents "write": four versions of a pricing page, a plan, and a screenshot from the phone concepts.
