# Desktop app

The desktop app is a GPUI client for macOS, Linux and Windows, built around a chat column with docs, diffs and a terminal one keystroke away. It never blocks on the network: everything renders from the local cache first.

## Layout

- **Sidebar:** one list of sessions across every machine, sorted as an inbox (decision [0036](decisions/0036-sessions-as-an-inbox.md)): Needs you, then Done, then Working, lighter and with a running timer, at the bottom. Each row shows its status, title, project and what it is doing in plain words. Hovering a done row shows Close; Cmd+W closes the open session and opens the next one to clear. Closing archives, with an Undo over the sidebar for 8 seconds (Cmd+Z), and a Closed section at the end reopens anything; inbox zero is when everything is working or closed. A session still working can't be closed; Cmd+W says so instead. Right-click a row for Rename, Fork, Close or Reopen, and its folder. The top of the sidebar holds the traffic lights, search and New session.
- **Main:** the selected session's chat, or the new-session page.
- **Viewer:** everything the session made or changed, beside the chat (below). Toggled with Cmd+\\ and resized from its edge; can pop out into its own window.
- **Status bar:** usage limits, machines online, and how many sessions need you (Cmd+J jumps to the next).
- **Narrow windows:** the chat keeps at least 440 pt. The viewer shrinks first, down to 360 pt; below that the sidebar folds away behind a button in the chat header, and the status bar still shows what needs you.

## Chat

- Virtualized message list that stays at 120 fps with tens of thousands of events; streaming text renders incrementally without re-laying-out earlier messages.
- Tool calls collapse to one line ("Edited 3 files", "Ran tests: 2 failed") and expand inline; file edits open straight into the diff view.
- Approvals appear inline with allow once, allow for session, and deny with a reason.
- **Jump to my messages:** a joined up/down pair in the chat header, beside Fork and Close, steps through your own messages; Alt+Up and Alt+Down do the same. An arrow with nowhere to go dims and says so. Once you've scrolled back, a "Latest" pill above the composer returns to the end. Still to come: a popover listing them all with search.
- **Selectable text:** replies, your messages, code, tool lines, notes, diffs and the terminal all join one window-wide selection (`select.rs`), so a drag can run across blocks and ⌘ C copies it. Links still open on a plain click.
- Composer: multi-line, drafts saved per session as shared state, file and doc mentions with fuzzy completion, image paste, the agent's slash commands, and queued messages while the agent is working.
- Interrupt, retry the last turn, fork a session from any message, and rename sessions (auto-titled by default).
- **Fork:** the header's Fork, or Fork from here on any message you sent, opens a new session carrying the conversation so far; its agent resumes it as a new agent session, so the original goes on untouched.
- **Edit from here:** hovering a message you sent offers it; you confirm in place whether to rewind the chat or the chat and files, and the message returns to the composer.
- **Attachments:** files dropped on the window or pictures pasted with Cmd+V ride along as chips. Pictures reach the agent as images; every file is also named by its path.
- **Notifications** when a session needs you or finishes, unless you're looking at it.

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

## Viewer

The viewer shows, as tabs, everything the open session made or changed ([0037](decisions/0037-viewer-system-web-view.md)). Every viewable file an agent writes becomes a card in the chat that opens it here. Writing the same file again adds a version; nothing is replaced. A card for an older version says what replaced it.

- **Pages** (HTML files, and forwarded dev servers) run live in the system web view through a `dent://` scheme, with the network like any page.
  - **Versions:** a stepper (v3 of 4). An older version shows a "Back to v4" bar. A new version shows straight away when you're following the latest, with a brief "updated to v4" and the scroll position kept.
  - **Widths:** desktop, tablet (768) and phone (390), the narrow ones framed in the panel.
  - **Point:** click anything on the page, and the element (tag, text, selector) rides along with your next message as a chip you can remove.
  - **Errors:** page errors appear in a bar under the page with **Ask to fix**, which sends the error to the agent straight away.
  - **Dev servers:** an address like `http://localhost:5173` in a command's output or a reply becomes a card and a live page, loaded by the address the server answers on (127.0.0.1 or ::1), since WebKit here won't load `localhost` by name. A running server shows "running" instead of versions. Links to other sites open in your browser.
  - **Pop out:** a page can move to its own window and back. The panel says where it went; closing the window brings it back.
  - **Covered pages:** when search or a menu has to cover the page, dent shows the page's last snapshot in its place.
  - Reload and open in your browser.
