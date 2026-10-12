# dent — fast-first workflow. See AGENTS.md.

default:
    @just --list

# Build and run a binary crate (default: desktop). Extra args go to the binary.
dev crate="dent-desktop" *args:
    cargo run -p {{crate}} -- {{args}}

# Compile and run unit tests for the given crates (default: whole workspace).
check *crates:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z "{{crates}}" ]; then sel="--workspace"; else sel=$(printf -- '-p %s ' {{crates}}); fi
    cargo check $sel --all-targets
    cargo test $sel --quiet

# Everything that must pass before merging.
verify:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace --quiet
    scripts/check-boundaries.sh

# Regenerate Codex app-server types from the installed (pinned) codex. Needs cargo-typify.
gen-codex-types dir="spikes/codex-app-server":
    python3 -I scripts/gen-codex-types.py {{dir}}
    cargo fmt --all
