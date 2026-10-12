# Daemon

The daemon is one Rust binary per machine that owns every agent session, git operation and file read on that machine. A tiny companion process, the session holder, keeps the agent processes and terminals themselves alive, so upgrading the daemon never interrupts work (see Durability). It runs as a system service (launchd on macOS, systemd on Linux, a Windows service), so it starts at boot and outlives every client.

## Agent drivers

Each agent sits behind one `AgentDriver` trait: start a session, send a user message, interrupt, answer an approval, resume after restart, and emit normalized events. Drivers translate their agent's wire format into dent's event types; nothing above the driver knows which agent it is talking to.

| Driver | How it runs | Approvals | Limits |
| --- | --- | --- | --- |
| Claude Code | `claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --permission-prompt-tool stdio` as a long-lived child process; NDJSON in and out ([docs](https://docs.claude.com/en/docs/claude-code/headless)) | `control_request` `can_use_tool` on stdout, answered with a `control_response` (`allow` with `updatedInput`, or `deny` with a message the agent sees) | `rate_limit_event` with `five_hour` and `seven_day` utilisation and reset times; sent when it changes, not every turn, so the daemon keeps the last value |
| Codex | `codex app-server`: JSON-RPC over stdio, Codex's own rich-client interface ([app-server docs](https://learn.chatgpt.com/docs/app-server)) | Server-to-client `item/commandExecution/requestApproval` and `item/fileChange/requestApproval`, answered with `accept`, `acceptForSession`, `decline` or `cancel`. There is no reason field, so "deny with a reason" declines and then steers the turn with the reason (`turn/steer`) | `account/rateLimits/read` plus the `account/rateLimits/updated` notification (see Usage limits) |

- Pin a tested version of each agent CLI per daemon release, and run a contract test suite against every new agent version before bumping. Codex's protocol is documented as unstable; Claude Code's stream format changes too.
- Generate Codex's protocol types from its schema dump rather than hand-writing them (`scripts/gen-codex-types.py`; see [spike 02](spikes/02-codex-app-server.md)).
- Model availability depends on the account: a ChatGPT sign-in rejects some models (for example the user's configured default), so the model picker comes from `model/list`, and a rejected model is shown as such rather than as a generic turn failure.
- Use the agents' own credentials and login flows. The daemon never reads or reuses their tokens.
- Agents keep their own transcripts. On restart the daemon resumes the agent's session by id (`--resume` for Claude Code, thread resume for Codex) and reconciles against its own event log.
- The third driver is the Agent Client Protocol (ACP), which now has session resume, usage updates and a 1.0 Rust SDK ([updates](https://agentclientprotocol.com/updates)). One ACP driver brings in every agent that speaks it.

## Usage limits

Usage is account-level and read from the provider through each agent's own CLI, so it counts every way the account is used (the terminal, other machines, the web, dent) and reads the same on every device. dent never reads or refreshes agent tokens; the CLIs do.

| Agent | Read | Live updates between reads |
| --- | --- | --- |
| Claude Code | `get_usage` control request (`skip_behaviors: true`) on a short-lived `claude -p --no-session-persistence --strict-mcp-config` process: `five_hour`, `seven_day`, per-model weekly windows, the `limits[]` rows with severity, and extra-usage spend | `rate_limit_event` from any running session |
| Codex | `account/rateLimits/read` on `codex app-server`: `primary` and `secondary` windows with `usedPercent`, `windowDurationMins` and `resetsAt`, plus credits and per-model limits | `account/rateLimits/updated` from any running session |

- **Windows are generic.** Plans differ (a free Codex plan has a single 30-day window), so windows are labelled by duration (5h, week, 30d), never hard-coded as "5-hour and weekly".
- **Keyed by account, not machine.** Readings are stored per provider account (Claude's account UUID, Codex's account id) with the time they were fetched.
- **Synced through the hub.** Each reading is shared state; the newest `fetched_at` wins, so the phone and every desktop show the newest reading from whichever machine took it. Machines signed in to the same account share one poller, chosen by the hub; the rest stay passive and contribute live updates. Without a hub, each desktop and daemon polls on its own.
- **Polling:** read at startup (showing the last reading meanwhile), every 5 minutes while limits are visible, doubling while nothing changes up to 20 minutes; 15 minutes doubling to 40 in the background; wake at each window's reset plus 5 seconds to show it empty and confirm; never while the machine sleeps.
- **Startup fallback:** before the first live read, Claude Code's own cached reading in `~/.claude.json` (`cachedUsageUtilization`, ignored if its account doesn't match).
- **Signed out vs API key:** neither has plan limits; `claude auth status --json` and Codex's `account/read` tell them apart, and the status bar says which.
- `get_usage` is undocumented, so it is covered by the per-version contract tests; if it breaks, the last `rate_limit_event` is the fallback.

## Session event log

Every session is an append-only log of typed events in SQLite: user message, assistant text delta, tool call, tool result, approval requested, approval answered, file changed, status change, usage update, error. Each event gets a monotonic sequence number per daemon.

- Clients subscribe with "everything after sequence N" and receive a live stream afterwards; reconnecting is the same call.
- Streaming deltas are coalesced into a final message event when a turn completes, so history stays compact while live views stay smooth.
- Session status (working, blocked on approval, idle, done, errored) is derived from events and indexed, so every client can show it without replaying logs.
- SQLite runs in WAL mode with one writer task per daemon; reads never block writes.

## Persistence

"Everything survives" means three layers:

1. **Agent processes** keep running while clients disconnect. The session holder holds their stdio, so they survive daemon upgrades and crashes too.
   - Output goes to disk first, and each event records the byte it ended at, so a restarted daemon picks up exactly where it left off ([spike 06](spikes/06-session-holder.md)).
   - The daemon stops an agent's process after 30 s with no reply under way, and resumes its session from the transcript on the next prompt.
2. **Daemon restarts** (upgrade or crash) carry on with agents still running in the holder. After a reboot, or if the holder itself dies, each session resumes from the agent's own transcript and the event log; in-flight turns are marked interrupted and can be continued with one click.
3. **UI state** (open sessions, panes, scroll position, markdown tabs, drafts) is shared state, synced like projects (see Sync).

## Git

The daemon does all git work on its own machine and sends results to clients: status, diffs per file, stage and unstage per file and per hunk, discard, commit, small edits written back to the working tree. Every write goes through the `git` command so behaviour matches the user's git config and hooks exactly. `gix` is used read-only for fast status and object reads, and imara-diff computes diffs. Hunk staging writes the new file contents as a blob and updates the index entry, with no patch files. A file watcher pushes status changes, so diff views update live as agents edit.

## Terminals

The session holder owns terminal PTYs the same way it owns agents, so a terminal survives client and daemon restarts and lives on the session's machine. It runs the user's login shell in the chosen directory and streams raw output; clients render it with a terminal emulator (see Desktop app).

## Files and markdown

The daemon serves file reads, directory listings and a watch stream, scoped to project roots. Markdown files changed by an agent show up as events, which drives live reload in the viewer and the "new docs" list per session.

## Checkpoints

Before every turn the daemon snapshots each workspace's working tree using a temporary index, write-tree and commit-tree, kept under hidden refs/dent/ refs. Untracked files over a size cap are skipped, and the user's own index is never touched. Claude Code's own checkpoints miss edits made through Bash and subagents ([docs](https://code.claude.com/docs/en/checkpointing)), so the daemon's snapshot is the reliable one.

- Rewind code, conversation, or both to any of your messages.
- "Summarize from here" asks the agent to compact older context, freeing room without starting over.
- Snapshots are pruned with the session, and never pushed.

## CLI and API

The `cli` client speaks the same protocol as the apps, so scripts and agents can drive any machine.

- Spawn a session, send a message, wait until a session is blocked or done, read its output, list and archive sessions.
- The same commands are offered to agents as an MCP server, so one agent can start another and wait for it.
- Tokens are scoped per project and revocable from the device list.

## Adopting terminal sessions

Sessions started with plain `claude` or `codex` in a terminal are not lost to the app.

- The daemon lists recent sessions from each agent's local store, and adopting one resumes it under the daemon with full history.
- A thin `dent claude` and `dent codex` launcher keeps the terminal UI while the session streams live to the app, so you can take over from the phone and hand back.

## Performance limits

Long sessions must stay light: memory and storage cannot grow without bound as a session ages.

- Diagnostic logs never go into the event store; they rotate by size.
- Memory per session is capped; old events stay on disk and page in on demand.
- SQLite's write-ahead log is checkpointed on a schedule.
- CI runs a 100,000-event session and fails if memory or open time regresses.

## Goals and long runs

A goal is a session told to keep working until a stated outcome is met, using each agent's native goal mode where it has one.

- The session shows the goal, progress the agent reports, elapsed time and the usage it has consumed.
- An optional budget stops the run at a share of the 5-hour or weekly limit.
- A run that hits a usage limit pauses with "waiting for reset at 3:40" and resumes on its own when the window resets.

## Moving sessions between machines

Any session can move to another machine, for example from the laptop to the EC2 box before closing the lid.

1. The source daemon pauses the session at a turn boundary.
2. It sends the target the event log, the agent's own transcript, and the workspace state: the branch, unpushed commits as a git bundle, and uncommitted changes as a patch.
3. The target clones the repo if it lacks it, applies that state, imports the transcript and resumes the agent.
4. The session keeps its identity and history; the old machine shows where it moved.

If the target lacks a secret or a tool the session used, the move is refused with what is missing, before anything stops.

## Automations

An automation starts a session from a prompt on a schedule or on an event, and drops the result into the session list as Done.

- **Triggers:** a schedule; GitHub events such as a PR opened, a check failed or an issue labelled; or a webhook.
- **Where it runs:** a chosen machine, or a fresh ephemeral or hosted machine that is torn down afterwards.
- **Who fires it:** the hub, so automations run while your laptop is closed. If the chosen machine is offline, the run waits, moves to a fallback machine, or is skipped and reported, as configured.
- **Guards:** a usage threshold above which runs wait, a time and usage budget per run, and a permission mode per automation (default: no pushing without review).
- **Results:** every run lands in the session list under Done with its summary, diff and PR, ready to continue or close.

## Browser for agents

The browser runs on the agent's machine, so it works the same for remote and ephemeral machines.

- The daemon runs a headless Chromium per session on demand and offers it to the agent as browser tools over the Chrome DevTools Protocol.
- Clients show a live view of that browser and can take over the mouse and keyboard.
- You can draw on a screenshot and send the annotation to the agent as feedback.

## Subagents

Subagent output is kept as a tree, not flattened into the parent's stream.

- Each subagent has its own status, tool calls and result, nested under the step that started it.
- A single subagent can be stopped without stopping its parent, where the agent supports it.

## Turn status and notifications

You are notified when an agent is blocked or has finished and is waiting for your next prompt, and never while it is just waiting on something it started. Status comes from the agents' own signals first. Claude Code's fields were confirmed in [spike 01](spikes/01-claude-code-stream-json.md); an interrupted turn ends with `result` subtype `error_during_execution` and `terminal_reason` `aborted_streaming`, which is not an error and not a notification.

| Situation | Claude Code signal | Codex signal | Notify |
| --- | --- | --- | --- |
| Working, including waiting on a command or build it started | Stream events still flowing, or a tool call without its result | Item events in progress | No |
| Blocked on an approval | `control_request` with subtype `can_use_tool` | `item/commandExecution/requestApproval`, `item/fileChange/requestApproval` | Yes, time-sensitive |
| Turn ended while background work it started is still running | `result` arrives while `system/background_tasks_changed` lists tasks; when they finish, `task_notification` arrives and the agent starts a follow-up turn on its own (a new `system/init`) | Not applicable | No; notify when the follow-up turn's `result` arrives with no background tasks left |
| Finished, waiting for your next prompt | `result` message ends the turn | `turn/completed` | Yes |
| Error or usage limit hit | Error result or limit event | Error notification | Yes |

- Sessions launched with `dent claude` keep the terminal UI, so there the daemon uses Claude Code's Stop and Notification hooks instead of the stream.
- The notification says whether the agent finished or asked you a question, using the last message.
- Per project you can mute finished-turn notifications and keep only blocks.

## Decision model

A small decision model answers questions with fixed answers in a few hundred milliseconds for a fraction of a cent. Jev ([overview](https://apidog.com/blog/what-is-jev/)) and Cloudflare's Clef, which accepts the same requests and has open weights ([deep dive](https://flaviocopes.com/clef/)), both fit behind one interface.

- **Use at launch:** routing quick captures and new sessions to a project and repo, or deciding they are a new project (see Desktop app, Quick capture).
- **Fallback only:** if the spike finds agents or situations where turn status is ambiguous, for example ACP agents that do not report background work, the model classifies the final message as done, asking, or still waiting. Native signals always win when present.
- **Privacy:** calls go from your daemon with your own key, never through the hub, since they send session content to a third party. The feature is opt-in, and self-hosted Clef is the private option.
- **Optional:** with no key configured, every decision falls back to a simple rule and the app still works.
