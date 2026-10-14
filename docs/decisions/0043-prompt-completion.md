# 0043. Prompt completion: `/` opens the message, `@` anywhere, dent keeps its own state

Status: accepted (2026-10-14) · Spec: [desktop.md](../desktop.md), [daemon.md](../daemon.md)

## Decision

The composer and the new-session field share one completion layer (`complete.rs`). It looks at the word the caret is in:

- `/word` that opens the message offers commands. The agent only reads a command there, so a `/` later in the text, or a path like `/Users/me`, opens nothing.
- `@word` anywhere offers the project's files and folders. Picking a folder writes `@folder/` and keeps the menu open inside it; picking a file writes `@path ` (quoted, `@"my notes.md"`, when it has a space), the form Claude Code reads with no tool call.
- Arrows move round the list, the pointer lights the row it's over, Tab or Return picks, Esc puts the menu away until the caret leaves the word or it's erased to its sign. Return sends instead of picking when the command is already typed in full, or when the lit row matched only by its description.
- The list is whole and scrolls. Ranking: exact name, name start, a word inside the name, anywhere in the name, the description, then letters in order. "No matching command" and "No matching files or folders" say when nothing fits.

Where the commands come from: the session's own list once its agent has started; before that, the list the daemon's usage probe already reads from `initialize` (built-ins, the person's commands, skills and plugins), so `/` works in a new session and after a restart. A new session's files come from `FilesIn { dir }`, its chosen folder.

dent does `/model`, `/effort`, `/rename` and `/clear` itself, picked from the menu or sent: they open dent's menus, rename the session, or start a new session in the same folder. Commands that drive the terminal app's own screen, or that the agent reports as unavailable to apps (`/color`, `/focus`, `/fast`, `/vim`, …), aren't offered.

A command the agent runs itself answers only in the turn's result (`/context`, `/usage`); when a turn ends with no reply, the daemon logs that result as the reply.

## Why

`/` showed nothing in a new session or after a restart, because commands were learned only from a running agent and kept in memory; the new-session field had no completion at all; and Esc on the menu also sent the keyboard to the inbox. Sending `/model` to the agent switched its model behind dent's chips, which then lied, and `/clear` would empty the agent's memory under a transcript that still showed everything.

Checked against Claude Code 2.x in `-p` stream-json mode with a tiny Haiku prompt and tools off: `@file` and `@"quoted path"` are read by the CLI; local commands (`/context`, `/usage`, `/model`, `/rename`, `/config`) answer with no model turn; `/fast` and `/focus` say they aren't available.

## Consequences

- Project-level commands (`.claude/commands` in the folder) appear once the session's agent starts; the probe runs outside any project.
- macOS hands a text field's Esc back as a command after keystroke interceptors have run, so Esc is decided in one place, the Escape action, never in the interceptor.
- `#` (pull requests) and other sessions as `@` targets are not offered yet.
