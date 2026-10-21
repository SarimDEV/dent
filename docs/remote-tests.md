# Remote tests

The real tests behind [remote-plan.md](remote-plan.md). A one-way decision is accepted only when its tests pass on real machines, with real Tailscale and real agents. Loopback and simulated checks don't count for these.

Each test says how to run it and what passing means. Results go in the spike's findings note, with numbers.

## Stage 0 results

Run on the rig with the spike code, not the real daemon (2026-10-20). Every test still runs against the real code before release.
- **Passed:** R-B1, R-B2 ([spike 10](spikes/10-tailscale-trust.md)), R-B4 ([spike 15](spikes/15-sleep-restart-and-serve.md)), R-D1 ([spike 09](spikes/09-moving-sessions.md)), R-D2 ([spike 11](spikes/11-moving-git.md)), and R-E1–E6 on Linux ([spike 13](spikes/13-service-and-updates.md); R-E3 and R-E4 with the launcher prototype, and no `dent doctor` yet).
- **Passed in part:**
  - R-A1 for 10 minutes, not 60 ([spike 12](spikes/12-link-under-real-networks.md)).
  - R-A3 without the "froze for 12 s" report, because the spike daemon doesn't measure its own lag (spike 12).
  - R-A4 with freezes up to 75 s. macOS kills the VM's process when it's frozen for 2 minutes ([spike 15](spikes/15-sleep-restart-and-serve.md)).
  - R-A5, where a restart showed as "quit". [0065](decisions/0065-launcher-and-updates.md) now has the launcher tell the daemon why it's stopping (spike 15).
  - R-A6 without the 5-minute cooldown, which the spike doesn't have ([spike 14](spikes/14-routes-identity-and-boot.md)).
  - R-B7 as far as what a cloned VM keeps; the startup check isn't built (spike 14).
  - R-D4 without dirty submodules or the size cap (spike 11).
- **Not run yet:** the rest.

## Stage 1 runs (the real code)

