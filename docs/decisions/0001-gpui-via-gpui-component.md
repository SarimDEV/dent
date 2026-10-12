# 0001. Desktop UI: GPUI through pinned gpui-component

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

The desktop app uses GPUI via gpui-component, with the GPUI snapshot pinned exactly and upgraded monthly. Only the `desktop` crate may import GPUI.

## Why

Native on macOS, Linux and Windows, fast enough for the frame budgets, and it is dent's differentiator. GPUI is pre-1.0 with thin docs.

## Consequences

All logic lives in GPUI-free crates, enforced by `just verify`. If GPUI breaks us, Tauri is the escape hatch for the view layer only. Diff editing builds on gpui-component's editor.
