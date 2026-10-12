# Desktop app

The desktop app is a GPUI client for macOS, Linux and Windows, built around a chat column with docs, diffs and a terminal one keystroke away. It never blocks on the network: everything renders from the local cache first.

## Layout

- **Sidebar:** one list of sessions across every machine, sorted as an inbox (decision [0036](decisions/0036-sessions-as-an-inbox.md)): Needs you, then Done, then Working, lighter, at the bottom. Each row shows its status, project and machine. Closing a session takes it off the list; inbox zero is when everything is working or closed. Machines and their connection state sit at the bottom.
- **Main:** the selected session's chat.
- **Side panel:** docs, diff or terminal, toggled and resized by keyboard; can pop out into its own window.
- **Status bar:** usage limits, machine connectivity, blocked-session count.

## Chat

- Virtualized message list that stays at 120 fps with tens of thousands of events; streaming text renders incrementally without re-laying-out earlier messages.
- Tool calls collapse to one line ("Edited 3 files", "Ran tests: 2 failed") and expand inline; file edits open straight into the diff view.
- Approvals appear inline with allow once, allow for session, and deny with a reason.
- **Jump to my messages:** Alt+Up and Alt+Down move between your own messages; a thin rail on the right edge marks each of them and is clickable; a popover lists them all with search.
- Composer: multi-line, drafts saved per session as shared state, file and doc mentions with fuzzy completion, image paste, the agent's slash commands, and queued messages while the agent is working.
- Interrupt, retry the last turn, fork a session from any message, and rename sessions (auto-titled by default).

## Composer and session controls

