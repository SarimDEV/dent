# 0075. A watch on each agent's memory

Status: accepted (2026-10-23, built while the owner was away; theirs to change) · Related: [0065](0065-launcher-and-updates.md)

## Decision

- Every 10 s, while the holder runs, the daemon measures each agent's whole process group (the agent and everything it started) with one `ps`. Measuring never starts the holder.
- **Past 4 GiB,** the session says once how much memory the agent uses, and where dent will stop it.
- **Past half the machine's memory** (never under 6 GiB), dent stops the agent's group, as Stop does. The turn ends interrupted, with "dent stopped it: it used 9.1 GB of memory, past this machine's limit of 8.0 GB". It doesn't carry on by itself: it would likely do the same again. The next message starts the agent afresh from its conversation.
- `DENT_MEMORY_WARN`, `DENT_MEMORY_STOP` (bytes) and `DENT_MEMORY_EVERY_MS` set these, for tests and for machines that need others.
- It runs on Linux too. There, under systemd, the agents' slice already makes the kernel kill an agent rather than the daemon when memory runs out. This stops one sooner, and says why.

## Why

T3 Code's users report memory spikes past 10 GB, and macOS has no limit that would stop them: a runaway agent takes the Mac down into swap, and every other agent and app with it. Stopping one agent with a clear reason, and keeping its conversation, costs far less than that.
