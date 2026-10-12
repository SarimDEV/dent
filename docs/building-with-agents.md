# Building with agents

dent is built, verified and maintained by coding agents, so the repository is shaped for them: the everyday loop takes seconds, heavier checks run only before merging or overnight, every screen can be driven and photographed by a script, and every decision is written down where an agent will find it.

## Fast first

Speed of iteration comes before test coverage, especially early on.

| Tier | When it runs | What it runs | Target time |
| --- | --- | --- | --- |
| Inner loop | Every save | `just dev` rebuilds and relaunches only what changed; `just check` compiles and runs unit tests for the crates you touched | Seconds |
| Before merge | When a change is ready | `just verify`: lints, all unit tests, replays and UI checks for the areas the change touched | A few minutes |
| Nightly | Overnight | Screenshot comparisons, simulation, fuzzing, performance budgets, real Claude Code and Codex runs | Unlimited |

- **See changes instantly:** a dev gallery window shows any screen or chat block in any state with fake data, with no agents, machines or network needed.
- **Fast builds:** many small crates, dependencies optimised even in debug builds, a fast linker, and a parallel test runner.
- **Tests arrive as things settle:** early phases test only what protects data and sessions. Screenshot, fuzz and performance suites switch on for a screen or format once it stops changing every day.
- **Nightly failures become tasks,** not blockers on whatever you are doing that day.

## A repository agents can work in

- **AGENTS.md** at the root (with CLAUDE.md pointing to it) holds the architecture map, crate boundaries, commands, conventions and the definition of done.
- **This spec, split by area** into `docs/`, with acceptance criteria for every feature.
- **Decision records** in `docs/decisions/`, one short file per choice (GPUI, iroh, git through the CLI), so agents do not relitigate settled questions.
- **Small crates with one job each**, each with a short README; logic crates never import GPUI, enforced by a dependency check in CI.
- **Two commands:** `just check` while working and `just verify` before merging (see Fast first); agents never run the nightly suite on their own.

## Verifying behaviour

- **Fake agents:** a replay driver plays recorded Claude Code and Codex sessions, so most tests spend no usage; a scripted fake agent produces edge cases on demand (approvals, crashes, giant outputs, slow streams).
- **Deterministic simulation:** time, network and processes sit behind interfaces, so tests replay sleep, network loss, daemon restarts and multi-device races exactly, and the reliability requirements run nightly.
- **Multi-device tests:** several daemons and clients in one test run, talking over a local iroh relay.
- **Nightly real agents:** the latest Claude Code and Codex run against a test repo on a dedicated machine with its own logins.

## Verifying the interface

- **Automation socket:** debug builds of the desktop app expose a local control socket to open screens, click and type by accessibility label, wait for a state, take screenshots and dump the accessibility tree. Agents use it to reproduce bugs, prove features work, and attach before-and-after screenshots to every UI change.
- **Screenshot tests:** every screen in fixed states at three window sizes in Paper and Carbon, compared with approved images; text snapshots of the view and accessibility trees run on every platform.
- **Design comparison:** the mockups in `docs/design/` are the visual reference; a reviewer agent compares new screenshots with it and with the palette tokens, and flags drift.
- **iPhone and web:** SwiftUI snapshot tests and XCUITest flows on a macOS runner; browser automation tests for the web client.
- **Fresh-eyes review:** every pull request is reviewed by a separate agent that did not write it, against the acceptance criteria and the screenshots.

## Staying maintainable

- **Upgrades are scheduled tasks:** a monthly agent run bumps pinned dependencies one at a time, runs `just verify` and the replay suite, and opens a pull request showing any changed schemas or screenshots.
- **Lints as guardrails:** no `unwrap` in daemon code, no direct I/O in the client core, no GPUI outside UI crates, no new dependency without a decision record.
- **Built-in diagnosis:** structured logs, crash reports, and `dent doctor`, which checks agent versions, logins, connectivity and disk space, and is the first thing an agent runs on a bug report.
- **Feature flags** for anything risky, so unfinished work merges early and stays off.
- **Docs change with code:** a change in behaviour updates its spec section and decision record in the same pull request.
