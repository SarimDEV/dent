# 0076. How new sessions start is kept on each machine

Status: accepted (2026-10-24, the owner's call) · Supersedes the per-project defaults of [0054](0054-project-defaults-prs-and-scripts.md) · Mockups: `docs/design/mockups/settings-*.png` on the settings-designs branch

## Decision

**Defaults live on the machines.** Each daemon keeps how new sessions start there: agent, model, permission, effort, and a new worktree or not (`Machine.defaults`, set with `SetDefaults`). Every device asks each machine as it connects, so the Mac, the phone and any other device start sessions alike. A part never set is left to what's under it (the agent's own model, Full access per [0048](0048-full-access-default.md), no worktree).

**A project's own, on purpose.** A project can have its own defaults over its machine's (`Machine.project_defaults`, by its folder there), set only by choosing to: nothing is remembered from what a session happened to start with. This replaces 0054's "each project remembers what its last new session started with".

**The composer's choice is for that session.** Picking a model, permission, effort, agent or worktree in the new-session composer changes that session only; the next one starts from the defaults again. Each of those menus ends with **Make this the default on _machine_**, **Only for _project_**, and **On all machines**, or, when the choice is the default, says so. A choice changed inside an open session is that session's alone, too.

**Changes reach machines that are away.** A change is kept on this device (`Prefs.pending_defaults`) until the machine answers it, sent again whenever it's back; meanwhile it counts already. A machine too old to keep defaults says so once, and this device's last choices stand there.

**Nothing changes under you on the first run.** A machine with no defaults yet takes what this device's sessions used last. 0054's remembered per-project values aren't carried over: they were never chosen on purpose.

**Settings › Agents** (next): an All machines tab, then one per machine (those that don't fit behind "N more", with a search), grouping a setting's values when machines differ ("Mixed: 3 differ · Ask: gpu-box, gpu-2, build-box · Full: the other 17"), with Make all; each machine's tab holds its defaults, its agents and its own settings.

## Why

The owner's call (2026-10-24), from three designs and a fourth with an All tab, tested against twenty machines. Defaults kept on one device leave every other device starting sessions differently, which a phone would make plain; defaults that change whenever you pick something in the composer change every machine's from a quick one-off choice. Another tool with the same layering skips machines that are offline and never catches them up; here they catch up.

## Consequences

- A remote machine's defaults cost one `Machine` request on connect (it was asked for this Mac already).
- A device offline for a long while sends its kept changes when it's back, over what another device set meanwhile: last write wins, per machine and project.
- 0054's per-project memory is read from old prefs and let go.
