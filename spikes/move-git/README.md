# spike move-git

Remote plan stage 0, tests R-D2 and R-D4 ([docs/remote-tests.md](../../docs/remote-tests.md)). `move.sh pack` turns a worktree into two commits on top of HEAD (the working tree, and the index) inside a bundle. `move.sh apply` lays them down in a new worktree on another machine, with every refusal and rollback the move needs. Findings: [spike 11](../../docs/spikes/11-moving-git.md).
