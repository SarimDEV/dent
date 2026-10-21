# AGENTS.md

dent is a fast, chat-first app for driving coding agents (Claude Code and Codex first) on any machine you own. A Rust daemon on each machine owns the agent sessions; desktop (GPUI), iPhone (SwiftUI), web and CLI clients are views with a cache. Full spec: [docs/spec.md](docs/spec.md).

The name is always lowercase: **dent**.

## Architecture in one screen

```
clients (desktop · iphone · web · cli)
   │  one versioned protocol over a direct WebSocket (Tailscale or LAN); hub on hold
   ▼
daemon (one per machine) ── session-holder (owns agent processes + PTYs, survives daemon upgrades)
   │  SQLite event log per daemon, monotonic seq; clients subscribe "everything after N"
   ▼
agents (claude -p stream-json · codex app-server · ACP)
hub (on hold): relay and backup; machines share no state (0058)
dent-push: stateless Cloudflare Worker forwarding E2E-encrypted phone notifications to APNs
```

- The daemon is the source of truth for anything it runs. Machines share no state: each daemon owns its sessions, checkouts and machine settings, each client device owns its preferences and inbox state, and the view across machines is built in the client ([0058](docs/decisions/0058-independent-machines.md)). Clients never hold authority over a daemon's data.
- Everything is an event in an append-only log. Reconnecting = subscribing from the last seq.
- Agents sit behind one `Driver` trait (`crates/daemon/src/agent`). Nothing above a driver knows which agent it is: an agent's wire format, flags, names and files live in its own folder (`daemon/src/claude/`), and a test fails if anything else reaches in. Clients ask a session's `Caps` before offering something ([0055](docs/decisions/0055-agent-layer.md)).
- Borrow hard engines, own thin layers (event model, block tree, sync format, UI). Every borrowed piece is pinned and wrapped. See [docs/durability.md](docs/durability.md).

## Crates

All under `crates/`. Package names are prefixed `dent-`.

| Crate | Job | May depend on |
| --- | --- | --- |
| `protocol` | Wire messages, event types, versioning | serde, serde_json |
| `client-core` | Connections, sync, cache, search, Markdown → block tree. **No I/O and no runtime:** the host feeds it bytes and the time and does the I/O it asks for ([spike 07](docs/spikes/07-client-core.md)), so it binds to iOS (UniFFI) and wasm | `protocol` |
| `daemon` | Drivers, event log, git, files, checkpoints, providers | `protocol` |
| `session-holder` | Owns agent processes and PTYs. Tiny, rarely changed | std + minimal |
| `desktop` | GPUI app. The only crate allowed to use GPUI | `client-core`, `protocol`, `ambient` |
| `ambient` | Ambient backgrounds for the new-session page: simulations drawing into a display list the desktop paints. No I/O, no dependencies ([docs/ambient.md](docs/ambient.md)) | std only |
| `web` | wasm-bindgen facade over `client-core` | `client-core` |
| `cli` | `dent` binary: scripts, agents, `dent daemon install`, `dent doctor` | `client-core`, `protocol` |
| `fake-claude` | Stands in for `claude -p` in tests and demos | serde_json |
| `hub` | Hosted/self-hosted hub. **AGPL-3.0**; everything else is MIT | `protocol` |

`spikes/` holds throwaway Phase 1 experiments (each its own crate, named `spike-*`, with a findings note in `docs/spikes/`). Spikes may use any dependency without a decision record and are excluded from the default build; production crates never depend on them. `push/` is dent-push, a TypeScript Cloudflare Worker (`npm test` runs it locally). `ios/` (Xcode) and `relay/` (deployment files) come later.

Boundaries enforced by `just verify`: logic crates never import GPUI.

## Commands

