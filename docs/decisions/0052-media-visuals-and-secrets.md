# 0052. Media plays from disk in the web view; visuals and secrets go through dent's own tools

Status: accepted (2026-10-18) · Spec: [desktop.md](../desktop.md), [daemon.md](../daemon.md)

## Decision

- **Video, sound and PDF** open in the viewer's web view, which already plays and shows them: a PDF loads as itself, a video or a sound in a small page of its own. The `dent://` scheme answers `Range` requests from the file on disk, a piece at a time, so a long film never loads whole. A file a reply names (in backticks, a link, a path) that's on disk gets a card under the reply the first time, with a still made by Quick Look (`qlmanage -t`, off the main thread, kept per file and version in the temp folder). A file you sent opens the same way from its chip.
- **Visuals.** dent's MCP server (`dent-daemon plan-tool`) gains `show_visual` (`title`, `html`): it only says it showed; dent reads the page from the call, as it does a plan, and shows it like a page the agent wrote, under a folder no project has (`.dent-visuals/<title>.html`), so nothing is written to the project. A page's card shows a picture of it, from the viewer's own snapshot, once the viewer has drawn it.
- **Secrets.** `ask_secret` (`name`, `why`) shows a card with a masked field. The daemon saves what you type as `NAME=value` in the project's `.env` or in a file of the session's own under its state (made readable by you alone; a `.env` git would track is added to `.gitignore` first), writes where it went to `<state>/asks/<session>/<NAME>.answer`, and the tool, which waited for that file, tells the agent where the secret is and never what. The event log keeps the answer with its value left out; the value isn't sent to the agent or kept by the desktop.
- dent's tools are allowed up front, and the agent's prompt says when to use them.

## Why

- The owner keeps failing to get videos the agent made onto their screen; the web view plays them with nothing new to build or ship.
- A visual that needs a file in the project leaves litter in the repo; the plan tool already showed reading a call's input is enough.
- A secret pasted in chat goes into the transcript and the model's context for good. A card that only the daemon reads keeps it out, and an env file is what projects already load.

## Not chosen

- A native video player (AVFoundation): more code than the web view, for the same playback.
- Drawing visuals live inside the chat: the web view is one native layer over the viewer, so the chat shows a picture and a click opens the live page.
- Handing a secret back as the tool's result: it would be in the transcript, the thing to avoid.
- Asking for a secret through Claude Code's permission prompt: it isn't asked in Full access, dent's default.
