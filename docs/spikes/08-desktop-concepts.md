# Spike 08: desktop concepts — the inbox, starting by typing, and the viewer

`spikes/desktop-concepts` is the desktop design from [0036](../decisions/0036-sessions-as-an-inbox.md) and [0037](../decisions/0037-viewer-system-web-view.md) running in GPUI with fake agents: no agent, account or network is touched.

```
cargo run -p spike-desktop-concepts -- [--dark] [--size WxH] [--pace N] [--reduce-motion]
cargo run -p spike-desktop-concepts -- --demo tour   --shots DIR [--record FILE.mov]
cargo run -p spike-desktop-concepts -- --demo checks --pace 0.5
```

Stills and recordings show the dent window only. Set `WINID` to a build of `tools/winid.swift` so stills capture the window by number. `--record` needs `RECORDER` set to a build of `tools/record-window.swift`, which records that one window through ScreenCaptureKit; there is no screen-recording fallback. The window capture draws the transparent title bar's traffic lights as a dark blob, which the encode step paints over.

## What it shows

- **The inbox** in the sidebar: Needs you, Done, Working (lighter, with live timers), and a Closed disclosure. Hovering a done row shows Close; Cmd+W closes the open session and opens the next one to clear. Undo sits in a toast over the sidebar for 8 seconds, with Cmd+Z.
- **Starting by typing:** Cmd+N opens a page with one box. The project follows the words a beat behind until you pick one; machine (least busy), model, permissions and worktree are remembered, each a click to change.
- **The chat:** tool calls fold to one tree line, files the agent made become cards that open in the viewer, approvals and questions are answered in place, sending while it works queues the message below the reply, and Stop really stops, with Resume.
- **The viewer:** a live page with a version stepper, desktop, tablet and phone widths, Point (click an element and it rides along with your next message), and console errors with Ask to fix; a rendered doc; an image; changes with click-a-line review comments sent as one review; the terminal.

## Findings

- **The system web view embeds cleanly in a GPUI window.** wry's `build_as_child` takes GPUI's window handle directly; the page sits exactly over a GPUI-measured rectangle (positioned from a `canvas` prepaint each frame) and moves with panel resizes and viewport changes without lag. IPC, injected scripts and a custom scheme all work.
- **The web view is always on top of GPUI drawing.** Anything floating has to avoid the preview or hide it. The concept hides the preview while a menu is open and keeps toasts over the sidebar. Tooltips on the preview toolbar can fall behind the page; toolbar controls carry text labels where it matters.
- **Error capture and pointing need nothing from the page.** A 60-line init script reports `error` events and `console.error`, and a capture-phase click listener turns a click into tag, text and selector. Live reload keeps scroll through `sessionStorage` on the `dent://` origin.
- **Typing into gpui-base's textarea from a script needs real keystrokes.** `set_value` doesn't emit a change event, so the project guess never ran under the script until typing went through `window.dispatch_keystroke`, the same path as a person.
- **Narrow windows need a rule.** At 1100 px the viewer crushed the chat. Now the chat keeps 440 px, the viewer shrinks to 360 first, and below that the sidebar folds away behind a button in the chat header while the status bar keeps the needs-you count.
- **Design review, two passes.** The first found dead controls, magenta used for page errors on a Done session and for removed diff lines, a broken Changes panel in narrow windows, no keyboard path through the inbox, duplicated controls and tutorial copy. All fixed: a page error now moves its session into Needs you, removed lines use a warm clay tint (design.md updated), Changes has a compact mode under 560 px, Esc focuses the inbox (arrows, Enter, E), Cmd+K searches every session including closed ones, and preview toolbar controls describe themselves in the toolbar, because tooltips would fall behind the page. The second pass confirmed the fixes.
- **Status squares now match the phone.** On the desktop, Waiting and Idle were both outlines that differed only in colour. Every status now has its own 4×4 pattern (needs solid, done a corner step, working a drifting checker, waiting dots, idle an outline).

## Checks

`--demo tour` (16 checks) and `--demo checks` (24) drive the real event path (mouse moves, clicks, keystrokes) and pass:

- the preview reports the page's error; Ask to fix sends it; the fix arrives as v3 with no errors and the preview follows it;
- pointing at the Pro button attaches it; the change arrives as v4; an older version opens with a way back;
- Allow runs the command and the session finishes; a line comment joins the review and sending it starts the agent;
- Cmd+W closes a done session with Undo, Undo restores it; a needs-you session can't be closed; closing opens the next session in inbox order;
- a double press on Send queues once and never stops; the queued message runs after the reply, and every reply lands before the message after it;
- nothing arrives after Stop, Stop offers Resume, Resume restarts;
- a 14-line draft keeps the composer inside the window with room to read;
- drafts stay with their session; the project follows the words until picked; menus close on Escape; the live page hides behind other tabs and returns on its own;
- a page error moves its session into Needs you;
- Cmd+K finds a closed session and opening it reopens it;
- Esc from an empty composer focuses the inbox, Down moves, E closes and Cmd+Z undoes, all without leaving the inbox; Alt+Cmd+Up moves between sessions from anywhere.

One run in fifteen failed a single check while the owner was using the computer: the checks drive the real window, so real input can land mid-step. Run them with hands off.

Stills were checked in Paper and Carbon at 1440×860, 1100×720 and 960×680.

## What changes in the spec

- [desktop.md](../desktop.md): the side panel is the viewer, with its own section; machines move from the sidebar to the status bar; the narrow-window rule.
- [0037](../decisions/0037-viewer-system-web-view.md): the system web view through wry for pages.
- [design-system.md](../design-system.md): Drift is the working checker.
