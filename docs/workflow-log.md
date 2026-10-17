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

### Late in the full walk, a daemon restart loses the agents
- Seen: 2026-10-16. The edges scene's "after a restart the approval still waits" fails in every full `checks`/`stills` run (8 of 8) and passes when the scene runs alone (7 of 7, including with 54 sessions, an earlier crash restart, or your terminal open first). Timed in one full run: the app reconnects 0.22 s after the kill, yet the holder logs "no daemon for a while; closing stdin" for every agent exactly 10 s later (the walk's orphan time) and exits, so the new daemon never held a connection to it; the session then ends "dent restarted while the agent was working". Earlier full runs used a 2 s orphan time and lost the agents sooner; walks now use 10 s, which fixed the take-back check but not this.
- Cost: 3 failing checks per full walk run; a real restart late in a long day may lose running agents the same way (unconfirmed outside walks).
- Fix: find which earlier scene leaves the restarted daemon without a holder connection (bisect the scenes before `edges` in `checks()`, with holder.log and daemon.log kept), then fix that path.
- Check: `scripts/walk.sh checks` passes the edges restart checks.

### Real walkthroughs used for every visual check
- Seen: 2026-10-15. Each UI tweak in the chat-polish round was checked with a full real-agent `walk.sh compare` run (Haiku, ~40 tool calls): six runs at ~3 min plus build each, and the owner waited on every one ("the walk is so annoying… it takes forever").
- Cost: ~25 min of waiting in one session, plus real usage limits.
- Fix: a fake-agent scene that replays one recorded real turn (narration, tool rows, plan, approval, end) and stills it in both themes in under 30 s; the real walk only for a final check, once.
- Check: a visual tweak goes from edit to still in under a minute.

### Real walkthroughs stall while the Mac is locked
- Seen: 2026-10-15. A `scripts/walk.sh compare` run sat at "Starting dent…" for 14 min: the owner was away, the screen had locked after ~48 min idle, and a window that isn't drawn never ticks the walk. It printed nothing, so it looked like a hang. Re-run under `caffeinate -dimsu`, it finished in ~3 min.
- Cost: 14 min plus a wasted real-agent run, each time a long walk starts while the owner is away.
- Fix: `walk.sh` wraps the desktop in `caffeinate -dimsu`, and a step stuck longer than its timeout without a frame says so on stderr.
- Check: an unattended real walk on a locked-screen schedule finishes, or fails within a minute, saying why.

### A design brief was read too literally
- Seen: 2026-10-15. Asked to make the chat feel like Claude desktop's, the work started on Markdown typography; the owner meant the whole feel of an agent at work (tool calls, the working line, rhythm). One research run (~10 min) was redirected and then restarted with a rewritten brief.
- Cost: ~15 min and one throwaway research pass.
- Fix: for a "make it feel like X" request, write the brief back in one paragraph (what experience, which moments) and check it before researching.
- Check: no restarted research passes on the next design request.

### `just verify` builds every spike
- Seen: 2026-10-15. Cargo.toml lists `spikes/*` as members and `verify` passes `--workspace`, so clippy and tests compile all 12 spike crates (gpui-chat, core-wasm, desktop-concepts…) every time. AGENTS.md says spikes are left out of the default build.
- Cost: clean `just verify` on the new Mac took 4 m 19 s; a warm run with nothing changed takes 8 s. 26 of the 35 test binaries it links run zero tests (mostly spikes). The real cost is on cold builds and after changes to shared crates, when spikes rebuild too: still to be measured.
- Fix: verify uses the default members (`crates/*`). A separate `just verify-spikes` covers spikes when one is touched.
- Check: time `just verify` warm, before and after.

### Verify and walkthroughs aren't scoped to the change
- Seen: 2026-10-15. `just verify` always runs every test in every crate, and the full walkthrough suite (~272 checks) runs once per batch, besides each feature's own scene. `docs/building-with-agents.md` promises "replays and UI checks for the areas the change touched".
- Cost: unmeasured. A small change still pays for the whole workspace. Measured once: a one-step Esc fix ran the full `walk.sh checks` (310 checks, 3 min 43 s) to see a single new check, with no way to run just the Stop scene.
- Fix: `just verify` picks crates from `git diff` against `origin/main` (full run only when shared crates change or before pushing to main). The full walkthrough suite runs in the background while the next item starts, not as a blocking step.
- Check: wall-clock time from "change done" to "landed" on a typical batch item.

### Promised build speed-ups aren't in place
- Seen: 2026-10-15. `docs/building-with-agents.md` promises a fast linker and a parallel test runner. There's no `.cargo/config.toml` and no nextest.
- Cost: unmeasured.
- Fix: try cargo-nextest for `just check`/`verify`; measure link time of `dent-desktop` before deciding on a linker. Write a decision record for whatever is kept.
- Check: warm `just check desktop` time and test-suite time.

