# 0065. A small launcher owns which daemon version runs: trial, snapshot, commit or roll back

Status: accepted (2026-10-20, owner's call after stage 0) · Evidence: [spike 13](../spikes/13-service-and-updates.md) (R-E1–E6 on Linux) · Before release: [R-E7](../remote-tests.md#e-install-service-and-updates) on macOS · Supersedes the self-update half of [0022](0022-install-service-manager-cargo-dist.md) (cargo-dist still builds the installers) · Plan: [remote-plan.md](../remote-plan.md)

## Decision

### The launcher

**`dent-launcher` is what the service manager runs.** It is a small binary using only the standard library, like the holder, changed rarely, with its own protocol version. It owns three versions: active, pending and rollback. They live in `versions/<v>/` under the state folder, and their record is written by temp file, fsync, rename and directory fsync. A record it can't read stops startup rather than guessing.

### An update

1. The daemon asks for the update. Only the launcher chooses what runs.
2. The launcher stages the release: downloaded with a stall timeout on each chunk and no total cap, checked against its checksum.
3. It preflights the release:
   - the release's launcher protocol must be supported;
   - there must be free disk space for the database snapshot plus headroom.

   A failed check stops here, and nothing is stopped.
4. It records `pending`, then acknowledges.
5. It tells the old daemon why it's being stopped (update, with the expected downtime), so clients show "restarting" rather than "quit" (a bare SIGTERM can't say which; [spike 15](../spikes/15-sleep-restart-and-serve.md)). Then it stops the old daemon. The holder keeps every agent running.
6. It snapshots the database (main file, `-wal`, `-shm`) once per update. It clones on APFS and reflink filesystems and copies elsewhere, and never overwrites the snapshot on a retry.
7. It starts the new daemon as a **trial**. The trial migrates, binds its socket and listeners, then **parks**: it doesn't attach to the holder's agents or accept commands. Then it reports `prepared`.
8. The launcher writes `committed` durably and tells the daemon, which unparks.
9. A trial that crashes, or isn't prepared within 30 minutes (migrations on large databases are slow), is stopped. The snapshot is restored behind a durable restore marker, so an interrupted restore finishes first. The old version runs again.
10. The welcome reports `{ id, from, to, outcome }` ([0060](0060-remote-wire.md)), so clients show "updated" or "rolled back" by id, not by noticing a version change.

### Policy

- A version that rolled back is never retried automatically.
- A release that changes the protocol range is never installed automatically.
- Automatic updates install whenever the machine is idle enough. The holder keeps replies running, so the window can be wide. A holder upgrade still waits for idle (spike 06).
- One button updates every machine. Each machine shows which sessions keep running and which will restart.
- The current and previous versions are kept. Others are pruned after a commit, never while a process runs them.

### Service

- **macOS: a per-user LaunchAgent.** Claude Code's login lives in the user's keychain, and the agents need the user's file permissions. It runs while the user is logged in. `dent doctor` and onboarding say so, and point a Mac server at automatic login.
- **Linux: a systemd user unit.** Install enables linger, or prints the exact `sudo loginctl enable-linger` line first.
  - Each agent runs in its own scope under a dedicated agents slice, so running out of memory kills only that agent. The slice's `memory.events` `oom_kill` counter, read before and after an agent dies, tells "out of memory" from a plain kill. A scope's own `Result` can't ([spike 13](../spikes/13-service-and-updates.md)).
  - A restart started from inside the service is handed to a transient unit (`systemd-run --user --collect`), which runs the restart and any follow-up step. Anything after a restart call made inside the service never runs, `--no-block` or not.
- **Environment:** `service.env` (0600, literal `KEY=VALUE`) holds the person's environment, outside the generated unit or plist, and survives reinstalls. PATH is captured at install and re-read from the login shell with markers and a 5 s timeout. `dent doctor` shows where it came from.
- **Install safety:**
  - Install writes an intent marker before swapping and clears it only once the service runs.
  - An older CLI can't downgrade the service without `--allow-downgrade`. The version is checked again after stopping.
  - A daemon that finds its state folder locked exits with a code the launcher treats as "stay idle", not a restart loop.

