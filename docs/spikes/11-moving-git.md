# Spike 11: moving a worktree's git state to another machine

Date: 2026-10-20 · git on studio (macOS 26.5) and dent-linux (Ubuntu 24.04) · Tests [R-D2, R-D4](../remote-tests.md#d-moving-sessions) for [0064](../decisions/0064-moving-sessions.md) · Code: [spikes/move-git](../../spikes/move-git)

Question: can a session's worktree, with commits that were never pushed and every kind of uncommitted change, be moved to another machine exactly, without touching the person's own index? And are the dangerous cases refused before anything changes?

**Answer: yes, and better than the plan assumed.** A second commit holding the index carries the staged/unstaged split too, so nothing is lost.

## How it works

**Pack, on the source:**
- **Two commits on top of `HEAD`:**
  - *Working tree:* built in a copy of the index, with `add --all` (untracked files in, ignored files out unless already in the index).
  - *Index:* `write-tree` of the real index.
- **The person's own index is safe:** it's only read. Split index and fsmonitor are off for these commands. The copy's timestamp is reset, so same-size edits made in the same second are re-read.
- **The bundle** holds both commits and the branch, minus everything the target says it already has.

**Apply, on the target:**
1. Check the hash, the bundle's heads and `bundle verify`. If base commits are missing, fetch origin with prompts off, then verify again.
2. Move the branch, fast-forward only, with a compare-and-swap.
3. Add a worktree.
4. `checkout --no-overlay <working tree> -- .`, which carries deletions.
5. `read-tree -m <index tree>`.

Every early exit, a refusal included, undoes what the run made.

## Results

The source worktree had:
- two commits that were never pushed (one with a binary);
- a file staged and then edited again;
- an unstaged edit;
- a staged rename;
- a mode change;
- an unstaged deletion and a staged deletion;
- untracked files in a new folder;
- an ignored `.env` force-added and then edited;
- an untracked binary;
- an ignored `node_modules`.

| Check | Result |
| --- | --- |
| `git status` on the target vs the source | Identical, all 10 lines, including `MM`, `AM`, `R`, staged and unstaged deletions |
| File contents and executable bits (SHA-256 of every file) | Identical |
| `git diff` and `git diff --cached` | Identical |
| Ignored `node_modules` | Stayed behind, as intended |
| Bundle size | 6.5 KB, only the commits the target lacked |
| Branch checked out elsewhere at the same commit | Detached worktree at that commit |
| Older branch on the target | Fast-forwarded to the source's tip |

| Refusal | Result |
| --- | --- |
| Target branch has its own commits (diverged) | Refused, branch untouched, no worktree |
| Branch checked out elsewhere at another commit | Refused, no worktree |
| Truncated bundle | "The bundle is damaged" |
| Bundle bytes changed | "Doesn't match its hash" |
| Nested git checkout in the worktree | Refused on the source before packing |
| New worktree folder already exists | Refused. The branch it had just created is deleted again, no temporary refs are left, and the folder is untouched. A clean retry succeeds |

The rollback-on-refusal bug was found and fixed in this spike: the first version only rolled back on a failed command, not on an explicit refusal.

## What changes in the plan

- **The index travels too.** [0064](../decisions/0064-moving-sessions.md) said the staged/unstaged split would be lost; it isn't. Remove that from "what doesn't move".
- **The bundle uses the target's own list of tips.** That is more exact than "minus what origin has", and covers commits pushed only to a fork.

## Not covered yet

- Base commits missing on the target and fetched from a shared origin. This rig has no origin both machines can reach without the owner's GitHub; it's covered when moves are built.
- Dirty submodules (the check exists; no submodule fixture yet).
- A destination `post-checkout` hook that writes files.
- Moves over 200 MiB.
