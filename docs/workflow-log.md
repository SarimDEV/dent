# Workflow log

Things that slow the work down or make the output worse, written down by the agent as they happen, so the owner can say "work the log" and have them fixed. The rules for keeping it are in [AGENTS.md](../AGENTS.md#improving-how-we-work).

## What gets logged

Anything that costs time or quality and would happen again:

- **Slow loops:** a build, `just verify`, test or walkthrough run over ~2 min, or one that ran when nothing it covers had changed.
- **Too much testing:** the full walkthrough suite or E2E runs more often than the change needs; the same check run twice for one change.
- **Rework:** the owner corrected something, a change was redone, or a claim turned out wrong.
- **Weak output:** UI that looks generic or unpolished, code that's padded or duplicated, docs that drifted from the code, reports that overstate what was checked.
- **Broken or flaky tooling:** a script, check or test that failed for reasons unrelated to the change.
- **Manual steps:** anything done by hand that a script could do, and anywhere the work waited on the owner when it didn't need to.
- **Lost context:** knowledge that lived only in one session, machine or memory and had to be rebuilt.

## Entry format

```
### <short title>
- Seen: <date>, <what happened, with numbers: minutes, runs, files>
- Cost: <time lost per occurrence × how often, or what quality suffered>
- Fix: <the proposed change>
- Check: <how we'll know it worked: the number to re-measure>
```

Open entries are ranked by cost, highest first. When one is fixed, move it to Done with the before and after numbers. Delete an entry that turns out to be wrong.

## Open

### Daemon test suites take over 2 minutes, run whole for one-file changes
- Seen: 2026-10-23, `cargo test -p dent-daemon` takes ~2.5 min (`remote_break` 61 s, `daemon` 42-48 s, `cli_break` 26 s), and was run whole about 15 times in one session, mostly for changes to one module.
- Cost: ~35 min that session; it also loads the machine so walks and timing-sensitive tests flake.
- Fix: run the one test binary a change touches while working, and the whole suite once before the PR (as `just verify` does for the changed crates).
- Check: whole-suite runs per PR ≤ 2.

### A stale build cache pointed the desktop's release link at a deleted worktree
- Seen: 2026-10-23, `cargo build --release -p dent-desktop` failed to link: the cached `Info.plist` path was `~/dent/wt-inline/...`, a worktree since removed. `cargo clean -p dent-desktop --release` fixed it (348 MB removed, 2 min rebuild).
- Cost: ~5 min once; it'll recur whenever a worktree that built release goes away.
- Fix: have the desktop's build script emit `rerun-if-changed` for the plist it uses, so a moved plist is noticed.
- Check: building release after removing a worktree links first time.

### Edits chained with commit and merge in one command shipped a fix that never applied
- Seen: 2026-10-23, a scripted edit's anchor didn't match after `cargo fmt`, the command went on to commit, open and merge #34 with a description naming that fix; #35 carried it.
- Cost: one extra PR, and a description that overstated what merged.
- Fix: never chain an edit and a commit, PR or merge in one command; check `git diff --stat` before committing.
- Check: no PR description names a change its diff doesn't have.

### A fresh review worktree verifies from cold
- Seen: 2026-10-20, review of PR #12. The first `just verify` in a new worktree took 3 min 38 s (its own empty `target/`, and a Cargo.lock change selects every crate); the next run took about 15 s.
- Cost: ~3.5 min per review or batch started in a new worktree.
- Fix: a shared `CARGO_TARGET_DIR` for worktrees of the same repo (cargo locks it, so two sessions building at once queue rather than collide), or seed a new worktree's `target/` from the main checkout's.
- Check: first `just verify` in a new worktree under 1 min.

### A scroll fix patched one view before the cause was searched
- Seen: 2026-10-20, trackpad drift in the diff. The first PR fixed only the diff and text views; the owner asked again to search the whole codebase for the cause. The same cause (GPUI giving each nested scroller its share of a swipe) was in chat code blocks, wide and tall tables, and every vertical-only scroller.
- Cost: one extra review round and a second build-and-walk pass (~15 min).
- Fix: one rule in `swipe.rs`, with a unit test that fails on any scroller left out ([0057](decisions/0057-one-swipe-one-axis.md)). Before a fix, grep for every instance of the mechanism, not the symptom.
- Check: the next owner-reported UI bug's first PR covers every instance.

### A reference checkout compared against was out of date
- Seen: 2026-10-19, the projects and worktrees audit. The local reference checkout used for comparison was 60 commits behind its remote; the owner asked whether it was current before anything was built on it. None of the 60 touched the parts compared, but that was luck.
- Cost: one owner correction, and a comparison that could have been wrong.
- Fix: fetch a reference checkout (`git fetch` and compare with `origin/HEAD`) before reading it, and say which commit was read.
- Check: no audit or comparison that names a stale commit.

### A daemon test fails now and then under load
- Seen: 2026-10-18. `clients_joining_mid_reply_see_it_once_and_events_never_skip` failed once in `just verify` (daemon.rs:1528, the late client's partial text), passed 5 of 5 alone and on the next full run. Nothing in the batch touched streaming.
- Cost: one re-run of `just verify` (~15 s) and a few minutes checking it wasn't the change.
- Fix: find what the late clients race against (likely the 15 ms joins against a fast fake reply) and wait on an event, not a sleep.
- Check: 20 full `just verify` runs without it failing.
- Found 2026-10-18 (second failure): the reply sometimes finishes before the last late clients join, so they stream nothing and hold the reply whole from the log. The daemon was right; the test assumed every client joined mid-reply. The test now takes either, and still fails on text seen twice or cut short. Left open until the check above holds.

### Before stills taken after the change, not before
- Seen: 2026-10-18, chat parity batch. The stills meant to be taken first were skipped; main was built in a second worktree (3 min 17 s, its own target folder) to take them after.
- Cost: ~4 min and several GB of build output per batch it happens in.
- Fix: take before stills at the start of a batch, as the batch workflow says; when a scene needs the new fake agent, copy only `crates/fake-claude` into a main worktree.
- Check: batches that needed a second build for before stills.

### Walkthroughs need an unlocked screen, and no one is there to unlock it
- Seen: 2026-10-15. Locked, macOS draws no windows, so walkthroughs stop dead. A `walk.sh compare` sat at "Starting dent…" for 14 min while the owner was away; another day two runs hung 10+ min each before the cause was found (about 30 min).
- Fixed in part: `walk.sh` refuses to start on a locked screen and says so, keeps the display awake while it runs (`caffeinate -d`), and a still that doesn't come back in 15 s fails instead of waiting forever.
- Still open, deferred by the owner (2026-10-18): the display locking whenever the owner steps away. Options: keep-awake for a whole work session, or not locking on display sleep; both the owner's call.
- Check: walk runs lost to a locked screen per week.

### Switching machines lost context and time
- Seen: 2026-10-15. Moving to this Mac took about an hour of manual setup (Xcode tools dialog stuck, rustup, `just` from source without Homebrew, an SSH key, the winid helper), the owner stepped in five times, Screen Recording only surfaced after the first walkthrough, and batch 1 was left uncommitted on the old Mac.
- Cost: an hour per machine switch, plus work stranded until the owner can reach the old machine.
- Deferred by the owner (2026-10-18). The likely fix is a product feature rather than a script: dent's own guided "set up this machine" step in onboarding, which installs and checks what's needed and asks for every one-time permission up front. Meanwhile AGENTS.md carries the working agreements, and unfinished work goes to a WIP branch at the end of a session.
- Check: time a clean-machine setup.

## Done

### The idle frame-rate check was flaky
- Seen: 2026-10-15. "idle, the window draws no more than a caret blink (2.5 a second)" failed at 2.7 frames a second, then passed at 2.3 on an unchanged re-run.
- Fixed by "carets keep blinking in fields that lost focus" (below).
- Result: 0.3 frames a second in each of 6 full runs since, the last on 2026-10-18 (433 checks, 0 failed, 6 m 15 s).

### Real walkthroughs used for every visual check
- Seen: 2026-10-15. Each UI tweak in the chat-polish round was checked with a full real-agent `walk.sh compare` run (Haiku, ~40 tool calls): six runs at ~3 min plus build each, and the owner waited on every one.
- Fix: `scripts/walk.sh look --shots DIR`. The fake agent replays one turn recorded from the real agent (`crates/fake-claude/assets/real-turn.jsonl`: Haiku with dent's own flags and plan tool fixing a word count in a scratch project; narration, a plan, reads, an edit, a failed run and the fix, Claude Code refusing a compound command, an approval, the answer) at its recorded pace, and stills each moment in both themes. `compare` stays for a final check, once.
- Result: edit to eight stills in 23 s, build included (was ~3 min plus build, and real usage). Its first run found a real bug: the plan never showed when the agent sent dent's plan tool `{"plan": "[{step, status}]"}` (another tool's `update_plan` shape, as a string) instead of the schema's `items`. The desktop now reads every shape (test `a_plan_reads_in_every_shape_agents_send`).
- To record another turn: run `claude -p` with the flags in `crates/daemon/src/claude/mod.rs` (plus `--model haiku`, `--permission-mode acceptEdits`) in a scratch project, stamp each stdout line with its arrival ms, keep the `stream_event`, `assistant` and `user` lines, put `{cwd}` for the project path (inside streamed tool input too), mark one call `"ask": true`, and delete the run's transcript from `~/.claude/projects`.

### Verify and walkthroughs weren't scoped to the change
- Seen: 2026-10-15 and 2026-10-17. `just verify` checked every crate whatever changed; a one-step Esc fix ran the full `walk.sh checks` (310 checks, 3 min 43 s) to see one new check; the full suite took 6 min 13 s on the chat-polish batch.
- Fix: `scripts/affected.py` lists the crates changed since `origin/main`, committed or not, plus every crate that depends on them; anything build-wide (Cargo.toml, Cargo.lock, the justfile, `.cargo`) means all of `crates/*`, and docs-only changes check nothing. `just check` and `just verify` use it; `just verify-all` and `just verify-spikes` are there when wanted. Walk scenes join with `+` (`walk.sh find+keys`) to run only what a change touched, and an unknown name now fails with the list instead of silently running the whole suite. AGENTS.md: the full suite once per batch, in the background.
- Result: a desktop-only change checks 1 crate instead of 21 (10 + 11 spikes); a protocol change checks the 7 that use it; docs only, none. Warm `just verify` 11–12 s.

### `just verify` built every spike
- Seen: 2026-10-15. `verify` passed `--workspace`, so clippy and tests compiled all spike crates; 26 of 37 test binaries ran no tests.
- Fix: verify uses the default members (`crates/*`) through `affected.py`; spikes only when touched, or `just verify-spikes`.
- Result, cold (clean target dir, clippy then test build): 228 s and 953 CPU-s with spikes; 210 s and 804 CPU-s without. Warm verify was already short (17 s before, 11–12 s after).

### Promised build speed-ups weren't in place
- Seen: 2026-10-15. `docs/building-with-agents.md` promises a fast linker and a parallel test runner.
- Measured instead of adding: a warm `cargo test` is 8.4 s, 6.5 s of it inside the tests (one daemon binary is 6 s alone), so nextest could save 2 s at most, for one more tool to install on every machine. Linking `dent-desktop` takes 0.36 s (`-Z time-passes`), so a faster linker gains nothing. Neither is kept, so no decision record.
- Note for measuring: `RUSTC_BOOTSTRAP=1` changes cargo's fingerprint, so the next ordinary build rebuilds everything (about 140 s). Measure in a separate target dir.

### First build is long
- Seen: 2026-10-15: clean `just verify` 4 m 19 s on the new Mac.
- Measured: of 804 CPU-s in a clean default build, merman (core and render) is 118 s, alongside GPUI's ~150 s; both compile in parallel, so trimming merman saves little wall-clock. Trimming diagram types isn't wanted anyway: 0015 and spike 04 have dent render every Mermaid type an agent writes. Clean default verify is now 210 s. sccache isn't worth it until machine switches are common.

### `walk.rs` was one file of 6,897 lines
- Seen: 2026-10-15 at 3,939 lines; 6,897 by 2026-10-18.
- Fix: `crates/desktop/src/walk/`: the runner and a table of scenes in `mod.rs` (996 lines, replacing a 260-line `match`), one file per scene (largest `checks.rs`, 929), helpers shared by several scenes in `common.rs`.
- Result: a change to one scene reads one file of 30–550 lines.

### Checks that tested a stand-in, not the thing
- Seen: 2026-10-15. The attach scene's drag check passed while both hints showed at once.
- Fix: audited all 398 `Until` checks for a name promising something visible with a test that reads no drawn probe: 21 flagged. Six were stand-ins and now read a probe drawn by the feature: the plan above the composer (new `plan-strip`, also above `composer`), the working line (new `working-line`) for the working square and a subagent's step, the sidebar shown or hidden (new `sidebar`), and the page in the viewer (`preview`). The rest read the very state the view draws from (the mode chip's `permission_shown`, the queue's files, `mdstream::terminate`). AGENTS.md now says checks read a probe of what they name.

### A daemon test raced its own fake agent
- Seen: 2026-10-18. `a_stop_or_a_stuck_start_is_seen_through_across_a_daemon_restart` failed every run: the fake's long answer and the stop's grace after a restart were both 2.0 s.
- Fix: the test asks for the answer slowly (ten times longer than the grace).
- Result: `just verify` passed three runs in a row (175 s with a full rebuild, then 11 s and 12 s).

### Process rules that lived only in log entries
- A design brief read too literally (2026-10-15, ~15 min and a restarted research pass), claims made without checking (2026-10-15), scripted multi-file edits misfiring (2026-10-15, twice), two sessions on the same repo at once (2026-10-15).
- Fix: AGENTS.md "Working rules": write a "make it feel like X" brief back and get a yes first; say "unverified" when behaviour wasn't read or run; the editor tool for edits, scripted edits anchored on whole lines and asserted to match once; one writer per area (check `git worktree list` and open branches first).
- Seen again while fixing this list: a scripted edit anchored on `}),` landed in the wrong check. Caught in the diff, redone with the editor tool, which is the rule now.

### Late in the full walk, a daemon restart lost the agents
- Seen: 2026-10-16. "After a restart the approval still waits" failed in 8 of 8 full walks and passed in every `edges` run alone. The holder closed every agent's stdin 10 s after the kill, as if no daemon had come back.
- Cause: a real bug, not the walk. The holder lists its processes with every command line in the frame's header. An agent's command line runs to kilobytes (prompt and tool config), and the reader capped headers at 64 KB. With about 45 live agents, the new daemon's `list` failed, it took the connection as closed, and the holder let the agents go. Any daemon restart or upgrade with that many live sessions would have lost them all.
- Fix: the header limit is now 16 MiB. Writers were never bounded, so a holder already running needs no change. Test `a_long_process_list_fits_in_one_frame` (200 agents with 4 KB command lines) fails at the old limit.
- Result: the full `walk.sh checks` passes every restart check. Found by keeping the walk's daemon log and printing what `restore` saw: 45 agents, then a second connect.

### The idle walk check is flaky: carets keep blinking in fields that lost focus
- Seen: 2026-10-16. "idle, the window draws no more than a caret blink" read 2.3–6.0 frames a second across 15 runs, failing on `main` as well as the chat-fixes branch (main: 1 of 6, the branch: 5 of 9). Four fields blinked at once (composer, new-session field, side chat, rename) though none had focus. `TextareaState::focus` (gpui-base 0.7.1) starts the blink directly, but GPUI (gpui-pre 0.3.8) tells a field it lost focus only while the window is active, and walk windows never are; in real use, any field dent focused while in the background blinked for good.
- Cost: a false failure in about half the branch's runs; about 2 needless repaints a second per stray caret.
- Fix: without touching the crate. dent focuses its fields through `ui::FocusField::focus_field`: with the window in front it calls the field's `focus` as before; behind, it only moves the keyboard (`Window::focus`), and GPUI's focus-in on activation starts the blink. Deactivating already blurs the focused field.
- Result: the edges walk's rest after thirty sessions' fields had the keyboard drew 0.3 frames a second in each of its runs (720x600 light, 900x700 dark, 720x600 light again; limit 0.5). The original check in the full `checks` run read 0.3 frames a second in each of its 5 runs since (limit 2.5; it had read 2.3–6.0).

### walk.sh died before building on the Mac's bash
- Seen: 2026-10-16. Every `scripts/walk.sh` run without `DENT_WALK_RELEASE` stopped at once with "release[@]: unbound variable": macOS bash 3.2 treats an empty array as unset under `set -u`. The wrapper exited 0, so a run looked like it passed with no checks run.
- Cost: every walkthrough since the `DENT_WALK_RELEASE` switch landed; one wasted run found it.
- Fix: expand the array as `${release[@]+"${release[@]}"}`.
- Result: `walk.sh checks` builds and runs again.

### Ambient walk padded with settle time
- Seen: 2026-10-15. `just ambient-walk` took 35 s with a warm build (52 s cold): 1.6 s of waiting before each of 13 stills (~21 s) and a 3 s idle check. The owner asked why it took so long. The backgrounds prefill their history, and the longest fade-in is 0.5 s.
- Cost: ~13 s per run, on demand only.
- Fix: 0.6 s per still and a 2 s idle check. Re-measured at 22 s, all checks passing, stills fully settled.
- Check: `just ambient-walk` stays under 25 s with a warm build.


### A design review caught what the walkthroughs passed
- Seen: 2026-10-15, batch 2. Every new check passed, yet the reviewer found two blockers (showing the sidebar by hand squeezed the chat below its minimum; Send fell to its own row) and five should-fixes. And the full suite found a real bug no scene of its own showed: focus left on the terminal after the viewer switched away, so every shortcut went dead.
- Fix: review stills before landing, not after; run the full suite before calling a batch done, since scenes leave state for the next one.
- Result: all fixed in the same pass; full suite 308 checks, 0 failed, 3 m 28 s.

### Walkthroughs passed when stills failed, and stills could catch the wrong moment
- Seen: 2026-10-15, new Mac. Without Screen Recording permission every shot printed "could not create image from window" and the run still said "13 checks, 0 failed". Stills were also taken on a background thread while the scene moved on, so the narrow `drop-hint` still showed the state after the drop.
- Fix: a `Shot` step now waits until its picture is saved, and an unsaved still is a failed check ("the still was saved").
- Result: with no capture tool the attach scene now reports 3 failures instead of 0; the narrow `drop-hint` still shows the hint. Each scene run costs about 1 s more (attach: 14 s → 15 s).

### The plan check raced its own fake turn
- Seen: 2026-10-18, review fixes. "The plan moves along and the turn ends" failed twice in a row on the full walk (433 checks, 1 failed, about 9 min each). The fake's `make a plan slowly` holds each of two steps 12 s, so the turn runs about 24 s, but the check allowed 20 s from a point a second or two in. It passed earlier only when the steps before it happened to take longer.
- Fix: the check allows 30 s.
- Result: see the next full run in this batch's PR.
