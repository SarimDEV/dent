# iPhone app

The iPhone app is a SwiftUI client for chatting, approving, reading docs and getting notified. Its networking, sync and cache come from the same Rust client core as the desktop, compiled for iOS and exposed to Swift with UniFFI, so sync rules and the protocol exist in exactly one place.

## Features

- **Home:** projects and sessions with live status; blocked sessions pinned to the top.
- **Chat:** full history from cache, live streaming, send, queue, interrupt, and jump between your own messages. Tool calls collapse as on desktop.
- **Approvals:** inline in chat, and actionable straight from the notification (allow, deny) without opening the app.
- **Notifications:** agent blocked, turn finished, error, limit nearly reached; configurable per project.
- **Markdown viewer:** the same rendering rules as desktop, with live reload while the app is open.
- **Usage limits:** in the app header and as a home-screen widget.
- **Live Activity:** working agents and their status on the lock screen and Dynamic Island while any session is running.
- **Sign-in:** sign in once and every machine is there; approving a new device takes one tap here.
- **Pull requests:** PR and CI status per session, with failures sent to the agent in one tap.
- **Previews:** open a session's forwarded dev server in Safari.
- **Face ID lock:** optional.
- **Long chats stay fast:** the app renders from its cache and pages history in, so even old chats open instantly.
- **Inbox:** automation results and finished goals, ready to continue or discard.
- **Goals:** progress and the time a paused run resumes after a limit reset.
- **Apple Watch:** session status, approvals with allow and deny buttons, and usage limits as a complication.
- **Quick capture:** from the share sheet in Safari or any app, a Shortcuts action, the Action button or a home-screen widget, with the same project routing as desktop.

## Not on the phone

Diffs, staging and the terminal stay desktop-only. The phone can open a session's changed-files list read-only, so you know what changed before you get to a computer.

## iPad and Mac

The same SwiftUI app runs on iPad with hardware-keyboard shortcuts. The Mac uses the GPUI desktop app, not this one.
