# 0044. Project picker: one field that searches, goes to a path, makes and clones

Status: accepted (2026-10-14) · Spec: [desktop.md](../desktop.md#new-session), [daemon.md](../daemon.md)

## Decision

The new session's project chip opens a menu with a field on top. What's typed decides what it shows:

- Words search the projects: name start, a word in the name, the name anywhere, a folder on the path, then letters in order; newest first among equals. The first nine show ⌘1–⌘9. "No project" (the scratch folder) stays offered.
- A path (`/…`, `~…`) lists the folder's subfolders as you type, from the daemon. Tab steps into the lit one; "Use …" takes the folder typed in full; "Make …" creates one that isn't there.
- A name that matches nothing offers New project "<name>": `~/dent/projects/<slug>`, a Git repository with a README and a first commit. A pasted Git URL offers Clone, into the same folder. Both pick the result when done; a missing Git identity keeps the project and says what's missing.
- ⌘⌫ or the lit row's × takes a project off the list without touching its folder, with Undo (⌘Z). Folders added here are remembered by the daemon.

Menus that search are a mode of the shared chip menu (`Menu::searches`), not a separate popover: the same rows, keys and placement serve every chip.

## Why

The old menu was a fixed list with "Choose a folder…": nothing to search once the list grows, no way to type a path, and nothing to start or clone a project. The flows follow a well-tested pattern in comparable tools (a palette that searches, browses typed paths and adds projects from a name or URL), folded into one field so there's no separate "add project" screen to learn.

## Consequences

- Clone uses the machine's git and credentials with prompts off; a private repository that needs a password fails with git's reason instead of hanging.
- New projects and clones live in `~/dent/projects`, beside `~/dent/scratch`; a setting for another place can come later.
- Setting a field's text from code now reports a change (`put`, `put_at`), as typing does, so whatever follows a field never misses it.
