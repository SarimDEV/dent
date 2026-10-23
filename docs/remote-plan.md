# Remote plan: many machines

How dent goes from one machine to many: the decisions that are hard to undo, the stages, what each stage must handle, and the real tests that settle each decision before it is built on.

Status: stage 0 done (2026-10-20). Doors 1–6 and 8 are accepted; door 7 (moving sessions) stays proposed until its interrupted-move and real-move tests pass (R-D3, R-D5). Each record lists the tests still to run against the real code before release; [remote-tests.md](remote-tests.md#stage-0-results) says which ran in stage 0 and how far.

## Why this comes next

The owner develops remotely most days, so remote has to be the best part of dent, ahead of a second agent. A lot of the design is already in place:
- the direct links of [0033](decisions/0033-direct-websocket-hub-on-hold.md) and [spike 05](spikes/05-direct-links.md): Noise over a WebSocket, pairing with a 6-digit code, Tailscale trust, honest liveness;
- the session holder ([0004](decisions/0004-session-holder.md), [spike 06](spikes/06-session-holder.md)), which already runs in the daemon;
- the I/O-free client core ([spike 07](spikes/07-client-core.md)).

Before building on them, we studied about 400 failure modes from mature multi-machine agent tools: their fix history, issue trackers and unfinished work. This plan folds every lesson that applies into the stages below. Several earlier choices change as a result:
- the 3 s silence rule;
- the 2 s backoff cap;
- a `tailscale whois` per connection;
- prompts refused while a machine is away;
- moves sent as a patch.

## What dent already has

Keep these. Each one answers failures from that study.

- **Agents survive daemon restarts and updates** (the session holder). Without it, a server update kills every running turn, and the turns have to be restarted from a "continue" prompt.
- **Machines say when they go away.** Asleep, quitting and restarting are announced, so a sleeping laptop shows "asleep", not "reconnecting" for hours.
- **Identity is a key, not an address.** A different machine at a known address is caught in the handshake.
- **Everything is encrypted by dent,** so there are no rules about which token may travel over which route.
- **Sequence numbers make resume exact.**
- **A full client cache** opens any session instantly, works offline and keeps a torn-down machine readable.
- **One client core** shared by desktop, phone and web, so the rules can't drift between apps.

## The one-way doors

Each has its own decision record. Everything not listed here is a two-way door, tuned by feel: timings, wording, layout, load balancing.

| # | Door | Decision | Record | Settled by |
| --- | --- | --- | --- | --- |
| 1 | Shared state | None. Every machine is independent; the client merges the view | [0058](decisions/0058-independent-machines.md) | Owner call |
| 2 | Machine identity | A key bound to the install, a locked data folder, everything keyed by (machine, id) | [0059](decisions/0059-machine-identity.md) | R-B6, R-B7 |
| 3 | The wire | Noise over a WebSocket; framed and chunked with a control lane; log epoch; frozen maintenance subset; refusal codes; idempotency keys | [0060](decisions/0060-remote-wire.md) | R-A1–A12 |
| 4 | Trust | A pinned key from pairing, or a live Tailscale check with strict rules; removal only when authenticated | [0061](decisions/0061-trust.md) | R-B1–B9 |
| 5 | Liveness and retry | Any frame counts as life; teardown only after long silence; probe before replacing; capped jittered backoff from v1 | [0062](decisions/0062-liveness-and-retry.md) | R-A1–A8 |
| 6 | Away machines | Prompts queue on the device and send once on reconnect, shown as waiting | [0063](decisions/0063-queued-prompts.md) | Owner call, R-C2 |
| 7 | Moving a session | A new id on the target with a tombstone; a guarded state machine; the real transcript with a text fallback; git as a snapshot commit in a bundle | [0064](decisions/0064-moving-sessions.md) | R-D1–D5 |
| 8 | Install and update | A small launcher owns which version runs: trial, database snapshot, commit or roll back | [0065](decisions/0065-launcher-and-updates.md) | R-E1–E7 |

## Stages

Each stage ends with something used for real, as in [build-order.md](build-order.md). Stage 0 runs before any of the records above is accepted.

### Stage 0: prove the doors (spikes, on the Mac and the Linux VM)

The test rig is the owner's Mac plus `dent-linux`, a Lima VM on it. The VM is on the LAN at its own address and on the tailnet under the owner's account; [remote-tests.md](remote-tests.md#the-rig) has the details. The iPhone comes later, but every rule below is written so the phone can follow it.

1. **Can a Claude session move between machines and resume?** Copy a real transcript from the Mac (Claude Code 2.1.301) to the VM (2.1.305) under a different path, then `--resume`. Then the reverse, newer to older. This decides door 7, so it runs first. (R-D1: **done**, [spike 09](spikes/09-moving-sessions.md))
2. **Tailscale trust for real.**
   - Read `whois` through the LocalAPI on the Mac's standalone Tailscale and on Linux.
   - Check a machine shared in from another account.
   - Check a tagged machine.
   - Check a connection arriving through `tailscale serve` from loopback.

   This decides door 4. (R-B1–B4: B1 and B2 **done**, [spike 10](spikes/10-tailscale-trust.md); B4 **done**, [spike 15](spikes/15-sleep-restart-and-serve.md); B3 optional; [results](remote-tests.md#stage-0-results))
3. **The link under a bad network.** Extend `spikes/link` with the new liveness, backoff and route rules, then run it between the Mac and the VM:
   - packet loss and reordering;
   - added latency;
   - a frozen daemon;
   - a frozen VM, standing in for a sleeping machine;
   - the LAN going away while Tailscale stays, and coming back;
   - address changes;
   - hours unreachable;
   - an hour-long soak.

   This decides doors 3 and 5. (R-A1–A10: A1 and A3 **run**, [spike 12](spikes/12-link-under-real-networks.md); A6 **run**, [spike 14](spikes/14-routes-identity-and-boot.md); A4 and A5 **run**, [spike 15](spikes/15-sleep-restart-and-serve.md); some only in part, see the [results](remote-tests.md#stage-0-results))
4. **Git across machines.** A script that packs a worktree as a snapshot commit in a bundle and applies it on the VM. It must reproduce `git status` exactly:
   - binaries, deletions, renames and file modes;
   - staged, untracked and force-added ignored files.

   It must also refuse diverged branches, nested repositories and dirty submodules. (R-D2 **done**; R-D4 **done** except dirty submodules and the size cap; [spike 11](spikes/11-moving-git.md))
5. **A daemon under a service manager.**
   - A systemd user service in the VM: linger, surviving SSH logout, the agent PATH, one agent running out of memory.
   - A launcher prototype that trials an update with a failing migration and rolls back. (R-E1–E6) (**done** on Linux, [spike 13](spikes/13-service-and-updates.md))

Output: a findings note per spike in `docs/spikes/`, and each record moved from proposed to accepted or rewritten.

### Stage 1: machines (Phase 4's core)

Exit: from a café on Tailscale, the laptop drives agents on another machine, closes the lid, reopens it and loses nothing; a machine asleep for hours reads as asleep.

- **Production links:**
  - The Noise channel, pairing and the per-machine connection actor move from the spikes into `client-core` and the daemon.
  - The daemon listens on the wildcard address and filters each peer by route and trust ([0061](decisions/0061-trust.md)).
- **Machines in Settings:** found nearby, paired, trusted by Tailscale, pause/off, remove, details, and diagnostics with a hop-by-hop failure line.
- **The inbox across machines:**
  - The cache is keyed by (machine, session).
  - A machine label appears only when more than one machine is connected.
  - An away machine's sessions read as their last state plus the machine's state, never as working.
  - Hide, pin and order never need the owning daemon.
- **Queued prompts** ([0063](decisions/0063-queued-prompts.md)).
- **Honest liveness end to end:**
  - announced sleep, quit and restart;
  - silence timeouts;
  - "froze for 12 s" reports;
  - the expected-restart reconnect cadence.

### Stage 2: projects, files and work across machines

Exit: one project lives on two machines; a new session goes to the right one; files an agent makes open on any device.

- **One repository resolver** on the daemon. It works out the group key (origin), the PR base (upstream) and the display name. It covers SSH aliases, local-path remotes, bare repositories and partial clones.
- **The machine picker** in the new-session box. It lists every copy: connected first, away ones visible but disabled. Draft rules:
  - Picking a machine, branch, worktree or attachment pins the draft.
  - A machine change keeps the text and model and drops branch, worktree path and uploads.
  - A per-project default machine is offered.
- **"Add to another machine":** a durable daemon job that clones the group key's remote, recreates every remote, and copies chosen ignored files as bytes (`.env*` ticked by default). The new session waits until setup finishes.
- **Settings across machines:** each setting is marked as a client preference or a machine setting. Machine settings are edited per machine, or on all connected machines at once, with Mixed, Not saved and Retry ([0058](decisions/0058-independent-machines.md)).
- **Files across machines:**
  - One "fetch file from machine" primitive, streamed with ranges so video seeks.
  - Cached by (machine, resource, content hash), never by a temporary URL.
  - Save and Share on every device.
- **Remote niceties:**
  - `localhost` links in a remote session are rewritten to the machine's reachable name.
  - The agent is told where the person is ("on the laptop, over Tailscale").
  - Open in editor builds SSH links for VS Code, Cursor and Zed.

### Stage 3: install, service and updates

Exit: `dent daemon install` on a fresh Mac and a fresh Linux box; an update rolls out across both while replies keep streaming; a broken release rolls back by itself.

Status (2026-10-22): the launcher, `dent-daemon install`/`update`/`status`/`uninstall`/`doctor`, `service.env`, the holder outside the service, and each agent in a scope of its own under `dentagents.slice` (so running out of memory is said as such) are built, and run for real on the Linux VM and a Mac ([remote-tests.md](remote-tests.md#stage-3-results)). Turns cut off by a reboot or a dead holder now carry on by themselves, with the guards below. Still to come: release downloads and the one button in the app.

- **The launcher** ([0065](decisions/0065-launcher-and-updates.md)), `service.env`, a two-phase install marker, and keeping the current and previous versions.
- **`dent doctor`:** service state, linger, the agent PATH and where it came from, `SSH_AUTH_SOCK`, listening addresses, firewall, Tailscale, Bonjour and the local network permission, and offline database repair.
- **One button updates every machine.** Each machine shows which sessions keep running (holder) and which restart.

### Stage 4: moving sessions

Exit: a session moves from the Mac to the Linux box mid-task and the agent carries on with full context, and every interruption point in the move test leaves exactly one live copy.

- [0064](decisions/0064-moving-sessions.md) end to end, including Continue here and the text fallback.

Status (2026-10-22): built and accepted. Moves are carried by `dent move`, with R-D3 automated and R-D5 run for real. Still to come: the app's move button and the record's link (a UI to design), cloning a project the target lacks as part of the move, and the text fallback when an agent can't resume.

### Stage 5: the phone

The client core already holds the rules, so the phone is mostly UI plus the platform constraints in [0062](decisions/0062-liveness-and-retry.md#iphone) and [0061](decisions/0061-trust.md#iphone).

## Must-handle lists

Each line is a failure that has happened in a shipped multi-machine tool. The stage that owns it must have a test for it, or a written reason why it can't happen in dent.

### Connection (stage 1)

- One connection actor per machine in the client core. Views signal it; they never open their own sockets. A notification extension shares through the app group.
- A failure inside one attempt (panic, unknown error) is caught and retried; it never kills the actor.
- Each failure is classified as transient (retry on a timer) or blocked (wait for a change: foreground, network change, the machine re-announcing, Retry). "Needs update" and "not trusted here" are blocked.
- Backoff is reset only after about 30 s of healthy connection, so a session that opens and dies keeps escalating.
- Retry means: probe if connected, otherwise attempt now with a fresh ladder. Wakeups never cancel an attempt in flight. An explicit disconnect is never lost among wakeups.
- Liveness:
  - The ping is answered in the I/O task, before any app work, database lock or queued output.
  - The socket reader never waits on a subscription's buffer. A full subscription is dropped and told to resume from its sequence number.
- Large payloads are chunked (64 KiB) with control messages (ping, approvals, stop) interleaved between chunks. One undecodable message fails its stream, not the connection.
- States:
  - The handshake, the hello, resumed subscriptions and caught-up are separate milestones. "Online" comes after the hello; "catching up" is shown per session.
  - A failed subscription is not "reconnecting".
  - Every connection has a generation number, and anything from an older one is dropped.
- Catch-up: attach live first, replay up to the head captured at resume, de-duplicate by sequence. A cursor ahead of the daemon's head, or a different log epoch, means a full resync.
- A resumed attach never recreates a stopped resource (terminals later).
- Reads made during a reconnect wait for the next session; they don't fail as unavailable. A machine never flashes "offline" at startup before its first attempt.
- Desktop keeps running sessions subscribed on every machine. Phone and web subscribe to what's visible plus a summary.
- Routes:
  - Kept as hints with their source (Bonjour, learned from the daemon, manual, Tailscale), last success and round trip. Never used as identity.
  - The daemon reports its current addresses in the hello and when they change. Clients learn them, drop learned ones it stops reporting, never learn loopback, and never touch a route the person added.
- Coming home: while on a slower route, check faster ones on network change, foreground, Bonjour sighting and every 60 s. Switch only once the new session is ready, then move subscriptions. A route that answered but then failed is left alone for 5 minutes.
- When a known address answers with a different key, that route is stale for the old machine. Show "A different machine is now at 192.168.1.20" with Add.
- The daemon's port is persistent. If it's taken, choosing another checks with a connect, not only a bind (macOS lets a bind succeed on a port held on all interfaces).
- Advertised addresses:
  - Come from the interface with the default route first.
  - Never include loopback, link-local, Docker, VM, bridge or VPN interfaces (other than Tailscale), 198.18.0.0/15, or inactive interfaces.
  - Bonjour is restricted to physical Wi-Fi and Ethernet.
- A route is labelled Tailscale only when Tailscale confirms the address. 100.64.0.0/10 is shared with other VPNs and carrier NAT.
- The daemon watches interface changes. It re-announces Bonjour, re-reads Tailscale and pushes new addresses. Tailscale coming up after the daemon started is handled without a restart.
- The OS "offline" signal never tears down a socket; it's a hint to probe. While disconnected and "offline", LAN routes are still tried now and then: the OS says offline on networks with no internet, which is where LAN pairing lives.
- Discovery and address lookup restart on network change and retry on a timer.
- The client's own sleep: on wake, probe every machine at once, and don't show toasts for drops that happened while asleep.
- Expected restarts (announced, or an update in progress) reconnect every 0.5–1 s for up to 2 minutes, then fall back to normal backoff with "didn't come back".
- Daemon-side:
  - Shared caches store successes only, and run detached from any one connection.
  - A write to a dead socket is handled per connection.
  - Long jobs (updates, clones, moves) are owned by the daemon with an id; clients reattach, and a disconnect never cancels them.
  - The hello and resume path does no discovery work.
  - SQLite runs on its own blocking thread, which network tasks never wait on.
  - The daemon tracks its own scheduling lag and reports the last stall in the hello.
  - Idle timers subtract time the machine was asleep.
- Presentation:
  - About 1 s of grace before "Reconnecting" after an unannounced drop; none for announced states.
  - A dropped live session says "Reconnecting", and "Can't reach" only after a real attempt failed.
  - Status rows never shift content.
  - One away machine never sets the app-wide status.
  - Retry is on every machine that isn't online.
  - After a few failures, "Reconnecting" decays to "Can't reach · last seen 3 min ago".
- Words follow the failure kind: never connected, went silent, closed by the machine, or refused with a reason.
- Diagnostics:
  - Hop by hop: this device → network path (LAN, Tailscale up, peer online) → reachable → key matches → trusted → version → daemon ready → holder alive. The first failing hop gets a fix.
  - The last 20 attempts are kept, each with its route, the step reached, duration and error.
  - "Copy diagnostics" gives one text block.
- Only causes actually observed are stated; anything else is "may be".
- Tailscale error text and `whois` output are never logged raw (they can hold auth keys and names).

### Trust and identity (stage 1)

See [0059](decisions/0059-machine-identity.md) and [0061](decisions/0061-trust.md). In short:
- **The handshake:**
  - The responder's key is checked before the client sends its own.
  - Nothing that changes trust is accepted unless it arrives inside an authenticated session.
  - "Can't verify" never deletes trust.
- **Pairing:**
  - The version range and reachability are checked before a code counts as a guess.
  - A dropped connection doesn't burn a guess.
  - A retry resumes the same pending pairing.
  - Each outcome has its own code: wrong, expired, closed, paused with the time left, version mismatch with both versions, different machine, already paired.
- **Tailscale trust** is checked live on every connection and never saved as trust. Over the LAN, a machine trusted only through Tailscale gets "trusted on Tailscale only", which demotes that route without marking the machine untrusted.
- **Identity:** two installs on one device are two devices. A cloned data folder is caught at startup.

### Projects, files and settings (stage 2)

- **One repository resolver.** Every feature uses it: grouping, cloning, PRs, prompt links.
  - The group key comes from origin, the PR base from upstream (asked of `gh` rather than guessed from remote names), and `gh repo set-default` is honoured.
  - SSH aliases are resolved with `ssh -G`.
  - Local-path remotes are ignored, bare repositories and partial-clone annotations handled, and URLs read from `git config`, not parsed from `remote -v`.
  - The last known key is kept, so a deleted folder stays in its group as "folder missing".
  - A watcher on `.git/config` regroups as soon as a remote changes.
- **Same-named things:**
  - A machine's name is set by the person; a disambiguator shows only on a name clash.
  - A machine kind (laptop, desktop, mini, server, VM, cloud) comes from the hardware and can be overridden. The same icon appears everywhere the machine is named.
- **Draft and machine-change rules.** Every async retarget carries a token, and stale results are dropped.
- **References must resolve where the session runs.**
  - A session reference must resolve on the target, or be attached as a transcript the client builds from its cache.
  - A folder dropped on a remote session is refused, or offered as a copy.
- **Media:**
  - Fetched from the machine that owns it, never from a local file at the same path.
  - "Not found" names the machine and the path.
  - Images and pages an agent publishes embed their media.
- **A negative cache tied to a path** is cleared by whatever creates the path (a clone, a new worktree).
- **Settings:**
  - "Not loaded" is never a value.
  - A daemon lists the setting keys it stores, and a setting a daemon can't store is greyed out with "update dent there".
  - A write aimed at a machine that's gone fails closed; it never widens to "all".
- **Usage** counts an account once across machines, by account identity.
- **Notifications:**
  - Never replayed on reconnect or first load.
  - Grouped by (machine, session).
  - Dropped when their machine is hidden or removed.
  - Agents waiting on you come before working ones.
- **Search** keys every result by machine. Results from away machines are marked cached.
- **"Forget this machine"** removes its sessions and projects from every cache.

### Lifecycle (stage 3)

See [0065](decisions/0065-launcher-and-updates.md). Also:
- **Running under the service manager:**
  - The daemon gets a working PATH (captured at install, re-read from the login shell with markers and a timeout) and an `SSH_AUTH_SOCK` for agents that push.
  - Each agent runs in its own systemd scope, so running out of memory kills that agent, says so in the session, and leaves the daemon alone.
  - Commands run from inside the service (an agent running `dent update`) hand the restart to the service manager asynchronously.
- **One daemon per data folder:** a lock, plus a distinct exit code that the launcher treats as "stay idle", not a restart loop.
- **Install safety:**
  - An older CLI can't downgrade the service without `--allow-downgrade`.
  - Install writes an intent marker before swapping and clears it only after the service runs.
- **After a reboot or a holder crash,** interrupted turns can continue. The guards: never while waiting on the person, at most 3 in 5 minutes, and stable ids. The cut work is named in the notice.
- **Startup never notifies** for history.

### Moving (stage 4)

See [0064](decisions/0064-moving-sessions.md) for the protocol and the list of interruption points.

## Platform constraints to design in now

- **macOS:**
  - Every bundle that spawns agents or browses Bonjour declares `NSLocalNetworkUsageDescription` and `NSBonjourServices` (`_dent._tcp`). Without it, agents' own LAN access (ssh, curl, dev servers) fails too, because macOS attributes child processes to the app.
  - A LaunchAgent stops at logout ([0065](decisions/0065-launcher-and-updates.md)).
  - Keep-awake (`caffeinate -s`) holds a closed lid only on power, never on battery. Settings must say so.
- **iPhone (stage 5, but it shapes the core now):**
  - The transport uses Network.framework, not URLSession. App Transport Security blocks unencrypted WebSockets to Tailscale's 100.x addresses through URLSession, and Noise already encrypts.
  - After 10 s or more in the background, every machine is redialled at once, because iOS kills suspended sockets silently.
  - Going inactive (Control Center, Face ID, a call) is not going to the background.
  - The Local Network permission sheet fails the first LAN attempt. Explain it first, then restart attempts when the app becomes active again.
  - Path changes come from NWPathMonitor, with the first path seeded.
  - Repeatedly dialling an unreachable Tailscale peer can stall the phone's Tailscale tunnel. Check the peer is online in the netmap where possible, and back off offline peers to minutes.
  - The notification key uses `kSecAttrAccessibleAfterFirstUnlock` in a shared app-group keychain, so the extension can read it while the phone is locked.
  - The keychain service name and access group are constants, not derived from the app's display name. Startup failures are never cached.
- **Web:** hidden tabs get about one timer a minute, so the daemon judges liveness and the page only answers. On becoming visible, the page probes. A shared worker holds one socket for all tabs.
