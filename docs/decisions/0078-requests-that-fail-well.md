# 0078. Requests across machines fail well: classified, never blindly repeated, always ended

Status: accepted (2026-10-30, the owner's call) · Builds on [0060](0060-remote-wire.md) (the wire) and [0063](0063-queued-prompts.md) (what waits for a machine) · Lessons from T3 Code: `docs/internals/connection-runtime.md` there, and its handoff state machine

## Why

Each of about 45 kinds of request handled waiting, time limits, cancelling and errors its own way, or not at all. A picker could wait forever ([#66](https://github.com/SarimDEV/dent/pull/66)). After a link dropped, every unanswered request was sent again on the assumption that "a resend changes nothing twice". An audit of every handler found that about 15 kinds were not like that:
- **Discard one hunk** named it by position: sent again, it put back the hunk after it.
- **Stop** could stop a newer turn.
- **New project** and **clone** made a second copy.
- **Commit** could commit what had changed since, unasked.
- **Adopt** could take a session in twice.
- **Answer**, **take back** and **remove device** said that what had worked had failed.

T3 learned the same lessons:
- It never replays a change on reconnect; whether a change may be retried belongs to the change.
- It uses idempotency keys.
- A wait belongs to the request that started it, and is let go when its scope changes.
- Each connection has a generation, and late callbacks from an older one are dropped.
- Deadlines are monotonic and measure stalls, not totals.
- Moves are explicit state machines.

## Decision

1. **Every request says what a second copy would do** (`Request::resend`, with no catch-all arm, so a new request can't build without saying):
   - `Read` only looks.
   - `Repeatable` changes nothing more the second time, because it carries an id the daemon checks or sets a value outright.
   - `Once` would do something else or more.

   After a link drops (`dent-remote`), or the local daemon restarts (`desktop/live.rs`), unanswered `Read` and `Repeatable` requests go again. A `Once` request that went is never repeated. Its client is told the connection dropped before the machine answered, "so that may or may not have happened; look before trying again". A request that never went still goes.
2. **Discard names its hunk by what it read** (`Discard.seen`, `hunk_print`). Only a hunk that still reads as it was shown is put back, wherever it now is, so a repeat or a stale view can't take another. Each diff the daemon makes for it has its own scratch folder, so two discards at once never read each other's files.
3. **One record per request in flight**, in `client-core`. It holds the machine, the kind, the scope it belongs to, when it was sent, its deadline, the link generation, and its state (queued, sent, done, failed, unknown). It ends in one of four ways, each with its own words and one next step:
   - **Didn't reach** (the machine was away): nothing happened. Retry, or it waits, as messages do (0063).
   - **Refused:** the machine's reason, in place.
   - **No answer, or the link dropped after it went.** A read is said not to have answered, with Retry. A change "may or may not have happened", with Check, which looks again at what's there.
   - **Stalled:** a long job (a clone, a pack, a move) made no progress for a while, told apart from one that crashed.

   A wait ends when its scope does (a closed picker, another machine chosen). Features read this record instead of keeping timers of their own.
4. **Every change carries an id** the daemon remembers for a while. A repeat gets the first answer, not a second effect, so every `Once` becomes `Repeatable`. An absolute setter also says what it was set from, so a late repeat can't undo a newer change from another device.
5. **The daemon answers every request exactly once**, even when something inside it fails. Long jobs say each step, so a client can show progress and tell a stall.
6. **One suite of failure tests over every kind of request**, against a link made to fail:
   - dropped before the request goes;
   - dropped after it goes;
   - an answer after the deadline;
   - an answer after its scope has gone;
   - an answer twice;
   - a reconnect partway.

## Consequences

- **Step 1, built:** points 1 and 2, with tests: only what can be repeated goes again, and a discard sent twice puts back one hunk.
- **Step 2, built:** point 3's deadlines (`client-core::inflight`). Every request has a patience (`Request::patience`): 30 s for most, 2 minutes for big reads and Git, 10 minutes for what clones, starts an agent or moves. Its time counts only while its machine is connected. Past that, whatever waited on it hears an error in words: a read "didn't answer in 30 seconds"; a change "may or may not have happened; look before trying again". A request for a machine that's gone ends at once. A read asked in the background ends quietly, and only what shows its own wait says so. A message waiting in the outbox is left to its link to send again.
- **Step 3, built:** point 4's keys (protocol 5).
  - Every change is sent with a key on its envelope (`encode_request`): the client's run and the request's number, the same however often it's sent.
  - A daemon of protocol 5 runs a keyed change once (`daemon/answered.rs`) and keeps its answer for a day, on disk, so a restart doesn't forget it. A copy gets that answer; a copy arriving while the first still runs waits for it, which closes the races the audit found in Create and Adopt.
  - To such a daemon, a client sends every change again after a dropped link. To an older daemon, step 1's rule still holds.
  - Tested against a real daemon: a keyed new project sent again on another connection, and after a restart, makes one project and gets the first answer.
- **Step 4, built:** point 5's long jobs and stalls.
  - While a request with 2 minutes' patience or more runs, the daemon says `Out::Progress` for it every 5 s. It isn't the answer: clients keep the request pending.
  - Once heard from, a request's limit becomes a minute of quiet (`inflight::STALL`). A healthy 20-minute clone never times out, and a stalled one is said within a minute: "stopped answering partway through".
  - Every git the daemon runs starts at `git::cmd()`, which never prompts. It also gives up when HTTP drops below 1 KB/s for a minute or SSH stops answering keepalives. A guard test (`every_git_starts_here`) holds every git in the daemon to it, and catches one planted outside.
  - Tested against a real daemon: a clone from a server that accepts and then says nothing sends heartbeats, then fails by itself after about 60 s with git's reason.
- **Still to come:**
  - an absolute setter that also says what it was set from, so a late copy can't undo another device's newer change;
  - a wait tied to the view that started it, generically;
  - named steps for long jobs (clone percentages);
  - the full failure suite.
- **Some errors appear where there were none.** A stop, commit or new project caught by a dropped link now says it may not have happened, instead of being silently repeated.
- **Starting numbers, from T3:**
  - opening a link times out after 15 s;
  - pings go every 5 s, and a link drops after 15 s with no frame;
  - retries back off up to 30 s;
  - a deadline counts stalls, on a monotonic clock.
