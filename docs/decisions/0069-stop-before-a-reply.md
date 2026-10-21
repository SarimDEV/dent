# 0069. Stopped before any reply, a message goes back to the box and out of the conversation

Status: accepted (2026-10-21, owner's report) · Spec: [desktop.md](../desktop.md), [daemon.md](../daemon.md)

## Decision

- **When:** you stop a turn (Stop, Esc, ⌘.) and nothing has come back yet: no words, no tool call, no question or approval, no slash-command answer. Thinking doesn't count; it's never shown as a reply.
- **What happens:** the daemon logs `Withdrawn { ids }` with the turn's messages, then the turn's end. Clients drop those messages from the chat, with no "Stopped" note and nothing unread. The session's status goes back to what it was before. The device that pressed Stop puts the messages back in its reply box, files and folded pastes included, *before* anything typed since (what you type next carries them on). Waiting messages come back after it, as before.
- **The agent forgets them too:** its next process starts from the last reply before the first withdrawn message (`--resume … --resume-session-at <reply> --fork-session`), or as a new conversation if nothing had been said. A pending go-back always starts a new process for the next message (`hand_over`), so a restart in between, with the old agent still in the holder, can't send it the old conversation.
- **When it stays sent (today's behaviour):** background work is running in the agent (a new process would end it); a message is a slash command (it may have done its work already); or the log can't say exactly where the conversation stood: the reply before was cut short and has no id, or an earlier message never got a reply. Going back to an earlier reply would then lose more than was stopped.

## Why

Claude Code does this ("Interrupting a prompt before any response now automatically restores your input so you can edit and resubmit"), and so does Claude's desktop app. The owner pressed Esc before Claude answered and typed more, expecting one message. dent kept the message in the chat and the agent's conversation, emptied the box, and marked the session Stopped. The model saw it twice once the extended version was sent.

## Consequences

- One mechanism for "a message coming back": the desktop's take-back (waiting messages on Stop, Edit, Send now) also handles withdrawn ones, flagged `sent`. Only `Withdrawn` brings them back; if a reply wins the race, the turn's end drops them quietly.
- Everything that reads the log skips withdrawn messages: rebuild after a restart, forks (and their starting point), titles, and the exact go-back point (`conversation_at`), which also now accounts for rewinds and forks when rewinding.
- Nothing is steered into a turn being stopped: a Send now during the stop waits for the turn to end, so a take-back never includes a message the person didn't stop.
- A go-back point is used up only once an agent has started from it; a start that fails, or a retry after an unreachable agent, starts from the same point.
- A message that waited in the queue entered the conversation where it was handed over, not where it was queued: rewinds, forks and take-backs find it there (before, rewinding to one could start a new conversation).
- Forks leave out a turn taken back whole (its thinking and end too).
- Every message sent from the desktop is remembered as it was in the box (not only ones with folded pastes), so one stopped before the daemon has echoed it comes back too.
- Clients that don't send Stop themselves (the CLI, or another device) see the message leave; it comes back only on the device that pressed Stop.
