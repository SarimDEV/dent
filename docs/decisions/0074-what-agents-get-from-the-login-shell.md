# 0074. What agents get from the login shell

Status: accepted (2026-10-22) · Spec: [daemon.md](../daemon.md) · Related: [0065](0065-launcher-and-updates.md)

## Decision

- The daemon reads the login shell's environment as a terminal has it: the account's own shell (from the user database: a service has no `SHELL`), interactive and login, so what `.bashrc` or `.zshrc` sets counts. It reads `env` between markers, so banners a profile prints, and a shell like fish that keeps PATH as a list, don't get in.
- Agents always get PATH (each folder once) and, when the shell has them, locale, certificates and proxies (`LANG`, `LC_*`, `SSL_CERT_*`, `NODE_EXTRA_CA_CERTS`, `*_PROXY`), plus `SSH_AUTH_SOCK` and Homebrew's variables. This is the list T3 Code passes, without its desktop-session ones.
- **Under the service only**, they also get each agent's own credentials, named by its driver (`Driver::env_names`). For Claude Code that's `CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY`, gateway and Bedrock/Vertex settings.
- These go to agents through the holder's spawn, and to one-off agent commands (titles, side questions, status), never on a command line, where `ps` shows them to everyone on the machine. They're read afresh as each agent starts and never written to a file.

## Why

A headless box has no keychain. The usual way to sign Claude Code in there is `claude setup-token` with the token exported from a shell profile. The first run of the real service on the VM said "Not logged in" for exactly that reason. Passing credentials is what running `claude` over SSH would do.

The app doesn't pass them. On a Mac signed in through the keychain, an `ANTHROPIC_API_KEY` left in `.zshrc` would quietly move every session from the subscription to the API.

## Consequences

- A new agent driver lists its credential variables in its own folder, and nothing else changes.
- `dent-daemon doctor` (to come) shows where PATH came from (`AgentEnv::source`).
