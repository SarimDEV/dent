# 0055. Every agent sits behind one driver; sessions keep their agent; clients offer what it can do

Status: accepted (2026-10-20) · Builds on [0008](0008-claude-code-stream-json.md), [0009](0009-codex-app-server.md), [0010](0010-acp-third-driver.md) · Spec: [daemon.md](../daemon.md#agent-drivers)

## Decision

**One driver per agent.** `crates/daemon/src/agent` holds the `Driver` trait and the list of drivers, `DRIVERS`, whose first entry is the default. A driver says:
- what the agent is: its id, names, the CLI version dent was tested against, and what it can do (`Caps`);
- how to start a session's process (`launch`), and what each line it writes means, as dent's own `agent::Event`;
- how to say what a session asks (`message`, `request`, `answer`, `refuse`), and what an answer means (`reply`);
- how to run one-offs: titles, branch names, commit messages, side questions;
- how to read usage, models and commands without a model call (`probe`), and how it stands on the machine (`status`: installed, version, signed in);
- what it has in its own terminal app: sessions to pick up, and folders it worked in.

Claude Code is the one driver today (`crates/daemon/src/claude`). Sessions, the daemon's shared state and the server use only the trait and dent's types. A test fails if anything outside a driver's folder reaches into it.

**What every agent shares stays above the drivers.** These live in `agent::log`, one set of rules for every agent:
- tool output kept at its start and end;
- pictures saved beside the session;
- a subagent's report kept longer;
- the thought on the working line;
- a sheet in plain words.

The session's state machine (turns, queue and steer, stop, idle, rewind, restarts) doesn't change by agent. Agent processes speak one JSON message per line, which covers Claude Code's stream-json, Codex's app-server and ACP.

**Sessions keep their agent.** `Created`'s `agent` was always logged. The store now keeps it on the session's row, and a database from before gets the column with `claude`. Forks and side questions run on their session's agent, and so do titles, branch names and commit messages. A session whose agent this daemon has no driver for is left as it is, never run by another.

**Per agent, not per daemon.** Models, the commands a new session starts with, usage readings and their caches are kept by agent. `Request::Models` and `Request::Usage` take an optional agent (none: the default), so older clients ask exactly what they did.

**Clients learn agents from the welcome, then how they stand.** `Reply::Welcome` lists every agent: its names and `Caps`. `Request::Agents` adds how each stands, which takes seconds to ask: installed, version, whether it's the tested one, signed in to a plan, an API key or signed out. The answer is cached for a minute. That list is the input onboarding and Settings will need.

**Clients offer what the agent can do.** Desktop requests that need a capability ask `App::can` first, which says "<agent> can't …" rather than sending something that would fail. Stop on background work hides where the agent can't stop one piece on its own. "Always in this project" is off where it can't save project rules. Names in the chat come from the agent: "Claude asks for a secret", "Claude is compacting now". A daemon too old to describe its agents counts as Claude Code with everything on.

## Why

The spec always said "nothing above the driver knows which agent it is talking to" ([daemon.md](../daemon.md)). The code had grown the other way: the session task called Claude Code's wire format directly, and Claude Code's name was written into notices. The owner asked (2026-10-20) for the groundwork for more agents to be built before any second agent. That way, adding Codex is one module, not a rewrite of the session.

## Consequences

- Adding an agent means one new module beside `claude/` and one entry in `DRIVERS`. Nothing else in the daemon changes.
- There is no agent picker in the new-session box yet. `Request::Create` takes `agent`, and the desktop sends its `Defaults::agent` (kept in prefs), which nothing sets until a second agent ships. The picker should only show when more than one agent is installed.
- The desktop keeps only the default agent's models for its menus. Per-agent model menus come with the second agent.
- Usage in the status bar is still the default agent's.
- `just verify` covers the layering with `agent::tests::only_drivers_name_their_agent`.
