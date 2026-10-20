# 0059. A machine is its key, bound to its install; everything is keyed by machine

Status: proposed (2026-10-20) · confirmed by [R-B6, R-B7](../remote-tests.md#b-trust-and-identity) · Builds on [0033](0033-direct-websocket-hub-on-hold.md), [spike 05](../spikes/05-direct-links.md) · Plan: [remote-plan.md](../remote-plan.md)

## Decision

**A machine's identity is its X25519 key** (spike 05), and its id is the key's fingerprint. Addresses, names and Bonjour instances are only hints.

**The key is bound to the install.** Beside the key, the daemon stores a binding: the hardware id (`IOPlatformUUID` on macOS, `/etc/machine-id` on Linux) and the state folder's real path. At startup:
- A matching binding means it starts.
- A mismatch means the state folder was copied (Migration Assistant, a restored backup, a cloned VM or a baked cloud image). The daemon refuses to serve under the old identity, and says so in its log and in `dent doctor`. `dent daemon reidentify` makes it a new machine, keeping its sessions as history.
- A missing binding is written once, on the first start after this record ships.

**The client backstop:** a client that sees one key answer at two different addresses at the same time flags both as "possibly cloned". This catches cloned VMs that also share `/etc/machine-id`.

**Identity safety:**
- Key generation runs under the state folder's lock, so two daemons starting together can't make two keys.
- A damaged key stops the daemon; it never quietly becomes a new machine (spike 05).
- One daemon per state folder (spike 06's lock). A second daemon exits with a code its supervisor treats as "stay idle", not "restart".
- A beta channel uses its own state folder, key, port and Bonjour name.

**Everything is keyed by (machine, id):**
- Every cached record in the client core: sessions, events, projects, files, search rows, notifications.
- Session links (`dent://…`), which carry the machine id. An older link without one opens a unique match, or nothing.
- Session ids stay random UUIDs, and are never assumed unique across machines. Restored or cloned machines repeat them.

**The event log has an epoch:** a random id made when the database is created, and changed whenever the log is reset or restored. Clients resume by (epoch, sequence), so a reset log can never be mistaken for the old one ([0060](0060-remote-wire.md)).

**Names:** a machine's name is set on that machine and stored by its daemon. It defaults to the computer's name. The machine kind (laptop, desktop, mini, server, VM, cloud) comes from the hardware and can be overridden. Two installs on one device (dent and dent Beta) are two machines with two keys.

## Why

Tools that use a stored random id as identity have shipped every failure here:
- Migration Assistant copied the id, so the new Mac posed as the old one.
- Machines restored from the same backup shared session ids, and lists opened the wrong machine's session.
- Four servers on one data folder each resumed the others' sessions.
- A preview build sharing a home folder disabled the main one.

A key already gives dent a tamper-proof identity. Binding it to the install closes the copy hole, and keying the cache by machine closes the duplicate-id hole.

## Consequences

- The client core's cache schema uses (machine, id) primary keys from its first version.
- `Subscribe` gains the log epoch ([0060](0060-remote-wire.md)).
- R-B7 copies a state folder to a second VM, and restores a snapshot beside the running VM, to prove both the startup check and the client backstop.
