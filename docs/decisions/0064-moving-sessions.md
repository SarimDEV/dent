# 0064. Moving a session: a new id on the target, a guarded move, the real transcript, git in a bundle

Status: proposed (2026-10-20) · confirmed by [R-D1–D5](../remote-tests.md#d-moving-sessions) · Supersedes the steps in [daemon.md](../daemon.md#moving-sessions-between-machines) · Plan: [remote-plan.md](../remote-plan.md)

## Decision

### Identity

**A moved session gets a new id on the target.** The source session becomes a read-only record that says where it went (`departed → (machine, session)`). The target's session records where it came from. Clients show the two as one continuous conversation.

Each daemon numbers its own events and machines share nothing ([0058](0058-independent-machines.md)), so one id living on two daemons would need someone to decide which copy is live, and renumbering. Two ids and a pointer need neither.

### Who carries it

**A client trusted by both machines relays the move,** so daemons never need to trust each other:
- The transfer is in chunks, can be resumed from an offset, and is verified by hash.
- The source keeps everything needed in a move folder under its state folder, so any client trusted by both machines can resume an interrupted move.
- The phone can start a move. Running it needs a client that stays connected, or it pauses and resumes later.

### The state machine on the source

`none → pending → departing → departed`, or back to `none` or `failed`. Every step is a compare-and-set written in the session's event log, with a move id.
- **`pending`:** the turn or its background work is still running. The move waits for it to end. Any new message from the person cancels the move.
- **`departing`:** the session is fenced. One check, used by every command that can start agent work or change the worktree, refuses:
  - sends;
  - rewind;
  - fork;
  - side questions;
  - model and mode changes;
  - terminals;
  - automations bound to the session.

  Queued prompts must be sent or cancelled before a move starts.
- **`departed`:** the target accepted the move. The source keeps its worktree until the person removes it.
- **`failed`:** the session works again. Two things can cause it: the target's typed **not accepted** reply, which is sent only from code that runs before the target writes anything; or a failure before anything was sent.

  Any other failure leaves the session `departing`, with **Retry** (same move id) and **Continue here**, which warns that the target may already hold a copy.
- **Before the source sends anything,** it marks the move "uncertain" on disk.

### Preflight

Read-only; nothing stops. It checks:
- the target daemon's version and capabilities;
- the target's agent CLI is at least the source's version (or the versions R-D1 proves safe);
- the secrets, tools and MCP servers the session used are present on the target;
- the repository can be reached on the target, and the branch hasn't diverged or been checked out elsewhere there;
- there are no nested repositories or dirty submodules;
- sizes are under the cap.

The person also sees the list of ignored files that will be carried, `.env*` ticked by default from the project's carry list.

### Packing

The agent process exits at a turn boundary (through the holder), so its transcript is complete.
- **Working tree:** a snapshot commit, built in a copy of the real index, so the person's staging, split index and fsmonitor are untouched. Same-second edits are re-checked, and untracked files are included.
- **Bundle:** contains only commits the target doesn't already have. The target reports what it has, so commits pushed only to a fork still travel. It is capped at 200 MiB; past that, "push the branch, then move".
- **Conversation:** the agent's transcript and its side files (subagents, saved tool results, file history), dent's saved attachments, and dent's event log.
- **Manifest:** hashes of each part. It is written with fsync.

### Accepting, on the target

In this order:
1. **Look up the move id first.** A move already accepted returns its session before the upload or git are touched. The target's session id is derived from (source machine, source session, move id).
2. **Apply git** into a new worktree. Every ref change is a compare-and-swap, and so is its undo, so a branch someone advanced meanwhile is never reset. Branches move fast-forward only. Every resource the move creates is recorded as owned, and rolled back on any failure before acceptance.
3. **Place the transcript** under the target's folder for that worktree, as the agent expects.
4. **Import dent's events** as the new session's history.
5. **Record "resume pending",** in the same transaction as step 4.

### Resuming

`--resume` from the new worktree, with a first message naming the move and the old and new paths. If resume fails, the session starts fresh with a summary built from dent's event log. That log has tool calls, not just text. The session says so, and records the delivery so a retry never sends the history twice.

### What doesn't move

- Ignored files outside the carry list.
- The split between staged and unstaged changes (everything arrives unstaged, and the move says so).
- Running terminals and dev servers.

### Interruption points

R-D3 kills each side, or the link, at every one of these, and requires exactly one live copy afterwards:
1. After `pending` is recorded.
2. After `departing` is recorded, before packing.
3. During packing.
4. After "uncertain" is written, before the first chunk.
5. Mid-transfer.
6. After the last chunk, before the target replies.
7. Inside the target's git apply.
8. After the target's acceptance, before the source hears.
9. After the source records `departed`, before cleanup.
10. While resuming on the target.

## Why

The study of a mature tool's unfinished move feature gave the protocol: the state machine, the uncertain mark, the not-accepted proof, Continue here, the derived import id and the git snapshot. That tool moves only the text of the conversation, so its moved agent starts with a short summary and none of its tool results. dent drives Claude Code's own transcript, so it can resume the real session, which is better as long as R-D1 proves it works across machines and versions. The patch in the old plan lost binaries, deletions, file modes and renames. A snapshot commit carries them exactly.

## Consequences

- [daemon.md](../daemon.md#moving-sessions-between-machines) is rewritten to this record once it's accepted.
- Moves arrive with stage 4 of the plan, after links, projects and the launcher.
