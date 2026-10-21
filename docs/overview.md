# Overview

dent (dent.sh) is a fast, chat-first app for driving coding agents (Claude Code and Codex first) on any machine you own, from a cross-platform desktop app or a native iPhone app. Agents run under a Rust daemon on each machine, so every session survives closed lids, dropped connections and restarts.

**Goals**

- Chat with agents running on any machine (laptop, Mac server, Linux box, EC2) as if they were local, and see everything on any device after signing in once.
- Never lose a session: agent processes, chat history and UI state all persist.
- Group work into projects that span many repos and many machines.
- Review diffs and stage files faster than VS Code does.
- Read the agents' markdown output in an excellent, live-reloading viewer.
- Approve, reply and get notified from the iPhone.
- Always show Claude and Codex 5-hour and weekly limits.
- Feel instant: every interaction under one frame, with deliberate motion and full keyboard control.

**Non-goals**

- A code editor. Diff edits are small touch-ups, not authoring.
- A new agent loop. dent drives existing agents; it does not call model APIs itself.
- Multi-user collaboration. One owner per install; the design should not block teams later.

**Principles**

- The daemon is the source of truth for anything it runs. Machines share no state; the view across them is built in the client ([0058](decisions/0058-independent-machines.md)). Clients are views with a cache.
- Everything is an event in a log, so any client can catch up from any point.
- Agents are plugins behind one adapter interface, so adding a third agent touches one module.
- Public from day one: cross-platform, documented protocol, no hard-coded personal setup.
