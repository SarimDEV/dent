# Spike 08: desktop concepts — the inbox, starting by typing, and the viewer

`spikes/desktop-concepts` is the desktop design from [0036](../decisions/0036-sessions-as-an-inbox.md) and [0037](../decisions/0037-viewer-system-web-view.md) running in GPUI with fake agents: no agent, account or network is touched.

```
cargo run -p spike-desktop-concepts -- [--dark] [--size WxH] [--pace N] [--reduce-motion] [--background]
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

## Second round: speed, page snapshots, dev servers and pop-outs

- **Long sessions stay fast.** The transcript is a virtualized list that only lays out what's on screen and is patched row by row as messages change; a 214-step session scrolls like a short one. The diff is virtualized too (a 1,712-line lockfile), inside a horizontal scroll sized to its longest line, and keeps its place when a comment opens.
- **Nothing is redrawn without a reason.** The window redraws for page events, a toast ending, a new second on a working timer, and the working squares (8 fps, none with Reduce Motion). Measured on an M-series Mac: 8 frames a second with agents working, 1.3 with Reduce Motion, 0.3 with nothing moving; idle CPU over 30 s fell from 1.49 s to 0.51 s with Reduce Motion. Markdown and pictures are parsed once and cached; sessions are shared, not copied, on each change.
- **A hidden page leaves its picture behind.** When something has to cover the page (search, a menu), dent draws a snapshot of it in its place instead of a blank panel. Snapshots come from the web view itself (WKWebView, WebView2 and WebKitGTK each have one), are decoded off the main thread, and are taken at most every 700 ms after the page settles.
- **Dev servers preview like files.** A running server is a page with a "running" status instead of a version stepper. The stand-in is a small admin app that polls its own API every 2 s; the preview shows it updating, Point works on it, and links to other sites open in the browser instead of inside the panel.
- **Pages pop out.** A page can move to its own window and back; the panel shows where it went, with Show window and Bring it back. Closing the window brings it back.
- **The empty side looks intended.** With nothing open in the viewer, the chat column centres with the composer.
- **Test runs no longer bring the window forward, mostly.** `--background` (implied by `--demo`) runs as an accessory app without activating. wry activates the app each time it creates a web view; dent hands activation back straight after, but the window can still come forward briefly. The checks drive the real window, so keep runs few and announced.
- **Linux and Windows are parked.** On Linux, wry needs an Xlib handle while GPUI gives XCB, so dent wraps it; on Wayland, a child web view isn't possible, so pages open in their own window. In a Linux container the checks passed 38 of 38 on Wayland and 35 of 38 on X11 (the three frame-rate checks). Windows type-checks but has not run. Both wait until after the Mac release.

## Checks

`--demo tour` (16 checks) and `--demo checks` (38) drive the real event path (mouse moves, clicks, keystrokes) and pass:

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
- Esc from an empty composer focuses the inbox, Down moves, E closes and Cmd+Z undoes, all without leaving the inbox; Alt+Cmd+Up moves between sessions from anywhere;
- search covers the page with its snapshot; a page pops out and comes back;
- the dev server's data changes live, an outside link opens in the browser, and Point works on the running app;
- the 214-step transcript opens at its end, the lockfile diff scrolls to line 1,203, and a comment deep in it keeps its place;
- three idle stretches stay under their frame budgets.

One run in fifteen failed a single check while the owner was using the computer: the checks drive the real window, so real input can land mid-step. Run them with hands off.

Stills were checked in Paper and Carbon at 1440×860, 1100×720 and 960×680.

## What changes in the spec

- [desktop.md](../desktop.md): the side panel is the viewer, with its own section; machines move from the sidebar to the status bar; the narrow-window rule.
- [0037](../decisions/0037-viewer-system-web-view.md): the system web view through wry for pages.
- [design-system.md](../design-system.md): Drift is the working checker.
- [desktop.md](../desktop.md): dev servers, snapshots and pop-outs in the viewer; the idle frame budget.
- [risks.md](../risks.md): Linux and Windows deferred.
