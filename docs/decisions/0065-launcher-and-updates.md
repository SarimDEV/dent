# 0065. A small launcher owns which daemon version runs: trial, snapshot, commit or roll back

Status: proposed (2026-10-20) · confirmed by [R-E1–E7](../remote-tests.md#e-install-service-and-updates) · Supersedes the self-update half of [0022](0022-install-service-manager-cargo-dist.md) (cargo-dist still builds the installers) · Plan: [remote-plan.md](../remote-plan.md)

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
5. It stops the old daemon. The holder keeps every agent running.
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
  - Each agent runs in its own scope under the daemon's slice, so running out of memory kills that agent and the session says so.
  - A restart started from inside the service goes through the service manager without waiting (`--no-block`, or a transient unit).
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
- Stage 3 of [remote-plan.md](../remote-plan.md) builds this. Stage 0 proves it with a prototype in the VM (R-E3, R-E4).
