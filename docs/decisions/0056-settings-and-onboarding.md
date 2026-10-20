# 0056. Settings and the first-run welcome share one list; the daemon says how things stand

Status: accepted (2026-10-20) · Builds on [0055](0055-agent-layer.md) · Spec: [desktop.md](../desktop.md#settings-and-the-first-launch)

## Decision

**One list, two doors.** Settings (⌘,) and the first launch's welcome are one sheet drawn from the same rows: the agents, this Mac, and dent's own choices. They're drawn the way the command sheets are (a mark in the margin, the name, how it stands beneath, controls at the end), so there's no second visual language for settings.

**The daemon says how things stand.** The rows come from `Request::Agents` (installed, version against the tested one, signed in, where to install, what signs in) and `Request::Machine` (Git, who commits, the GitHub CLI, start at login). The desktop only reads whether notifications are allowed, which only the app can know.

**A preference row is its menu item.** Switches and buttons dispatch the same actions as the menu bar, so the two can't drift.

**Fixes happen where they're done.** Signing in runs the agent's own sign-in in Terminal, and installing opens the agent's setup page. dent never handles a token or installs anything itself.

**The welcome is once.** It opens on the first launch and never again. Prefs from before it existed count as seen, so nobody already using dent is greeted. What can't wait (the default agent missing or signed out) is said on the new-session page instead, for as long as it's true.

**Per-project choices stay out.** 0054's per-project defaults are still remembered by starting a session, not set here.

## Why

The owner asked for onboarding and settings (2026-10-20), and for the multi-agent groundwork to be finished while sticking with Claude Code. The workflow log had already named a guided "set up this machine" step as the fix for slow machine switches (2026-10-18). Building it on the daemon's answers means a remote machine's setup reads the same way once there are machines.

## Consequences

- No new dependency; the notification read needs the `UNNotificationSettings` feature of the pinned `objc2-user-notifications`.
- The `settings` and `welcome` walkthroughs run in `checks`. `signed-out`, `missing` and `limited` set the agent up so by name in `walk.sh`.
- Start at login is refused unless the home folder is the account's real one, so a walkthrough or a test can never register a launch agent.
