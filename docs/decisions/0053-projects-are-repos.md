# 0053. A project is a repository; dent makes a session's worktree

Status: accepted (2026-10-19) · Supersedes the Model and Worktrees sections of the earlier [projects.md](../projects.md) · Refines the Worktree row of [0036](0036-sessions-as-an-inbox.md) · Spec: [projects.md](../projects.md), [desktop.md](../desktop.md#new-session)

## Decision

**A project is one repository** (or one folder that isn't one): its main checkout. A worktree of it is the same project, so sessions and the project list group under the main checkout, whichever worktree they run in. There is no separate idea of a project spanning several repositories.

**dent makes a session's worktree, when you ask for one.** Under a new session's field, the worktree chip offers:

- **Current checkout:** the session works in the project's folder, on whatever is checked out there.
- **New worktree:** before the first message goes, the daemon runs `git worktree add` on a new branch into `~/dent/worktrees/<project>/<id>`, from the branch chosen in the chip beside it (the one checked out by default). The branch starts as `dent/<8 hex>` and is renamed from the task (`dent/fix-footer-on-mobile`) by a quick model while the agent starts.
- **An existing worktree** of the project, to carry on there; for that session only.

New worktree or current checkout is remembered as what you used last. In a new worktree:

- The main checkout's ignored `.env` files are copied in, and a secret saved to the project's `.env` from a worktree goes to the main checkout's too, so the next worktree has it.
- A `.dent/setup` script, if the repository has one, runs with `sh` before the first message, with `DENT_ROOT` (the main checkout) and `DENT_WORKTREE` set. The chat says how it went; the task goes either way.

**A session shows its branch**, under its reply box, marked "Worktree" when it has one of its own. The daemon says the branch when the session starts and whenever it changes at the end of a turn (a `workspace` event).

**The header has one Git button**, the branch's next step: commit (a message written from the diff), commit and push, commit, push and open a pull request (with the GitHub CLI, its title and body written from the commits), push, pull, or the open pull request. A push to the default branch asks for a second press.

**Closing a session never touches its worktree.** A closed session offers Remove worktree, which keeps the branch and its commits; one with uncommitted changes asks again ("Remove anyway"). Reopening a session whose worktree was removed brings it back on its branch.

## Why

The owner's call (2026-10-19), after asking for the choice of where a new task runs: a project is a repository, as the code already had it, and worktrees work the way a tool built on them does well. The earlier spec said agents make worktrees and dent only notices them, which left no way to choose one before starting; and a project spanning repositories made "a new worktree" ambiguous (of which repository?) while nothing used it.

## Consequences

- The project list folds worktrees, including ones made from a terminal, into their main checkout.
- Removing is always a separate, explicit step, so a closed session's uncommitted work is never lost by closing.
- Commit messages, pull request text and branch names come from the same small one-shot model call as titles ([0050](0050-titles-and-seen-marks.md)), run outside the project.
- Not yet: starting from the remote's latest base branch (the worktree starts from the local branch), choosing a session's machine, moving a session to another project, starting a session from a pull request, and writing a commit message yourself.
