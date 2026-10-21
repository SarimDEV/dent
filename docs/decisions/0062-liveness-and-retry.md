# 0062. Liveness and retry: any frame is life, probe before replacing, capped backoff from v1

Status: proposed (2026-10-20) · R-A1, R-A3 ([spike 12](../spikes/12-link-under-real-networks.md)) and R-A6 ([spike 14](../spikes/14-routes-identity-and-boot.md)) passed; still to confirm: [R-A4, R-A5, R-A7, R-A8](../remote-tests.md#a-links-liveness-and-routes) · Changes spike 05's timings · Plan: [remote-plan.md](../remote-plan.md)

## Decision

The client core owns one connection actor per machine. Views only send it signals: connect, disconnect, retry, foreground, background, network changed. The numbers below are where we start. R-A1–A8 tune them; the rules are what is fixed.

### Liveness

- **Any frame counts as life,** not only a pong. Frames are counted as they arrive at the socket, so the app being busy can't hide a live link.
- **Pings:** both sides ping every 2 s while idle.
- **Silence:** after 3 s with no frame the machine shows "Reconnecting…" while the socket is kept. After 5 s a replacement is dialled in parallel, and whichever proves alive first wins. After an outage, TCP's own retransmission backoff keeps the old socket silent long after the network is back, so the replacement turns a 12 s stall into a sub-millisecond swap ([spike 12](../spikes/12-link-under-real-networks.md)). While silent, a new dial starts every second. The socket is torn down only after **15 s with no frame**.
- **The daemon waits longer than the client:** its idle limit is 30 s, twice the client's teardown, so the client decides when a link is dead and its replacement takes over. A daemon limit at or below the client's resets links the client would keep.
- **Announced states:** asleep, quitting and restarting show at once. Silence falls back to offline after 15 s.
- **Probes:** foreground, Retry, a network change and the OS saying "offline" all probe the live socket rather than replacing it. A probe waits max(3 s, 4 × smoothed round trip), is extended while frames keep arriving, and gives up at 15 s.

### Retry

- **The first attempts are quick:** 0.1 s, 0.25 s, 0.5 s, 1 s.
- **Then capped, jittered doubling:** from 2 s, with each wait random in the upper half of its step. The cap is 60 s while the app is in front and the machine was online in the last 10 minutes, and **5 minutes** otherwise.
- **Backoff resets only after 30 s of healthy connection.** A link that opens and dies keeps escalating.
- **Signals skip the wait at once and restart the ladder:** foreground, a network change, the machine re-announcing on Bonjour or being seen as online in Tailscale's netmap, or Retry.
- **Blocked states wait for a signal** instead of a timer: `incompatible`, `not_trusted`, `trusted_on`.
- **Budget:** after the first 10 minutes, no machine is dialled more than 60 times an hour.
- **Expected restarts are faster:** after an announced restart or during an update, redial every 0.5–1 s for up to 2 minutes. If it doesn't come back, fall back to the ladder and show "didn't come back".

### Routes

- **First connect:** all known addresses race, a quarter second apart, and the first to answer with the right key wins (spike 05).
- **Coming home:** while on a slower route (LAN is preferred over direct Tailscale, then relayed Tailscale, then the relay), faster routes are checked:
  - on a network change;
  - on foreground;
  - when the machine is seen on Bonjour;
  - every 60 s.
- **Switch make-before-break:** the new session must be ready before subscriptions move and the old one closes.
- **Cooldown:** a route that answers but then fails is left alone for 5 minutes.

### Presentation

- **A drop:** about 1 s of grace before "Reconnecting" after an unannounced drop. A dropped live session says "Reconnecting"; "Can't reach" appears only after a real attempt failed. After a few failures it settles on "Can't reach · last seen 3 min ago".
- **Status is per machine.** App-wide status is only about this device being offline.

### iPhone

- **Long background:** after 10 s or more in the background, the session is replaced at once rather than probed, because iOS kills suspended sockets silently. Shorter returns probe.
- **Inactive is not background.** Going inactive (Control Center, Face ID, a call) is not going to the background. Coming back from it only checks for a better route.
- **Path changes:** NWPathMonitor reports every change (interface, gateway, set of interfaces), with the first path seeded. A change probes the socket and races the other routes.
- **Offline tailnet peers:** before dialling a tailnet address, check Tailscale's netmap for the peer where possible. Back off offline peers to minutes, with at most one dial in flight per machine.

### Web

Hidden tabs get about one timer a minute, so the daemon judges liveness and the page only answers. On becoming visible, the page probes. One shared worker holds the socket for all tabs.

## Why

Spike 05's 1 s ping, 3 s offline and 2 s backoff cap passed on a LAN. Shipped tools learned these numbers from real failures:
- Dropping on one late pong looped reconnects over WireGuard, LTE and busy servers.
- Short probes dropped working slow links.
- A 2 s cap stopped no one: old clients retrying every 17 s were most of one tool's load.
- A phone redialling an offline tailnet peer every 25 s stalled its Tailscale tunnel every few minutes.
- Hidden browser tabs flapped every minute overnight.
- Healthy sockets were replaced on every foreground and offline report.

Clients in the field can't be updated retroactively, which is why this is a one-way door.

## Consequences

- R-A8's dial count is checked against the hourly budget.
- The client core's actor tests cover each rule, from a list of scenarios kept in [remote-plan.md](../remote-plan.md#connection-stage-1).
