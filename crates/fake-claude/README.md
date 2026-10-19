# dent-fake-claude

Stands in for `claude -p` stream-json in tests and demos, so nothing touches a real account. Words in a message pick what it does: `run`, `ask`, `write <path>`, `background`, `subagent`, `long`, `fail`, `crash`. `replay a real turn` plays back one turn recorded from the real agent (`assets/real-turn.jsonl`), for the `look` walkthrough. `FAKE_CLAUDE_SPEED` scales its pauses (0 is instant).
