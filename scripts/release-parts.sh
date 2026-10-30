#!/usr/bin/env bash
# Build this machine's parts of a dent release into OUT (decision 0077):
#   daemon-<os>-<arch>.tar.gz   dent-daemon, dent-session-holder, dent-launcher, dent
#   app-macos-aarch64.tar.gz    dent.app (macOS only)
# CI sets DENT_MAIN_BUILD and DENT_COMMIT, which the binaries keep; with them
# a build updates itself. DENT_SIGN picks the macOS signing identity
# (scripts/bundle.sh).
set -euo pipefail
cd "$(dirname "$0")/.."
out=${1:?usage: release-parts.sh OUT}
mkdir -p "$out"
out=$(cd "$out" && pwd)
os=$(uname -s | tr '[:upper:]' '[:lower:]'); [[ $os == darwin ]] && os=macos
arch=$(uname -m); [[ $arch == arm64 ]] && arch=aarch64
cargo build --release -q -p dent-daemon -p dent-session-holder -p dent-launcher -p dent-cli
target=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
parts=$(mktemp -d)
for b in dent-daemon dent-session-holder dent-launcher dent; do
  cp "$target/release/$b" "$parts/$b"
done
if [[ $os == macos ]]; then
  # The same identity as the app, so macOS keeps what it allowed the service.
  id=${DENT_SIGN:-}
  for b in dent-daemon dent-session-holder dent-launcher dent; do
    codesign --force --sign "${id:--}" --identifier "sh.dent.${b#dent-}" "$parts/$b" >/dev/null
  done
  scripts/bundle.sh >/dev/null
  tar -czf "$out/app-$os-$arch.tar.gz" -C "$target/release" dent.app
fi
tar -czf "$out/daemon-$os-$arch.tar.gz" -C "$parts" dent-daemon dent-session-holder dent-launcher dent
rm -rf "$parts"
ls -l "$out"
