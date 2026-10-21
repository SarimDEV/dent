# Spike 14: coming home, cloned machines, and starting at boot

Date: 2026-10-20 · studio (macOS 26.5) and dent-linux (Ubuntu 24.04 VM, Lima 2.2.1) · Tests [R-A6](../remote-tests.md#a-links-liveness-and-routes) and [R-B7](../remote-tests.md#b-trust-and-identity) for [0059](../decisions/0059-machine-identity.md), [0060](../decisions/0060-remote-wire.md) and [0062](../decisions/0062-liveness-and-retry.md)

## Coming home (R-A6)

The client followed a session with two routes, the LAN first and Tailscale second, using 0062's rules and a 20 s home check for the test. About 28 s into the run, the LAN was blocked for 60 s.

| Moment | What happened |
| --- | --- |
| LAN blocked (about 28 s) | After 5 s of silence the replacement dial raced both routes; Tailscale answered and the client moved at 33.6 s, with 1 ms between links |
| LAN back (about 89 s) | The next home check found the LAN; moved back at 96.4 s |
| Whole run (150 s) | Disconnected for 8 ms in total; 765 events, 0 gaps, 0 repeats; 30 of 30 prompts delivered once |

Make-before-break works as designed. The default home check stays at 60 s, as in 0062.

## Cloned machines (R-B7)

`limactl clone` copied dent-linux, and the clone was started while the original was stopped.

| | Original | Clone |
| --- | --- | --- |
| `/etc/machine-id` | 9699…4539 | **the same** |
| dent's identity key (state folder) | — | **the same** (copied) |
| Tailscale identity | `dent-linux`, 100.70.46.115 | **the same**: the clone joined the tailnet *as* dent-linux |
| DMI product UUID (`/sys/class/dmi/id/product_uuid`) | e808…132d | 4ce2…5c34, different, but readable only by root |
| Network card addresses | 52:55:55:5f:21:45 … | 52:55:55:50:15:d9 …, different |

The clone was stopped at once and deleted, so it never ran beside the original.

**What changes:** [0059](../decisions/0059-machine-identity.md) binds the key to `/etc/machine-id` on Linux, and that doesn't catch a cloned VM. The binding on Linux becomes the machine id **plus the permanent addresses of the physical network cards** (interfaces with a `device` link under `/sys/class/net`), which any user can read. A mismatch means "copied". A single card changing (a new USB adapter) is not counted; only all of them changing is. On macOS, `IOPlatformUUID` is readable by the user and differs per machine. Clients flagging one key at two addresses stay the backstop. Tailscale itself shows the hazard: a cloned node takes over the original's name and address.

## Starting at boot

After the VM restarted, the spike daemon's systemd service failed:
- it tried to listen on the Tailscale address (100.70.46.115) before Tailscale had brought it up ("Cannot assign requested address");
- `Restart=on-failure` tried five times in quick succession;
- systemd's start limit then gave up. The daemon would be dead after every reboot until someone started it by hand.

**What changes:** this is the evidence for the rule in [remote-plan.md](../remote-plan.md#connection-stage-1) that the daemon listens on the wildcard address and filters each connection by the route it arrived on. It must never bind an interface address at startup. The launcher's own restart policy should also use backoff rather than systemd's quick restarts, so one bad start can't strand the service.

## Not covered yet

- Address changes (R-A7). They need the daemon to report its addresses in the hello, which the spike doesn't do. The behaviour without it is already clear: a stale LAN address fails over to Tailscale and never comes home.
- Clients flagging one key seen at two addresses at once.
