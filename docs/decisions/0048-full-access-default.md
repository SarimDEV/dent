# 0048. New sessions start in Full access

Status: accepted (2026-10-15) · Spec: [desktop.md](../desktop.md#new-session)

## Decision

A new session's permission mode is Full access until you pick another one; whatever you pick then becomes the default, as before. Existing installs move to Full access once, when they first run a build with this change; a choice made after that is kept.

## Why

The owner's call. Supervised asked before every command and edit, burying a run under approval cards and receipts, while an `ask` rule in the project's own settings still stops Full access where it matters (a `git commit`, say). People who want to approve each step pick Supervised once and keep it.

## Consequences

- Shift+Tab still steps Ask → Edits → Plan and never into Full.
- The one-time move is recorded by a marker file beside the prefs (`desktop-full-access-default`), not a field in them, so an older build rewriting the prefs can't undo a choice made since. Walkthroughs create the marker up front, since they check approvals.
