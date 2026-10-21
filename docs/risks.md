# Risks and open questions

The biggest risk is Anthropic's terms for third-party apps that use subscription logins (see Durability). The usage-limit risk is now mostly resolved by the stream's rate-limit events.

| Risk | Impact | Mitigation |
| --- | --- | --- |
| ~~Claude limits only reach status line scripts~~ Resolved in [spike 01](spikes/01-claude-code-stream-json.md): headless stream-json emits `rate_limit_event` with five-hour and seven-day utilisation and reset times | — | Keep the last value per account; show "unknown" until the first event |
| Agent protocols change: Codex app-server is documented as unstable, Claude Code's stream format moves too | Broken sessions after an agent update | Pinned agent versions per daemon release, contract tests, generated Codex types |
| Provider terms for driving subscription CLIs from a third-party app | Public release could conflict with a provider's terms | Drive only the official CLIs with their own login; review each provider's terms before going public |
| GPUI is pre-1.0 with thin docs; Linux and Windows support is newer than macOS | Slower UI work, upstream breakage | Pin a Zed commit, keep UI code separate from the client core, spike the hardest views (chat list, diff) in phase 0 |
| iOS cannot hold background connections | No notifications without a hosted relay; requires an Apple developer account | Hub push from the start of the iPhone phase |
| The daemon is remote code execution by design | A stolen account or device controls every machine | New devices need approval on an existing device, two-factor sign-in, instant revocation, keys in the OS keychain |
| ~~Mermaid and math have no mature pure-Rust renderers~~ Resolved in [spike 04](spikes/04-markdown-math-mermaid.md): RaTeX and merman cover the fixtures and render in milliseconds, natively | — | Cache by content hash; web-view fallbacks stay named in Durability |
| Hub outage | Sign-in and new devices stop; relayed connections drop | Known devices connect directly from cache; the hub runs in two regions with a status page |
| Relay bandwidth cost | Thin or negative margins on the paid hub | Direct connections first, relay only on failure, relay usage measured per account from day one |
| Lost encryption keys | Locked out of synced state and backups | Recovery key at first sign-in; approval from any existing device |
| macOS asks before a launchd program reads Documents, Desktop or Downloads ([spike 06](spikes/06-session-holder.md)) | Agents can't work in projects there, or the person sees unexplained prompts | Install outside those folders; setup explains the prompt (or Full Disk Access) before the first agent works there; `dent doctor` checks it |
| dent-push is only tested against a stand-in for Apple ([spike 07](spikes/07-client-core.md)): HTTP/2 from Workers to APNs is unverified, and the simulator doesn't run the notification extension for simulated pushes | Notifications might need a small server instead of a Worker; decryption on the phone untested on a real push | Deploy a test Worker and send real sandbox pushes once there's an Apple developer account; a tiny VM is the fallback |
| One WebSocket message per event is slow on iOS (about 26 µs each in URLSession, [spike 07](spikes/07-client-core.md)) | A 100,000-event catch-up takes 3 s on a phone instead of under 1 | Batch events into messages of up to about 64 KB while catching up |
| Linux and Windows desktop are untested in daily use ([spike 08](spikes/08-desktop-concepts.md)): Wayland pages open in their own window, X11 needs a handle conversion, Windows has only type-checked | Rough edges for the first non-Mac users | macOS first; Linux and Windows after the Mac release, with the container checks kept running |
| The event log grows faster now that tool output is kept whole up to 32 KB (8 KB head and 24 KB tail past that; a subagent's report up to 64 KB), where it used to stop at 4 KB, and web fetch and search results are logged with their calls | A session full of long outputs is several MB of SQLite and of catch-up on a phone: 32 KB of coloured build output measures 39.5 KB once JSON-escaped (each ESC becomes `\u001b`), so 200 such calls are about 8 MB. The local log measured on 16 Oct had 817 events in 179 KB, with no heavy session yet | Measure a real heavy session before the iPhone catch-up work; if it bites, keep the large body out of the event row (a side table fetched on Show all) or compress it. Catch-up batches still return at least one event however large |
| The web client is a third UI | More work, and drift from the desktop | Shares the client core; scoped to chat, approvals, docs and diffs |
| A Claude transcript may not resume on another machine or another Claude Code version (absolute paths, side files, cwd-keyed folders) | Moves lose the agent's context | R-D1 resumed across both machines and versions in both directions ([spike 09](spikes/09-moving-sessions.md)); a different account and long sessions are untested. Fall back to a summary built from dent's own event log ([0064](decisions/0064-moving-sessions.md)) |
| Tailscale's LocalAPI is reachable differently on each build; the App Store build's sandbox may block it | No same-account trust on that machine | Pairing with a code always works; `dent doctor` names the build ([0061](decisions/0061-trust.md)) |
| A macOS LaunchAgent stops at logout | A Mac server that rebooted with nobody logged in runs no daemon | Say so in onboarding and `dent doctor`; point Mac servers at automatic login ([0065](decisions/0065-launcher-and-updates.md)) |
| Keep-awake can't hold a closed lid on battery | A laptop closed on battery sleeps mid-task | Say so beside the setting; the machine announces sleep and its sessions read as paused |
| On iPhone, URLSession blocks unencrypted WebSockets to Tailscale's 100.x addresses, and redialling an offline tailnet peer can stall the phone's Tailscale tunnel | Phone can't reach machines over Tailscale, or breaks Tailscale for other apps | Network.framework transport; check the netmap and back off offline peers ([0062](decisions/0062-liveness-and-retry.md)) |
| Copied state folders (Migration Assistant, restored backups, cloned VMs) duplicate a machine's identity | Two machines answer as one | Bind the key to the install; clients flag one key at two addresses ([0059](decisions/0059-machine-identity.md)) |

**Decided**

- **Name:** dent, at dent.sh.
- **Sign-in:** GitHub, Google, and email and password at launch; providers are pluggable, and single sign-on for company teams comes later.
- **Hub pricing:** free for up to 3 machines, then a subscription.
- **Project docs folder:** \~/dent/projects/\<project>/docs on every machine hosting the project, synced between them.
- **Multiple accounts per agent:** yes, with limits shown per account.
- **Machines share no state** ([0058](decisions/0058-independent-machines.md)); prompts for an away machine wait on the device ([0063](decisions/0063-queued-prompts.md)).
- **Connectivity:** LAN and Tailscale, direct and encrypted end to end, with code pairing on the LAN and automatic trust on your own Tailscale account; a relay comes later, free for up to 3 devices ([0033](decisions/0033-direct-websocket-hub-on-hold.md), [spike 05](spikes/05-direct-links.md)).
- **Phone notifications:** dent-push, a stateless Cloudflare Worker separate from the hub, forwarding end-to-end encrypted notifications. Free with fair-use limits per phone; hitting a limit is always shown, never silent ([0035](decisions/0035-dent-push.md)).
- **License:** MIT for the daemon, session holder, client core, desktop, iPhone and web apps and the CLI; AGPL-3.0 for the hub, so anyone running a modified hosted copy must publish their changes.

**Open questions**

- **Hub price** above the free tier: deferred until closer to launch.
