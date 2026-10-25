# Projects

A project is a repository: its main checkout, or a folder that isn't a repository yet ([0053](decisions/0053-projects-are-repos.md)). A worktree of it belongs to the same project, so sessions group under the main checkout whichever worktree they run in.

## Model

| Entity | What it is | Fields |
| --- | --- | --- |
| Project | A repository's main checkout on a machine (or a plain folder) | path, name, last used |
| Worktree | A linked checkout of a project, on a branch of its own | path, branch |
| Session | One conversation with one agent | folder it works in (the project's, or a worktree's), branch, machine, agent, title, status |

- A session can have no project: it runs in `~/dent/scratch`.
- Picking a repo for a session on a machine without a checkout offers to clone it there.

## Starting from an idea

A new idea should cost one keystroke, not a setup screen.

- Cmd/Ctrl+N opens a new session: you start typing, in the project you were in, or the last one you used; typing never changes it (decision [0045](decisions/0045-no-project-guessing.md)). It runs with the model, permissions and worktree choice you used last.
- If the idea has no repo yet, the session gets a fresh scratch folder with `git init`, and the agent can create a remote later.
- Promoting a scratch folder to a repo or adding existing repos happens whenever you want, without moving the session.
- The project is shown under the composer before you send, and can be changed there (the picker can also make or clone one), or later with "Move to project".

## Worktrees

dent makes a session's worktree when you ask for one ([0053](decisions/0053-projects-are-repos.md)).

- Under a new session's field, the worktree chip picks **Current checkout**, **New worktree** (remembered as what you used last), or one of the project's eight newest worktrees to carry on in. With New worktree, a second chip picks the branch it starts from: the one checked out by default, the chosen and checked-out ones first, and a field to search the rest.
- New worktree is only offered for a repository with a commit to start from; elsewhere (no project, a plain folder, an empty repository) the session works in the folder as it is, whatever was chosen last.
- A new worktree goes in `~/dent/worktrees/<project>/<id>` on a new branch, `dent/<id>` at first and renamed from the task (`dent/fix-footer-on-mobile`) as the agent starts.
- The main checkout's ignored `.env` files are copied in. A secret saved to the project's `.env` from a worktree session is saved to the main checkout's too.
- If the repository has `.dent/setup`, it runs with `sh` in each new worktree before the agent starts, with `DENT_ROOT` (the main checkout) and `DENT_WORKTREE` set. Meanwhile the session shows as working ("Setting up"), and what you send waits in its queue, in order; it goes when setup is done, whatever came of it. The chat says how it went (a failure with its last line), and a script still running after ten minutes is stopped. Commit it, and every worktree has it.
- A session shows its branch under its reply box, marked "Worktree" when it has its own. A branch the agent switches to shows at the end of the turn.
- Worktrees made in a terminal count too: the project list folds them into their main checkout, and they can be picked to carry on in.
- Closing a session never touches its worktree. A closed session offers **Remove worktree** (its branch and commits stay; one with uncommitted changes asks again), and reopening one whose worktree is gone, or sending to one, brings it back on its branch.
- Diffs and docs work the same in a worktree as in the main checkout.

## Getting work out

The session header's Git button is the branch's next step, worked out from where it stands: **Commit** (no remote), **Commit & push**, **Commit, push & PR**, **Push**, **Push & PR**, **Pull**, or the open pull request (**PR #12**). A branch that has diverged from its remote says so instead.

- Commit messages are written from the diff, and pull requests' title and body from the branch's commits, by the same quick model as titles.
- Pull requests use the GitHub CLI (`gh`), signed in; without it, the button stops at pushing.
- A push to the remote's default branch asks for a second press ("Push to main?"); the question goes after five seconds unanswered.
- Once pushed, the session's pull request shows in its header ("#12 · Checks failing"), kept up to date after Git actions, after each turn and every two minutes ([0054](decisions/0054-project-defaults-prs-and-scripts.md)). Failing checks make the button **Fix checks** (the agent gets which failed and the log's end); a review asking for changes makes it **Address review**. Either is news (you're notified once), and a finished session in that state sits in **Needs you**, as [0036](decisions/0036-sessions-as-an-inbox.md) has a failed check, saying why in the list ("Checks failing · PR #12"). A merge is a label, "Merged · PR #12"; the session stays until you close it. When GitHub can't be asked (offline, signed out), what was last known stands.

## Project scripts

Each file in a repository's `.dent/actions/` is a script: its name is the file's, and its first comment says what it does (`# Run the tests`). The session header's **Run** menu lists them, and choosing one runs it in your terminal, in the session's folder (from elsewhere, in a subshell that goes there). A terminal just opened takes it once its shell has started; one with something running in front (a dev server, an editor) is left alone, and dent says so. Commit them, and every worktree has them.

## Defaults per project

A project can have its own defaults for new sessions (model, permissions, effort, new worktree or not) over its machine's, set on purpose from the composer's menus (**Only for _project_**) and kept on that machine ([0076](decisions/0076-defaults-on-machines.md), replacing 0054's remembering what the last session started with).
- While the agent's turn runs, the button is off: it may be halfway through a file.
- Where the branch stands is read with a fetch from `origin` at most every two minutes per repository, so a branch behind its remote offers Pull. The fetch never prompts: one that needs a password or passphrase is skipped.

## Project docs

The docs view for a project merges two sources:

1. **Markdown inside the project's repos and worktrees**, indexed by the daemons that host them and grouped by repo and by the session that last wrote each file.
2. **A project docs folder** for plans, notes and specs that belong to the project rather than one repo. It lives at \~/dent/projects/\<project>/docs on every machine hosting the project and is synced between them, so agents on any machine can read and write the same notes.

The docs folder is small markdown, synced per file with latest-edit-wins; a conflicting edit is kept as a sibling copy rather than lost. Every session's system prompt can point the agent at it, so agents share context across repos and machines.

## Environment and secrets

Secrets attach to what needs them, not to projects: a repo's test database URL is needed whichever project you touch it from. Three scopes resolve in order, the most specific winning.

| Scope | Typical contents | Example |
| --- | --- | --- |
| Account | Tokens you use everywhere | A GitHub token per host, a personal npm token |
| Repo | What the code needs to build and test | `DATABASE_URL`, test API keys |
| Checkout | Values that differ on one machine | The laptop's local database instead of the shared one |

- Secrets are encrypted shared state, so a repo's secrets reach any machine that checks it out, including a fresh EC2 box.
- They are injected into the agent's environment and never appear in event logs or backups in plain text.
- `.envrc` files load through direnv when present.
- An agent that needs a missing secret asks through an approval-style card; you paste it once and choose its scope.
