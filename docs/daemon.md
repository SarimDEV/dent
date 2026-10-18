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
- Agents keep their own transcripts. On restart the daemon resumes the agent's session by id (`--resume` for Claude Code, thread resume for Codex) and reconciles against its own event log. dent names Claude Code's session up front with `--session-id`.
- Messages sent while the agent works wait in the daemon until the turn ends and can be taken back; **Send now** steers one into the running turn. Stop interrupts, then stops the process after 2 seconds if the turn hasn't ended ([0038](decisions/0038-local-core.md)). A stop request says what asked ("the Stop button", "⌘."), and the daemon logs it, so a stop no one remembers can be traced.
- Events reach the live feed in `seq` order (the log stays locked until each is sent), and a client joining mid-reply gets the reply so far once: deltas already in its snapshot are skipped by number. A client that stops reading is held to a bounded queue; it falls behind the feed and catches up from the log, and one stuck on a write for 30 s is dropped. Catch-up passes over log rows this daemon can't read.
- Clients on the same machine connect over a Unix socket only the user can open; other machines use the WebSocket from [0033](decisions/0033-direct-websocket-hub-on-hold.md).
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
- The same short-lived process answers `initialize`, whose models and slash commands are kept as the ones every session starts with, so a new session offers `/` commands before its agent runs ([0043](decisions/0043-prompt-completion.md)). A session's own list replaces them once its agent starts.
- A command the agent runs itself answers as a `Command` event with a typed outcome and, when it has one, a view (a meter, limit bars) ([0046](decisions/0046-commands-by-type.md)). Commands are sent with typed arguments read from their hints; the probe also reads `/config`'s settings and `get_settings` (no session saved, hooks off). Commands whose answer is a dialog in the terminal (`/status`, `/mcp`, `/skills`, `/memory`, `/hooks`, `/permissions`) answer with a `Sheet` view built from their control requests (`claude/sheets.rs`); `Act` requests do what a row offers and the sheet comes back live as `Out::Sheet`, polled while a sign-in finishes in the browser. An MCP server's configuration never goes into a sheet, since its arguments can hold a key.
- After each turn and each compaction the daemon asks for `get_context_usage` and logs the count with the window size, the auto-compact threshold and what fills it. Compacting and retrying are short-lived `Activity` messages, not log events; a compaction is a `Compacted` event. `request_user_dialog` kinds dent shows are declared at `initialize` and become questions.

## Session event log

Every session is an append-only log of typed events in SQLite: user message, assistant text delta, tool call, tool result, approval requested, approval answered, file changed, status change, usage update, error. Each event gets a monotonic sequence number per daemon.

- Clients subscribe with "everything after sequence N" and receive a live stream afterwards; reconnecting is the same call.
- Streaming deltas are never logged: each message's final event replaces what streamed of it, and clients drop the stream when it arrives, so history stays compact while live views stay smooth. A call whose input is still streaming (a long file being written) is a live activity, named by its target once that has streamed.
- A tool result is logged whole up to 32 KB. Longer output keeps its first 8 KB and last 24 KB, cut at line breaks (never inside a colour code), with a line between them saying how many lines were left out; the whole output stays in the agent's own transcript. A subagent's report (the result of a call that runs one) is its whole answer and keeps up to 64 KB the same way. So one result is at most about 64 KB in the log, and a catch-up batch (64 KB) holds at least one.
- Session status (working, blocked on approval, idle, done, errored) is derived from events and indexed, so every client can show it without replaying logs.
- SQLite runs in WAL mode with one writer task per daemon; reads never block writes.

Two small things live in the log beside the conversation ([0050](decisions/0050-titles-and-seen-marks.md)):

- **Seen marks:** `seen { upto }` when someone looks at the session; it is unread while a turn's end or a question is past the mark. Looking doesn't change the session's last-activity time.
- **Titles:** after the first turn, a one-turn `claude -p` on `haiku`, with no tools, run in the state folder's `titles/`, proposes a title, logged as `settings`. Never on its own after a rename; `retitle` asks again.

- **Carrying on at a reset:** `resume { at }` (Unix seconds) holds a session's resume; the session's own timer sends "Carry on from where you stopped…" when it comes, and `resume {}` takes it back. Anything sent meanwhile takes it back too. It survives restarts (it's in the log).
- **Background work:** `system/task_started` names the call that started each task, so a task carries its kind and call; `stop_task` stops one through Claude Code's own control request. Work that dies with the agent is named in the notice.
- **Thinking:** thinking deltas set the `thinking { latest }` activity to the last finished sentence; it clears with the answer, a call, or the turn's end.
- **MCP servers:** `init`'s `mcp_servers` that failed, or want sign-in (claude.ai's own connectors aside), log one notice per server and status.
- **Versions:** a fork can stop before a given event (`until`): carrying on an earlier version is a fork up to the rewind that left it.
- **Awake:** see [0051](decisions/0051-global-shortcut-and-keep-awake.md).

