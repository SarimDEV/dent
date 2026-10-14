# 0045. A new session's project is never guessed from what you type

Status: accepted (2026-10-14) · Supersedes the Project row of [0036](0036-sessions-as-an-inbox.md) · Spec: [desktop.md](../desktop.md#new-session), [projects.md](../projects.md)

## Decision

A new session's project is a choice you can see before you type, and typing never changes it:

- From inside a session (Cmd+N, New session, `/clear`), the new session starts in that session's folder.
- Otherwise it starts in the project of the last session you started or picked, remembered across launches; "No project" (the scratch folder) when there's none.
- The chip under the field changes it: the picker, Cmd 1–9, a typed path, a new project or a clone.

Quick capture follows the same rule: it starts in the last project, shown under the prompt, and Tab opens the picker.

## Why

Picking the wrong project is far worse than one click to pick the right one: the agent works, and writes, in the wrong repository. A guess from words can't be trusted (names rarely appear as typed, and unrelated tasks fell back to the latest project while looking chosen). Starting where you are, or where you last were, is what you expect and is visible before you send.

## Consequences

- The project is never decided by a model; the decision model stays for other questions.
- "Move to project" remains the way to fix a session started in the wrong place.
