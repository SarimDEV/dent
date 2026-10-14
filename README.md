# dent

A fast, chat-first app for driving coding agents on any machine you own, from a native desktop app or your iPhone. Agents run under a daemon on each machine, so sessions survive closed lids, dropped connections and restarts.

Early days: Phase 2 (the local core). The macOS desktop app runs Claude Code sessions on this machine: replies stream in, approvals and questions are answered in place, messages queue or steer, Stop stops, sessions survive the app quitting and the daemon restarting, and what the agent makes opens beside the chat. See [docs/spec.md](docs/spec.md) for the spec and [AGENTS.md](AGENTS.md) for working in the repo.

## Try it (macOS)

```sh
scripts/bundle.sh --install      # builds dent.app with its daemon and installs it in ~/Applications
open ~/Applications/dent.app
```

dent drives the `claude` you already have, signed in as you; it uses your Claude limits the way the terminal does. To try it without touching an account, `scripts/walk.sh checks` runs the app against a stand-in agent in a scratch home.

## License

MIT, except `crates/hub`, which is AGPL-3.0.
