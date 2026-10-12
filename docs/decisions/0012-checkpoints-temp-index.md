# 0012. Checkpoints via a temporary index under refs/dent/

Status: accepted (2026-10-11) · Spec: [daemon.md](../daemon.md)

## Decision

Before each turn, snapshot each workspace with a temporary index, write-tree and commit-tree, stored under hidden `refs/dent/` refs. Untracked files over a size cap are skipped; the user's index is never touched.

## Why

Catches edits made through shell commands and subagents that the agent's own checkpoints miss.

## Consequences

Restore is a plain read-tree. Snapshots are pruned with the session and never pushed.
