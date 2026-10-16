# Ambient backgrounds

An optional live background behind the new-session page: View › Ambient Background, None by default. Each one borrows a real convention (a prism, a seismograph drum, a tide chart, a stippled specimen plate) and shows something true about your sessions. They're for fun, so they stay out of the way: cheap to run, never in the critical path, and easy to add to. Decision: [0049](decisions/0049-ambient-backgrounds.md).

## How it fits together

- **`crates/ambient` (`dent-ambient`)** holds every background. No dependencies, no GPUI and no I/O, so any client could paint one.
  - A background implements `Ambient`: `update(&Ctx)` advances the simulation and never sees the theme; `draw(&DrawCtx, &mut Scene)` paints it with a `Palette`.
  - `Scene` is a small display list: rects (solid or two-stop gradient), dots, strokes, segments, filled polygons, circles, text and rect clips. `Scene::fill_ramp` slices a shape for gradients with more stops.
  - `Runner` does everything common:
    - Caps input: at most 6 keys a frame, one session start per 0.6 s.
    - Smooths the pointer, whose weight `k` eases in and out.
    - Clamps time steps to 0.1 s.
    - Fills missing state from a synthetic source of the same shape.
    - Contains panics, so a failed background just stops drawing.
    - Drops non-finite geometry.
- **`crates/desktop/src/ambient.rs`** handles the desktop side:
  - Holds the choice and builds the menu.
  - Turns window state into `Signals`: tool calls and files touched (from the client-core timelines, new entries only), session names and states, recent titles, the five-hour usage window, the machine, and the open project's files.
  - Paints the scene with GPUI quads, paths, glyphs and content masks.
  - Asks for frames only while the new-session page is showing, the window is visible, a background is chosen and reduce motion is off. Reduce motion gives a still frame.
  - Rebuilds the slower signals only when they change, and paints straight from the runner's scene.
  - Starts the background afresh when the page comes back after a minute away.

## Real data, honestly

- **Event lists** (`calls`, `touches`, `started`) are only ever real while the app is connected. A quiet afternoon looks quiet. Stand-in events appear only with no live state at all: tests, the stress pass, a disconnected app.
  - They count only once the daemon has replayed its history and while the page is showing, so neither history nor an hour spent in a session arrives as one burst.
- **Figures shown as facts** (usage and its reset time, how many sessions are running) are never made up: with no reading, they aren't shown.
- **State lists** that are empty are filled from the stand-in, as scenery, so a background never draws an empty frame:
  - `sessions`, `log`, `files`, `machines`.
  - The project's files are sized by their real line counts, counted once per project on a background thread.
  - Machines are always the stand-in for now: dent has one daemon and no latency measurement, so Survey network labels its legs `~NN ms`.

## Adding a background

1. Write `crates/ambient/src/bg/<id>.rs` with `pub const SPEC: Spec` and a struct implementing `Ambient`. `bg/graph.rs` is the reference: module doc, named constants with units, state changes only in `update`.
2. Add `pub mod <id>;` to `bg/mod.rs`, and add `bg::<id>::SPEC` to `ALL` in `lib.rs`. That's all: the menu, the walkthrough and the tests pick it up from `ALL`.
3. Run `cargo test -p dent-ambient`.

Rules every background follows:

- **Colours:** only from the palette, never hard-coded; `spec[0..6]` for kinds of work. No dither of any kind.
- **Motion:** frame-rate independent (`math::ease`, exponential decay). Things fade or grow in, never pop. Anything that scrolls glides sub-pixel.
- **Caps and labels:** every growing collection has a cap, and labels never overlap (`geom::spread`).
- **Cost:** heavy work in `update`, cached in reused buffers; `draw` only paints. Aim for under 2 ms per frame at 1440×900 in release.
- **Physics** stays honest and is named in a comment (Snell's law, a damped oscillator, a 12.42 h tide).

## Testing

| What | When | Cost |
| --- | --- | --- |
| `cargo test -p dent-ambient` (part of `just check` and `just verify`) | Always | Under 0.1 s: every background hammered with hostile input at tile, phone, desktop and degenerate sizes |
| `just ambient-stress` | When something looks wrong | Release build; every background at 5 sizes and 3 frame rates, plus timings |
| `just ambient-walk` | When something looks wrong | The real app: a still of each background (`--shots DIR`, `--dark`), checks that frames stop with none chosen and once a session starts |

For recordings, `DENT_AMBIENT_DEMO=1 scripts/walk.sh demo-<id>` plays a short choreography on the stand-in's busy afternoon, at full frame rate, printing `demo: rolling` and `demo: cut` around the part worth filming (`crates/desktop/tools/record-window.swift` records the window).

The prototype used to design them is the gallery at https://claude.ai/artifact/ESBBECQZDRDk57tzbvJQhG; new backgrounds are easiest to sketch there first.
