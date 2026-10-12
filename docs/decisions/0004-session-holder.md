# 0004. A session holder owns agent processes and terminals

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

A tiny, rarely changed `session-holder` process owns agent child processes and PTYs. The daemon connects to it and reconnects after an upgrade.

## Why

Service managers kill child processes when a service restarts, so a daemon upgrade would otherwise interrupt every session.

## Consequences

Two processes per machine. The holder's interface must stay small and stable. If the holder dies, sessions resume from the agent's own transcript.
