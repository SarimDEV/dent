# Hub, sync and networking

Signing in is the whole setup: install the daemon, sign in, and that machine appears on every device you own within a second. A hosted hub handles accounts, devices, connections, relaying, push notifications, shared state and backup. Everything it stores is end-to-end encrypted, and the hub can be self-hosted.

## Three kinds of state

| State | Lives on | How it moves | Conflicts |
| --- | --- | --- | --- |
| Sessions, events, terminals, git, files | The daemon running them | Clients subscribe by sequence number; encrypted backup to the hub (paid) | None: one writer |
| Projects, settings, UI state, drafts, device list, secrets | The hub, as the record | Every client and daemon syncs with the hub and keeps a local copy | Offline edits queue and apply on reconnect; latest edit wins per field |
| Client cache | Each device | Filled from daemons and the hub | None: a cache |

- Each shared-state field is its own value, so renaming a project on the phone and adding a repo on the laptop both survive.
- The hub orders edits, which removes the clock-drift and gossip problems of syncing device to device.

## Signing in and adding machines

1. Install the daemon with one command. It opens a sign-in link, or prints one on a headless machine.
2. Sign in with GitHub, Google, or email and password. Sign-in providers are pluggable, so Apple or single sign-on for company teams can be added later. The machine joins your account and shows up on every device.
3. On a new phone or laptop, sign in once and approve it with one tap on a device you already have, which hands over the encryption keys.

- Ephemeral machines join with a short-lived token from the hub instead of a sign-in.
- Removing a device from the list revokes it everywhere at once.
- A recovery key, saved at first sign-in, restores access if every device is lost.

## Connections

All traffic is one protocol over QUIC, with a stream per subscription so a large diff never blocks chat.

- The hub introduces devices and helps them open a direct encrypted connection through NATs and home routers; when that fails, traffic relays through the hub.
- A direct LAN or Tailscale path is used automatically whenever it is reachable, because it is fastest.
- If the hub is down, devices that already know each other keep connecting directly with cached addresses and keys.
- Clients connect to every daemon at once and reconnect with backoff, resuming every stream from its last sequence number.

## Honest liveness

A remote client must never show an idle session as working or leave a dead session on screen, so liveness is designed in rather than inferred.

- Each machine shows connected, reconnecting, asleep or offline, with last seen time.
- A session on a sleeping machine shows "paused, machine asleep", never "working".
- An approval answered on one device disappears from every other device immediately.
- Stop is acknowledged by the daemon only once the agent process has actually stopped.

## Encryption

- Session content, shared state, docs and backups are encrypted with keys only your devices hold. The hub stores ciphertext plus the metadata it needs: device names, timestamps and sizes.
- Device keys live in the OS keychain; the iPhone app can require Face ID.
- The hub cannot read code or chats, so search and anything else that reads content runs on your devices.

## Push notifications

The hub sends Apple push notifications with encrypted payloads; a notification service extension on the phone decrypts and shows the real text. Approvals that block an agent are sent as time-sensitive so they break through Focus modes. Desktop clients use native OS notifications directly.

## Client cache

Each client stores every event it has received, plus markdown files and diffs it has opened, in a local SQLite database with full-text indexing.

- Opening any session renders instantly from cache, then streams the gap.
- Offline and torn-down machines stay readable, greyed out, with last seen time.
- Search across all sessions and docs runs locally, so it is instant and works offline.
- A storage limit per machine keeps metadata for old sessions and drops bodies until reopened; with backup, they reload from the hub.

## Hosted services

The local product is free; hosted conveniences are paid. Users always bring their own Claude and Codex logins, so dent never resells model access.

| Service | What it gives | Price |
| --- | --- | --- |
| Daemon, desktop and iPhone apps | Everything over LAN, Tailscale or a self-hosted hub | Free |
| Hosted hub | Sign-in, device list, connections, relay, push, shared state, fleet updates | Free for up to 3 machines, then a subscription |
| Web client | Open sessions from any browser, served by the hub | Included with the hub |
| Encrypted session backup | History survives a lost or torn-down machine; new devices get full history at once | Subscription |
| Hosted cloud machines | One click runs a session on a cloud box that auto-joins and tears down when idle | Usage-based |

- The web client uses the same client core compiled to WebAssembly, with a web UI for chat, approvals, docs and diffs.
- The self-hosted hub is the same open-source binary, so privacy-minded users lose nothing but convenience.
