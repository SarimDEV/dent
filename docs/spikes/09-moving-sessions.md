# Spike 09: does a Claude session resume on another machine?

Date: 2026-10-20 · Claude Code 2.1.301 (studio, macOS 26.5) and 2.1.305 (dent-linux, Ubuntu 24.04 VM), Haiku · Test [R-D1](../remote-tests.md#d-moving-sessions) for [0064](../decisions/0064-moving-sessions.md)

Question: if dent copies a Claude Code transcript to another machine and runs `--resume` there, does the agent carry on with its full context? This includes tool results and subagent reports, across a different OS, a different path and a different Claude Code version, in both directions.

**Answer: yes.** It also showed one real hazard, which a first message fixes.

## What was done

1. **On studio:** a session in a scratch repo (`/Users/sarimdev/dent/scratch/move-test`). The agent read a file holding a code word, ran `wc -l`, fixed a bug with Edit, and had a subagent count characters. The transcript is 61 lines (243 KB) plus `<session>/subagents/` (the subagent's own transcript and metadata). It names the Mac path in 36 `cwd` fields and 14 tool `file_path`s.
2. **To dent-linux:** the repo and the transcript, under the folder Claude Code uses for the new path (`~/.claude/projects/-home-sarimdev-guest-work-move-test/`), then `claude --resume <id>` from `/home/sarimdev.guest/work/move-test`.
3. **The reverse:** a session made on dent-linux (2.1.305), with a subagent, resumed on studio (2.1.301) under a Mac path.

## Results

| Check | Result |
| --- | --- |
| Mac → Linux, newer CLI: recall a file's contents, a command's output and a subagent's report, without tools | All three correct, from the transcript alone |
| Linux → Mac, older CLI | All correct (the older CLI read entries written by the newer one) |
| The agent's sense of place | Noticed on its own that the environment changed from macOS to Linux and the path from `/Users/…` to `/home/…` |
| Edit after the move, with no notice | **First tried the old Mac path.** It failed only because `/Users` can't be created on Linux, then it used the new path |
| Edit after the move, with a first message naming the old and new folders | Every edit went to the new path; none to the old one |
| Transcript without the `subagents/` folder | Still resumed, with the subagent's report recalled (the report sits in the main transcript) |
| Resume from a folder other than the one the transcript is under | Worked: this CLI version finds a session by id across all project folders |
| Transcript re-keyed to a new session id (the id replaced throughout the file) | Resumed under the new id with full context |
| Version stamps after resuming | Old lines keep the writer's version, new lines the reader's (40 × 2.1.301, then 44 × 2.1.305) |

## What changes in the plan

- **The move notice is required, not optional.** Without it, an agent that moved between machines where the old path also exists (Mac to Mac, or the same user on two Linux boxes) could edit the wrong checkout. dent's first message after a move always names the old and new roots and says the old ones don't exist here. A test for this is added to R-D5: move between two folders that both exist on one machine, and check that nothing touches the old one.
- **The target can give Claude's session a new id too,** by rewriting the transcript. Then the two machines never hold the same Claude session id, which matches dent's own new-id rule in [0064](../decisions/0064-moving-sessions.md) and rules out an accidental resume of the stale copy on the source.
- **Side files are nice to have.** The subagent folder lets the target show and continue subagents in detail, but resume doesn't depend on it. Copy it when present; its absence downgrades, it doesn't fail.
- **Version checks can be a range, not "target ≥ source".** Older-from-newer worked across 2.1.301 and 2.1.305. dent keeps a tested range per driver and warns outside it, rather than refusing.
- **Placing the transcript under the target's folder is still right.** It's where the CLI writes new turns, and future versions may stop searching other folders, but it isn't load-bearing today.

## Not covered yet

- A different Claude account on the target: prompt cache, model availability, and whether signed thinking blocks validate.
- A long session (thousands of lines, compactions, plans, a file-history folder).
- Background tasks and monitors that were still running when the move started. The plan quiesces first, so this should be refused rather than tested.
