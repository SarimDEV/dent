# Build order

Everything in this spec gets built; the order only decides what you can use daily soonest. Each phase ends with something you use for real, before the next starts. During a phase only fast checks run; its reliability requirements must pass before the phase is called done.

1. **Spikes.** Claude Code stream-json with approvals and rate-limit events, Codex app-server, each agent's turn-end and background-work signals, a GPUI chat list and diff view at 100,000 events on pinned gpui-component, RaTeX, merman and streaming Markdown against real transcripts, a direct WebSocket link over Tailscale and the LAN with device pairing, the session holder surviving a daemon upgrade, and the client core on iOS and WebAssembly.
   - Exit: every Risks item marked "spike" has an answer written down.
2. **Local core.** Daemon, event log, Claude Code driver, desktop chat with jump to my messages, the renderer registry with text, code, math, tables and charts plus its full test harness, queue and steer, context meter, model and permission modes, checkpoints and rewind, attachments, side chat, subagent tree, persistence across restarts, command palette.
   - Exit: you use it instead of the Claude Code terminal for a week.
3. **More agents and status.** Codex and ACP drivers, usage limits in the status bar, goals with budgets and auto-resume, multiple accounts, overview board, desktop notifications, approvals.
   - Exit: both agents run side by side with limits always visible.
4. **Many machines.** Direct connections over Tailscale and the LAN, device pairing, shared state, client cache, projects, quick capture with project routing, environment and secrets, search, honest liveness, sharing links, the CLI and MCP server.
   - Exit: from a coffee shop on Tailscale, the laptop drives agents on the Mac server, and a restart of anything loses nothing.
   - The hub (sign-in, encryption of shared state, relays, push) is on hold ([0033](decisions/0033-direct-websocket-hub-on-hold.md)) and slots in before the iPhone phase needs push.
5. **Review.** Diffs, staging, small edits, commit and push, inline comments sent to the agent, PR and CI status, auto-archive.
   - Exit: you stop opening VS Code and GitHub to review.
6. **Docs.** Markdown viewer with Mermaid and math, live reload, project docs folder sync.
7. **Terminal and previews.** Embedded terminal on session-holder PTYs, port forwarding and previews, the agent browser and browser panel, adopting sessions started in a terminal.
8. **iPhone.** Client core through UniFFI, SwiftUI app, hub push with time-sensitive approvals, notification actions, widget, Live Activity, Face ID, Apple Watch.
   - Exit: you answer an approval from your lock screen.
9. **Web client and backup.** Browser client served by the hub, encrypted session backup.
10. **Machines on demand.** Provider interface, command provider, AWS provider, hosted cloud machines, join tokens, safe teardown, fleet updates, moving sessions between machines, automations and the inbox.
11. **Public release.** Linux and Windows polish, installers, billing, own relays on Hetzner, Anthropic approval for subscription logins, protocol docs, license, website.
