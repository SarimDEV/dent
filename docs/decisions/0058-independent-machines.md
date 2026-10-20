# 0058. Every machine is independent; the client merges the view

Status: accepted (2026-10-20, owner's call) · Supersedes the shared-state part of [hub.md](../hub.md) and [0020](0020-hub-single-binary.md) · Plan: [remote-plan.md](../remote-plan.md)

## Decision

No state is shared between machines. Each daemon is the only record for what it holds, and each client device keeps its own. The view across machines exists only in the client, built from its connections and its cache.

| Owner | What it holds |
| --- | --- |
| Each daemon | Its sessions and their event logs, its project checkouts, its machine settings (agent paths, worktree folder, projects folder, keep-awake), and project settings for its own checkouts |
| Each client device | Preferences (theme, shortcuts, layout), the machine list with keys and routes, inbox order, pins and hidden items, drafts, queued prompts ([0063](0063-queued-prompts.md)), machine weights |

- **Every setting is marked** in the protocol as a client preference or a machine setting, and machine settings as portable (same meaning everywhere, such as a default model) or machine-local (paths, binaries).
- **Editing a machine setting on several machines is a bulk edit, not a sync.** The client writes it to each chosen machine that is connected and loaded, and reports each result:
  - The control shows **Mixed** when the chosen machines disagree.
  - An offline machine is **Not saved**, with Retry, which skips the machines that already saved.
  - Machines added later get nothing.
  - A value that wasn't loaded is never written as if it were real.
- **Daemons list the setting keys they store,** so a client greys out a setting an older daemon would drop, with "update dent on <machine>".
- **Anything a daemon acts on while no client is attached stays on that daemon:** archiving merged sessions, PR polling, keep-awake, and later automations. Pure display rules stay in the client.
- **A project on several machines** is one group in the client, keyed by the repository resolver's group key. Its name, icon and default machine are written to each copy as a bulk edit.
- **Client-owned state is per device.** A pin on the laptop isn't a pin on the phone. Syncing it between devices could come later, through a channel the owner's devices share; it is out of scope now.

## Why

The owner's call (2026-10-20): every machine stays independent. It removes a whole service from the critical path, means no machine depends on another to work, and suits Tailscale, where machines come and go. Tools that share nothing do pay for it in settings that drift, so the bulk-edit rules above are taken from their fixes rather than learned again.

## Consequences

- The hub stays on hold ([0033](0033-direct-websocket-hub-on-hold.md)). If it returns, it is a relay and a backup, not the record for shared state, unless a later record says otherwise.
- [AGENTS.md](../../AGENTS.md) and [hub.md](../hub.md) no longer say the hub is the record for projects, settings and UI state.
- Inbox actions (hide, pin, order) never need the owning daemon, so a dead machine can't block the inbox.
- Moving a session ([0064](0064-moving-sessions.md)) is relayed by a client both machines trust, so daemons never need to trust each other.
