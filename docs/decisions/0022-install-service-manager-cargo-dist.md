# 0022. Install and update: service-manager and cargo-dist

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

service-manager registers launchd, systemd and Windows services; cargo-dist builds installers and its updater handles self-update.

## Why

Cross-platform service install without hand-written unit files.

## Consequences

Manual install still works.
