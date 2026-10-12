# 0036. Sessions are an inbox; a new session starts by typing

Status: accepted (2026-10-12) · Spec: [desktop.md](../desktop.md), [iphone.md](../iphone.md), [projects.md](../projects.md)

## Decision

There is one list of sessions, on the desktop and the phone, sorted by what it needs from you:

1. **Needs you:** an approval, a question, an error, a failed check.
2. **Done:** finished and waiting for you to steer it or close it. Automation results and finished goals land here too; there is no separate inbox.
3. **Working:** running, or paused until a usage limit resets. Shown lighter, at the bottom, because there's nothing to do yet.

Closing a session takes it out of the list (it stays searchable, and its worktree is offered for cleanup). **Inbox zero** is when everything is working or closed.

A new session opens straight into an empty chat with the cursor in the composer. Under the composer, its settings are already chosen and each one is a single tap to change:

| Setting | Chosen by default |
| --- | --- |
| Project | Filled in from what you type; "New project" or "No project" if nothing fits |
| Machine | The least busy online machine that has the project's repo |
| Model | The one you used last |
| Permissions | What you used last (ask, auto or full access) |
| Worktree | What you used last (new worktree or the main checkout) |

## Why

The owner wants dent to be simple: one place to look, and starting work should feel cheap. Most of the time a working session needs nothing from you, so it shouldn't sit above the ones that do. Treating the list as an inbox gives a clear goal (get it to zero) and makes closing finished work part of the routine. Remembering the last choices fits how the owner works (always full access, always a new worktree) without hiding the choices from anyone who works differently.

## Consequences

- The desktop sidebar becomes this list instead of a tree of projects. Projects become a property of a session, shown in its row and usable as a search filter.
- The phone loses its tab bar: the session list is the app, with search and New Session in the bottom toolbar and Settings behind a gear.
- "The Inbox" for sessions without a project goes away; such a session is in the list like any other, marked "No project".
- The daemon reports which machine is least busy (working sessions and load) so both apps pick the same one.
- Concepts: `spikes/phone-concepts` (screens `home`, `home-zero`, `new`, `new-typed`).
