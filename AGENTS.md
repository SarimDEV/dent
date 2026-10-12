# AGENTS.md

dent is a fast, chat-first app for driving coding agents (Claude Code and Codex first) on any machine you own. A Rust daemon on each machine owns the agent sessions; desktop (GPUI), iPhone (SwiftUI), web and CLI clients are views with a cache. Full spec: [docs/spec.md](docs/spec.md).

The name is always lowercase: **dent**.

## Architecture in one screen

```
clients (desktop · iphone · web · cli)
   │  one versioned protocol over QUIC (iroh): direct first, hub relay as fallback
   ▼
daemon (one per machine) ── session-holder (owns agent processes + PTYs, survives daemon upgrades)
   │  SQLite event log per daemon, monotonic seq; clients subscribe "everything after N"
   ▼
agents (claude -p stream-json · codex app-server · ACP)
hub: accounts, devices, relay, push, shared state (E2E encrypted), backup
```

- The daemon is the source of truth for anything it runs; the hub is the record for shared state (projects, settings, UI state). Clients never hold authority.
- Everything is an event in an append-only log. Reconnecting = subscribing from the last seq.
- Agents sit behind one `AgentDriver` trait. Nothing above a driver knows which agent it is.
- Borrow hard engines, own thin layers (event model, block tree, sync format, UI). Every borrowed piece is pinned and wrapped. See [docs/durability.md](docs/durability.md).

## Crates

All under `crates/`. Package names are prefixed `dent-`.

| Crate | Job | May depend on |
| --- | --- | --- |
| `protocol` | Wire messages, event types, versioning | serde only |
| `client-core` | Connections, sync, cache, search, Markdown → block tree. **No direct I/O** (I/O sits behind traits) so it binds to iOS (UniFFI) and wasm | `protocol` |
| `daemon` | Drivers, event log, git, files, checkpoints, providers | `protocol` |
| `session-holder` | Owns agent processes and PTYs. Tiny, rarely changed | std + minimal |
| `desktop` | GPUI app. The only crate allowed to use GPUI | `client-core`, `protocol` |
| `web` | wasm-bindgen facade over `client-core` | `client-core` |
| `cli` | `dent` binary: scripts, agents, `dent daemon install`, `dent doctor` | `client-core`, `protocol` |
| `hub` | Hosted/self-hosted hub. **AGPL-3.0**; everything else is MIT | `protocol` |

`spikes/` holds throwaway Phase 1 experiments (each its own crate in the workspace). `ios/` (Xcode) and `relay/` (deployment files) come later.

Boundaries enforced by `just verify`: logic crates never import GPUI.

## Commands

| Command | When | What |
| --- | --- | --- |
| `just dev` | Every save | Build and run the thing you're working on (`just dev <crate>`) |
| `just check` | While working | `cargo check` + unit tests for the crates you touched (`just check <crate>…`, default: all) |
| `just verify` | Before merging | fmt, clippy `-D warnings`, all tests, crate-boundary check |

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

## Definition of done

1. `just verify` passes.
2. The behaviour matches its spec area file (updated if it changed), with a decision record for any new choice or dependency.
3. You can say how to run it, and you ran it.
4. UI changes come with a screenshot.
5. Phase 1 spikes: a findings note in `docs/spikes/` saying what was learned and what changes in the spec.
