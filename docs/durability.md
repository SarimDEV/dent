# Durability

dent borrows every hard engine and owns only the thin layers that make it dent: the event model, the block tree, the sync format and the UI. Each borrowed piece is pinned, wrapped behind dent's own interface, and upgraded on a schedule, so an upstream change becomes a planned task instead of a broken app. Research as of 2026-10-11.

## What we borrow, what we own

| Area | Decision | If it breaks |
| --- | --- | --- |
| Claude Code | Drive `claude -p` stream-json directly; detect features from `system/init` capabilities, not version numbers ([headless docs](https://code.claude.com/docs/en/headless)) | Fall back to a small Node sidecar on the official Agent SDK |
| Codex | `codex app-server` over stdio, v2 methods only, no experimental API; Rust types generated in CI from its JSON schema at a pinned version ([app-server docs](https://learn.chatgpt.com/docs/app-server)) | Schema diff on every bump shows exactly what changed |
| Other agents | One ACP v1 client for everything else, crate pinned ([ACP](https://agentclientprotocol.com/protocol/initialization)) | ACP v2 is a draft; support v1 and v2 side by side when it lands |
| Desktop UI | GPUI through gpui-component, with the GPUI snapshot pinned exactly and upgraded monthly ([gpui-component](https://github.com/longbridge/gpui-component)) | All logic lives in GPUI-free crates; Tauri is the escape hatch for the view layer |
| Diff editor | Built on gpui-component's editor | Diffs computed with imara-diff, independent of the view |
| Terminal | alacritty\_terminal through a pinned fork | libghostty once it reaches 1.0 |
| Markdown | pulldown-cmark, plus a stable-prefix streaming layer and a terminator that closes the unfinished tail | We own the streaming layer; it is small |
| Math | RaTeX, pure Rust, with an iOS framework ([RaTeX](https://github.com/erweixin/RaTeX)); KaTeX on the web | Web-view KaTeX fallback |
| Mermaid | merman, a headless Rust port producing SVG ([merman](https://github.com/Latias94/merman)) | Offline web view running mermaid.js |
| Highlighting | arborium tree-sitter grammars, with tokens carried in the block tree so iPhone and web need no highlighter ([arborium](https://fasterthanli.me/articles/introducing-arborium)) | Plain monospace code |
| Vega-Lite | vl-convert on desktop and in the daemon, sending SVG to the phone ([vl-convert](https://github.com/vega/vl-convert)) | Show the data table instead |
| Connectivity | iroh 1.0, stable wire format and API, Swift bindings, open-source relay ([iroh 1.0](https://iroh.computer/blog/v1)); n0's free public relays while building and sharing with a few friends; relays on Hetzner (about €5 a month each, set up by an agent) or n0's $19/month tier before public launch; relay addresses are a setting, never hard-coded | Self-hosted iroh relay; Tailscale stays an optional transport |
| Web client transport | iroh in the browser is relay-only, so the web client may use a WebSocket through the hub instead ([browser docs](https://docs.iroh.computer/deployment/wasm-browser-support)) | Hub WebSocket path |
| Encryption | Our own small layer: one account key wrapped per device, per-field XChaCha20-Poly1305 records, QR or tap approval from an existing device, printed recovery key | Standard primitives only; no sync framework to outgrow |
| Hub | One Rust binary with SQLite and the iroh relay, the same build hosted and self-hosted | One codebase rather than a Workers version plus a self-hosted version |
| Push | APNs sent from the hub; self-hosted hubs send content-free pushes through the hosted one | Notifications fall back to in-app |
| Process lifetime | A tiny, rarely changed session holder owns agent processes and terminals; the daemon reconnects to it after an upgrade, because service managers otherwise kill child processes on restart ([systemd](https://man7.org/linux/man-pages/man5/systemd.kill.5.html)) | Sessions resume from the agent's transcript |
| Install and update | service-manager for launchd, systemd and Windows services; cargo-dist with its updater ([service-manager](https://docs.rs/service-manager/latest/service_manager/)) | Manual install still works |
| Shared core on iPhone | UniFFI pinned, built by our own script rather than cargo-swift; a hand-written Swift wrapper adds cancellation and main-actor safety ([UniFFI](https://github.com/mozilla/uniffi-rs)) | The core stays free of I/O so it can be rebound |
| Web core | A separate wasm-bindgen facade over the same I/O-free core | Same |
| Git | The git CLI for every write (staging, commits, worktrees, hooks, push); gix read-only for fast status ([gix status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)) | Plain git always works |
| Checkpoints | A temporary index, then write-tree and commit-tree, kept under hidden `refs/dent/` refs; untracked files capped by size; the user's index never touched | Restore is a plain read-tree |
| Storage | SQLite with FTS5 everywhere, external-content indexes, scheduled WAL truncation ([WAL](https://sqlite.org/wal.html)) | Standard SQLite tooling |

## Practices

- **Pin everything** with exact versions and a committed lockfile; upgrade each dependency on a calendar, never by accident.
- **Record replays** of every agent's output per version and run them before merging changes to an agent driver.
- **Nightly canary** against the latest Claude Code, Codex and ACP agents; warn users when their agent version is outside the tested range rather than blocking them.
- **Watch single-maintainer dependencies:** merman, RaTeX and gpui-component each have a named fallback above.
- **Size budgets** for the iPhone binary and the desktop app, checked on every pull request.

## Decisions this changes

- **Claude usage limits are solved:** the stream emits `rate_limit_event` with five-hour and weekly utilisation and reset times, so the status line is only a fallback.
- **The daemon no longer holds agent processes itself;** the session holder does, so daemon upgrades never interrupt work.
- **Git writes shell out** instead of using a Rust git library, and checkpoints use a temporary index.
- **Mermaid and math no longer need a script engine or web view** in the normal case.

## Open risk: Anthropic's terms

The Agent SDK overview says third-party developers may not offer claude.ai login or subscription rate limits for their products unless previously approved ([Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview)). Anthropic's support page says subscription use by the SDK, `claude -p` and third-party apps continues for now ([support](https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan)).

- For personal use, dent spawns the user's own `claude` binary with their own login and never touches tokens.
- Before a public or paid release, ask Anthropic for approval, and design so Claude also works with an API key.
- Do not use the Claude Code name in branding.