Pictures in a tool's result (`image` blocks) are written to the session's attachments folder as `tool-<call>-<n>.<ext>` and named in the `tool_result` event's `images`; one that can't be written reads `[image]` in the output.

## Persistence

"Everything survives" means three layers:

1. **Agent processes** keep running while clients disconnect. The session holder holds their stdio, so they survive daemon upgrades and crashes too.
   - Output goes to disk first, and each event records the byte it ended at, so a restarted daemon picks up exactly where it left off ([spike 06](spikes/06-session-holder.md)).
   - The daemon stops an agent's process after 5 minutes with nothing running, and resumes its session from the transcript on the next prompt ([0038](decisions/0038-local-core.md)).
2. **Daemon restarts** (upgrade or crash) carry on with agents still running in the holder. After a reboot, or if the holder itself dies, each session resumes from the agent's own transcript and the event log; in-flight turns are marked interrupted and can be continued with one click.
3. **UI state** (open sessions, panes, scroll position, markdown tabs, drafts) is shared state, synced like projects (see Sync).

## Git

The daemon does all git work on its own machine and sends results to clients: status, diffs per file, stage and unstage per file and per hunk, discard, commit, small edits written back to the working tree. Every write goes through the `git` command so behaviour matches the user's git config and hooks exactly. `gix` is used read-only for fast status and object reads, and imara-diff computes diffs. Hunk staging writes the new file contents as a blob and updates the index entry, with no patch files. A file watcher pushes status changes, so diff views update live as agents edit.

## Terminals

The session holder owns terminal PTYs the same way it owns agents, so a terminal survives client and daemon restarts and lives on the session's machine. It runs the user's login shell in the chosen directory and streams raw output; clients render it with a terminal emulator (see Desktop app).

## Files and markdown

The daemon serves file reads, directory listings and a watch stream, scoped to project roots. `Files` lists a session's folder for `@` mentions (git's tracked and untracked files, or a walk that skips hidden folders, `node_modules` and `target`); `FilesIn` lists any folder, for a session that hasn't started.

