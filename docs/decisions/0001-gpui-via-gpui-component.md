# 0001. Desktop UI: GPUI through pinned gpui-component

Status: accepted (2026-10-11) · Spec: [durability.md](../durability.md)

## Decision

The desktop app uses GPUI via gpui-component, with the GPUI snapshot pinned exactly and upgraded monthly. Today: `gpui-kit =0.7.1`, which pins `gpui-pre =0.3.8`; GPUI is built at opt-level 3 even in dev builds. [Spike 03](../spikes/03-gpui-chat-and-diff.md) measured 100k-event chat and 100k-line diff well inside the frame budget. Only the `desktop` crate may import GPUI.

## Why

Native on macOS, Linux and Windows, fast enough for the frame budgets, and it is dent's differentiator. GPUI is pre-1.0 with thin docs.

## Consequences

All logic lives in GPUI-free crates, enforced by `just verify`. If GPUI breaks us, Tauri is the escape hatch for the view layer only. Diff editing builds on gpui-component's editor.
