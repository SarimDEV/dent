# Chat rendering

Every message is parsed into a tree of typed blocks, and each block type has one self-contained renderer registered in one place. Adding a content type means adding one renderer and its sample inputs; parsing, streaming, testing and copying pick it up automatically. Specimens of text, code, math, tables, charts and an interactive stock card are in [design/mockups/](design/mockups/) (`R1-text`, `R2-code`, `R5-charts`).

## Pipeline

1. **Parse:** the shared Rust client core parses Markdown incrementally (CommonMark plus GitHub extensions, math, alerts, footnotes) into a block tree. Fenced blocks dispatch on their language tag: `mermaid`, `math`, `diff`, `json`, `csv`, `vega-lite`, `dent-chart` and so on.
2. **Detect:** untagged content is still recognised: JSON-shaped text, ANSI terminal output, unified diffs, file paths with line numbers, URLs, and PR, issue and commit references.
3. **Render:** each client draws the block tree with its own renderers (GPUI on desktop, SwiftUI on iPhone, the DOM on web), all fed by the same parsed tree.
4. **Cache:** expensive renders such as math and diagrams run off the UI thread and are cached by content hash.

## The renderer contract

| Part | What every renderer provides |
| --- | --- |
| Match | Which block kinds or language tags it handles |
| Streaming state | How it looks while its block is still arriving |
| Final render | The finished view, themed through palette tokens only |
| Fallback | The source as a plain code block if anything fails; a block never disappears |
| Header and actions | The shared block header: name, details, then actions such as Copy, Open, Data, Source |
| Plain text | Text for copying, search and screen readers |
| Fixtures | Sample inputs, including malformed and partial ones, with approved golden images |
| Budget | Maximum render time and memory, enforced in tests |

## Three ways to draw

