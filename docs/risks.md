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
| The web client is a third UI | More work, and drift from the desktop | Shares the client core; scoped to chat, approvals, docs and diffs |

**Decided**

- **Name:** dent, at dent.sh.
- **Sign-in:** GitHub, Google, and email and password at launch; providers are pluggable, and single sign-on for company teams comes later.
- **Hub pricing:** free for up to 3 machines, then a subscription.
- **Project docs folder:** \~/dent/projects/\<project>/docs on every machine hosting the project, synced between them.
- **Multiple accounts per agent:** yes, with limits shown per account.
- **Connectivity:** LAN and Tailscale, direct and encrypted end to end, with code pairing on the LAN and automatic trust on your own Tailscale account; a relay comes later, free for up to 3 devices ([0033](decisions/0033-direct-websocket-hub-on-hold.md), [spike 05](spikes/05-direct-links.md)).
- **License:** MIT for the daemon, session holder, client core, desktop, iPhone and web apps and the CLI; AGPL-3.0 for the hub, so anyone running a modified hosted copy must publish their changes.

**Open questions**

- **Hub price** above the free tier: deferred until closer to launch.
