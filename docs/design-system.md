# Design system

The rules every dent surface follows: spacing, sizes, type, shape, colour, elevation, states and motion. [design.md](design.md) says what Field Notes looks like; this file says how it is built. Every value here is a named token in code, and nothing in a component is a raw number.

## Principles

1. **The page comes first.** Chat and docs are set like a book page; chrome is quieter, smaller and lower in contrast, so the eye lands on the reading column.
2. **One loud thing.** Magenta means "needs you" and nothing else. Primary actions are ink, focus is ink, selection is a neutral tint.
3. **Structure by hairline, elevation only for what floats.** Everything in the page plane is flat with 1px hairlines. Only menus, tooltips, the palette and toasts get a shadow.
4. **Motion explains a change, it never decorates.** Things that happen tens of times a day, or from the keyboard, are instant. Rare and important things (an approval arriving, a theme switch) earn motion.
5. **Effects fit the frame budget.** Motion is opacity, offset, colour and flat quads. No live blur, no animated shadows, no relayout of text per frame. An animation stops requesting frames the moment it settles, so a quiet window costs nothing.

## Spacing

A 4 px grid, with 2 px allowed only inside a component for optical fixes.

| Token | px | Typical use |
| --- | --- | --- |
| `space.half` | 2 | Inside chips, status square insets |
| `space.1` | 4 | Icon to label, tight inline gaps |
| `space.1h` | 6 | Gap between a status square and its text |
| `space.2` | 8 | Control padding, gap between buttons |
| `space.3` | 12 | Card padding, gap between paragraphs |
| `space.4` | 16 | Panel gutter, gap between chat turns |
| `space.6` | 24 | Gap between regions |
| `space.8` | 32 | Chat column gutter |
| `space.12` | 48 | Empty states |

