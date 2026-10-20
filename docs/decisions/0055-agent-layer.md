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

**Seeing dent with an agent that can do less, without another agent.** `DENT_CAPS_OFF` (a list like `steer,rewind,mode:plan`) turns capabilities off for every driver, through `Driver::caps`, which every use goes through. The daemon tests and the `limited` walkthrough use it.

## Consequences

- Adding an agent means one new module beside `claude/` and one entry in `DRIVERS`. Nothing else in the daemon changes.
- Every request that needs a capability is refused by the daemon too, so a CLI or another client can't ask for what fails. A steer to an agent that can't be steered queues instead, and a model change it can't take live is kept for its next process.
- The desktop keeps each agent's models and usage. A session's menus offer its own agent's models, and the status bar shows each agent with readings, the default's first.
- The new-session box has an agent chip, shown only with more than one agent ready ([0056](0056-settings-and-onboarding.md)).
- `just verify` covers the layering with `agent::tests::only_drivers_name_their_agent`, and the "can't" paths with `an_agent_that_can_do_less_is_never_asked_to`.
