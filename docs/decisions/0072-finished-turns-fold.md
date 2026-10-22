# 0072. A finished turn's work folds into one line

Status: accepted (2026-10-22, owner: "all that T3 Code does") · Spec: [desktop.md](../desktop.md)

## Decision

- When a turn ends, its narration, tool groups, thoughts and small notes before the answer fold into "Worked for 2m 14s ›", placed where the work began. The answer, pages, cards (approvals, questions, plans, secrets), errors and your messages stay where they are.
- Only a turn with a tool call or a thought folds; a turn still running never does.
- Opening or closing keeps what's above in place (the chat stops following its end for that toggle), as T3 does, so the work unfolds below the line instead of pushing it out of view.
- On by default; Settings turns it off (stored as `show_all_work`, so a fresh install folds).

## Why

A finished turn's work is mostly done with: the answer is what you came back for. Folding it makes a long session read as questions and answers, with the work one click away. T3 Code does this, and the owner asked for its chat behaviour.

## Consequences

- Each item's place in its fold is part of its row signature, so rows re-measure exactly when a turn folds or opens.
- Walks start unfolded so their checks keep reaching tool rows; the `fold` scene checks folding itself.
