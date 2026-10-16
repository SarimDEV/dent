# 0049. Ambient backgrounds on the new-session page

Status: accepted (2026-10-15) · Spec: [ambient.md](../ambient.md), [desktop.md](../desktop.md), [design-system.md](../design-system.md)

## Decision

- The new-session page can show an ambient background, chosen in View › Ambient Background (None by default, kept in prefs by id; an unknown id means none).
- Backgrounds live in a new crate, `dent-ambient`, with no dependencies and no GPUI. Each draws into a small display list that the desktop paints. A registry (`ALL`) drives the menu, the walkthrough and the tests, so adding one is a file and a line.
- They read real data: tool calls, files touched and sessions started come only from live timelines. Empty state lists (session names, recent titles, project files, machines, usage) fall back to a stand-in.
- They animate at the display's full rate, only on the new-session page, only while the window is visible, and never with reduce motion on. Starting a session stops them.
- No dither: smooth gradients, fades, lines and stipple dots.
- Testing stays out of the inner loop: fast unit checks always (under 0.1 s); the stress pass (`just ambient-stress`) and the walkthrough (`just ambient-walk`) only on demand.

## Why

The owner wanted something of dent's own on the empty page, useful rather than decorative, and cheap enough not to matter. Keeping the simulations free of GPUI and I/O keeps them testable in milliseconds, keeps the desktop's share to one painter, and leaves the door open to other clients. Real events only, so a background never claims work that didn't happen.

## Consequences

- This is a scoped exception to "motion explains a change, it never decorates" and to "only the working line repaints a quiet window": the new-session page with a background chosen redraws every frame. With none chosen (the default), nothing changes.
- Prism and Spectrograph use a six-colour spectrum (`Theme::spectrum`), an exception to colour meaning status. Needs-you magenta stays out.
- Survey network's legs are estimates (`~NN ms`) until dent measures latency between machines.
