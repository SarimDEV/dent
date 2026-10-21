# Spike 10: Tailscale trust through the LocalAPI

Date: 2026-10-20 · Tailscale 1.102.4 standalone (studio, macOS 26.5) and 1.104.2 (dent-linux, Ubuntu 24.04) · Tests [R-B1, R-B2](../remote-tests.md#b-trust-and-identity) for [0061](../decisions/0061-trust.md)

Question: can the daemon tell, quickly and without running the Tailscale CLI, whether a connecting peer belongs to the same Tailscale account?

**Answer: yes, on both machines, in about a millisecond.** There is one catch on macOS.

## How to reach the LocalAPI

| Machine | Endpoint | Credential | Who can use it |
| --- | --- | --- | --- |
| Linux (tailscaled) | Unix socket `/var/run/tailscale/tailscaled.sock`, host `local-tailscaled.sock` | None | Any local user (`srw-rw-rw-`); `whois` worked as the normal user |
| macOS standalone (`io.tailscale.ipn.macsys`) | `127.0.0.1:<port>`, port from the symlink `/Library/Tailscale/ipnport` | Basic auth with an empty user and the token in `/Library/Tailscale/sameuserproof-<port>`, plus the header `Sec-Tailscale: localapi` | Admin users only: the token file is `root:admin 0640` |

Without the token the port answers 401. The port and token can change when Tailscale restarts, so the daemon re-reads both whenever a call fails with a connection error or 401. The App Store build keeps its LocalAPI inside its sandbox, which wasn't tested here. [0061](../decisions/0061-trust.md) already says account trust is unavailable where the LocalAPI can't be reached.

## Results

`GET /localapi/v0/whois?addr=<ip>:<port>` and `GET /localapi/v0/status` (for the daemon's own user):

| Peer | From studio | From dent-linux |
| --- | --- | --- |
| The other machine on the owner's account | user 4132274656315929, `m2sarim.dev@gmail.com`, no tags: matches own user | same: matches |
| `cakebal`, shared in from another account | user 1835637162501976, `jj0141601@gmail.com`: no match, so a code is needed | same |
| `127.0.0.1` | 404 `no match for IP:port` | 404 |
| The VM's LAN address 192.168.64.2 | 404 | — |
| 1,000 lookups | p50 0.83 ms, p99 2.12 ms, max 12.6 ms | p50 0.15 ms, p99 0.59 ms, max 1.5 ms |

For comparison, the CLI (`tailscale whois --json`) took 0.02 s here, but runs a process per call.

## What changes in the plan

- **The rule in [0061](../decisions/0061-trust.md) holds as written:**
  - compare the peer's `Node.User` with the daemon's own `Self.UserID`;
  - require `Node.Tags` to be empty;
  - treat any lookup failure (404, 401, timeout) as "not trusted by account", never as an error that blocks pairing.
- **Loopback and LAN addresses can never match,** so connections through Tailscale Serve, or over the LAN, fall to pairing by construction. A real Serve forward later confirmed it (R-B4, [spike 15](15-sleep-restart-and-serve.md)).
- **On macOS, a non-admin user can't read the token.** For that user the daemon can't use account trust, and says so in `dent doctor`. A per-connection CLI call is not the fallback, for the reasons in 0061.
- **Not yet tested:** a tagged node (R-B3, needs the owner to create a tag) and leaving the tailnet mid-session (R-B9, needs the daemon).
