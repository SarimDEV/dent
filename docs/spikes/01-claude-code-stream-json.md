# Spike 01: Claude Code stream-json

Date: 2026-10-11 · Claude Code 2.1.296 · Code: [spikes/claude-stream](../../spikes/claude-stream)

Question: can the daemon drive `claude -p` over stream-json with approvals, interrupts, resume, usage limits and reliable turn status?

**Answer: yes, all of it.** No sidecar needed.

## Invocation

```
claude -p --input-format stream-json --output-format stream-json --verbose \
  --include-partial-messages --permission-prompt-tool stdio [--model M] [--resume SESSION_ID]
```

- One long-lived process per session; it stays alive across turns and across interrupts.
- First write a `control_request` `initialize`. Its response carries `commands`, `models`, `agents`, `pid`, `claude_code_version`, `current_permission_mode`, and an `account` object (personal data — never log or record it unscrubbed).
- User messages: `{"type":"user","message":{"role":"user","content":"…"},"parent_tool_use_id":null,"session_id":""}`.

## What we learned

| Topic | Finding |
| --- | --- |
| Approvals | `--permission-prompt-tool stdio` makes the CLI send `control_request` `{subtype:"can_use_tool", tool_name, input, description, permission_suggestions, tool_use_id, blocked_path}`. Answer with `control_response` `{behavior:"allow", updatedInput}` or `{behavior:"deny", message}`. The deny message reaches the model verbatim, so "deny with a reason" is free. `permission_suggestions` gives ready-made "allow for session / always" options. |
| Usage limits | `rate_limit_event` carries `unifiedWindows.five_hour` and `.seven_day` with `utilization` (0–1) and `resetsAt` (unix seconds), plus `status` and overage fields. It is **not sent every turn** — only on the first response and when it changes — so the daemon must persist the last value per account. The status-line fallback is not needed. |
| Turn end | `result` ends every turn: `subtype` `success` / `terminal_reason` `completed` normally. |
| Interrupt | `control_request` `interrupt` → response `{still_queued: []}`, then a synthetic user message `[Request interrupted by user]` and `result` `error_during_execution` / `aborted_streaming`. The process and session survive; the next message just works. |
| Queueing | The interrupt response's `still_queued` shows the CLI queues user messages sent mid-turn natively, which is the basis for queue-or-steer. |
| Background work | A `run_in_background` Bash emits `system/background_tasks_changed` (full task list) and `task_started`. The turn's `result` arrives while the task runs. When it finishes: `task_updated`, `task_notification` (with summary and output file), `background_tasks_changed` (empty), then **the agent starts a follow-up turn on its own** (new `system/init` … `result`). So: suppress "finished" while tasks are listed; notify on the follow-up turn's `result`. Implemented and unit-tested in `status.rs`. |
| Turn start | `system/init` is emitted at the start of every turn (including self-started ones), not just once. |
| Streaming | `--include-partial-messages` adds `stream_event` wrappers around raw API events (`content_block_delta`/`text_delta`, etc.). Complete `assistant` messages still arrive, so history can store only those. `system/thinking_tokens` gives a running thinking estimate. |
| Resume | `--resume <id>` in a new process keeps the same `session_id` and full history. |
| Mid-session controls | `set_permission_mode`, `set_model`, `set_max_thinking_tokens`, `get_context_usage`, `mcp_status`, `get_settings` all work. Unknown subtypes return a clean error. `rewind_files` is disabled by default (fine: we own checkpoints). |
| Context meter | `get_context_usage` returns `totalTokens`, `maxTokens`, `percentage`, but takes ~3 s after a turn. Drive the meter from each `assistant` message's `usage` (input + cache tokens) and use `get_context_usage` only for the detailed breakdown. |
| Subagents | Not exercised; messages carry `parent_tool_use_id`, which the renderer already indents. Confirm with a real subagent in Phase 2. |
| User config | The spawned CLI loads the user's own settings, hooks, plugins, MCP servers and CLAUDE.md (hook events appear as `system/hook_started`/`hook_response`). Approvals still reach us even with a user `PermissionRequest` hook configured. `--bare`, `--setting-sources` and `--strict-mcp-config` exist if we ever need isolation. |

## Changes to the spec

- docs/daemon.md: Claude Code driver row now names `--permission-prompt-tool stdio`, the `can_use_tool` shape and `rate_limit_event`; the turn-status table describes the background-work follow-up turn and interrupt result.
- docs/risks.md: the Claude usage-limit risk is resolved.
- docs/durability.md: feature detection uses the `initialize` response and `system/init`.

## For the owner

- **Should dent-launched Claude sessions load your full personal Claude config** (hooks, plugins, MCP servers)? Today they do, which matches the terminal. I'd keep that as the default; it isn't listed as open in the spec, so just flagging it.
- Recordings contain the `account` object and local paths. They're gitignored; the replay driver will need a scrubber before any recording becomes a committed fixture.

## How to run

```
just dev spike-claude-stream --cwd /path/to/a/scratch/repo --model haiku
```

Type a message. When it asks `approve? [y / n reason]`, answer on the next line. `/interrupt`, `/mode plan`, `/model sonnet`, `/context`, `/quit`. Raw traffic is recorded in `spikes/claude-stream/recordings/`.
