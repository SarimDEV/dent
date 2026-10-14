# 0042. Your terminal: your own shell, drawn by dent with alacritty_terminal

Status: accepted (2026-10-13) · Spec: [desktop.md](../desktop.md#terminal)

## Decision

The viewer's Terminal tab is the person's own shell, not a log of the agent's commands. Opening it (⌘ 3, Ctrl+\`, or the header's terminal button) starts their login shell in the session's folder: zsh or bash on macOS and Linux, PowerShell on Windows, with their profile, PATH and aliases, as an editor's terminal panel does. One shell per session, kept while dent runs and ended when the session closes.

- [alacritty_terminal](https://crates.io/crates/alacritty_terminal), pinned at `=0.26.0`, runs the pty (ConPTY on Windows), parses escape codes and keeps the grid, scrollback and selection. dent draws the grid in the code face with GPUI, one styled line per row, in colours tuned to Paper and Carbon.
- The agent never sees the terminal and never types into it; its own commands stay in the chat as tool calls.
- While the terminal has focus every key without ⌘ goes to the shell (Esc, Ctrl-C, arrows, Option-arrows by word). ⌘ shortcuts stay dent's, except ⌘C and ⌘V, which copy the terminal's selection and paste (bracketed when the program asks), and ⌘⌫, which clears the line.

## Why

People reach for a terminal beside the agent for small things: an `export`, a quick `ls`, a script the agent suggested. That wants their real shell and nothing else. An app can't host Terminal.app or another app's window inside its own, so every editor runs the shell itself and draws it; alacritty_terminal is the maintained, cross-platform engine for that in Rust, under Apache-2.0. Showing the agent's commands a second time, as the tab used to, only repeated the chat.

## Consequences

- The shell runs on this machine. A session on another machine still opens a local shell, in the home folder when the session's folder isn't here; running it on the session's machine through its holder comes later.
- The shell starts at the pane's measured size, so it doesn't redraw its prompt for an immediate resize; later resizes reach it as they happen.
- Rendering one styled line per row is simple and fast enough for a small pane; a full-screen program at large sizes may want a cell renderer later.
