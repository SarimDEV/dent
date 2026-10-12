# 0005. License: MIT, with the hub under AGPL-3.0

Status: accepted (2026-10-11) · Spec: [risks.md](../risks.md)

## Decision

MIT for the daemon, session holder, client core, desktop, iPhone and web apps, and the CLI. AGPL-3.0 for the hub.

## Why

Permissive for everything users run; anyone running a modified hosted hub must publish their changes.

## Consequences

`crates/hub` carries its own LICENSE. The hub must not be a dependency of MIT crates.
