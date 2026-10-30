# 0077. Every merge to main is a build, and every dent updates from it with one button

Status: accepted (2026-10-30, the owner's call) · Builds on [0065](0065-launcher-and-updates.md) (the launcher, trials and rollback) · Unblocks the private half of [#16](https://github.com/SarimDEV/dent/issues/16)

## Decision

**Merging is the release.** CI (`.github/workflows/builds.yml`) builds every merge to main and publishes it to one rolling GitHub release, `edge`:
- the parts, named by build: `app-macos-aarch64-<n>.tar.gz` (dent.app, Apple silicon), and `daemon-<os>-<arch>-<n>.tar.gz` (daemon, holder, launcher, CLI) for macOS on Apple silicon and Linux on x86_64 and aarch64. Linux builds on Ubuntu 22.04, so they need glibc 2.35 or newer;
- `latest.json`, uploaded last, which names the newest build, its commit, and each part's URL, size and sha256 (`scripts/feed.py`).

The last few builds' parts stay, for anyone mid-download; older ones are deleted. There are no tags, version bumps or uploads by hand.

**A build knows its number.** CI's run number and commit are kept in every binary (`DENT_MAIN_BUILD`, `DENT_COMMIT`, read by `dent-update`). A dent built anywhere else is number 0 and never updates itself: it's updated by building it again. Rows say `build 412`. The launcher names a version of main `0.1.0-b412-3f2a9c1e`.

**Each machine makes its own update ready.** No machine serves another ([0058](0058-independent-machines.md)).
- **A daemon under the launcher** reads the feed every 10 minutes. It downloads a newer daemon part, checks it, and stages it beside the running version. It then says so in its welcome and to every device (`Available`).
- **The app** does the same for itself, from a dent.app.
- **A build that went back** here isn't offered again. The next build is.

**One button runs it all.** Update all, in Settings › Machines, or Update in the status bar:
1. Every machine with a build ready is asked to run it (`Request::Update`). The launcher trials it and rolls it back if it fails, and the holder keeps agents running ([0065](0065-launcher-and-updates.md)).
2. Once they're back, or after 150 s, the app writes where it was (its session, the chat's place, the window) and quits. A small script swaps the new dent.app in and opens it, and the new app puts you back where you were.
3. The script waits for the new app to say it started with its own build number, which no other dent can write. If that hasn't happened within 60 s, the script puts the old one back and opens it. It notes the build, so that build is never offered again and the app says it went back. Any step of the swap that fails opens the old app again. An app run from a disk image, from where macOS moved a download aside, or from a folder it can't write to doesn't offer to update.

A row's Update does the same for one machine. Quitting and reopening dent runs nothing new. Each press carries an id, so a request sent again after a reconnect (its answer lost as the daemon stopped) changes nothing. A daemon keeps one download at a time and removes staged builds a newer one replaced; a build it can't stage isn't tried again.

**Old and new keep working together.** With a build on every merge, mixed versions are normal. `scripts/compat.sh` drives a daemon from one build with the CLI from another, over the remote link: it pairs, starts a session with the stand-in agent, and waits for its answer. CI runs it both ways round:
- on every pull request, against main's newest build;
- before publishing, against the previous build.

**Only main publishes, newest first.** Publishing runs one at a time, only for main, and never replaces a newer build: a re-run of an old workflow stays unpublished.

**One signing identity.** macOS ties what it allowed (folder access) to an app's signature. CI signs every build with one self-signed certificate (secrets `DENT_SIGN_P12`, `DENT_SIGN_PASS`), so updates don't ask again. Without the secrets, builds are signed ad hoc: they still run, and since a service's permissions belong to the launcher, which an update doesn't replace, the daemon keeps its access anyway. Notarising for strangers' downloads is #16's public half.

**Installing the first time:** `scripts/install.sh`, piped from curl. It installs dent.app, and with `--service` (always on Linux) the daemon as a service. Downloads made by curl and by dent itself aren't quarantined, so Gatekeeper doesn't stop them.

## Why

The owner builds dent in dent, from one Mac connected to another that does the work. Every fix used to mean pulling, building and restarting by hand on each machine. The design keeps that loop short and quiet:
- **The update itself:** one press, sessions running through it, and the window back where it was.
- **Upkeep:** one workflow file, no server, and no build machine to keep.

0065 already made a daemon's update safe. What was missing was builds to update to, and a way for the app to update itself.

## Consequences

- **Public releases** become a tag on a build of main that has been used for a while, read from another feed by the same updater. Nothing is rebuilt.
- **A bad merge reaches the owner first.** It's caught by `just verify` before merging, the compatibility check before publishing, and automatic rollback after.
- **The launcher isn't updated by Update.** It changes rarely by design ([0065](0065-launcher-and-updates.md)). A new one comes with `install.sh --service`.
- **Trials, end to end with real builds:**
  - `scripts/update-trial.sh`, on Linux: build 1 updates to build 2 under the launcher, and a session made before is there after.
  - `scripts/app-update-trial.sh`, on a Mac: build 1 updates to build 2, and a build 3 that can't start goes back to build 2 and stays there.
- **The walk scene `updates`** shows the rows, Update all, the status bar, a press under way, and the session opening again.
