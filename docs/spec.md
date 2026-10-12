# dent spec

The spec is split by area. Each file is the source of truth for its area; a change in behaviour updates its file (and decision record) in the same commit.

| Area | File |
| --- | --- |
| Goals, non-goals, principles | [overview.md](overview.md) |
| Field Notes visual direction, palette, type, theming | [design.md](design.md) |
| Pieces and repository layout | [architecture.md](architecture.md) |
| Daemon: drivers, event log, git, terminals, checkpoints, CLI, automations | [daemon.md](daemon.md) |
| Hub, sync, connections, encryption, push, hosted services | [hub.md](hub.md) |
| Projects, repos, checkouts, worktrees, docs folder, secrets | [projects.md](projects.md) |
| Desktop app | [desktop.md](desktop.md) |
| Chat rendering and the renderer registry | [chat-rendering.md](chat-rendering.md) |
| iPhone app | [iphone.md](iphone.md) |
| Remote and ephemeral machines | [remote-machines.md](remote-machines.md) |
| Reliability requirements | [reliability.md](reliability.md) |
| How the repo is built and verified with agents | [building-with-agents.md](building-with-agents.md) |
| Build order (phases) | [build-order.md](build-order.md) |
| What we borrow vs own, pinning practices | [durability.md](durability.md) |
| Risks, decided items, open questions | [risks.md](risks.md) |

Also:

- [decisions/](decisions/) — one short record per settled choice. Don't relitigate these without a new record.
- [spikes/](spikes/) — findings from Phase 1 spikes.
- [design/](design/) — architecture diagram and the Field Notes HTML/CSS mockups.