- **Docs:** Markdown rendered as a page (see Markdown viewer), with copy and open in editor.
- **Images:** screenshots and charts the agent made, fitted to the panel, with actual size.
- **Changes:** the session's files with counts and the diff, virtualized so a lockfile of thousands of lines scrolls freely. Click a line number to comment; comments collect into one review that **Send to agent** delivers with file and line for each (see Review loop).
- **Terminal:** your own shell in the session's folder (see Terminal).
- **Keys:** Cmd+1 is the first thing the session made, Cmd+2 changes, Cmd+3 the terminal.
- The web view is a native layer, so nothing floating may overlap it: the preview hides while a menu is open, and toasts sit over the sidebar.

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
- Syntax highlighting with tree-sitter grammars.
- TeX maths drawn natively from RaTeX: inline, `$$…$$` and ` ```math ` blocks. Mermaid diagrams are rendered off the UI thread and cached by source and theme, with their source shown dimmed until they're ready.
- Tables share column widths. Past 13 rows a table shows 10½ and scrolls on its own, with a row count, Copy (Markdown) and Copy CSV. Display maths, diagrams and small tables offer Copy on hover. Wide blocks scroll sideways ([chat-rendering.md](chat-rendering.md#sizes-and-overflow)).
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

- Your own login shell in the session's folder, in the viewer's Terminal tab, opened with Cmd+3, Ctrl+\` or the header's terminal button ([0042](decisions/0042-terminal-alacritty.md)). The agent never sees it or types into it.
- Rendered with `alacritty_terminal` for emulation, drawn with GPUI in the code face.
- Every key without Cmd goes to the shell while it has focus; Cmd+C and Cmd+V copy and paste.
- Later: running on the session's machine through the session holder's PTY, so it survives closing the app; tabs and splits.

## Previews and ports

Agents on remote machines start dev servers you cannot otherwise reach.

- The daemon detects ports that agent processes open and forwards them to your machine automatically.
- Each forwarded port appears on its session and opens in the viewer as a live page, or in your own browser.
- The phone can open a preview too, relayed through the hub.

## Browser panel

A panel shows the live view of the session's agent browser (see Daemon, Browser for agents), with take-over and an annotate-and-send tool. Forwarded previews open in your own browser by default and here on request.

## Side chat and sharing

- **Side chat** (Cmd/Ctrl+;, or the button beside Fork): ask a question with the session's full context without adding to it. It opens as a tab in the right pane. Each question runs a short-lived Claude Code that resumes the conversation as a fork, read-only (no commands, no edits), so the session itself never sees it; questions and answers are kept in the session's log, apart from the conversation. **Make it a fork** turns an answer into a new session that carries on from that forked conversation. A question still open when dent restarts is marked unanswered.
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
| Viewer: first artifact / changes / terminal | Cmd/Ctrl+1 / 2 / 3 |
| Toggle the viewer | Cmd/Ctrl+\\ |
| Stop the agent | Cmd/Ctrl+. |
| Undo close | Cmd/Ctrl+Z |
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
| Redraws with nothing moving | Under 1 a second |

- Panels slide and resize, messages settle in as they stream, status dots pulse gently while working.
- Reduced-motion settings swap motion for fades.
- The window redraws only when something on screen changes: a page event, a timer's second, a toast ending, the working squares. Long transcripts and diffs are virtualized lists that lay out only what's visible.
- Frame time is measured in development builds, with a visible overlay (gpui-fps) and a CI check on scripted interactions. Scripted input goes through `Window::dispatch_event` with real wheel and key events; `ListState::scroll_by` is not a substitute on a bottom-aligned list ([spike 03](spikes/03-gpui-chat-and-diff.md)).
