# 0003. Git writes through the git CLI; gix read-only

Status: accepted (2026-10-11) · Spec: [daemon.md](../daemon.md)

## Decision

Every git write (stage, unstage, commit, worktrees, push) shells out to `git`. `gix` is used read-only for fast status and object reads; imara-diff computes diffs. Hunk staging writes a blob and updates the index entry — no patch files.

## Why

Behaviour matches the user's git config and hooks exactly. Rust git libraries lag on writes.

## Consequences

`git` must be installed on every machine running a daemon. Plain git always works as a fallback.