Projects are the folders sessions ran in, the projects in `~/.claude.json` (less any in temporary space, which were one-offs), and folders added from the picker (kept in the store's `kv` table, as are the ones taken off the list). `Folders` completes a typed path to its subfolders (hidden ones only after a typed dot); `AddProject` keeps a folder on the list, making it if asked; `NewProject` makes `~/dent/projects/<name>` (`-2`, `-3` when taken) with `git init`, a README and a first commit, and says so when the commit can't be made; `CloneProject` runs `git clone -- <url>` there for remote URLs only, with prompts turned off so a private repository fails rather than waits. Markdown files changed by an agent show up as events, which drives live reload in the viewer and the "new docs" list per session.

## Checkpoints

Before every turn the daemon snapshots each workspace's working tree using a temporary index, write-tree and commit-tree, kept under hidden refs/dent/ refs. Untracked files over a size cap are skipped, and the user's own index is never touched. Claude Code's own checkpoints miss edits made through Bash and subagents ([docs](https://code.claude.com/docs/en/checkpointing)), so the daemon's snapshot is the reliable one.

- Rewind the conversation, or the conversation and files, to just before any of your messages ("Edit from here"). The working tree comes back from the snapshot taken before that message; files made since are removed. The conversation picks up at the agent's last reply before it, as a new agent session (`--resume … --resume-session-at <message> --fork-session`), so the old transcript stays whole. Rewinding while a turn runs is refused.
- "Summarize from here" asks the agent to compact older context, freeing room without starting over.
- Snapshots are pruned with the session, and never pushed.

## CLI and API

The `cli` client speaks the same protocol as the apps, so scripts and agents can drive any machine.

- Spawn a session, send a message, wait until a session is blocked or done, read its output, list and archive sessions.
- The same commands are offered to agents as an MCP server, so one agent can start another and wait for it.
- Tokens are scoped per project and revocable from the device list.

## Adopting terminal sessions

Sessions started with plain `claude` or `codex` in a terminal are not lost to the app.

- The daemon lists recent sessions from each agent's local store, and adopting one resumes it under the daemon with full history. For Claude Code that's `~/.claude/projects`: only sessions run in a terminal (`entrypoint` `cli`), in folders that still exist, not already in dent and not touched in the last minute (likely still open). Listing reads each transcript's head and tail; adopting imports the conversation and resumes it with `--resume`.
- The import keeps what the person saw: their prompts, including pictures (saved under the state folder), slash commands and `!` commands; replies and tool calls; stops; usage-limit and API errors as errors, not replies; compactions in the order they happened; and what local commands such as `/model` printed. Tooling lines (task notifications, reminders, command caveats) and subagents' own transcripts stay out of the main conversation. Its title is the session's own name in Claude Code when it has one.
- `dent-daemon check-transcripts [DIR] [--into STATE]` runs every transcript through the import and checks it against the raw lines: nothing unreadable, no reply, tool call or result dropped, every event read back unchanged. It prints counts and session ids only. With `--into`, every conversation becomes a session in a scratch state folder, and `dent-desktop --walk sweep` opens each one and times it.
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
- A subagent sent to the background answers its call at once with a launch receipt, which is not its result: it stays working past the turn's end until it leaves the agent's background tasks, and its report is the last thing it said.
- In the chat, a subagent's row leads with its state ("Subagent working:"), shows its latest five steps with a line that opens the earlier ones, and the working line and sidebar name its current step.
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

### Turns always end

Other harnesses that drive Claude Code this way have hit the same failures again and again: threads stuck on "Working", conversations that can no longer be resumed, and agents still running after the app said they had stopped. dent guards against each one:

- **A turn is open from the message dent hands over until a `result` or the process exits.** Nothing else, such as heartbeats or status lines, opens or closes one. A restarted daemon whose agent is gone ends the open turn as interrupted.
- **Stop always ends the turn.** dent sends an interrupt and allows 2 s. Then it stops the agent's whole process group, and 5 s after that it lets go of a process that still hasn't exited. If the agent can't be reached at all, it is stopped at once. Messages that were still waiting come back to the composer instead of disappearing.
- **An agent stuck starting** (one holding a lock, say) gets a minute to answer `initialize`. After that the turn ends with the last line Claude Code printed.
- **A conversation Claude Code can't find** ("No conversation found with session ID", when a process was stopped before it wrote a transcript) doesn't break the session. dent starts a new conversation, says so, and sends the message again.
- **Every `control_request` gets an answer.** Approvals and questions go to you. An MCP server asking for input (`elicitation`) is declined with a note, and any other request is refused. An unanswered request would stall the agent for good.
- **Approvals still open when a turn ends** are expired, so nothing waits on an answer that can't be used.
- **A setting changed to what it already is does nothing.** In particular it never restarts a running agent.
- **A model or permission change is kept once the agent takes it.** A running agent is asked first; if it refuses, the old setting stays and the refusal shows in the chat. With no agent running (or one winding down) the change is kept at once and the next agent starts with it.
- **Background work is never stopped by a setting.** An effort change while background work runs takes effect with the next message once the work is done. Whatever ends the agent process (a crash, a rewind, the holder going) ends its background work too: the session says so and leaves Background, so Done is still reported.
- **An agent winding down after going idle takes nothing more.** A message sent in that moment starts a new agent; a message that can't be written is tried once more with a fresh agent, and otherwise ends the turn with the error.
- **Stop and start limits survive a daemon restart.** A Stop is kept until its turn ends, so a restarted daemon still stops the process if the turn doesn't end; an agent that hadn't said anything yet still gets only its minute to start. Control request ids carry the daemon's boot id, so a late answer to an old request never matches a new one.
- **Requests a client sends again happen once.** Clients resend requests that got no reply after a reconnect. Create is recognised by its first message's id, Fork by its own id, a side question is promoted to one session only, and a rewind to where the session already is (nothing said since) does nothing, so files edited by hand since aren't put back. This holds across daemon restarts.
- **"Always allow" never allows more than the agent offered.** The rules Claude Code suggests are logged with the question, so an answer after a restart saves the same rule at the same scope; for a question from an older log, where they weren't kept, "always" allows once.
- **A session whose folder was deleted says so** instead of failing to start the agent with a spawn error.
- **The PATH agents get is re-read from the login shell** in the background each time an agent starts, so tools installed after dent started are found. A missing `claude` is reported as such.
- **"Done" is notified once**, when the turn and any background work it started have both finished.

## Decision model

A small decision model answers questions with fixed answers in a few hundred milliseconds for a fraction of a cent. Jev ([overview](https://apidog.com/blog/what-is-jev/)) and Cloudflare's Clef, which accepts the same requests and has open weights ([deep dive](https://flaviocopes.com/clef/)), both fit behind one interface.

- **Use at launch:** routing quick captures and new sessions to a project and repo, or deciding they are a new project (see Desktop app, Quick capture).
- **Fallback only:** if the spike finds agents or situations where turn status is ambiguous, for example ACP agents that do not report background work, the model classifies the final message as done, asking, or still waiting. Native signals always win when present.
- **Privacy:** calls go from your daemon with your own key, never through the hub, since they send session content to a third party. The feature is opt-in, and self-hosted Clef is the private option.
- **Optional:** with no key configured, every decision falls back to a simple rule and the app still works.
