# 0050. Sessions title themselves; what you've seen is kept by the daemon

Status: accepted (2026-10-17) · Spec: [desktop.md](../desktop.md), [daemon.md](../daemon.md)

## Decision

- **Titles.** After a session's first turn, the daemon asks Claude Code for a 3 to 6 word title: `claude -p` on the `haiku` model, one turn, every tool disallowed, given the first messages and replies (the start and the latest few when the conversation is long). It runs in dent's state folder (`titles/`), so it leaves nothing in the project's Claude Code history, and the answer is logged as a `settings` event like a rename. A session is titled on its own once; a session you've renamed, one picked up from a terminal, or one from before this change is never titled on its own. **Regenerate Title** in a session's menu asks again, whatever its name.
- **Seen marks.** Looking at a session (it is open and the window is in front) logs `seen { upto }` in the session's event log. A session is unread while the last event that finished a turn or asked something is past its mark. On the first start with this change, every existing session gets a mark, so an upgrade marks nothing as new.

## Why

- Titles were the first 60 characters of the first message. Claude Code titles its own sessions (`ai-title`), but not when run headless as dent runs it: none of 51 recent headless transcripts on the owner's Mac had one. A small model costs a sliver of the person's limits once per session, and the title then says what the session is about.
- Seen state belongs with the session, not one window's prefs: another desktop or the phone (phase 4 and 8) should agree on what's new. An event per look is cheap, and clients send one only when there is something new to mark.

## Not chosen

- Reading Claude Code's transcript for `ai-title`: headless runs don't write one.
- Forking the conversation to title it (as Ask on the side does): each fork copies the transcript into the project's history.
