# dent — fast-first workflow. See AGENTS.md.

default:
    @just --list

# Build and run a binary crate (default: desktop). Extra args go to the binary.
dev crate="dent-desktop" *args:
    cargo run -p {{crate}} -- {{args}}

# With no crates named: the ones this branch changed, and everything that
# depends on them (scripts/affected.py).
# Compile and run unit tests for the given crates (default: what changed).
check *crates:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z "{{crates}}" ]; then sel=$(python3 -I scripts/affected.py); else sel=$(printf -- '-p %s ' {{crates}}); fi
    if [ -z "$sel" ]; then echo "check: no crate changed since origin/main"; exit 0; fi
    cargo check $sel --all-targets
    cargo test $sel --quiet

# Covers the crates this branch changed against `base`, and everything that
# depends on them; spikes only when touched.
# Everything that must pass before merging, for what changed.
verify base="origin/main":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo fmt --all --check
    sel=$(python3 -I scripts/affected.py {{base}})
    if [ -n "$sel" ]; then
      cargo clippy $sel --all-targets -- -D warnings
      cargo test $sel --quiet
    else
      echo "verify: no crate changed since {{base}}; formatting only"
    fi
    scripts/check-boundaries.sh

# verify for every crate in crates/*, whatever changed.
verify-all:
    cargo fmt --all --check
    cargo clippy --all-targets -- -D warnings
    cargo test --quiet
    scripts/check-boundaries.sh

# verify for the spikes too (they're left out of the default build).
verify-spikes:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace --quiet

# Ambient backgrounds, on demand only (docs/ambient.md): every background at
# five sizes and three frame rates, with timings.
ambient-stress:
    cargo test -q -p dent-ambient --release -- --ignored --nocapture stress

# Ambient backgrounds in the real app: a still of each and its frame cost (held
# to 50 fps and 4 ms a frame in release). Needs WINID; pass --shots DIR, --dark.
# DENT_WALK_RELEASE=1 times the optimised build.
ambient-walk *args:
    scripts/walk.sh ambient {{args}}

# Regenerate Codex app-server types from the installed (pinned) codex. Needs cargo-typify.
gen-codex-types dir="spikes/codex-app-server":
    python3 -I scripts/gen-codex-types.py {{dir}}
    cargo fmt --all
