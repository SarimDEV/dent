# 0032. Type: the Book pairing, bundled, with fonts as theme roles

Status: accepted (2026-10-11) · Spec: [design.md](../design.md) (Type)

## Decision

Chat text in Literata, interface chrome in IBM Plex Sans, code and labels in Commit Mono, display in Instrument Serif. All are bundled in `assets/fonts/` and loaded from bytes at startup. Each role is a theme setting, so themes can replace any face.

## Why

Chosen from five pairings rendered in the real desktop window. Geist, the first choice, has become the default look of AI-generated interfaces. A reading serif for chat suits the reason Field Notes exists (hours of reading) and sets dent apart. All faces are under the SIL Open Font License, which allows bundling with the licence text alongside.

## Consequences

About 3 MB of fonts in each app binary. `assets/fonts/OFL-*.txt` ship with the apps. Variable fonts render their default instance in GPUI, so bundle static weights. Plex, Fira with Departure Mono, and Atkinson remain good alternative themes.
