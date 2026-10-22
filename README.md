# dent

A fast, chat-first app for driving coding agents on any machine you own, from a native desktop app or your iPhone. Agents run under a daemon on each machine, so sessions survive closed lids, dropped connections and restarts.

Early days. The macOS desktop app runs Claude Code sessions: replies stream in, approvals and questions are answered in place, messages queue or steer, Stop stops, sessions survive the app quitting and the daemon restarting, and what the agent makes opens beside the chat. Charts, tables and mock-ups the agent shows appear live in the conversation itself, in dent's colours, and the agent can look at a page before showing it. A finished turn's work folds into one line above its answer.

It drives other machines too: pair a Mac or Linux box once with a code, then its sessions sit beside this Mac's. The app shows plainly when a machine is asleep, unreachable or has removed you, wakes it over the network where it can, keeps messages until it's back, opens the files its agents make, and can bring a project from this Mac to it. See [docs/spec.md](docs/spec.md) for the spec and [AGENTS.md](AGENTS.md) for working in the repo.

## Try it (macOS)

```sh
scripts/bundle.sh --install      # builds dent.app with its daemon and installs it in ~/Applications
open ~/Applications/dent.app
```

dent drives the `claude` you already have, signed in as you; it uses your Claude limits the way the terminal does. To try it without touching an account, `scripts/walk.sh checks` runs the app against a stand-in agent in a scratch home.

### Another machine

Installing on other machines is still to come ([#16](https://github.com/SarimDEV/dent/issues/16)); for now, build the daemon there from source and run it:

```sh
cargo build --release -p dent-daemon
target/release/dent-daemon &         # listens on port 7480, over Tailscale or loopback
target/release/dent-daemon pair      # shows a pairing code
```

Then in the app, Settings › Machines › Add machine, with its address and that code. Devices on the same network (not Tailscale) need `DENT_REMOTE_LAN=1` on the daemon.

## License

MIT, except `crates/hub`, which is AGPL-3.0.
