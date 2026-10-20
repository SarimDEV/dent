# Spike 12: the link under real network trouble

Date: 2026-10-20 · studio (macOS 26.5) as client, dent-linux (Ubuntu 24.04 VM) as daemon, over the LAN bridge · `tc netem` and `iptables` on the VM · Tests [R-A1, R-A3](../remote-tests.md#a-links-liveness-and-routes) for [0062](../decisions/0062-liveness-and-retry.md) · Code: `spike-link remote` in [spikes/link](../../spikes/link)

Question: under real network trouble, do spike 05's liveness numbers (1 s ping, 3 s silence, backoff capped at 2 s) hold up? Do 0062's (2 s ping, 15 s teardown, replacement dial at 5 s, capped jittered backoff)?

**Answer:**
- Steady loss breaks neither. Stalls break spike 05's numbers every time.
- 0062's numbers keep the link through stalls of up to about 12 s, and move to a fresh connection the moment the network returns.
- Two rules the plan didn't have came out of this run: the daemon's idle limit must be longer than the client's, and the replacement dial matters more than expected.

## How it was run

`spike-link remote follow` pairs with the daemon on the VM once, then follows a session there while a prompt goes every 10 s on its own connection (retried until acknowledged, as the client core's outbox does). Both rule sets ran side by side on the same link where noted. Each run logs every drop and reconnect, then a summary: events, gaps, repeats, drops, dials, time disconnected, longest silence and worst round trip.

## Results

**Steady loss** (R-A1): 200 ms ±100 ms delay, 2% loss, 1% reordering on the VM's side, for 10 minutes, both rule sets together:

| Rules | Drops | Events | Gaps / repeats | Longest silence | Worst round trip |
| --- | --- | --- | --- | --- | --- |
| spike 05 | 0 | 1,430 | 0 / 0 | 1.8 s | 1.1 s |
| 0062 | 0 | 1,422 | 0 / 0 | 2.8 s | 1.3 s |

TCP retransmission hides steady loss. It isn't what breaks short timeouts.

**Stalls**, as on a Wi-Fi handoff or in a dead spot: everything on the daemon's port dropped for a few seconds every minute.

| Run | Blackouts | Drops | Dials | Disconnected in total | Notes |
| --- | --- | --- | --- | --- | --- |
| spike 05 rules | 3 × 4 s, 3 × 8 s | 6 (every one) | 14 | 32.5 s | Each 8 s one took 3 dials to come back |
| 0062 rules, daemon idle 10 s | same | 2 | 3 | 1.2 s | Both drops were the *daemon* closing the link after 8 s blackouts |
| 0062 rules, daemon idle 15 s | 8, 12, 20 s × 2 | 3 | 8 | 15.1 s | One 12 s blackout was still reset; both 20 s ones dropped at 15 s as designed |
| 0062 rules plus replacement at 5 s, daemon idle 30 s | 8, 12, 20 s × 2 | 2 real (+3 swaps) | 25 | 14.8 s | 8 and 12 s: moved to a fresh link **under 1 ms** after the network returned. 20 s: back 3.6 s after it returned |

No run lost or repeated an event, and every prompt was acknowledged exactly once.

**A frozen daemon** (R-A3): `SIGSTOP` for 5, 12 and 30 s, with 0062's rules:

| Freeze | Result |
| --- | --- |
| 5 s | Link kept; replies resumed |
| 12 s | Link kept; replies resumed |
| 30 s | Dropped at 15 s; reconnected 0.9 s after the daemon resumed |

Replacement dials during a freeze fail at their 3 s handshake limit (the kernel accepts the connection, but nobody answers) and do no harm.

## What we learned

1. **After an outage, TCP's own retransmission backoff keeps the old link silent long after the network is back.** A 12 s blackout can read as more than 15 s of silence. A fresh connection doesn't inherit that backoff, which is why the replacement dial at 5 s turns a 12 s stall from a reset into a sub-millisecond swap. It stays in [0062](../decisions/0062-liveness-and-retry.md), now with evidence.
2. **The daemon's idle limit must be longer than the client's teardown** (here 30 s against 15 s), so the client decides when a link is dead and the replacement can take over. A daemon limit at or below the client's reset links the client would have kept. Added to 0062.
3. **Reading must be its own task.** The spike's receive wasn't safe to cancel mid-message, so moving to a 250 ms tick meant moving reading into a task that feeds a queue. That is the "reader never waits on anything" rule in [remote-plan.md](../remote-plan.md#connection-stage-1).
4. **What's left after a long blackout is the dial timeout.** After a 20 s blackout, reconnecting took 3.6 s after the network returned, because a dial started during the blackout waits out its 3 s limit. Starting a new dial every second while silent, rather than one at a time, should bring that under a second. This is a tuning number.
5. **The dial budget needs the 10-minute switch.** With the cap at 60 s and waits in the upper half, an hour of failures is about 80 dials, more than 0062's budget of 60. The 5-minute cap after 10 minutes of failure, already in 0062, is what keeps it within budget. The spike doesn't model that switch yet, so R-A8 waits until it does.

## Not covered yet

- Coming home from Tailscale to the LAN (R-A6) needs the route-switch logic, which isn't built in the spike.
- A frozen machine (R-A4), address changes (R-A7), hours unreachable (R-A8), chunking under load (R-A9), versions (R-A10) and two clients (R-A11).
- "Froze for 12 s" reports from the daemon after a stall. The spike's daemon doesn't measure its own lag.