### First build is long
- Seen: 2026-10-15, new Mac. GPUI builds at opt-level 3 even in dev, and merman-render compiles with every diagram feature (30+).
- Cost: first `just verify` on a clean machine: 4 m 19 s (clippy step alone 1 m 54 s), on 2026-10-15. This hits every new machine or `cargo clean`.
- Fix: enable only the merman diagram types dent renders; consider sccache shared across machines.
- Check: clean `just verify` time.

### Switching machines lost context and time
- Seen: 2026-10-15. Moving to this Mac took about an hour of manual setup: the Xcode tools dialog got stuck, then rustup, `just` compiled from source because Homebrew was missing, an SSH key, and the winid helper. The owner's working preferences lived only in the old Mac's agent memory. Batch 1 (9 files, ~430 lines) was left uncommitted on the old Mac.
- Cost: an hour per machine switch, plus work stranded until the owner can reach the old machine.
- Also: the owner had to step in five times (Xcode tools dialog, SSH key, pushing batch 1 twice, Screen Recording permission), and the permission only surfaced after the first walkthrough, not at setup. The owner said they don't want to babysit.
- Fix: (a) move the working agreements from the handover into AGENTS.md so the repo carries them; (b) a `scripts/bootstrap.sh` that installs and checks everything (`just doctor`), listing every one-time permission (Screen Recording for stills) up front in one go; (c) rule: unfinished work is pushed to a WIP branch at the end of every session.
- Check: time a clean-machine setup, and the count of handovers that find unpushed work.

### Claims made without checking
- Seen: 2026-10-15 handover. An earlier session said restarting the window wouldn't touch the daemon; it does, whenever the build beside the app is newer. The owner acted on it.
- Cost: a wrong answer the owner relied on, and two stopped turns still unexplained.
- Fix: rule: describe behaviour only after reading the code or running it, and say "unverified" when neither was done.
- Check: corrections from the owner per week.

### Checks that test a stand-in, not the thing
- Seen: 2026-10-15. The attach scene's "dragging files over the window says where they'd go" only checked that a drag was active and the reply box was drawn, never that a hint was. It passed while both hints showed at once, and while the narrow still showed none.
- Cost: bugs ship behind a green suite; the owner finds them in the stills, or in use.
- Fix: audit `Until` checks in `walk.rs` for ones that test a proxy; draw a probe on the thing itself (as `drop-message` / `drop-shell` now do) and check that.
- Check: count of checks that name a probe of the feature they describe.

### The idle frame-rate check is flaky
- Seen: 2026-10-15. "idle, the window draws no more than a caret blink (2.5 a second)" failed at 2.7 frames a second, then passed at 2.3 on an unchanged re-run. It sits right after the daemon-restart scene.
- Cost: a 3 m 15 s re-run of the full suite to tell a flake from a regression.
- Fix: let the window settle (wait for no reconnect or streaming activity) before measuring, and measure over a longer window so one stray frame can't tip it.
- Check: the check's spread over 5 runs stays under the limit.

### Scripted multi-file edits misfire
- Seen: 2026-10-15. Edits applied with Python string replacement in a script failed twice: once an anchor wasn't unique, and once a 4-space-indented anchor matched inside a 16-space-indented line in another scene, adding the new scene to the wrong place.
- Cost: about 5 minutes each to notice and repair; a silent wrong edit could ship.
- Fix: use the editor tool (it refuses non-unique anchors) for anything but mechanical renames; when scripting, anchor on whole lines and assert the line number.
- Check: misfired edits per batch.

### Walkthroughs need an unlocked screen, and no one is there to unlock it
- Seen: 2026-10-15, about 13:45. The screen locked while the owner was away. Locked, macOS draws no windows, so walkthroughs (mine and another session's in `dent-chat-polish`) stopped dead with no message. Two runs hung for 10+ minutes before I found why; a third went to a wrong theory (windows covering each other).
- Cost: about 30 minutes, and every UI check stops until the owner is back.
- Fixed in part: `walk.sh` now refuses to start on a locked screen and says so, keeps the display awake while it runs (`caffeinate -d`), and a still that doesn't come back in 15 s fails instead of waiting forever.
- Still open: the display sleeping and locking whenever the owner steps away. Options: keep-awake for the length of a work session, or a setting that doesn't lock on display sleep; both the owner's call.
- Check: walk runs lost to a locked screen per week.

### Two sessions on the same repo at once
- Seen: 2026-10-15. Another session works in `~/Documents/projects/active/dent-chat-polish` on branch `chat-polish`, cut from `316f679`, while this one lands batch 2 on main; both edit `crates/desktop/src/chat.rs`.
- Cost: merge conflicts later, and walkthroughs from both competing for the screen.
- Fix: one writer per area at a time; the second session works from the first's latest main, or waits.
- Check: conflicts per merge.

### `walk.rs` is one 3,939-line file
- Seen: 2026-10-15. Every walkthrough scene lives in `crates/desktop/src/walk.rs`.
- Cost: each edit means reading or searching a large file, which costs agent time and tokens and makes conflicts likelier.
- Fix: one module per scene under `walk/`, with shared steps in one place.
- Check: lines read per walkthrough change.

## Done

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
