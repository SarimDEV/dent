# 0051. A global shortcut through Carbon; the daemon keeps the Mac awake with caffeinate

Status: accepted (2026-10-18) · Spec: [desktop.md](../desktop.md), [daemon.md](../daemon.md)

## Decision

- **Global shortcut.** dent registers one shortcut that works from any app with macOS's `RegisterEventHotKey` (Carbon, linked as a system framework, no new crate). It needs no Accessibility permission, unlike watching every key. ⌥⇧Space by default, which macOS doesn't use (⌘Space is Spotlight, ⌃Space and ⌃⌥Space switch input sources); dent › Global Shortcut offers ⌃⌥⌘N or Off, kept in prefs. A press brings dent forward on a new session's field. Walks never register it.
- **Keeping the Mac awake.** While any session has a turn running or background work, the daemon runs `/usr/bin/caffeinate -i -s -w <daemon pid>`: no idle sleep, and on power no sleep with the lid closed. It stops the moment nothing works, and with the daemon (`-w`). On by default; dent › Keep Mac Awake While Working turns it off, sent to the daemon on each connect.

## Why

- Agents work for minutes to hours; a Mac that sleeps halfway stops them, and the owner already reached for `caffeinate` by hand. The daemon knows when work runs, and holds the machine only then.
- `caffeinate` is the system's own tool, needs no IOKit bindings, and lets go when the daemon dies.
- `RegisterEventHotKey` is the only way to a global shortcut without asking for Accessibility.

## Not chosen

- An NSEvent global monitor: needs Accessibility, and only watches.
- IOKit power assertions in the daemon: more code for the same effect, and nothing lets go if the daemon is killed.