| Engine | Used for | Why |
| --- | --- | --- |
| Native | Text, code, diffs, tables, terminal output, data trees, charts, stock cards, agent output | Fastest, fully themed, interactive |
| Native from a Rust engine | TeX math (RaTeX's display list drawn as glyphs, rules and paths), Mermaid (merman to SVG, rasterised by resvg off the UI thread) | Themed and crisp, no browser ([spike 04](spikes/04-markdown-math-mermaid.md)) |
| Sandboxed script engine producing SVG | Vega-Lite specs | Mature libraries without a browser |
| Isolated, offline web view | HTML previews, rare formats, and the fallback if merman or RaTeX fails | Formats that need a real browser layout |

- **Math:** RaTeX's display list is drawn with GPUI primitives in the KaTeX fonts. Glyphs from the KaTeX Size fonts are drawn from their outlines, because GPUI does not load fonts without an `m` glyph. Inline math reserves its width with invisible characters in the paragraph and is drawn at the text baseline.
- **Mermaid:** merman renders SVG with its resvg-safe preset, a host theme mapped from the palette tokens, and label widths measured with the bundled fonts through rustybuzz ([0041](decisions/0041-label-shaping-rustybuzz.md)). A recolour pass snaps any remaining colour to the nearest token (C4, sankey and architecture otherwise keep their own). resvg rasterises at 2x on a background thread ([0039](decisions/0039-svg-raster-resvg.md)), and the result is cached by source and theme. A diagram renders once its fence closes.

**In the desktop app today** (`crates/desktop/src/markdown.rs`, `math.rs`, `diagram.rs`):
- Inline maths, display maths (`$$…$$` and ` ```math ` fences), Mermaid fences and tables all behave as described on this page, in chat replies, plans and the doc viewer.
- Size glyphs come from outlines through ttf-parser ([0040](decisions/0040-glyph-outlines-ttf-parser.md)).
- Streaming (`mdstream.rs`): a reply still arriving keeps its finished top-level blocks and re-parses only the rest, after the terminator described under Streaming. A code block whose fence is still open is highlighted to its last full line, the line being written plain; only the reply still streaming can have one, so a finished reply whose agent never closed a fence is highlighted (and its maths typeset, its diagram drawn) in full. Finished text is parsed once and kept in a bounded cache that forgets what was used longest ago; a reply's prefixes never enter it. HTML blocks are one block, and fences inside quotes know when they close.
- Not built yet: the table side panel (Open), in-app search reaching rows that a capped table keeps out of view, parsing off the UI thread, and the row-at-a-time table parse.
- The walkthrough's `markdown` scene checks it: `scripts/walk.sh markdown`, also part of `checks`.

## Sizes and overflow

The reading column is 720 pt wide at most and shrinks with the window down to 360 pt; running text (paragraphs and lists) stops at a 540 pt measure, about 78 characters, while tables, code and pictures may use the whole column. Inline code sits a step quieter than the words around it. Nothing is ever clipped without a way to see it ([spike 04, stress](spikes/04-markdown-math-mermaid.md#stress-every-block-at-its-limits)).

- **Wide content scrolls sideways inside its block**, with a fade on whichever edge has more to see and a thin, always-visible bar along its bottom (as a tall table's scrollbar), so it reads as scrollable before anyone touches it. This covers tables, display maths, code and diagrams. The page itself never scrolls sideways.
- **Tables:**
  - All rows share one set of column widths, so columns line up.
  - Each column starts at its natural width, capped at 320 pt so long text wraps. Columns grow and shrink in proportion.
  - A column never gets narrower than its longest word, capped at 160 pt. A column of prose never goes below 140 pt. Below those limits the table scrolls instead.
  - Widths are measured in each run's own font, with header cells in medium weight.
  - Column alignment from the Markdown is honoured.
  - **Tall tables scroll inside the chat.** Past 13 body rows, a table shows its header and 10½ rows. The half row peeking out signals more, along with an always-visible scrollbar, a fade on whichever edge has more, and a row count in the footer.
  - The header stays put while the body scrolls. Only the rows in view are drawn, so a 5,000-row table costs no more than a short one once it has finished.
  - **Scroll handoff:** the wheel scrolls the table until it reaches an end, then carries on to the chat. A sideways swipe moves wide content and never the chat. Vertical wheel motion is never turned into sideways scrolling.
  - **While rows stream in,** a capped table keeps its height and the chat below it never moves. The count in the footer goes up.
  - **Open** shows the whole table in a side panel, with every row reachable. In-app search reaches rows the cap keeps out of view, and scrolls both the chat and the table to the match.
- **Code never wraps.** Long lines scroll inside the block.
- **Display maths** is centred when it fits and scrolls when it doesn't.
- **Inline maths wider than 260 pt** gets a line of its own at text size, which scrolls. The sentence continues on the next line.
- **Diagrams** shrink to fit the column, but never below 75% of their natural size, so labels stay readable. Past that they scroll. Tall diagrams keep their natural size.
- **Long links and words** wrap at the window edge.
- **Nesting:** list items are padded blocks with the marker positioned out of flow, never flex rows. Nested flex rows re-measure their text at every level, which doubled the frame cost per level.

## Block actions

- **Copy is one click on every block that has source:**
  - code blocks: the exact code, without fences;
  - tables: Markdown or CSV;
  - display maths: the TeX;
  - diagrams: the Mermaid source.
- Code and tables show their actions in a header or footer. Maths and diagrams have no chrome of their own, so their actions appear in the corner on hover.
- "Copied" confirms for a moment, in place.
- Copy text is built when clicked, never while drawing.

## Initial catalogue

| Category | Types | Engine |
| --- | --- | --- |
| Text | Headings, inline styles, links, lists, task lists, quotes, alerts, footnotes, definition lists, collapsible sections, rules, emoji, CJK and right-to-left text | Native |
| Code | Highlighted code blocks (tree-sitter), diffs, terminal output with ANSI colour, JSON, YAML and TOML trees, logs, file-and-line links | Native |
| Math | Inline and display TeX, aligned equations, matrices, chemistry | Native (RaTeX) |
| Tables | Markdown tables, CSV and TSV blocks, sorting, wide tables with a pinned first column | Native |
| Charts and markets | Line, bar, area and scatter charts, sparklines, Vega-Lite, stock cards, candlesticks | Native, Vega via script |
| Diagrams | Mermaid (all diagram types), Graphviz | Native (merman and resvg); Graphviz by script |
| Media | Images (local paths resolved on the agent's machine, or URLs), SVG, GIF, video, audio, PDF and HTML previews | Native or web view |
| References | Files, PRs, issues, commits, sessions, link previews | Native |
| Agent output | Tool-call trees, todo lists, plans, test results, search results with citations, file trees, subagents, errors, thinking | Native |

## Interactive blocks

- **Rich blocks from agents:** agents can write `dent-chart`, `dent-stock` and `dent-table` blocks with JSON data. dent ships their schema as an agent skill so agents use them; an unknown `dent-` block falls back to a code block.
- **Market data:** stock cards draw from data in the agent's tool results. Live refresh is optional, through a market-data provider and key you configure, and every card shows its source and time.
- **View state stays local:** hovering, switching a range, sorting or collapsing never changes the transcript.

## Streaming

- Only the end of a message is re-parsed as tokens arrive; finished blocks never move. A top-level block is committed once the line that starts the next block is complete, because a half-typed line ("2" before "2.") can still join the block above. While that line is still being typed (a long paragraph is one line), everything before the block above it is committed, since a line only changes the block just before it. The finished blocks aren't copied for each chunk either: only what follows them is replaced. A chunk near the end of a 117 KB, 1,200-block reply costs about 13 µs (it was about 2 ms: committing stalled while each paragraph's line was typed, and every chunk copied all finished blocks); `mdstream::tests::stream_cost` measures it.
- A terminator rewrites the unfinished tail before parsing: it closes open emphasis, code spans and links, drops delimiters with no content yet, and holds back what is still ambiguous (a `$` or `[` that is not resolved, a fence or table row being typed, a bare list or heading marker, a setext underline).
  - It works on the block being written: the last paragraph, list item or heading (a list item, a heading and the line after a heading each start one). A span left open in an earlier item or line ended with it, so nothing closes in the next block.
  - It only closes a `*`, `_` or `~` that plausibly opens a span: after a space, the start or an opening mark, and before a word. Globs (`*.rs`, `**/*.rs`), sums (`2*3`) and paths (`x.com/_private`) stay text. Nothing inside an autolink or tag (`<https://…>`) counts.
  - A `$` holds back what follows only when it plausibly opens maths: not a price (`$5`), a shell variable (`$HOME`, `$PATH`, `$name then`) or substitution (`$(pwd)`, `${x}`). A `[` holds back only while it can still be a link's words: on its own line, within 80 characters, with no `)` before its `]` (so `[0, 10)` shows). Either ends at the line's end.
  - A table's header row (a line starting and ending with `|`) waits until the delimiter row is in, also when it follows a line of text. That is the calmer choice: a header shown as text first would snap into a table a moment later. A line that only starts with a pipe (`|x| is…`) shows once it's complete.
  - A reference link or footnote whose definition comes later shows raw only until the definition arrives; then the finished part is read again, once, so `[the docs][1]` turns into a link before the reply ends.
- Chunks that arrive within one frame are parsed together, off the UI thread.
- A table at the end of the stream is parsed and measured a row at a time. Each complete row is parsed once against the header and never again, so a table's length doesn't change the cost of a chunk. Anything that isn't a plain row line falls back to the full parse.
- `$…$` is math only when it looks like TeX: a span that starts with a digit or space and has no `\`, `^`, `_` or `{` stays text, because in real agent output these were prices. `$$…$$` is always math.
- Tight and loose lists render with the same spacing, so a list turning loose mid-stream does not jump.
- Every renderer has a partial state: an open code fence is already a code block, a half-written table shows its complete rows, and unfinished math or diagrams show their source dimmed until they close.
- Nothing flickers between raw and rendered.
- **How text appears is a reading preference** ([0034](decisions/0034-streaming-reveal-setting.md)):
  - **Word by word** (default): text appears as it arrives.
  - **Word by word, smoothed:** whole words at a steady pace, at most about 0.25 s behind, with a 180 ms fade.
  - **Block by block:** each block appears once the next has started, with a 300 ms fade.

  Parsing and committing are the same for all three; only visibility changes.

## Testing

- **Spec suites:** all CommonMark and GitHub Flavored Markdown spec examples pass.
- **Golden images:** every fixture, plus the stress page (too wide, too tall, too long, too deep), renders at three column widths in Paper and Carbon, compared pixel by pixel in CI; changes need approval.
- **Every prefix:** each fixture also renders at every streaming prefix, with no crash, no movement of finished blocks, and a final result identical to the non-streamed render.
- **Real transcripts:** recorded Claude Code and Codex sessions replay through the renderer nightly and before releases.
- **Fuzzing:** the parser and every renderer are fuzzed with malformed Markdown, huge tables, deep nesting, giant lines and invalid TeX or Mermaid; the result must be the fallback, never a crash or a hang.
- **Performance:** a 1,000-block message and a 10,000-row table stay within the frame budget, while streaming and once finished. Committed blocks are measured once and cached, and a capped table draws only the rows in view. Each renderer stays inside its budget.
- **Interaction:** scripted wheel and click input, sent through the normal hit-testing, checks scroll handoff, sideways scrolling, every copy action, the side panel and search at three window sizes in both themes.
- **Copy and access:** copying returns the original Markdown; every block has a text alternative and keyboard-reachable actions.
- **Parity:** desktop, iPhone and web render the same fixtures, compared by text output and a screenshot review.
- **Specimen gallery:** a debug screen in the app shows every fixture in both themes, for visual review.

All of these run over the renderer registry, so a new renderer is tested the moment it is registered.

## Adding a content type

1. Write fixtures: normal, partial and malformed inputs.
2. Implement the renderer contract.
3. Register its match.
4. Approve its golden images; streaming, fuzz, performance and copy tests then run automatically.
