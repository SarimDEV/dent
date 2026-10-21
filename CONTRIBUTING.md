# Contributing to dent

Thanks for taking a look. dent is early, so the most useful things right now are bug reports, rough edges you hit while using it, and small focused pull requests.

## Before you start

- **Bugs and ideas:** open an [issue](https://github.com/SarimDEV/dent/issues/new/choose). For anything bigger than a small fix, open an issue first so we can agree on the approach before you spend time on it.
- **Security problems:** don't open a public issue. See [SECURITY.md](SECURITY.md).
- **Design decisions** live in [docs/decisions/](docs/decisions). Please don't reopen a settled decision in a PR; open an issue if you think one should change.

## Setting up

You need [Rust](https://rustup.rs) (stable) and [just](https://github.com/casey/just). The desktop app currently builds on macOS.

```sh
git clone https://github.com/SarimDEV/dent && cd dent
just dev                          # build and run the desktop app
just check                        # cargo check and unit tests for what you changed
just verify                       # everything that must pass before merging
```

`scripts/walk.sh checks` drives the real app against a stand-in agent (`crates/fake-claude`) in a scratch home, so you can test the UI without a Claude account. See [AGENTS.md](AGENTS.md) for the crate map, the commands and the conventions. It's written for coding agents, but it's the best guide for people too.

## Pull requests

- Keep each PR to one change, with a clear title and a short description of what and why.
- `just verify` must pass: formatting, clippy with warnings as errors, tests and the crate-boundary check. CI runs the same.
- **UI changes:** include a screenshot, before and after if something changed.
- **Behaviour changes:** update the matching file in [docs/](docs/README.md) in the same PR.
- **New dependencies:** add a short decision record in `docs/decisions/` saying why.
- The name is always lowercase: **dent**.

## Licensing

By contributing you agree that your work is licensed under the [MIT License](LICENSE), or under the [AGPL-3.0](crates/hub/LICENSE) for changes inside `crates/hub`.
