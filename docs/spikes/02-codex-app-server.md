# Spike 02: Codex app-server with generated types

Date: 2026-10-11 · codex-cli 0.162.1 (Homebrew cask `codex`; npm `@openai/codex`) · Code: [spikes/codex-app-server](../../spikes/codex-app-server)

Question: can the daemon drive `codex app-server` over stdio with Rust types generated from its schema, including approvals, interrupts, steering and rate limits?

**Answer: yes.** Across four runs, every inbound message (150+) decoded through the generated types with zero errors and no unknown methods.

## The CLI

- Still called `codex`, still `openai/codex`, released almost daily (0.161 → 0.162.1 in three days). Install: `brew install --cask codex` or `npm i -g @openai/codex`.
- `codex app-server` is still marked `[experimental]` in its help. It now also has `daemon` and `proxy` subcommands (a shared local app-server behind a control socket); not needed for dent, which owns its own process.

## Type generation

`scripts/gen-codex-types.py <crate dir>` runs `codex app-server generate-json-schema`, then:

1. Takes the v2 bundle (660 definitions) and merges in what it leaves out: the `ServerRequest` and `ClientNotification` unions, and the responses to server requests (approval decisions), each with only the definitions it references.
2. Commits the merged schema to `schema/codex-app-server.schema.json`, so a version bump shows up as a schema diff.
3. Runs `cargo typify` (0.10.0-alpha.2) → `src/generated.rs` (~24k lines, 834 types, builds in ~10 s). Three definitions typify can't represent stay as raw JSON: `CodexErrorInfo`, `ThreadItemsListCursor`, `McpServerElicitationRequestParams`.
4. Emits `src/dispatch.rs`: typify turns the notification union into an untagged 84-variant enum (slow, useless errors), so the generator writes method-keyed `Notification` and `IncomingRequest` enums with an `Unknown { method, params }` fallback, plus a `Request` trait that ties 94 of the 105 request params types to their method name and response type (`rpc.call(&ThreadStartParams { .. })`).

Rejected: the upstream `codex-app-server-protocol` crate (crates.io copy stuck at 0.63; the git version pulls in Codex internals like rollout, history, zstd, rmcp), and the third-party `codex-codes` crate (single maintainer).

## What we learned

| Topic | Finding |
| --- | --- |
| Handshake | `initialize` (clientInfo) → `initialized` notification. Then `thread/start` (cwd, model, `approvalPolicy`, `sandbox`) → `turn/start` with `input: [{type:"text", text}]`. `thread/start` takes a few seconds, so messages queue until it answers. |
| Approvals | Server requests `item/commandExecution/requestApproval` (with `command`, `cwd`, `reason`) and `item/fileChange/requestApproval`, answered by id with `{decision: "accept" \| "acceptForSession" \| "decline" \| "cancel"}`. **No reason field**: "deny with a reason" has to be a decline followed by `turn/steer`. `serverRequest/resolved` tells every client an approval is gone, which is what "answered once, everywhere" needs. |
| Status | `thread/status/changed` gives `idle`, `active` with flags `waitingOnApproval` / `waitingOnUserInput`, `systemError`. That maps straight onto working / blocked / idle / errored, more directly than Claude's events. |
| Turn end | `turn/completed` with `turn.status`: `completed`, `interrupted`, `failed`. Interrupt (`turn/interrupt` with thread and turn id) ends with `interrupted`; no error. |
| Steering | `turn/steer` with `expectedTurnId` works mid-turn, so queue or steer is native. |
| Limits | `account/rateLimits/read` and `account/rateLimits/updated` (sent several times per turn). On the **free plan** there is a single 30-day window (`windowDurationMins: 43200`) and no secondary window, so the UI can't assume 5-hour + weekly. One tiny turn (`touch` a file) used 1% of the free monthly allowance. |
| Account | `account/read` gives auth mode and `planType` (`free` here) plus the email; don't log it. |
| Models | The user's configured default (`gpt-5.3-codex`) is rejected for ChatGPT sign-ins with a 400 inside an `error` notification, and the turn fails. `model/list` returns what the account can use (`gpt-6-luna` worked). |
| macOS Gatekeeper | The Homebrew cask's helper `codex-code-mode-host` is signed by OpenAI but was quarantined and blocked by Gatekeeper. Codex then fails every tool call with "code-mode host exited during handshake". Fixed by clearing the quarantine flag on that file after checking its signature. `dent doctor` should detect this and explain it. |
| Errors in stderr | Codex logs ANSI-coloured tracing to stderr; the daemon should capture it to its rotating diagnostic log, not the event store. |

## Changes to the spec

- docs/daemon.md: Codex approvals have no reason field (decline + steer); model availability comes from `model/list`; new **Usage limits** section (also covers the owner's request below).
- docs/durability.md: how the types are generated.
- Decision 0031: usage limits are read per account through the agent CLIs and synced via the hub.

## Usage limits across machines

Usage limits should stay in sync whichever machine or tool you use Claude Code from. Both CLIs can be asked for account-level usage from the provider: Claude Code's undocumented `get_usage` control request (confirmed on 2.1.296: `five_hour`, `seven_day`, per-model weekly, extra-usage spend, `limits[]` with severity) and Codex's `account/rateLimits/read`. Because the provider counts everything, the reading already includes the terminal, the web and other machines. The spec's Usage limits section adds per-account storage, hub sync with one poller per account, and adaptive polling. The Claude spike has a `/usage` command that shows it working.

## How to run

```
just dev spike-codex-app-server --cwd /path/to/a/scratch/repo --model gpt-6-luna
```

Type a message (wait for the `── thread` line first). Approvals: `y`, `s` (for session), `n`. While a turn runs, a new message steers it. `/interrupt`, `/limits`, `/quit`. Regenerate types after a Codex bump with `just gen-codex-types`.