## Why

cargo-dist's updater replaces the binary and restarts. That can't trial a release, undo a database migration, or survive a crash halfway through. Tools that update servers people rely on found each step above by breaking without it:
- a server pinned to a broken build it chose itself;
- a migration that the old version can't read;
- a full disk turning every update into an outage;
- the updater killed by its own service restart;
- a client unable to tell a successful update from a rollback;
- old versions filling disks with gigabytes.

The holder already spares dent the worst part, agents dying on update. The launcher makes the rest safe.

## Consequences

- Three processes per machine: launcher, daemon and holder. The launcher and the holder are both small and stable. They aren't merged: the holder must never restart, and the launcher must be replaceable on its own.
- Stage 3 of [remote-plan.md](../remote-plan.md) builds this. Stage 0 proved it with a prototype in the VM (R-E3, R-E4, [spike 13](../spikes/13-service-and-updates.md)).

## As built (2026-10-22)

The launcher is `crates/launcher`; the daemon's side is `crates/daemon/src/launcher.rs`, install and the service in `service.rs`. What building it settled:

- **Two pipes, lines both ways.** The daemon finds them as descriptors 3 (to the launcher: `prepared`, `update <v>`, `auto-update <v>`) and 4 (from it: `committed`, `accepted`, `refused <why>`). No request files, nothing polled.
- **The trial binds before it parks:** the local socket and the remote port. A version that can't take them fails its trial. Outside a trial, a remote port taken by another program only leaves remote access off.
- **A rolled-back version is retried when someone asks for it** (`update`). It may have failed for something since put right, like a port another program held. Only an automatic update (`auto-update`) is refused; passing on a retry clears it.
- **Versions are named by their version and a hash of the daemon and holder** (`0.1.0-3f2a9c1e`), so two builds of one version never share a folder. A version describes itself (`dent-daemon manifest`); the launcher and its record use `key=value` lines it reads with the standard library alone.
- **The holder runs outside the service.** Under systemd it starts in a scope of its own (`systemd-run --user --scope`): stopping or restarting the service kills its whole cgroup, so restarting it mid-command no longer reaches the agents. On Linux its folder is in `XDG_RUNTIME_DIR`, which isn't swept the way `/tmp` is after ten days.
- **`service.env` is read by the launcher at start** on both platforms (a LaunchAgent can't read an environment file itself).
- **One daemon per machine, one way of running it.** The app never starts a daemon for a state folder with a launcher, nor asks one the launcher runs to restart beside a newer build (it would come back as the same version, again and again); `Welcome.managed` says which. The LaunchAgent (`sh.dent.launcher`) and the app's start-at-login item are refused together.
- **A daemon finding its folder locked exits 75**, and the launcher waits 30 s rather than looping. A trial that finds it locked never opened the database, which another daemon now has open: nothing is restored over it, and the version isn't held back (`not_tried`).
- **What the launcher gives a daemon is taken at startup and taken away:** its pipes are made close-on-exec and checked to be pipes, and `DENT_LAUNCHER*`, `DENT_TRIAL`, `DENT_VERSION` and `INVOCATION_ID` are removed, so nothing the daemon starts (the holder, agents, a `dent-daemon` in an agent's shell) takes them for its own. The committed daemon hears how the update went with `committed`.
- **The holder's folder on Linux is `/run/user/<uid>`,** found by uid so the service, an SSH login and cron agree; a holder an older build started in `/tmp` is used while it runs.
- **Installing never leaves a half-made service:** the downgrade check comes before anything is written, a version the record names that's gone is replaced, and a first install that never ran is taken out again. `uninstall` removes the launcher, so the app starts daemons for the folder again.
- **Install over an older install is an update**: staged, then trialled through the running launcher. A changed launcher or service definition restarts the service first, saying "restart" to devices.
