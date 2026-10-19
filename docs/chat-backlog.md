# Chat backlog

What's left to make the single-agent chat as close to perfect as it can be, collected on 2026-10-17. Nothing here is decided until it lands in [desktop.md](desktop.md) (and a decision record where it needs one).

**Keep multiplayer in mind.** More machines and more agents are coming ([build-order.md](build-order.md), phases 3–4). So state about a session (read or unread, waiting to resume, background shells, its title) belongs to the daemon and its event log, never the desktop's prefs, so every client and machine agrees. The desktop only keeps what is truly per-window (scroll, open panels). Anything the agent does goes through the driver layer, so Codex and ACP can fill it in later.

## 1. Check first (possible bugs)

- [x] **Scroll position per session.** Already kept (`read_at` in `sync_transcript`). Switch away from a session scrolled halfway back, then return: does it come back to the same place, or jump to the end? The chat follows the tail (`is_following_tail` in `chat.rs`), but nothing obvious keeps a per-session place. Expected: the same place, unless you were at the end.
- [x] **Scrolling up while a reply streams.** Following pauses when you scroll back (`pause_following_tail`); Latest returns. Scroll up mid-reply: the chat must stop following and stay still while text arrives below; scrolling back to the end (or Latest) follows again. Check with a long reply and a capped table arriving.
- [x] **Clicking a notification.** Now opens at the end of the session, also when it's already open. A click opens its session (`notify.rs`). Check that it also lands on what needs you (the approval, question or plan card, or the end of the finished reply), with the keyboard where it should be.
- [x] **Coming back after a turn finished elsewhere.** Coming back to the window marks the open session seen. Send, switch to another app, come back after the turn is done: the chat should be at the end of the reply (or where you left it, if you'd scrolled back), the reply box focused, the session no longer marked as needing you, no animation replaying, and no flash of the working line.
- [x] **What a failed send does.** Already right: unanswered requests are kept and sent again after reconnecting (`live.rs`), and the daemon ignores a message id it has seen, so it goes once. Send while the daemon is down or restarting: the message must never vanish. It comes back to the box, or waits and goes when the daemon is back, with its files.

## 2. Now: small things you'd notice in a day

- [x] **Unread.** A session that finished or changed while you weren't looking gets a dot in the sidebar until opened. Read state lives in the daemon (per user, later per device), not desktop prefs.
- [x] **Draft mark.** A session with unsent words or files in its box shows a small mark in the sidebar.
- [x] **Dock badge** with the count of sessions that need you, the same count as the status bar.
- [x] **Thinking time.** "Thought for 12s" (with the word count kept when opened), from the thinking block's start and end.
- [x] **Time per tool call** on its row once finished ("Ran the tests · 3m 12s"), so a slow step stands out.
- [x] **Shift-click to select several sessions** in the sidebar, then close (or pin) them together, with one Undo.
- [x] **Regenerate title.** (Claude Code writes no `ai-title` when run headless, so dent titles sessions itself: [0050](decisions/0050-titles-and-seen-marks.md).) Today a session is titled with the first 60 characters of its first message (`daemon::title`). Claude Code writes its own `ai-title` into its transcript, and dent already reads it for terminal sessions (`terminal.rs`). Use it for dent's own sessions as it arrives. Regenerate title (sidebar right-click) runs a short-lived, read-only Claude Code on the conversation, as Ask on the side does (`run_aside`), on a small model, and asks for a few words.
- [x] **Global hotkey** to bring dent forward into a new session's field from any app. Pick a default that doesn't clash, and make it changeable.

## 3. Now: turns that survive limits, crashes and sleep

- [x] **Retry by itself on a passing error** (Claude Code already retries and dent shows it on the working line; nothing to build) (overloaded, a dropped stream, a 5xx), a few times, with backoff and a countdown on the working line ("Overloaded, retrying in 8s"), and Stop still stops. Only after that does the turn end in the error note with Try again.
- [x] **Resume when the limit resets.** A turn stopped by a usage limit offers **Resume at reset** beside Try again, and the session shows as Limited in the sidebar with the time. The daemon holds the resume (it fires with the app closed), and waiting messages go after it. Phase 3's goals and auto-resume build on this.
- [x] **Continue after a crash or reboot.** A turn that something other than you stopped (Claude Code crashing, the Mac restarting) offers **Continue**: it resumes the agent's session and tells it what was cut off, including any background work that died. Later, maybe a setting that continues on its own.
- [x] **Keep the Mac awake while an agent works**, the way `caffeinate -i` does (a power assertion held by the daemon only while a turn runs, let go when idle or on low battery), so a closed-lid session on power keeps working. This needs a decision record.
- [x] **Background shells.** Dev servers, watchers and long builds the agent started in the background are listed while they run (from the agent's own tool calls), with their output, a Stop each, and a count beside the working line once the turn ends. Stopping a turn asks whether to stop them too.

## 4. Next: the composer and context

- [x] **`@` a session as context.** `@` offers sessions as well as files (or drag a session from the sidebar into the box). It attaches a summary and a pointer to that session, and works across machines later by session id.
- [x] **Send and start another.** One key sends the message and opens a new session's field. Another starts a new session from what's typed without leaving the one you're in. Check the shortcut map first.
- [x] **Saved prompts are skills.** Claude Code already treats a reusable prompt as a skill (`.claude/skills/…/SKILL.md`, or the older `.claude/commands/*.md`), and dent's `/` menu already lists them. So instead of a separate prompt library, add **Save as skill…** to a sent message's actions: name it, pick this project or everywhere, and it's in the `/` menu from then on.
- [x] **Searching what you've sent:** the popover listing your messages, with search, already planned under Jump to my messages.

## 5. Next: what the chat shows

- [x] **Tool screenshots as pictures**: an image a tool returns shows in its row, opens in the lightbox, rather than a path.
- [x] **Compact before sending** when the context is nearly full: a chip in the composer offers to compact first.
- [x] **A limit note that offers a way round:** besides Resume at reset, switch to another model, and (once there are several accounts) another account.
- [x] **Emoji-only messages shown large.**
- [x] **MCP server status in the chat**, when one disconnects or needs signing in, without opening `/mcp`.
- [x] **The agent's latest thought** as a live line while it thinks, not only "Working…".
- [x] **Earlier versions of an edited message:** after Edit from here, the old message and its turns stay reachable (‹ 2 / 3 › on the message) instead of being lost unless forked. The daemon would keep them as branches in the event log.

## 6. Unfinished from the spec

- [x] Parsing off the UI thread ([chat-rendering.md](chat-rendering.md), "Not built yet").
- [x] A table's Open side panel, and Find reaching rows a capped table keeps out of view.
- [x] Row-at-a-time parsing for a table still streaming.

## 7. From the October parity check

- [x] **Video, sound and PDF** play or show in the viewer, with a card under a reply that names one ([0052](decisions/0052-media-visuals-and-secrets.md)).
- [x] **Visuals** the agent shows with `show_visual`, and a picture of a page above its card.
- [x] **A private card for secrets** (`ask_secret`).
- [x] **Tab steps the effort.**
- [x] **Question cards:** files with a typed answer, Skip, ⌫ back.
- [x] **Restart the agent** to load new skills and plugins.
- [x] **A hook that turns a message away says why**, tested against Claude Code 2.1.301.
- [x] **One sound** when several sessions finish together.
- [ ] Undecided: snoozing a session until a time; stepping between turns.

## Not now

Still possible later, set aside for now:

- Searching back through what you've sent (as Ctrl+R does in a shell): undecided.
- Cost or tokens per turn; ⌘↑ / ⌘↓ to the top or end of the chat; collapse or expand every tool call.
- Dictation (press and hold to talk): maybe later. First check that the Mac's own dictation (fn twice) types into the reply box at all. Spellcheck isn't needed.
- Trying a turn again with a different model.
- A chat in its own window, or two side by side.
- Exporting a session as Markdown; saving a code block or a picture to a file.

## Decided against

- **A setting for Return queuing vs steering**: dent keeps Return queues, ⌘Return steers.
- **Open the project in an editor, branch in the header**: dent isn't for reading code.