Run 2026-10-20 with the real `dent-daemon` on dent-linux and the real `dent` CLI on studio (the client core's connection actor, hosted on tokio):

- **R-B1, for real:** `dent machine add 100.70.46.115:7480` was trusted through the Tailscale account (the daemon asked the LocalAPI about studio's address) with no code, in 0.45 s.
- **R-A1-style stall, IPv4 only:** while the link sat on the IPv4 route, the IPv4 port was blocked for 8 s. The client read "slow" at 2 s, and its replacement dial moved to Tailscale's IPv6 address at 4.6 s, without dropping. Sixty seconds later it came home to the preferred IPv4 route.
- **A full 20 s blackout (IPv4 and IPv6) with events streaming:** 40 prompts went to the fake agent at 2.5 s intervals while studio watched.
  - Slow at 2.7 s; torn down at 15 s; reconnected 0.26 s after the network returned, catching up from event 47.
  - **85 events received, 85 logged by the daemon, 0 gaps, 0 repeats.**
  - It came home to the IPv4 route later.
- **R-A5, for real:** a SIGTERM showed as "away (quit)" at once, and the link was back 4 s after the daemon restarted.

## The rig

- **studio:** the owner's Mac.
  - macOS 26.5, Apple Silicon, 10 cores, 32 GB.
  - Standalone Tailscale 1.102.4 (`io.tailscale.ipn.macsys`), so the LocalAPI is reachable.
  - Claude Code 2.1.301.
- **dent-linux:** a Lima VM on studio (config at `~/dent/vm/dent-linux.yaml`).
  - Ubuntu 24.04, 4 cores, 8 GB, its own address on the Mac's bridge (192.168.64.2).
  - On the tailnet as `dent-linux` under the owner's account.
  - Tailscale 1.104.2, Claude Code 2.1.305 signed in to the owner's subscription with a long-lived token.
  - Driven with `limactl shell dent-linux`. The owner never needs to touch it.
- **Also on the tailnet:**
  - `cakebal`, shared in from another account (the "different owner" case);
  - an iPhone, used once the phone phase starts.

Network conditions are made on the VM with `tc netem` (delay, jitter, loss, reordering) and `iptables`/`nft` (blocking a route). A frozen machine is the VM's processes stopped with SIGSTOP. A sleeping machine is the VM itself paused. Neither touches the Mac's own network, which the agent running the tests depends on.

Tests marked **(owner)** need the owner's hands: the Mac's lid, the Mac's Wi-Fi, the Tailscale admin console. They are batched so the owner does them once, in one sitting, from a checklist.

## A. Links, liveness and routes

| Id | Test | How | Passes when |
| --- | --- | --- | --- |
| R-A1 | Bad network soak | Client on studio, daemon on the VM. `netem` 200 ms ±100 ms, 2% loss, 1% reordering, for 60 min, with a reply streaming every few minutes | No teardown while frames keep arriving. Every reply arrives whole. "Slow" may show; "offline" never does |
| R-A2 | Slow first catch-up | 200,000-event session, VM link throttled to 2 Mbit/s, client reconnects | The socket is kept while catch-up arrives, with progress shown. No retry loop |
| R-A3 | Frozen daemon | SIGSTOP the daemon for 5, 12 and 30 s mid-reply, then SIGCONT | "Reconnecting" shows after grace; the socket is torn down only past the silence limit. After it resumes: "froze for 12 s", the reply completes, 0 gaps, 0 repeats |
| R-A4 | Frozen machine (sleep stand-in) | Pause the whole VM for 2 min and 30 min, then resume | Same as R-A3, plus: on resume the client redials at once rather than waiting out backoff. No toast pile-up |
| R-A5 | Announced sleep, quit and restart | The daemon on the VM announces each one, then goes | Shown within 100 ms of the announcement. An announced restart reconnects every 0.5–1 s and is back within 2 s of the daemon listening |
| R-A6 | Coming home | Connected over the LAN; block the LAN route on the VM; reconnect over Tailscale; unblock the LAN | Moves to Tailscale with the reply intact. Back on the LAN within 60 s of it returning, switching only once the new session is ready. A LAN route that answers and then fails isn't retried for 5 min |
| R-A7 | Address change | Change the VM's LAN address while connected | The daemon reports the new address. The client learns it, drops the old one, and is never sent to loopback |
| R-A8 | Unreachable for hours | Stop the VM's daemon for 3 h with the client running | Dials follow the capped, jittered ladder. The count is logged and stays under the budget in [0062](decisions/0062-liveness-and-retry.md). Retry dials at once |
| R-A9 | Lossy chunking | 5 MB tool output streams while approvals and pings flow; `netem` 1% loss | Approvals and pings are never stuck behind the payload by more than one chunk. Nothing is torn down |
| R-A10 | Versions | A daemon built from an older commit with a lower protocol range, and one with a higher range | "Needs update" names which side to update and both versions. The maintenance subset still updates the old daemon. A block never becomes the machine's "off" switch |
| R-A11 | Two clients | Desktop on studio and a second client (CLI) on the VM watch one reply; one answers an approval | Both get every word. The approval disappears from the other within 1 s |
| R-A12 | Many machines | 10 daemons on the VM under different state folders and ports | Settings and the inbox stay usable. Each machine syncs on its own; one that never answers holds up nothing |

## B. Trust and identity

| Id | Test | How | Passes when |
| --- | --- | --- | --- |
| R-B1 | Tailscale whois through the LocalAPI | Read `whois` for a connecting peer on studio (standalone Tailscale) and on the VM (tailscaled socket). Time it, then repeat 1,000 times | Returns the owner's login for each other. p99 under 50 ms. The CLI is never spawned on the connection path |
| R-B2 | Another account | Look up `cakebal`'s address | A different login, so trust needs a code |
| R-B3 | Tagged machine (optional, before a public release; needs the owner to create a tag and an auth key) | A second VM joins with a tag | Never trusted through the account check; asks for a code |
| R-B4 | Loopback through Tailscale Serve | `tailscale serve` on the VM forwards to the daemon; connect through it | The daemon sees loopback and refuses account trust |
| R-B5 | LAN pairing | studio finds the VM with Bonjour across the bridge and pairs with the code | Paired. A wrong code, a dropped connection mid-exchange, and a version mismatch each give their own outcome. Only a completed wrong exchange counts as a guess |
| R-B6 | Impostor at a known address | Stop the real daemon; start one with a new key at the same address and port, which then claims "you were removed" | The client aborts before sending its own key and ignores the claim. It shows "a different machine is now at …". The real machine is still trusted afterwards |
| R-B7 | Cloned identity | Copy the VM's state folder into a second VM, or restore the VM from a snapshot alongside the running one | The clone notices the install binding doesn't match and refuses to pose as the original. It offers to become a new machine. Clients that see one key at two addresses flag it |
| R-B8 | Removal | Remove studio on the VM, and the VM on studio | Cut off within 1 s, both ways. A removal that didn't come over an authenticated session is ignored |
| R-B9 | Leaving the tailnet | Log the VM out of Tailscale while it is connected through account trust | The session drops at the next check. A connection over the LAN is refused without a code |

## C. Product across machines

| Id | Test | How | Passes when |
| --- | --- | --- | --- |
| R-C1 | Away machine in the inbox | Sessions on both machines; stop the VM | Its sessions show their last state plus "asleep" or "offline", never working. Hide, pin and reorder still work |
| R-C2 | Queued prompt | Stop the VM's daemon; type a prompt for one of its sessions; edit it; start the daemon | Shown as "Waiting for dent-linux · sends when it's back", with Edit and Cancel. Sent exactly once on reconnect, and the edited text is what arrives. Cancelled ones never send. Other devices see it only once it has been sent ([0058](decisions/0058-independent-machines.md)) |
| R-C3 | Same repo, different remotes | Clone one repo on studio over SSH with an alias (`git@gh-work:…`) and on the VM over HTTPS; add a fork with `upstream` | One project, listed with both copies. The fork groups under its own owner/name; PRs target upstream |
| R-C4 | Add to another machine | Clone a project from studio onto the VM, copying `.env`, a binary ignored file and a non-UTF-8 file. Disconnect the client mid-copy | The job finishes on its own. The bytes match by hash. The new session waits for setup, then runs |
| R-C5 | Settings with a machine away | Change a machine setting on all machines while the VM is stopped | studio saved; the VM "Not saved" with Retry. Retry after it returns saves it once. Nothing widens to a machine that wasn't chosen |
| R-C6 | Files to another device | An agent on the VM makes a 200 MB video and a PNG; open both on studio | They stream, seek without re-downloading, and open again from cache after the URL expires |
| R-C7 | localhost links | An agent on the VM starts a dev server and prints `localhost:5173` | The link opens the VM's address from studio |

## D. Moving sessions

| Id | Test | How | Passes when |
| --- | --- | --- | --- |
| R-D1 | Transcript resume across machines (first) | A real Claude session on studio (2.1.301) reads files, edits, runs tools and has a subagent. Copy its transcript and side files to the VM (2.1.305) under the VM's cwd key, then `claude --resume`; then the reverse direction | The agent continues with context: it answers questions about earlier tool results without re-reading. Records which side files are needed, whether old absolute paths cause wrong reads, and whether older-from-newer works. If resume fails, the text fallback carries the session |
| R-D2 | Git snapshot | Worktree with commits, staged edits, unstaged edits, untracked files, a deleted file, a rename, a mode change, a binary and a force-added ignored file; pack on studio and apply on the VM | `git status`, `git diff` and `git diff --cached` on the VM match the source exactly, staged split included. The bundle excludes what the VM already has |
| R-D3 | Interrupted at every step | Kill the source daemon, the target daemon or the link at each step in [0064](decisions/0064-moving-sessions.md#interruption-points) | Exactly one live copy every time. A retry with the same move id finishes; Continue here releases the source with its warning |
| R-D4 | Refusals | Target branch diverged; branch checked out elsewhere on the target; a nested repository; a dirty submodule; bundle over the cap | Each is refused before anything stops, with what to do |
| R-D5 | A real move mid-task | Start a multi-step task on studio, move it to the VM after the first turn, and let it finish there. Then move a session between two folders that both exist on one machine | The work finishes on the VM with no repeated steps. The tombstone on studio links to it. In the same-machine move, nothing touches the old folder |

## E. Install, service and updates

| Id | Test | How | Passes when |
| --- | --- | --- | --- |
| R-E1 | systemd user service | Install on the VM; log out of every SSH session; reboot the VM | The daemon keeps running after logout and comes back after the reboot. `dent doctor` reports linger |
| R-E2 | Agent PATH | `claude` only in `~/.local/bin` (as installed on the VM) | The daemon under the service manager finds it, and `dent doctor` says where PATH came from |
| R-E3 | Update with a failing migration | Launcher prototype: update to a build whose migration fails halfway | The database is restored from the snapshot, the old version runs, the client is told "rolled back", and the version is never retried automatically. Agents in the holder keep streaming throughout |
| R-E4 | Disk nearly full | Fill the VM's disk to just under the snapshot size, then update | The update is refused before anything stops |
| R-E5 | Out of memory | An agent allocates until the kernel kills it | Only that agent dies. The session says "killed: out of memory"; the daemon and other agents carry on |
| R-E6 | Restart from inside | An agent on the VM runs the restart command | The restart completes through the service manager; the caller isn't left stopped |
| R-E7 | LaunchAgent at logout **(owner)** | Log out of studio and back in | Matches what [0065](decisions/0065-launcher-and-updates.md) says the person is told |

## F. Owner checklist (one sitting, once there's a second device)

Deferred until the phone app exists (2026-10-20). With one Mac, these tests can't be watched from the other end: the VM runs on the Mac, so it sleeps when the Mac does, and its link to the Mac doesn't use Wi-Fi. Stalls and freezes are covered from the VM side in [spike 12](spikes/12-link-under-real-networks.md). Run them with a reply streaming from studio to the phone:

1. Close studio's lid on battery for 5 minutes, then open it. Then the same plugged in.
2. Turn studio's Wi-Fi off for 30 s, then on. Switch it to a phone hotspot and back.
3. Log out of studio and back in (R-E7).

For each, the agent records what the client showed, when, and whether anything was lost.

## G. Phone (stage 5)

These are design constraints today, tested when the phone app exists. Every one has a matching rule in [remote-plan.md](remote-plan.md#platform-constraints-to-design-in-now):
- a long background, a short background, and inactive-only (Control Center);
- Wi-Fi to cellular with the LAN socket left silent;
- the Local Network permission sheet on first launch;
- pairing over Tailscale's 100.x addresses with App Transport Security in force;
- dialling an offline tailnet peer for an hour;
- the notification extension reading its key with the phone locked.
