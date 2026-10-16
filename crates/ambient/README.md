# dent-ambient

Ambient backgrounds for the new-session screen. Each background is a small simulation that reads a narrow `Signals` struct and draws into a display list (`Scene`) of rectangles, dots, lines, polygons, circles and text. The crate has no dependencies and no I/O; a client paints the scene (the desktop does it with GPUI in `crates/desktop/src/ambient.rs`).

How to add a background, and the rules every one follows, are in [docs/ambient.md](../../docs/ambient.md).

- `cargo test -p dent-ambient` runs the fast checks (well under a second).
- `just ambient-stress` runs the long stress and timing pass on demand.