| Command | When | What |
| --- | --- | --- |
| `just dev` | Every save | Build and run the thing you're working on (`just dev <crate>`) |
| `just check` | While working | `cargo check` + unit tests (`just check <crate>…`; default: the crates changed since `origin/main` and everything depending on them) |
| `just verify` | Before merging | fmt, clippy `-D warnings`, tests and the crate-boundary check, for the same changed crates and their dependents (`scripts/affected.py`). `just verify-all` for every crate, `just verify-spikes` for spikes too |
| `scripts/walk.sh look --shots DIR` | A visual change | The fake agent replays one recorded real turn; stills of each moment in both themes, in well under a minute |
| `scripts/walk.sh a+b` | A UI change | Only the scenes it touched (names in `crates/desktop/src/walk/mod.rs`). The full `checks` suite once per batch, started in the background while the next item begins; `compare` (the real agent) once, before a batch lands |

Never run nightly-tier suites (screenshots, fuzzing, real agents) on your own.

## Conventions

- Rust 2024 edition, stable toolchain. Exact-pinned versions for anything we wrap (agents CLIs, GPUI snapshot, iroh, merman, RaTeX). Committed `Cargo.lock`.
- No `unwrap`/`expect` in daemon code; use `anyhow` at binaries, `thiserror` in libraries.
- No new dependency without a decision record in `docs/decisions/`.
- Small crates with one job and a short README.
- Feature flags for risky, unfinished work so it can merge early.
- Docs change with code: behaviour change ⇒ update the spec area file and decision record in the same commit.
- Don't relitigate `docs/decisions/`. Anything listed as **open** in [docs/risks.md](docs/risks.md) needs the owner's call — ask.
- Public repo: no references to other agent harnesses or third-party products as comparisons.
- Small commits with clear messages.

## Fast first

Speed of iteration beats coverage, especially now. Early on, test only what protects data and sessions (event log, persistence, session-holder). Screenshot/fuzz/perf suites switch on per area once it stops changing daily.

## Improving how we work

The owner is often away from the machine doing the work, so the workflow has to improve itself. Treat slowness, rework and weak output as bugs in the process, and record them in [docs/workflow-log.md](docs/workflow-log.md) (what counts, and the entry format, are at its top).

- **Notice:** time builds, verify, tests and walkthroughs. Log anything over ~2 min, anything run when nothing it covers changed, every correction from the owner, and output you wouldn't be proud to show.
- **Record, don't detour:** add the entry and carry on with the task. Fix a process problem unasked only when it's small and blocking the current work. Say in one line at the end of the turn which entries you added.
- **Speak up:** if something is costing a lot right now (a suite run repeatedly, a step waiting on the owner), suggest the fix in chat as well as logging it.
- **Measure:** every entry carries numbers, and every fix is re-measured. "Feels faster" isn't a result.
- **Work the log when asked:** take the open entries highest cost first, fix, re-measure, and move each to Done with before and after numbers.
- **Be honest about output:** say what you checked and what you didn't. Describe behaviour only after reading the code or running it; otherwise say it's unverified.

## Working rules

- **Briefs:** for a "make it feel like X" request, write the brief back in one paragraph (which experience, which moments) and get a yes before researching.
- **Edits:** use the editor tool, which refuses an anchor that isn't unique, for anything but mechanical renames. A scripted edit anchors on whole lines and asserts each anchor matched exactly once.
- **One writer per area:** before starting, check `git worktree list` and open branches for work on the same files. A second session starts from the first one's latest main, or waits; two walkthroughs never share the screen.
- **Checks test the thing:** a walkthrough check reads a probe drawn by the feature it names (`drawn(d, "drop-message")`), not a stand-in such as "a drag is active".

## Definition of done

1. `just verify` passes.
2. The behaviour matches its spec area file (updated if it changed), with a decision record for any new choice or dependency.
3. You can say how to run it, and you ran it.
4. UI changes come with a screenshot.
5. Phase 1 spikes: a findings note in `docs/spikes/` saying what was learned and what changes in the spec.
