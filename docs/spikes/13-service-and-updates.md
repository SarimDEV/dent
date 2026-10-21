# Spike 13: running as a service, and updating without breaking

Date: 2026-10-20 · dent-linux (Ubuntu 24.04, systemd 255, cgroup v2) · Tests [R-E1–R-E6](../remote-tests.md#e-install-service-and-updates) for [0065](../decisions/0065-launcher-and-updates.md) · Code: [spikes/launcher-proto](../../spikes/launcher-proto), plus the spike-link daemon as a systemd user service

Questions: under systemd, does the daemon:
- keep running after logout;
- find the agent CLIs;
- survive an agent running out of memory;
- restart itself safely?

And can a small launcher trial an update with a database migration, and roll back every way it can fail, without touching agents?

**Answer: yes to all of it, with three findings that change the plan's details.**

## The service

| Test | Result |
| --- | --- |
| R-E1: log out of every session (`loginctl terminate-user`), with linger on | The daemon answered 29 of the next 30 seconds |
| … with linger off | It answered only while something happened to be logged in (11 of 30). Without linger, a headless box loses dent at logout. Lima turns linger on by default; a real VPS usually doesn't |
| R-E2: PATH under systemd | `/usr/local/sbin:…:/snap/bin`, and **`claude` not found** (it lives in `~/.local/bin`). Reading PATH from `bash -ilc` between markers, with a 5 s limit, took 20 ms and found it |
| R-E5: an agent allocates until the kernel kills it, in its own scope (`systemd-run --user --scope -p MemoryMax=…`) | Only the agent died (exit 137). The daemon kept the same pid |
| R-E6: the service restarts itself (`systemctl --user restart`, with and without `--no-block`) | The restart happened, but **nothing after the restart call ever ran**, because the caller dies with its own service. Handing the restart to a transient unit (`systemd-run --user --collect sh -c "systemctl --user restart …; <follow-up>"`) restarted it *and* ran the follow-up |

**Telling "out of memory" from "killed".** The scope's own `Result` said `success` even with `OOMPolicy=stop`, and the scope's cgroup disappears too fast to read. The `memory.events` counter of a dedicated parent slice (`dentagents.slice`) works: `oom_kill` went 0 → 1 when the hog was killed, and stayed at 1 for a plain `kill -9` with the same exit code. Reading it before and after an agent's SIGKILL exit tells the two apart, so the session can say "killed: out of memory".

## The launcher prototype

`launcher.py` follows [0065](../decisions/0065-launcher-and-updates.md) step for step:
1. preflight (launcher protocol, disk space);
2. record `pending`;
3. stop the old daemon;
4. snapshot SQLite (main, `-wal`, `-shm`, with `cp --reflink=auto`), once per update;
5. start the new version as a trial. It migrates, then reports `prepared` on a pipe, and stays parked until told `committed`;
6. commit, or restore behind a `restore-pending` marker.

The database held 20,000 events. A stand-in agent, started outside the launcher as the holder starts agents, counted throughout.

| Scenario | Result |
| --- | --- |
| A migration that adds a column, creates a table, then fails | Rolled back: the column and the half-made table are gone, v1 serves all 20,000 events, the state records `rolled_back` |
| Asking for that version again | Refused: "it rolled back before" |
| A release needing a newer launcher protocol | Refused before anything stopped; the daemon kept its pid |
| A good migration | Committed: column added, all events kept, the state records `committed` with from and to |
| A trial that never reports prepared (3 s deadline in the test) | Stopped at the deadline, rolled back, v1 serving |
| The launcher crashes halfway through a restore | The marker is left and the database is the broken one. On restart it finishes the restore before booting anything, and v1 serves |
| A filesystem with room for the database but not its snapshot | Refused with the sizes (16.6 MB free, 21.4 MB needed for a 4.4 MB database); nothing was stopped |
| The agent | Counting throughout all of the above |

19 of 19 checks pass (`run-tests.sh`).

## What changes in the plan

- **Restarting from inside the service** must hand off to a transient unit, not just use `--no-block`. The follow-up step (recording an outcome, starting a rollback) runs in that unit. [0065](../decisions/0065-launcher-and-updates.md) is updated.
- **Out-of-memory detection** reads the agents slice's `memory.events`. A scope's `Result` can't be trusted for this.
- **`dent daemon install` must check linger and enable it,** or print the one `sudo` line. A box where it's off loses dent at logout.
- **The launcher's shape held up as designed.** The real one is a small Rust binary with the same steps; the prototype's 220 lines suggest it stays small.

## Not covered yet

- macOS: the LaunchAgent at logout (R-E7, owner checklist), and APFS clone snapshots.
- A real daemon binary under the launcher: the spike daemon still restarts itself in place. The holder's agents carried on here only as a stand-in.
- Update downloads (stall timeouts, checksums).
