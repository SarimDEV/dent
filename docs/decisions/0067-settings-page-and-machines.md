# 0067. Settings is a page of its own; Machines shows every machine at a glance

Status: accepted (2026-10-20, owner's picks from the machines mockups) · Amends [0056](0056-settings-and-onboarding.md) · Plan: [remote-plan.md](../remote-plan.md), stage 1

## Decision

**Settings is a page.** ⌘, puts Settings in place of the sessions: its pages down the left (General, Agents, Machines) and **Sessions** to go back. Only pages with something on them are listed. The welcome stays a sheet over everything, as 0056 has it.

**Machines, top to bottom:**
- **The strip.** This Mac sits on the left. Each connected machine is placed by its real round trip on a log scale (1, 10, 100 ms), so closer means faster. Machines that aren't connected sit apart, under "Not connected". A live link carries a moving dot; an asleep machine's link is dashed in the waiting colour; a machine that can't be reached has no link.
- **The rows.** Each row has the machine's shape, its name, how it stands, and its sessions by status. A machine that's away has **Retry** in its row. Clicking a row opens its details: how it's trusted, its routes (the one in use marked), its dent and agent versions, and its key in groups of four. **Remove…** asks first, and says the machine's sessions stay on it.
- **Who can use this Mac.** Paired devices, each with **Remove…**, and the line saying your Tailscale devices connect with no code.
- **Pairing code** shows this Mac's code, with how long it lasts.

**Adding a machine.** The sheet lists this Mac's Tailscale account's own computers (from `tailscale status`; tagged devices and phones aren't offered), each with **Add**. Any other machine is added by its address. When the machine asks for a code, the sheet asks for it. The sheet ends on "<name> added", with its route and round trip.

**A shape per machine.** There are six solid shapes: circle, triangle, square, diamond, plus and star. Shapes are assigned from the machine ids in id order: each takes the shape its id hashes to, or the next one free. So up to six machines never share a shape, and devices with the same machines draw the same ones. State is never the shape's colour. It's a dot on the round holder (amber when asleep, grey when it can't be reached), and words beside a small shape.

**Working across machines.**
- **Starting work:** with more than one machine, the new-session tray starts with a machine chip that shows the machine's shape. Its menu lists this Mac, then each machine with what starting there would do now: "Online · 3 ms", or "Asleep · starts when it wakes". Picking a machine drops the project (projects are per machine) and asks that machine for its list. The words, model and settings stay. Folders, new and cloned projects, branches and @ files are all asked of that machine. A session started with no project gets that machine's own scratch folder: `Create` with an empty `cwd`.
- **A session on an away machine:** under the header it says "mac-mini is asleep", what that means for the session, and **Retry**. A working session there reads as waiting, never working. The composer says what's typed waits for the machine.
- **Messages for an away machine** ([0063](0063-queued-prompts.md)) are kept on this device in `outbox.json` and shown as the session's last message: "Waiting for mac-mini · sends when it's back", with Edit and Remove. Both act on this device, and nothing is sent. They go once, by their own ids, when the machine is back, and they survive a quit. One written more than 12 hours earlier in an earlier run says "Written 14 h ago · sends when you say", with **Send now**.

**Fixed words.** Online, Asleep, Can't reach, Connecting…, Tailscale, LAN, This Mac. With more than one machine, each inbox row names its machine after the project, and the status bar names only the machines that need a look, then how many there are.

**Nothing that doesn't work yet is shown.** Wake (it needs Wake-on-LAN), a per-device LAN switch and hiding a machine from the inbox are left out until they exist.

## Why

The owner picked the direction from the mockups (2026-10-20): Settings as its own page, a graph of machines at the top, expandable rows, a shape per machine, one inbox, the machine chip in the tray, and easy adding. A review of the mockups for consistency set the scales, the word list, and the rule that state is shown by a dot or words, never by the shape.

## Consequences

- The walkthroughs: `settings` steps through the pages; `machines` makes machines up to check the strip, the rows and the sheet; `machines-live` runs against the machines really saved and isn't in `checks`.
- Opening Settings lets go of the field that had the keyboard, so Esc and the page's switches reach the page.
- Motion comes from the vocabulary in [design-system.md](../design-system.md#the-vocabulary): rows Turn and their details Pop in; the sheet Pops in over a fading dim and each of its steps Arrives; the strip's machines Arrive staggered and Glide as round trips change; live links Pulse. Pulse repaints while the page is in front with a machine connected, the one cost exception besides the ambient backgrounds.
