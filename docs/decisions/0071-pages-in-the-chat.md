# 0071. Pages the agent shows live in the chat, and it can look first

Status: accepted (2026-10-22, owner's call: "live in the chat", and "all that T3 Code does") · Spec: [desktop.md](../desktop.md), [daemon.md](../daemon.md) · Builds on: [0052](0052-media-visuals-and-secrets.md)

## Decision

- **In the chat.** A page shown with `show_visual` sits in the conversation above the reply, on the chat's background, as wide as the reading column and as tall as the page, rather than beside the chat with a 240 pt still. It takes a viewer tab only when you open it beside.
- **Live where it can be, a picture where it can't.** The system web view is a native layer: GPUI can't clip it, draw over it or move it in step with a scroll. So a page is live only while its whole row is in view, nothing floats over the chat (menus, the palette, the slash menu, a picture opened large, a drag) and the chat has been still for 160 ms. Otherwise its last snapshot is painted in its place. A small pool (three) of web views is lent to the pages on screen; the rest wait with their picture.
- **Fitted, never scrolling inside.** The page reports its height (a ResizeObserver, on timers so a page waiting out of sight still measures), and the row takes it (80 to 4,000 pt). Wheel scrolls it can't use are handed to the chat. Viewport-height units are refused when the page is shown, since a frame that follows the page and a page that follows the frame grow without end.
- **dent's theme, as CSS variables** under the page's own styles (`:where` rules, so the page wins), written into the page before anything of its own runs and updated live; dent's fonts are served from `dent:`. The tool's description tells the agent the layout and the variables.
- **A look before showing** (`preview_visual`). The desktop draws the page off screen in the same web view, fonts and theme, fits it, pictures it (up to 4,000 pt) and gathers console errors. The answer goes back as a new request, `Looked`, named by a key both sides compute from the page (`look_key`). The daemon leaves it where dent's tool waits, as `ask_secret` does. No window in 40 s: the agent is told.

## Why

- The owner finds T3 Code's inline pages the best part of its chat and wants dent to match; a picture card that opens a side panel is a step removed from the conversation.
- Drawing the look in the desktop rather than a headless Chrome (T3's way) needs no browser download, and it draws with the exact engine, fonts and theme the chat uses, so "it fits" means it fits.
- A live page always on top would cover menus and lag behind scrolling; a picture while anything moves or floats keeps the chat honest, and live is what you get while reading.

## Not chosen

- **MCP Apps** (`ui://` resources from other MCP servers, rendered inline with a postMessage bridge, as T3 does). T3 does it only through Codex, whose app-server reads an MCP server's resources for its client. Claude Code doesn't hand them over, so dent would have to be an MCP client of every server itself (spawning them, their sign-ins). It belongs with the Codex driver (#17), or a decision to make dent an MCP host.
- Rendering the look in headless Chrome: a 150 MB download and a different engine from the one that draws the chat.
- Clipping the web view in a native container so it could stay live while partly out of view: more native code, and it still couldn't sit under GPUI's floating chrome.

## Consequences

- A web view lent to a page loads it afresh (another session's page may share its name) and goes live only once that load has said its height; each load is numbered so an earlier page's messages are dropped. Making a web view never brings dent to the front when it wasn't.
- A look waits for the page to load and sit still 0.9 s, so charts that animate in are drawn; it pictures at most 2,000 pt (taller, the model's image limit shrinks the words past reading) and reports the page's real height.
- Looks move on with dent's 8-a-second timer as well as its frames, so one asked for while dent is minimized is drawn and answered all the same (WebKit draws and pictures it there).
- Where no web view can sit in the chat (Wayland, where one is a window of its own), a page is a card naming it that opens it beside the chat.

- `PageEvent::Height` and `Wheel` from inline pages; `Preview::new_inline` (transparent, `INLINE_SCRIPT`).
- The protocol gains `Request::Looked`, `Look` and `look_key` (pinned by a test, with a copy in the fake agent).
- A page's still is kept per page and version while the app runs; a theme change drops them (they'd show the old colours).
