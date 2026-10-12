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
| Sandboxed script engine producing SVG | TeX math, Vega-Lite specs | Mature libraries without a browser |
| Isolated, offline web view | Mermaid diagrams, HTML previews, rare formats | Formats that need a real browser layout |

Which library needs which engine is confirmed in phase 1; Mermaid, for example, measures text with a browser and cannot run headless.

## Initial catalogue

| Category | Types | Engine |
| --- | --- | --- |
| Text | Headings, inline styles, links, lists, task lists, quotes, alerts, footnotes, definition lists, collapsible sections, rules, emoji, CJK and right-to-left text | Native |
| Code | Highlighted code blocks (tree-sitter), diffs, terminal output with ANSI colour, JSON, YAML and TOML trees, logs, file-and-line links | Native |
| Math | Inline and display TeX, aligned equations, matrices, chemistry | Script to SVG |
| Tables | Markdown tables, CSV and TSV blocks, sorting, wide tables with a pinned first column | Native |
| Charts and markets | Line, bar, area and scatter charts, sparklines, Vega-Lite, stock cards, candlesticks | Native, Vega via script |
| Diagrams | Mermaid (all diagram types), Graphviz | Web view or script |
| Media | Images (local paths resolved on the agent's machine, or URLs), SVG, GIF, video, audio, PDF and HTML previews | Native or web view |
| References | Files, PRs, issues, commits, sessions, link previews | Native |
| Agent output | Tool-call trees, todo lists, plans, test results, search results with citations, file trees, subagents, errors, thinking | Native |

## Interactive blocks

- **Rich blocks from agents:** agents can write `dent-chart`, `dent-stock` and `dent-table` blocks with JSON data. dent ships their schema as an agent skill so agents use them; an unknown `dent-` block falls back to a code block.
- **Market data:** stock cards draw from data in the agent's tool results. Live refresh is optional, through a market-data provider and key you configure, and every card shows its source and time.
- **View state stays local:** hovering, switching a range, sorting or collapsing never changes the transcript.

## Streaming

- Only the end of a message is re-parsed as tokens arrive; finished blocks never move.
- Every renderer has a partial state: an open code fence is already a code block, a half-written table shows its complete rows, and unfinished math or diagrams show their source dimmed until they close.
- Nothing flickers between raw and rendered.

## Testing

- **Spec suites:** all CommonMark and GitHub Flavored Markdown spec examples pass.
- **Golden images:** every fixture renders at three column widths in Paper and Carbon, compared pixel by pixel in CI; changes need approval.
- **Every prefix:** each fixture also renders at every streaming prefix, with no crash, no movement of finished blocks, and a final result identical to the non-streamed render.
- **Real transcripts:** recorded Claude Code and Codex sessions replay through the renderer nightly and before releases.
- **Fuzzing:** the parser and every renderer are fuzzed with malformed Markdown, huge tables, deep nesting, giant lines and invalid TeX or Mermaid; the result must be the fallback, never a crash or a hang.
- **Performance:** a 1,000-block message scrolls within the frame budget, and each renderer stays inside its budget.
- **Copy and access:** copying returns the original Markdown; every block has a text alternative and keyboard-reachable actions.
- **Parity:** desktop, iPhone and web render the same fixtures, compared by text output and a screenshot review.
- **Specimen gallery:** a debug screen in the app shows every fixture in both themes, for visual review.

All of these run over the renderer registry, so a new renderer is tested the moment it is registered.

## Adding a content type

1. Write fixtures: normal, partial and malformed inputs.
2. Implement the renderer contract.
3. Register its match.
4. Approve its golden images; streaming, fuzz, performance and copy tests then run automatically.
