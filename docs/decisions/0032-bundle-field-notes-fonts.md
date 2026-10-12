# 0032. Bundle the Field Notes fonts

Status: accepted (2026-10-11) · Spec: [design.md](../design.md)

## Decision

Ship Geist, Geist Mono and Instrument Serif inside the apps (`assets/fonts/`), loaded from bytes at startup, rather than relying on installed fonts.

## Why

The design depends on them and most machines don't have them. All three are under the SIL Open Font License, which allows bundling with the licence text alongside.

## Consequences

About 0.9 MB added to the binary. `assets/fonts/OFL-*.txt` must ship with the apps.
