# Projects

A project is a piece of work you are doing, like "AuthZ refactor" or a random idea you had this afternoon. It is not a repo: one project can touch several repos, and one repo can belong to several projects. Repos are registered once on their own, so their checkouts and secrets are shared by every project that uses them.

## Model

| Entity | What it is | Fields |
| --- | --- | --- |
| Repo | A codebase, identified by its remote URL | name, remote, secrets, default branch |
| Checkout | A repo on a particular machine | repo, machine, path, worktrees |
| Project | A piece of work | name, colour, repos it touches, docs folder, default agent and model, archived |
| Session | One conversation with one agent | project, machine, checkouts it works in, agent, title, status |

- A session runs on one machine but can work across several checkouts there, passed to the agent as additional working directories.
- Picking a repo for a session on a machine without a checkout offers to clone it there.
- Sessions can move between projects; a session started outside any project lands in an Inbox.

## Starting from an idea

A new idea should cost one keystroke, not a setup screen.

- Cmd/Ctrl+N in the Inbox, or "New project" with just a name, starts a session immediately on your default machine.
- If the idea has no repo yet, the session gets a fresh scratch folder with `git init`, and the agent can create a remote later.
- Promoting a scratch folder to a repo or adding existing repos happens whenever you want, without moving the session.
- After your first message, a suggestion offers the project the session most likely belongs to, or a new project when it looks like a fresh idea. One key accepts; ignoring it leaves the session in the Inbox.

## Worktrees

Agents create worktrees themselves, so dent detects rather than manages them.

- The daemon watches `git worktree list` for each workspace and shows worktrees under their repo, linked to the session that created them.
- Diffs, staging and docs work the same in a worktree as in the main checkout.
- A "new session in a fresh worktree" option exists for when you want it, and finished worktrees can be cleaned up from the UI.

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
