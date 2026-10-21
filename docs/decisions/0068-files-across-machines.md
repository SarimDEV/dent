# 0068. Files across machines: one resolver on the daemon, ranged reads inside the link, one cache on the client

Status: accepted (2026-10-20, owner took the recommendations: Full devices read any file the daemon's user can, its own secrets refused; a rewritten file shows its current version, marked; older daemons say "update dent") · Research: T3 Code's asset system and its bug history (40 failure modes), in the scratch notes `files-across-machines.md` · Plan: [remote-plan.md](../remote-plan.md), stage 2, "Files across machines"

## Decision

Any file a session names can be viewed, played, opened and saved from any device, over every way a device reaches a machine. This covers pictures, files linked in replies, media, agent-made pages and what they load, posters, and the latest version of an edited file. The ways in are LAN with a pairing code, Tailscale, the hub relay later, and the phone later. It works the same on each, and it never shows another machine's file.

**The daemon resolves every path.** The client sends a path as the session wrote it: absolute, `~/…`, `file://…` or relative. It also sends the folder that path was written against, when that isn't the session's folder (a page's own folder, say). The daemon expands `~` with its own home and resolves against the session's folder or worktree. Then it either answers with the absolute path it checked, or says why it can't.

A session's file is never looked for on another machine. There is no local fallback.

**The bytes travel inside dent's own link.** New requests:
- `Stat`: batched, giving kind, size, dimensions and a stamp (device, inode, size and modification time).
- `Read`: offset and length (at most 256 KiB per reply), pinned to an expected stamp, so a file rewritten mid-read restarts instead of mixing two versions.
- `Still`: the daemon makes a poster for a video or the first page of a PDF.

Errors are typed: missing, refused, unreadable, too big. Each carries the path checked.

No URL points at a daemon. Signed URLs would need TLS on a LAN, break through the hub, and keep working after a device is removed. Web views and the video player use the client's own `dent://` scheme, answered asynchronously from the cache and the link.

**Reads never hold up a session.** File requests run beside the request loop, not in it, with a few at a time per connection and per daemon. Their replies go out on a bulk lane behind control frames and live events. Because replies are small, the longest a live event can wait is one reply.

The client pulls with a small window that adapts to the link's speed, ordered by what's on screen, then prefetch, then background. A dropped link resumes from the blocks already cached. A seek is just new ranges.

**Who may read what.** A device with Full access (every paired device today) may read any file the daemon's user can read. A Full device can already have the agent read any file, so a narrower allowlist adds no safety; it only causes the "outside the session's folder" breakage. The daemon's own key, trust list and secrets are always refused. Writes stay inside the session's folder. A narrower access level later (a read-only phone) gets the session's folder, daemon-written files and paths the session's events name.

**One `Files` layer on the client** replaces every direct disk read of a session's file:
- Local sessions use the same resolve and open code as the daemon, from a shared crate, so the two can't drift.
- The cache is keyed by (machine, checked path, stamp), and capped with least-recently-used eviction.
- It re-checks when a session opens, when an event names a path, every 3 s while a file is open in the viewer, and on reconnect. A rewritten file is never shown with its old bytes, and a "not there" never outlives the file appearing.
- Every state is shown: loading (in a box sized from the last `Stat`), here, here but from a while ago because the machine is offline, not on the machine, refused, too big, and "update dent on the machine".
- Every error names the machine and the path checked.

A lint stops direct disk reads of session files in the desktop crate.

**Open in another app, Save, and Copy path.** For a remote session, the file is first copied into the cache, then opened, and the toast says "Copied from <machine>". Copy path copies the far machine's path, never the cache's.

**A rewritten file shows its current version everywhere.** A message whose file has changed since says so. Files the daemon wrote itself, such as attachments and tool pictures, never change.

**Fast as a rule:** draw what's already here in the first frame, fetch only the gap, and never let background bytes get in front of what's on screen. Budgets, each a test:
- a cached session drawn fully in the first frame;
- from launch, every session readable within 300 ms with no network;
- an uncached picture's thumbnail within 150 ms on a LAN, 350 ms over Tailscale and 800 ms through a relay;
- a video's first frame within 500 ms on a LAN, and a seek within what's already read in 50 ms;
- live events within 500 ms while a 64 MiB file transfers over a 1 MiB/s link.

**A client event log on disk comes first.** Today each launch catches up every machine's log from the start, and an offline machine's sessions are gone after a relaunch. The client keeps the log on disk, as [hub.md](../hub.md) already plans, and resumes from it.

**Caching rules.** Every cache here, the history and the files alike, can only make things faster, never wrong:
- **The daemon stays the source of truth.** A cached copy is used only after it has been checked against the daemon, by log epoch and sequence for history and by stamp for files. What's shown from it while a machine is offline is marked as such, never drawn as live.
- **Cached history belongs to one log.** Each machine's copy is tagged with the epoch of the log it came from. A different epoch (the machine's database was reset, or it's a reinstall) throws the copy away, and history comes again from the start.
- **A cached file belongs to one version:** (machine, checked path, stamp). A stamp that doesn't match is a different file.
- **Writes are atomic.** History lives in one SQLite file in the client's state folder, written in transactions, with the format's version in it. File copies are written to a temporary name and renamed into place. A damaged cache, or one in a format this build doesn't know, is deleted and rebuilt, never read in part.
- **Only what's been applied is saved,** in sequence order, so a resume never skips an event.
- **Private:** the cache folders are readable by this user only (0700), since they can hold a project's files, `.env` included. Forgetting a machine deletes its history and files.
- **Bounded:** file copies are capped, with least-recently-used eviction that never evicts what's open. History is kept per machine, and a forgotten machine's goes with it.
- **Each rule has a test:** a changed epoch, a truncated or corrupt cache file, a file rewritten with the same size and time to the second, a crash between the write and the apply, offline display, and a removed machine.

Old daemons answer the new requests with an error. `PROTOCOL` goes to 4, so the client can say "update dent on <machine>" instead of failing silently.

## Why

T3 Code built this with signed URLs and a client that resolves paths, and spent months on what followed:
- the wrong machine's file;
- `~` expanded with the wrong home;
- rewritten images that kept their old bytes;
- caches keyed by URL that re-downloaded everything on each reconnect;
- errors that named neither the machine nor the path;
- endless spinners while offline.

Each rule above answers one of those. The owner needs this to work over Tailscale, a plain LAN and the hub alike, to feel instant, and never to need chasing.

## Consequences

- The desktop gets a persisted event log, a `Files` layer, the `dent://` scheme answered asynchronously, and a scheduler per machine. More than twenty call sites move to it. `mirror.rs` goes.
- The daemon gets `Stat`, `Read` and `Still`, file requests off the request loop, a bulk lane in the link writer, and safe opening: no following links, regular files only, descriptors kept open for range reads.
- The test plan covers each of T3's failure modes, each carrier and each budget. That means unit tests in the shared crate, daemon tests over a framed link with a throttled one for starvation, and walk scenes against the VM over LAN and Tailscale.
