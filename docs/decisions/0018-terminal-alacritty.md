# 0018. Terminal: alacritty_terminal on session-holder PTYs

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

Terminal emulation with alacritty_terminal (pinned fork), drawn with GPUI, backed by PTYs owned by the session holder.

## Why

Proven emulator; terminals survive app and daemon restarts.

## Consequences

Move to libghostty once it reaches 1.0.
