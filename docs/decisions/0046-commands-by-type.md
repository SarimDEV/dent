# 0046. Commands, statuses and the agent's questions are handled by kind

Status: accepted (2026-10-14) · Builds on [0043](0043-prompt-completion.md) · Spec: [desktop.md](../desktop.md#chat), [daemon.md](../daemon.md)

## Decision

Claude Code has dozens of slash commands and many status messages, and both keep changing. dent doesn't handle any of them one by one. Each is sorted into a kind, and each kind has one way of working:

**What a command takes.** Every command ships a short argument hint (`[auto|<tokens>]`, `<low|high|ultracode [on|off]>`, `[style]`, `key=value`). The grammar is the same for all of them, so the daemon reads it once (`claude/commands.rs`) and sends each command with typed arguments:

- **Nothing:** it just runs.
- **A choice:** the options from the hint. A placeholder the agent can fill takes its own list: models, output styles, MCP servers. An alias that names a model says which model (`opus` → "Opus 5.5"). The choice in effect is marked, from `get_settings`.
- **Settings:** `/config`'s keys and their values, read from `/config`'s own usage text by the usage probe. That probe passes `--no-session-persistence` and keeps the person's hooks off, so it saves nothing and calls no model.
- **Text:** words of your own, with the hint as placeholder.

The `/` menu continues into a command's arguments: picking `/autocompact` shows `auto` and "Your own tokens", picking `/config` shows its keys and then a key's values. A choice with nothing after it runs at once. A value of your own steps the menu aside so you can type it.

**What a command answers.** When the agent runs a command itself (no model turn), the answer becomes a `Command` event with a typed outcome:

- **Done:** shown as a result card under the command line: a glyph and the command in the code face, the answer in the quiet voice, folded when long. Plain lines keep their breaks, and columns keep their alignment.
- **Terminal only:** "/theme works only in Claude Code in the terminal."
- **Unknown:** "Claude has no /fooo command", with the agent's suggestion as a button.
- **Failed:** marked as an error.

An answer that has a better shape as data gets a `View`. Commands that are dialogs in the terminal (`/status`, `/mcp`, `/skills`, `/memory`, `/hooks`, `/permissions`) have control requests that return the dialog's data, so they answer with a **sheet**: sections of rows, each with how it stands, a mark when it's off or needs you, and what you can do about it (turn an MCP server on or off, reconnect, sign in, open a file). A row's action goes back as `Act`; the sheet arrives again as it is now and replaces the newest one in place. One table maps each command to its control request, and one function per command reads its data into rows; the drawing is the same for all. `/context` becomes a meter: the bar in parts and the threshold where the agent compacts on its own, with parts loaded only when needed listed apart. `/usage` becomes labelled limit bars. The answer's text stays a fold away. A sent `/command` looks like a command line from the start, and its card takes its place, so nothing jumps.

**What the agent is doing.** Short-lived states are an `Activity`, sent to clients but never logged: compacting (with how long) and retrying (with a countdown, the agent's reason, and the attempt). The working line and the sidebar show it; a change of words eases in.

**What changed the conversation.** Compaction is a `Compacted` event: a divider, "Compacted · 152k → 18k tokens", or "Compacted automatically" when the agent did it on its own. After every turn and every compaction, the daemon asks the agent for `get_context_usage`, so the meter has the real count and the model's real window. The composer's gauge opens the same meter, with Compact now.

**What the agent asks.** Claude Code's own questions (`request_user_dialog`) are declared at `initialize` and shown in the question card. Today these are: retry a declined message with the fallback model, use usage credits, and the auto-mode notice. Only their choices answer them. A kind dent didn't declare is never answered, so the agent takes its default.

**Notices.** The agent's notifications, warnings and model-fallback messages become quiet notes. Its suggestions and chatter don't.

The only per-command knowledge left is one table that says which setting a command changes, so its current choice can be marked.

## Why

Other apps pass most commands through as text, partly because supporting several agents pushes them to the common subset, and partly because many of the control requests aren't documented promises. Handling commands by kind keeps the work proportional to the number of kinds, not the number of commands. It also covers commands that don't exist yet: a new command with a hint of a familiar shape gets its picker for free.

Everything degrades to text. If the hint can't be read, the command takes text. If an answer has no view, its words show. If the probe can't list settings, `/config` takes text.

## Consequences

- After /compact, the meter used to keep its old reading, and it measured every model against 200k. Both are fixed by asking for `get_context_usage`.
- The question card's "type your own answer" is hidden for questions only choices can answer.
- `/mcp`, `/status`, `/skills`, `/memory`, `/hooks` and `/permissions` work, as sheets. Editing permission rules waits on the daemon's own rule store, which `/permissions` will then edit.
