# 0008. Claude Code: drive `claude -p` stream-json directly

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

Run `claude -p --input-format stream-json --output-format stream-json --verbose` as a long-lived child; answer tool permissions on the control channel. Detect features from `system/init`, not version numbers. Usage limits come from `get_usage` and `rate_limit_event` (decision 0031).

## Why

No extra runtime; uses the user's own binary and login.

## Consequences

Fallback if the stream format becomes unworkable: a small Node sidecar on the official Agent SDK. The daemon never reads agent tokens.
