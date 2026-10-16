# 0047. Work recedes while the agent works; the answer stands out

Status: accepted (2026-10-15) · Spec: [desktop.md](../desktop.md), [chat-rendering.md](../chat-rendering.md), [design-system.md](../design-system.md)

## Decision

- A command reads as what it is for: the agent's own description, in the tense of the moment ("Running the tests", "Ran the tests", "Failed to run the tests"). The command shows when its line is opened; a command with no description reads "Ran a command".
- The working line is the step and the time, the clock running from the start; "Working…" stands in only while there are no words.
- Narration (everything said on the way to an answer) sits in the secondary ink; a turn's last reply, once the turn ends, reads in full ink.
- A paragraph hangs close over the work it led to, with a wider break after it, so each step reads as one unit.
- Running text stops at a 540 pt measure (about 78 characters); tables, code and pictures keep the 720 pt column.
- Inline code sits a step quieter than its text.
- Group lines name one or two changed files and end with a diffstat in the line's own inks; a lone subagent is named by its task.
- Allowing once leaves no note; lasting allows and denials do.
- Approvals lead with the agent's reason.
- dent gives each Claude Code session a plan tool of its own, `update_plan`: a few lines of MCP served by `dent-daemon plan-tool` (no new dependency), passed with `--mcp-config` and allowed up front. The session prompt asks the agent to keep its plan there and to describe every command. The plan shows open, as a checklist card above the composer, while the agent works; Claude Code's own todo list, where a version still has one, reads the same way.

## Why

Set beside another agent app on the same turn, dent read as busy while it was the calmer design: shell incantations in grey on every other row, every paragraph of narration at the answer's weight, lines of about 100 characters, and an even 8 pt rhythm that never grouped anything. The words were the same; the presentation made them louder. These changes make the work recede and keep dent's own voice (serif reading column, pixel squares, Field Notes inks) for the answer.

## Consequences

- Tool lines depend on the agent describing its commands; the session prompt asks for it, and the fallback is a plain "Ran a command".
- Current Claude Code has no built-in todo tool, so without dent's plan tool the plan card never appeared. The plan tool's calls are hidden from the tool rows.
- Diffstats use no red or green.
