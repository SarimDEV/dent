# 0037. The viewer: everything a session made, beside the chat; pages run in the system web view

Status: accepted (2026-10-12) · Spec: [desktop.md](../desktop.md#viewer), [chat-rendering.md](../chat-rendering.md) · Spike: [08](../spikes/08-desktop-concepts.md)

## Decision

The desktop's side panel is the **viewer**. It shows, as tabs, everything the open session made or changed: pages, docs, images, its changes and its terminal. Every viewable file an agent writes becomes a card in the chat that opens it there, and writing the same file again adds a version rather than replacing it.

- **Pages** (HTML, and local dev servers) run live in the operating system's own web view (WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux), placed over the panel as a child of the GPUI window through [wry](https://github.com/tauri-apps/wry), pinned at `=0.57.0`.
- Pages load from memory through a `dent://` scheme with a content policy that blocks the network and frames, so a preview is offline and isolated from everything but its own files.
- A small script injected into each page reports errors and element picks back to dent. It never runs in the agent's environment.
- Docs, images, diffs and the terminal stay native GPUI, as [chat-rendering.md](../chat-rendering.md) already says.

## Why

Agents make web pages constantly, and a page only means something when it runs. A real browser engine renders the page exactly as the person's browser will, costs nothing to bundle (it ships with the OS) and starts in milliseconds. Writing or embedding our own engine would be enormous; a separate browser window loses the side-by-side loop of looking, pointing and asking. wry is the maintained, cross-platform wrapper for exactly this, and GPUI already exposes the raw window handle it needs.

## Consequences

- The web view is a native layer above GPUI's drawing. GPUI can't draw over it, so menus, tooltips and toasts must not overlap the preview area: the preview hides while a menu is open, and floating UI is placed away from it (the close toast sits over the sidebar). This is the main cost of the choice.
- Linux needs GTK for WebKitGTK, and wry's child web views there work on X11 only. Wayland support is a known gap to check before the Linux release.
- One web view is shared and reloaded per page and version, so memory stays flat however many pages a session makes. Live reload keeps the scroll position.
- The `dent://` policy means pages that fetch from the network show a blocked request rather than silently working; dev-server previews (forwarded ports) load over `http://localhost` instead and are not offline.
