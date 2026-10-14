# 0037. The viewer: everything a session made, beside the chat; pages run in the system web view

Status: accepted (2026-10-12) · Spec: [desktop.md](../desktop.md#viewer), [chat-rendering.md](../chat-rendering.md) · Spike: [08](../spikes/08-desktop-concepts.md)

## Decision

The desktop's side panel is the **viewer**. It shows, as tabs, everything the open session made or changed: pages, docs, images, its changes and its terminal. Every viewable file an agent writes becomes a card in the chat that opens it there, and writing the same file again adds a version rather than replacing it.

- **Pages** (HTML, and local dev servers) run live in the operating system's own web view (WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux), placed over the panel as a child of the GPUI window through [wry](https://github.com/tauri-apps/wry), pinned at `=0.57.0`.
- Pages load from memory through a `dent://` scheme. Their own files resolve beside them, and links from the site's root are found from the page's folder upwards, the way a static site is served. Pages may use the network like any page in a browser: fonts, styles, scripts and images from the web, and requests out. (Until 2026-10-13 a content policy kept previews offline; agents' pages lean on the web too often for that to be useful.)
- A small script injected into each page reports errors and element picks back to dent. It never runs in the agent's environment.
- Docs, images, diffs and the terminal stay native GPUI, as [chat-rendering.md](../chat-rendering.md) already says.

## Why

Agents make web pages constantly, and a page only means something when it runs. A real browser engine renders the page exactly as the person's browser will, costs nothing to bundle (it ships with the OS) and starts in milliseconds. Writing or embedding our own engine would be enormous; a separate browser window loses the side-by-side loop of looking, pointing and asking. wry is the maintained, cross-platform wrapper for exactly this, and GPUI already exposes the raw window handle it needs.

## Consequences

- The web view is a native layer above GPUI's drawing. GPUI can't draw over it, so menus, tooltips and toasts must not overlap the preview area: the preview hides while a menu is open, and floating UI is placed away from it (the close toast sits over the sidebar). This is the main cost of the choice.
- Since GPUI can't draw over the page, dent draws the page's own snapshot in its place whenever something must cover it, so hiding it doesn't leave a blank panel.
- Linux needs GTK for WebKitGTK. Child web views work on X11 (through an Xlib handle dent builds from GPUI's XCB one); on Wayland the page opens in its own window. Linux and Windows are deferred until after the Mac release.
- wry activates the app whenever it creates a web view, so dent hands activation back when it runs in the background.
- One web view is shared and reloaded per page and version, so memory stays flat however many pages a session makes. Live reload keeps the scroll position.
- A page an agent wrote runs with the network, as it would in a browser. Links to other sites still open in the person's browser, since the viewer has no history to come back with. Dev-server previews (forwarded ports) load over `http://localhost`.
