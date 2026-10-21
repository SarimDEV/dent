# Spike 15: a frozen machine, an announced restart, and Tailscale Serve

Date: 2026-10-20 · studio (macOS 26.5) and dent-linux (Ubuntu 24.04 VM) · Tests [R-A4, R-A5](../remote-tests.md#a-links-liveness-and-routes) and [R-B4](../remote-tests.md#b-trust-and-identity) for [0061](../decisions/0061-trust.md) and [0062](../decisions/0062-liveness-and-retry.md)

## A frozen machine (R-A4)

The whole VM was frozen from the Mac (SIGSTOP on its Virtualization process) while the client followed a session with 0062's rules. This stands in for a laptop that sleeps without saying so.

| Freeze | What happened |
| --- | --- |
| 30 s | Torn down at 15 s; back as the VM resumed (about 0 s after) |
| 75 s | Torn down at 15 s; back **about 5 s** after the VM resumed. The client was waiting out its backoff step; the next dial found it. 0 gaps; every prompt was retried until it landed once (17 retries, 19 acknowledged) |
| 120 s | Not valid: macOS killed the VM's process after it had been frozen that long, a quirk of the test rig. The VM was restarted |

**What it confirms:** a machine that comes back unannounced is found within one backoff step. That step grows to a minute, and to 5 minutes after 10 minutes away, which is why 0062 has the machine announce `awake` (and re-announce on Bonjour) to make clients redial at once. A sleeping laptop doesn't freeze like this; it really sleeps and announces it. This covers the case where the announcement is lost.

## An announced restart (R-A5)

`systemctl --user restart` on the VM's daemon, mid-stream:

| | Result |
| --- | --- |
| Goodbye shown | At once (`said goodbye`) |
| Back | 1.0 s later, 0 gaps, 10 of 10 prompts |
| What it said | **"quit", not "restarting"**: systemd stops the daemon with SIGTERM, so the daemon can't tell a restart from a stop |

**What changes:** the launcher ([0065](../decisions/0065-launcher-and-updates.md)) tells the daemon why it's being stopped (restart, update or stop) before signalling it, so clients show "restarting" with the expected downtime instead of "quit".

## Tailscale Serve (R-B4)

`tailscale serve --tcp 7490 tcp://127.0.0.1:7480` on the VM forwarded a tailnet port to the daemon. A device that had never been paired then tried to pair with a deliberately wrong code:

| Route | Result |
| --- | --- |
| Directly to the daemon's Tailscale address | **Trusted by account**: the wrong code was never needed |
| Through Serve (the daemon sees 127.0.0.1) | **Not trusted**: asked for the code and refused the wrong one |

Account trust can't arrive through a forward, as [0061](../decisions/0061-trust.md) requires. The Serve config was removed afterwards.

## Seen again: starting at boot

When the VM was force-restarted, the daemon's service came up this time, where in [spike 14](14-routes-identity-and-boot.md) it failed. Whether it starts depends on whether Tailscale is up first. A race like that is exactly why the daemon must listen on the wildcard address.
