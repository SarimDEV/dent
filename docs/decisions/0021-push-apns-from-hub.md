# 0021. Push: APNs from the hub

Status: accepted (2026-10-11) · Spec: [hub.md](../hub.md)

## Decision

The hub sends APNs pushes with encrypted payloads; a notification service extension decrypts them. Self-hosted hubs send content-free pushes through the hosted hub. Blocking approvals are time-sensitive.

## Why

iOS cannot hold background connections.

## Consequences

Requires an Apple developer account. Falls back to in-app notifications.
