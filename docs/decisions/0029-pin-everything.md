# 0029. Pin everything, upgrade on a calendar

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

Exact versions and a committed lockfile; each dependency (and each agent CLI per daemon release) upgrades on a schedule via a monthly agent run, never by accident. Recorded replays per agent version run before merging driver changes.

## Why

An upstream change becomes a planned task instead of a broken app.

## Consequences

Nightly canary against the latest agents warns users outside the tested range rather than blocking them.
