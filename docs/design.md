# Design direction

The tokens and rules that build this look (spacing, type scale, states, motion) are in [design-system.md](design-system.md).

dent uses the Field Notes direction: warm paper in light mode, warm carbon in dark mode, fine grain, ink type, and status shown as small dithered pixel squares. HTML/CSS mockups of the main screen, docs, quick switcher and changes panel in both modes are in [design/mockups/](design/mockups/). They were written for a canvas runtime, so treat them as the exact reference for colours, type, sizes and layout rather than pages to open directly.

## Why Field Notes

- **Best for long reading:** chats, plans and diffs are most of the time spent in dent, and calm paper surfaces with roomy type hold up for hours.
- **Distinctive:** most agent apps are dark glass; warm paper with grain and pixel status looks like nothing else in the category.
- **Cheap to draw:** a static grain texture and flat surfaces, with no live blur, protect the frame budget on every platform.

## Borrowed from the alternatives

- **From Typewriter:** ASCII and pixel character as accents, not everywhere: `├─ └─` trees for tool calls, ASCII loaders and empty states, the pixel-font logo for splash and dent.sh.
- **From Phosphor (one cyan):** status told by pattern as well as colour, so it never depends on colour alone.
- **From Mist:** nothing by default; the painterly mood can return later as an optional theme.

## Palette

| Token | Paper (light) | Carbon (dark) | Used for |
| --- | --- | --- | --- |
| Surface | #F1EEE7 | #161513 | Window background |
| Raised | #FAF8F3 | #1F1E1B | Cards, composer, selected rows |
| Panel | #FAF8F3 | #1B1A17 | Docs and changes panels |
| Ink | #171614 | #ECE8DF | Text, primary buttons |
| Quiet ink | #6E6A62 | #8F8A80 | Labels, metadata |
| Faint ink | #8C877D | #706B63 | Line numbers, placeholders, separators (3:1, never for reading) |
| Hairline | #DCD7CC | #2E2C28 | Borders, dividers |
| Working | #2BB8D6 | #4FD3EC | Agent is working |
| Needs you | #E0309F | #F04FB8 | Blocked, approval, question |
| Done | #3FBF4F | #5FD86B | Finished, CI passed |
| Waiting | #E8A317 | #F0B43A | Paused, near a usage limit |
| Diff added | #E2F2DE | #1A2E1C | Added lines |
| Diff removed | #F3E2DA | #33211B | Removed lines: a warm clay tint, kept well away from the Needs-you magenta |

- Status squares dither the signal colour with a paler partner in light mode and a deeper one in dark mode.
- Signal colours are fills, never text on Paper, where they fail contrast.

## Type

The default pairing is called Book: agent replies read like a page, and the interface stays quiet around them.

| Role | Face | Size |
| --- | --- | --- |
| Chat (messages, docs body) | Literata | 15 px, 24 px line height |
| Interface chrome | IBM Plex Sans | 13 px, 20 px line height |
| Code, commands, diffs | Commit Mono | 12 px, 20 px line height |
| Labels (metadata; uppercase only for section labels) | Commit Mono | 11 px |
| Display (session titles, doc headings, quick switcher query) | Instrument Serif, italics for emphasis | 28 px titles, 20 px compact |

- All five faces are bundled (SIL Open Font License), so nothing depends on installed fonts.
- Fonts are theme settings per role, never hard-coded, so a theme can swap any of them. Plex, Fira with Departure Mono pixel labels, and Atkinson Hyperlegible were the runners-up and make good alternative themes.

## Texture and motion

- Grain is a pre-rendered noise texture: multiplied at low opacity on Paper, screened at lower opacity on Carbon.
- Approval cards resolve in with a short dither dissolve, the theme switch is a 200 ms colour tween, and a working session's square drifts, stepped like pixel animation; reduced-motion settings make all of it instant. The full motion vocabulary and its rules are in [design-system.md](design-system.md).
- Nothing blurs live behind scrolling content, and every effect is held to the frame budgets in Reliability requirements.

## Fixes before building

- Make status squares larger and give each status its own pattern (solid, checker, dots, outline).
- Quiet the hard offset shadow on approval cards; it reads loud, especially in light mode.
- Shrink the serif session title as the chat scrolls so it stops costing vertical space.

## Theming

dent has its own small theme format and imports themes from other ecosystems, rather than copying any one app's format.

- **Format:** about 40 named colours with dent meanings (the palette tokens above plus syntax colours keyed to tree-sitter names), and dent's style settings: a font per role (chat, chrome, code, labels, display), grain amount, status patterns, corner radius and motion speed. Theme files reload live as they are edited.
- **Defaults:** Field Notes Paper and Field Notes Carbon, following the system appearance.

| Source | What it brings | How |
| --- | --- | --- |
| VS Code themes | Syntax colours (near one to one) and UI colours (mapped to dent surfaces) | Converted from Open VSX or from themes installed locally; the VS Code Marketplace's terms limit it to Microsoft products |
| Base16 / tinted-theming | Hundreds of complete 16-colour palettes | Every scheme fills the same slots, so each maps onto the whole app |
| Terminal schemes | ANSI colours for the embedded terminal | The iTerm2 colour scheme collection |
| Zed themes | The closest structure, since Zed also uses GPUI | Direct mapping |
| Obsidian themes | Not for the app: they are CSS for Obsidian's web layout | Possibly for the markdown viewer later |

- **Official ports:** a documented format and a template make it easy for families like Catppuccin, Rosé Pine, Dracula, Nord, Gruvbox and Tokyo Night to publish dent ports.
- **Licenses:** converting on the user's machine is fine; bundling a third-party theme in dent needs its license checked, though most are MIT.
- **Texture:** imported themes bring colours only, so grain and patterns get defaults the user can adjust.