- **Inside is tighter than between:** the gap inside a component is always smaller than the gap around it. Tool lines in a run sit 2 apart, 8 from the reply text around them; paragraphs of reply sit 16 apart.
- **One gutter per region:** panels use 16, the chat column uses 32, and everything inside a region aligns to its gutter (the composer's text lines up with message text).

## Sizes

| Token | px | Use |
| --- | --- | --- |
| `size.header` | 44 | Every panel header (sidebar, chat, diff, docs), so all column tops align |
| `size.row` | 28 | Sidebar and list rows |
| `size.toolbar` | 40 | Viewer toolbars under a panel header |
| `size.line` | 20 | Code, diff and tool lines (equals the code and UI line height) |
| `size.control` | 28 | Buttons and inputs |
| `size.control.sm` | 22 | Pills, inline buttons |
| `size.status` | 28 | Status bar |
| `size.square` | 12 | Status squares |
| `size.column` | 720 | Reading column max width |
| `size.measure` | 540 | Longest line of running text (about 78 characters) |

Hit targets are at least 24 px even when the visual is smaller.

## Type

Five sizes, two weights, three inks. Line heights sit on the 4 px grid.

| Token | Face | Size / line | Use |
| --- | --- | --- | --- |
| `type.display` | Instrument Serif | 28 / 32, compact 20 / 24 | Session title, doc title, palette query |
| `type.reading` | Literata | 15 / 24 | Agent replies, user messages, docs body, composer |
| `type.ui` | IBM Plex Sans | 13 / 20 | Sidebar titles, buttons, menus, tool lines, the working line |
| `type.code` | Commit Mono | 12 / 20 | Code, diffs, commands, a tool call's input and output |
| `type.label` | IBM Plex Sans | 11 / 16 | Metadata and section labels |
| `type.numeral` | Commit Mono | 11 / 16 | Numbers that tick: the working line's timer, a row's age |

- **Text size (⌘+ / ⌘−) scales `type.reading` and `type.code` only**, lines rounded to whole pixels; display, UI and label sizes, and every control and gutter, stay as designed. Anything measured from those two types reads them through `Type::zoomed()`.
- **Tool lines are two inks:** the verb in `quiet`, what it worked on in `secondary` (between `quiet` and `ink`), both to `ink` on hover. A count of work ("Edited 2 files, read 3 files") is one quiet phrase. The chevron trails the label.
- **Hierarchy comes from ink and weight before size.** Inks: `ink` (content), `quiet` (secondary, metadata), `faint` (line numbers, placeholders, separators). Weights: regular and medium only.
- **Uppercase is for section labels only** (`PROJECTS`, `NEEDS YOU`). Inline metadata (`claude · mac-mini · 100,003 events`) stays lowercase.
- **Numbers that tick** (timers, ages) are set in Commit Mono (`type.numeral`) so every digit is the same width and nothing shifts as they count. Everything else small is the interface face: mono is for code, commands and those numbers, so a screen carries three faces, not four ([0047](decisions/0047-calmer-work.md)).
- `·` separates metadata, in `faint`.

## Shape

| Token | px | Use |
| --- | --- | --- |
| `radius.0` | 0 | Status squares, dither, diff rows (pixels are square) |
| `radius.sm` | 4 | Buttons inside cards, pills, tags, keys |
| `radius.md` | 6 | Buttons, inputs, selected rows |
| `radius.lg` | 8 | Cards, approval cards, an opened tool group (user messages take 10) |
| `radius.xl` | 12 | Composer, menus, palette, dialogs |

- **Nested radius:** inner = outer − padding, never below `radius.sm`. A card at 8 with 12 padding holds buttons at 4.
- Radius grows with surface size; small things are nearly square.
- **Floating controls** over scrolling content (the chat's "Latest") are capsules at control height, at elevation 1, so they read as lifted off the page rather than part of it.
- **Paired steppers** (up and down through your messages) are one joined pair with a hairline between the halves: outlined beside outlined text buttons, plain among icons.

## Colour

Three layers. Components never use a hex value.

1. **Primitives:** the Field Notes hex values in [design.md](design.md) (Palette).
2. **Semantic tokens:** `surface`, `raised`, `panel`, `ink`, `quiet`, `faint`, `hairline`, `hairline.strong`, the five signals (`working`, `needs`, `done`, `waiting`, `idle`) each with a dither partner, `diff.added`, `diff.removed`.
3. **State tokens** are ink at an alpha, so one value works on every surface:

| Token | Paper | Carbon | Use |
| --- | --- | --- | --- |
| `state.hover` | ink 5% | ink 6% | Row and ghost button hover |
| `state.press` | ink 9% | ink 10% | Pressed |
| `tint` | ink 5% | ink 7% | Your messages' fill (no edge), a tool call's opened detail |
| `state.selected` | ink 10% | ink 12% | Selected row (twice hover, so a hovered row never reads as a second selection) |
| `signal.wash` | signal 7% | signal 12% | Approval card fill, signal tints |
| `signal.edge` | signal 45% | signal 50% | Approval card edge |
| `focus` | ink, 2 px, 2 px offset | same | Keyboard focus only, appears instantly |

- **Contrast:** `ink` is for reading, `quiet` meets 4.5:1 on every surface, `faint` meets 3:1 and is never used for anything you must read.
- **The agent's mark** (the Claude logo, in Claude's terracotta on every theme) names the agent wherever its model or usage shows. It is identity, not a signal.
- **Signals are fills, never text** on Paper. A signal colour appears as a square, a wash, an edge or a fill.
- **Diffs:** added and removed lines get a tinted row and a 2 px gutter bar in the signal colour.

## Elevation

| Level | Where | Treatment |
| --- | --- | --- |
| 0 | Panels, cards, messages, composer | Flat, 1 px hairline |
| 1 | Menus, tooltips, palette, toasts | `raised`, hairline, one soft shadow (0 8 24 at 12% on Paper); on Carbon one surface step lighter with a 1 px top highlight instead |

Shadows are drawn once per floating layer, never animated, never stacked on scrolling content.

## States

- **Hover:** a `state.hover` tint, instant. Rows tint edge to edge.
- **Press:** the background layer insets 1 px (reads as a 0.97 press) and steps to `state.press`, so neighbours never move. It springs back on release.
- **Selected:** `state.selected`, instant, edge to edge.
- **Focus:** a 2 px ink ring, keyboard only, instant.
- **Disabled:** `faint` ink, no hover, the not-allowed cursor, and a tooltip saying why ("Type a message first"). A form's main action (send, start) is one shared control, `ui::action`: ink when it can go, a faint wash when it can't, its colours tweening between the two over `dur.fast`.
- **Unavailable rows:** shown, not hidden, so you know where something went: quiet text, a tag naming why in place of the time ("In use"), the not-allowed cursor and a tooltip.
- **Busy:** keeps its label and size; progress shows inside it.
- **Short of room:** chips give up words in a fixed order and keep their icons, with a tooltip naming the value; labels never all truncate at once.
- **Menus** open towards the side with more room and are as tall as it allows; longer lists scroll, and their rows never squeeze.

## Motion

### Tokens

| Token | Value | Use |
| --- | --- | --- |
| `dur.fast` | 120 ms | Exits, press, colour changes |
| `dur.base` | 180 ms | Small enters: a message, a pill, a tooltip |
| `dur.slow` | 240 ms | Folds, collapses, an approval card arriving |
| `dur.theme` | 200 ms | The theme colour tween |
| `ease.out` | cubic-bezier(.23, 1, .32, 1) | Everything that enters or responds to the user |
| `ease.in_out` | cubic-bezier(.77, 0, .175, 1) | Something already on screen changing shape |
| `ease.linear` | — | Progress and time only |
| `spring.snappy` | 0.25 s, no bounce | Interruptible values: title compaction, selection |
| `spring.pop` | 0.3 s, bounce 0.2 | A completion mark, once |
| `stagger` | 30 ms per item, 150 ms cap | Several things arriving together |
| `steps.pixel` | 8 fps | Pixel animation (the working square) is deliberately stepped |

- Exits are quicker than enters. Never ease-in.
- Distances are small: 4 px for chrome, 6 px for a message. Presses read as 0.97, never smaller.
- Animations retarget from where they are when interrupted; nothing queues or snaps back.
- Time is wall clock, so 120 Hz makes motion smoother, not shorter.

### The vocabulary

Each motion has one name and one job.

| Motion | What happens | Where |
| --- | --- | --- |
| **Arrive** | Opacity 0→1 and a 6 px rise, `dur.base`, `ease.out`; staggered when several land together (more than three at once, like history loading, just appear) | New chat rows and tool lines |
| **Pop in** | Chrome arriving (`ui::Arrive`): opacity 0→1 and the last 4 px towards where it rests, away from what opened it, on `ease.out`. `dur.fast` for what you open often (menus, the slash menu, the palette and its dim, the right-click menu); `dur.base` for what comes to you (a tooltip, a toast, the Latest pill). Exits are instant | Menus, palette, tooltips, toasts, pills |
| **Turn** | A fold's chevron (`ui::disclosure`) turns from right to down on `spring.snappy` instead of swapping icons | Tool groups, thoughts, menu folds, the Closed section |
| **Stream** | Each new word fades in over `dur.base`; a block cursor in `working` breathes while text arrives | Agent replies |
| **Resolve** | The ordered-dither dissolve, the Field Notes signature, on small surfaces only | Approval cards arriving (`dur.slow`), images and diagrams loading |
| **Shift** | Every colour token tweens from the old theme to the new, `dur.theme`, `ease.out`; the grain crossfades. Nothing moves and text stays readable | Theme switch |
| **Fold** | A card's content fades out (`dur.fast`) and it collapses to one line (`dur.slow`, `ease.in_out`); the line arrives in its place | Approval resolved, tool output collapsed |
| **Confirm** | The done square pops from 60% on `spring.pop` | A tool, approval or turn finishing |
| **Tick** | Changed digits roll in their own column, up or down with the value, `dur.slow`, rightmost first with `stagger` | Usage, event and blocked counts |
| **Progress** | A hairline fill under a running tool line moves linearly towards 90% of its expected time and holds; a timer counts up | Running tools |
| **Lift** | A picture zooms from where it sits to the middle of the window (`dur.slow` + 40 ms, `ease.out`) as the backdrop fades dark a little ahead of it; it zooms back in `dur.base`, the backdrop clearing first | Opening a picture large |
| **Compact** | The serif title eases from 28 to 20 on `spring.snappy`; the live pill arrives with a 4 px slide | Reading history |
| **Drift** | The working square's checker shifts one cell, stepped at `steps.pixel` | Working status |
| **Hand-over** | The working line's words stay up at least 650 ms (2 s for a step) and change only once the new words have held 250 ms; the old rise 3 px out, the new rise 3 px in, `dur.base` in all | Working line |
| **Breathe** | After 3 s of work the working line's words ease between full and 60% opacity over 2 s | Working line |
| **Press** | See States | Buttons, rows |

### Never animated

- Keyboard navigation, session switching, focus, selection, scrolling, text selection.
- Row hover. Tooltips after the first one: while one is showing or went in the last 400 ms, the next shows after 60 ms instead of 500 ms, without its arrival, so a sweep along a toolbar reads each name at once. Every tooltip goes through `with_tip` for this.
- Streaming text layout: words are appended, never re-laid out or tweened in position.

### Reduced motion

With the system's reduce-motion setting on (read at launch and whenever the window comes forward; `--reduce-motion` forces it), every motion resolves instantly, the working square holds still, and streamed text still appears word by word.

### Cost rules

- Allowed per frame: opacity, offsets, colours, flat quads, and remeasuring at most the one row that is changing.
- The dither is a grid of flat quads, so it is for small surfaces only. Across a whole window it measured far over budget, which is why the theme switch is a colour tween.
- Only the working line repaints a quiet window (its square's drift, its words' breath and hand-over), and only while a session is working and visible. The one exception is an ambient background you chose for the new-session page, which redraws every frame while that page is visible ([0049](decisions/0049-ambient-backgrounds.md)).
- Every new motion is measured with the frame-timing bench before it lands (Reliability requirements).

## Lists of things you act on

A list where each row is something with a state and things to do about it (MCP servers, skills, permission rules: the command sheets, [0046](decisions/0046-commands-by-type.md)) reads like a settings pane, and every such list is drawn the same way:

- **Sections** are a `ui::section_label`, with `space.4` above it, an optional note in 12/16 `quiet` `space.1h` below, then `space.2` to the first row. Space separates sections; no lines.
- **A row** is the name in `type.ui` `ink`, how it stands and more beneath it in 12/16 `quiet`, and at its end either its controls at their natural width or, with none, its value in `quiet`. Everything lines up on the name's line. What's typed (a rule, a path, a command) is in the code face. A path keeps its end when short of room; an id keeps its start and end.
- **Marks** hang in the margin left of the names, so names line up whether or not a row has one, and only rows that are off (a ring) or need you (a signal dot) have one. Working is unmarked.
- **Controls:** on and off is a `ui::switch`; anything else is an outlined `ui::inner_button` at `size.control.sm`, even a fix: the mark carries urgency, and the same state always looks the same. A control at work keeps its label, goes off (`ui::off`) and says what it's doing on hover.
- **Errors** stay on the row, in `ink` under its name; the mark carries the colour.

## Code

Code blocks are highlighted (arborium, [0016](decisions/0016-highlighting-arborium.md)) within the palette: keywords in ink, bold (the code face has no medium); strings, functions, types and numbers are the ink mixed part way toward the done, working, waiting and idle hues (never needs, which is kept for what needs you); comments quiet and italic. Nothing is louder than the prose around it.