- **Queue or steer:** while the agent works, Enter queues the next message and Cmd/Ctrl+Enter steers, interrupting with it now. Queued messages can be edited, reordered or removed.
- **Context meter:** a ring beside the composer shows how full the session's context is, with one click to summarize.
- **Model, effort and mode:** switchable mid-session from the composer; resuming or forking keeps them.
- **Permission mode:** ask, accept edits, plan, or auto, always visible. Claude Code now defaults to an auto mode where a classifier approves tool calls ([write-up](https://hyrax.dev/blog/claude-code-auto-mode-default-august-14)), so actions the classifier blocked show as a reviewable list rather than one prompt at a time.
- **Attachments:** paste or drop images and files; they upload to the session's machine, even a remote one.
- **Accounts:** several Claude or Codex logins per machine, picked per project or session, each with its own usage limits.
- **Rewind:** each of your messages offers restore code, restore conversation, or both (see Checkpoints).

## New session

Cmd/Ctrl+N opens an empty chat with the cursor in the composer: you start by typing. Under the composer are the session's settings, already chosen, each a click (or a key) to change:

- **Project:** filled in from what you type, updating as you go; "New project" or "No project" when nothing fits.
- **Machine:** the least busy online machine with the project's repo.
- **Model, permissions, worktree:** whatever you used last.

## Quick capture

Press a global shortcut from anywhere, type an idea, press Enter, and an agent starts on it in the right project. You never have to switch to the app.

1. A system-wide shortcut (default Cmd/Ctrl+Shift+Space) opens a small floating prompt over whatever you are doing. It works while the app is closed, because a lightweight menu-bar or tray process stays running.
2. While you type, the decision model picks where the idea belongs: an existing project and repo, or a new project. The pick shows under the prompt as "AuthZ refactor · auth-service" or "New project", updating as you type.
3. Enter starts the session there on your default machine. Tab cycles through the next most likely projects first; one more key picks another machine.
4. A small toast confirms where it started, and the session is in the sidebar and on your phone straight away.

- **New projects** get a name suggested from the prompt and a scratch folder, or the repo the idea clearly refers to.
- **Low confidence** never guesses silently: when no project is clearly ahead, the prompt shows the top choices and waits for one key.
- **Change your mind:** "Move to project" works on any session, keeping its history.
- **Context comes along:** the shortcut can attach the current browser URL or selected text, if you allow it.
- **What the model sees:** project names, their repos and recent session titles, plus your prompt. Without a decision model configured, captures start with no project.
- **Platform note:** global shortcuts work directly on macOS and Windows; on Linux under Wayland they go through the desktop's global-shortcuts permission prompt.

## Markdown viewer

- CommonMark plus GitHub extensions: tables, task lists, footnotes, alerts, autolinks.
- Syntax highlighting with tree-sitter grammars; Mermaid diagrams and LaTeX math rendered to SVG off the UI thread and cached by content hash.
- Live reload on every write, keeping scroll position and briefly highlighting changed blocks.
- Outline sidebar, in-doc search, relative links and images resolved against the file's repo, links between docs opening as tabs.
- Tabs across machines and projects; a "recently written by agents" list per project.
- Read-only by default, with a raw-text toggle for quick edits.

## Diffs and staging

The goal is VS Code's Source Control flow, at native speed.

- Changes and Staged lists per repo or worktree, as a flat list or a tree, with counts.
- Per-file stage, unstage and discard; stage all and unstage all; hunk and line staging too.
- Side-by-side and inline views with syntax highlighting, word-level changes, collapsed unchanged regions, and next and previous change navigation.
- The modified side is editable for small fixes, saved straight to the working tree on the daemon's machine.
- A commit box with commit and amend, plus push, for the times the agent does not.
- Diffs are computed on the daemon and streamed per file; very large files render lazily.
- Status updates live as agents edit, without stealing focus or scroll.

## Review loop

- **Inline comments:** comment on any diff line; comments collect into a review that one key sends to the agent, with file and line for each.
- **Pull requests:** each session shows its PR, CI checks and GitHub review comments, refreshed without hitting GitHub rate limits.
- **Failures come to you:** a failed check or a new review comment notifies you, and one click sends the failure log or comment to the agent.
- **Auto-archive:** a session archives itself when its PR merges, and its worktree is offered for cleanup.

## Terminal

- A panel toggled with Ctrl+\`, opening in the session's directory on the session's machine, with tabs and splits.
- Rendered with `alacritty_terminal` for emulation drawn with GPUI.
- Backed by the session holder's PTY, so it survives closing the app.
- "Open in external terminal" launches the user's terminal app with an SSH session to the same machine and directory.

## Previews and ports

Agents on remote machines start dev servers you cannot otherwise reach.

- The daemon detects ports that agent processes open and forwards them to your machine automatically.
- Each forwarded port appears on its session with an open-in-browser button.
- The phone can open a preview too, relayed through the hub.

## Browser panel

A panel shows the live view of the session's agent browser (see Daemon, Browser for agents), with take-over and an annotate-and-send tool. Forwarded previews open in your own browser by default and here on request.

## Side chat and sharing

- **Side chat** (Cmd/Ctrl+;): ask a question with the session's full context without adding to it. It can be promoted into a real fork.
- **Sharing:** export a session as Markdown, HTML or JSON, or create a read-only link through the hub. The decryption key rides in the link itself, so the hub still cannot read it.
- **Move to machine:** one command moves the current session to another machine, with a preview of what will be copied.

## Overview, search and limits

- **Overview:** the same inbox as a board, for a wide window; one key jumps to the next session that needs you.
- **Command palette** (Cmd/Ctrl+K): sessions, projects, docs, commands and settings, fuzzy matched.
- **Search** (Cmd/Ctrl+Shift+F): full text across all sessions and docs from the local cache, filterable by project, machine, agent and date.
- **Usage limits:** always in the status bar: each Claude and Codex account's windows (5-hour and weekly, or whatever the plan has) as percentages with reset countdowns, turning amber and red near the limit. Account-level and synced across devices; see Daemon, Usage limits.

## Shortcuts

Every action has a command, and every command can be bound in a keymap file. The defaults follow platform conventions, and ? shows a searchable cheat sheet.

| Action | Default |
| --- | --- |
| Command palette | Cmd/Ctrl+K |
| Previous / next of my messages | Alt+Up / Alt+Down |
| Next blocked session | Cmd/Ctrl+J |
| New session | Cmd/Ctrl+N |
| Close session | Cmd/Ctrl+W |
| Toggle docs / diff / terminal | Cmd/Ctrl+1 / 2 / 3, Ctrl+\` |
| Stage / unstage file in diff list | S / U |
| Search everything | Cmd/Ctrl+Shift+F |

## Motion and responsiveness

Motion is spring-based, short and always interruptible: input during an animation retargets it instead of waiting for it.

| Budget | Target |
| --- | --- |
| Key press to pixels | One frame (8 ms at 120 Hz) |
| Open any cached session | Under 50 ms |
| Cold start to usable window | Under 300 ms |
| Animation length | 120 to 250 ms |

- Panels slide and resize, messages settle in as they stream, status dots pulse gently while working.
- Reduced-motion settings swap motion for fades.
- Frame time is measured in development builds, with a visible overlay (gpui-fps) and a CI check on scripted interactions. Scripted input goes through `Window::dispatch_event` with real wheel and key events; `ListState::scroll_by` is not a substitute on a bottom-aligned list ([spike 03](spikes/03-gpui-chat-and-diff.md)).
