# dent handoff

Everything needed to start building dent with Claude Code.

## What's here

- `docs/spec.md` — the full spec, exported from the living doc on 2026-10-11.
- `docs/design/architecture.png` — the architecture diagram from the spec.
- `docs/design/mockups/` — the HTML/CSS source of the Field Notes screens (light and dark: main, docs open, quick switcher, changes) and the chat-rendering specimen sheets (text, code/math/tables, charts and markets). They were written for the design canvas runtime, so treat them as the exact reference for colours, type, sizes and layout rather than pages to open directly.

The live spec and design canvas stay in claude.ai as the source of truth while they evolve; re-export when they change.

## How to hand this to Claude Code

1. Create a new repository and copy this folder's contents into it.
2. Start Claude Code in the repository (plan mode is a good first step) and paste the prompt below.

## Kickoff prompt

```
Read docs/spec.md end to end and look through docs/design/. We are starting to build dent.

Before any app code:

1. Write AGENTS.md (and a CLAUDE.md that just says to read AGENTS.md). Cover the architecture, crate
   boundaries, conventions, the fast-first workflow (just dev / just check / just verify), and the
   definition of done. Keep it short and practical.
2. Split docs/spec.md into per-area files under docs/ (keep spec.md as an index), and write one short
   decision record per decided item in docs/decisions/ (GPUI via pinned gpui-component, iroh with n0's
   free public relays for now, git through the CLI, the session holder, MIT + AGPL hub, Field Notes
   design, the renderer registry, and the others in the Decided list and Durability section).
3. Set up the Cargo workspace skeleton with the crates named in the Architecture section, a justfile
   with dev, check and verify, and fast debug-build settings. Keep it minimal: no heavy CI suites yet.

Then work through Phase 1 (Spikes) from the Build order, one spike at a time. Each spike is a small
crate or example plus a short findings note in docs/spikes/, and ends by telling me what you learned
and anything that changes the spec:

- Claude Code stream-json with approvals and rate_limit_event
- Codex app-server over stdio with generated types
- GPUI chat list and diff view at 100,000 events on pinned gpui-component
- RaTeX, merman and streaming Markdown against real transcripts
- iroh on n0's public relays with a hub prototype
- the session holder surviving a daemon upgrade
- the client core built for iOS and WebAssembly

Ask me before deciding anything the spec lists as open. Keep my inner loop fast: favour quick
feedback over test coverage at this stage. Work in small commits and tell me how to run each thing.
```
