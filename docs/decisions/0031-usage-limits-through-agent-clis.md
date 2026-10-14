# 0031. Usage limits come from the agent CLIs, per account, synced via the hub

Status: accepted (2026-10-11) · Spec: [daemon.md](../daemon.md) (Usage limits)

## Decision

Read usage through each agent's own CLI: Claude Code's `get_usage` control request and Codex's `account/rateLimits/read`, refined live by `rate_limit_event` and `account/rateLimits/updated`. Store readings per provider account with their fetch time, and sync them as hub shared state (newest wins), with one poller per account.

## Why

Usage is account-level on the provider's side, so this reading already includes use from the terminal, the web and other machines, and it matches everywhere. The CLIs handle auth, so dent never touches tokens. The same approach already works in practice in the owner's menu-bar app.

## Consequences

`get_usage` is undocumented: it gets contract tests per Claude Code version, with `rate_limit_event` as the fallback. Windows are labelled by duration because plans differ (a free Codex plan has one 30-day window).

The same short-lived CLI also answers `initialize`, which lists the account's models with their versions, exact model ids and effort levels. dent keeps that list (and refreshes it on every session start), so the model menu names real versions, such as "Opus 5.5", before any session runs, and never keeps its own copy of the catalogue.
