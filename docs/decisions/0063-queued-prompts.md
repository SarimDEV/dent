# 0063. A prompt for an away machine waits on the device and sends once when it's back

Status: accepted (2026-10-20, owner's call) · Supersedes "a prompt for a machine that's away is refused on the spot" in [hub.md](../hub.md) and [spike 05](../spikes/05-direct-links.md) · Plan: [remote-plan.md](../remote-plan.md)

## Decision

- **What waits:** a prompt for a session on a machine that is asleep, offline, reconnecting, restarting or needs an update. The same goes for a new session started there. It is kept in the client core's outbox and saved to disk, so it survives the app quitting.
- **How it shows:** as the session's last message, marked "Waiting for <machine> · sends when it's back", with Edit and Cancel. The inbox row says the same. Attachments wait on the device with it.
- **Sending:** exactly once, by its client-generated id, when the machine is back and caught up. The daemon ignores a repeated id and answers with the original result. Prompts for one session keep their order.
- **Old prompts ask first:** one written more than 12 hours earlier asks "Written 14 h ago. Send now?" before going.
- **What is refused:** a prompt for a machine that removed this device, or isn't trusted on any route. The reason is shown, and the draft is kept.
- **Only on this device:** the waiting prompt lives on the device that wrote it, because no state is shared ([0058](0058-independent-machines.md)). Other devices see it once it has been sent.

## Why

The owner's call (2026-10-20): queue it, while being clear about what's happening. A refusal makes you remember to come back. A silent queue surprises you hours later. A visible waiting message with Edit and Cancel avoids both. The client core already keeps unacknowledged prompts and resends them by id ([spike 07](../spikes/07-client-core.md)), so this mostly makes that visible and durable.

## Consequences

- Per-machine state never waits on other machines: one away machine never holds back prompts for a machine that's online.
- A waiting prompt pins its draft to its machine; moving it to another machine is an explicit Edit.
- Tested in [R-C2](../remote-tests.md).
