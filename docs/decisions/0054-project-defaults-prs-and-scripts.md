# 0054. Each project keeps its own defaults; a session shows its pull request; projects have scripts

Status: accepted (2026-10-19) · Builds on [0053](0053-projects-are-repos.md) · Spec: [projects.md](../projects.md), [desktop.md](../desktop.md#new-session)

## Decision

**Per-project defaults.** Each project remembers what its last new session started with: model, permissions, effort, and new worktree or not. Choosing a project on the new-session page brings those back. A project never started in keeps whatever is chosen, and "No project" is a project for this. There is no settings screen: starting a session is what a project remembers.

**A session's pull request in the chat.**
- **The badge.** Once a session's branch is pushed and has a pull request, its header shows it ("#12 · Checks failing", "#12 · Merged"), coloured by what it needs, and a click opens it on GitHub.
- **Keeping it current.** The daemon reads it with the GitHub CLI after Git actions, at the end of each turn, and every two minutes for open sessions. Only a pushed branch is ever asked about.
- **Briefing the agent.** When checks fail, the Git button offers **Fix checks**: it sends the agent which checks failed and the end of the first failure's log. When a review asks for changes, it offers **Address review**: the review and its comments on lines. Either way it is your message, sent as any other.
- **Telling you.** Checks starting to fail, changes asked for, and a merge count as news: the session is marked unread and you're notified once, unless you're looking at it. A finished session whose checks fail or whose review asks for changes is in **Needs you**, as 0036 lists a failed check. A failed attempt to ask GitHub never reads as the pull request having gone.
- **A merge.** A merged pull request is a label ("Merged · PR #12" in the list), nothing more. The session stays in the inbox until you close it, and its worktree stays until you remove it.

**Project scripts.** Each file in a repository's `.dent/actions/` is a script, named by its file and described by its first comment (`# Run the tests`). The session header's **Run** menu lists them, and choosing one runs it in your terminal in the session's folder. Commit them and every worktree has them, beside `.dent/setup`.

## Why

The owner's call (2026-10-19), from the audit of what a single machine's chat was missing:
- A task's story used to stop at "PR opened": the rest happened on GitHub.
- One global "what you used last" fits no one who works in more than one kind of project.
- Commands you run constantly shouldn't cost an agent turn.

On a merge the owner chose to label, not close. A session leaving the inbox by itself is surprising, and closing it on merge (as a reference app does by default) stays an option to try later.

## Consequences

- The PR watch costs one GitHub CLI call per open, pushed session every two minutes. Nothing runs for sessions that never pushed.
- A brief can be long (a log's last 80 lines). It is sent as your message, so it shows, and you can see what the agent was told.
- Not yet: closing a session on merge (to try later), editing scripts from dent, and a project's defaults for scripts' arguments.
