# 0011. Sessions are append-only event logs in SQLite

Status: accepted (2026-10-11) · Spec: [daemon.md](../daemon.md)

## Decision

Each session is an append-only log of typed events in SQLite (WAL, one writer task per daemon), with a monotonic sequence number per daemon. Clients subscribe with "everything after N". Deltas coalesce into final messages at turn end. SQLite with FTS5 everywhere, including client caches.

## Why

Catch-up and reconnect are the same call; standard tooling.

## Consequences

Diagnostic logs never go in the event store. WAL is checkpointed on a schedule.
