#!/usr/bin/env bash
# The app updating itself, end to end on a Mac, with real builds (decision
# 0077), in a scratch folder: build 1 finds build 2 in a local feed, quits,
# and build 2 opens in its place. Then build 2 is offered a build 3 that
# can't start: it must be put back, say so, and never be offered build 3
# again. Opens dent's window a few times; takes a few minutes.
set -euo pipefail
cd "$(dirname "$0")/.."
work=$(mktemp -d /tmp/dent-aut.XXXXXX)
target=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
bundle() { # bundle N: build N as a dent.app in $work/bN
  local app=$work/b$1/dent.app
  mkdir -p "$app/Contents/MacOS"
  cp "$target/debug/"{dent-desktop,dent-daemon,dent-session-holder} "$app/Contents/MacOS/"
  cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>sh.dent.trial</string>
<key>CFBundleExecutable</key><string>dent-desktop</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
PLIST
  # Under the hardened runtime, as builds signed for notarisation are.
  codesign --force --deep --options runtime --sign - "$app" >/dev/null
}
for n in 1 2; do
  DENT_MAIN_BUILD=$n cargo build -q -p dent-desktop -p dent-daemon -p dent-session-holder
  bundle "$n"
done
# Build 3 can't start.
mkdir -p "$work/b3"
cp -R "$work/b2/dent.app" "$work/b3/dent.app"
printf '#!/bin/sh\nexit 1\n' > "$work/b3/dent.app/Contents/MacOS/dent-desktop"

publish() { # publish N: build N is the newest
  rm -rf "$work/parts"; mkdir -p "$work/parts" "$work/feed"
  tar -czf "$work/parts/app-macos-aarch64.tar.gz" -C "$work/b$1" dent.app
  cp "$work/parts/app-macos-aarch64.tar.gz" "$work/feed/app-macos-aarch64-$1.tar.gz"
  python3 scripts/feed.py "$work/parts" "$1" "trial$1" "file://$work/feed" > "$work/feed/latest.json.new"
  mv "$work/feed/latest.json.new" "$work/feed/latest.json"
}
mkdir -p "$work/Applications"
cp -R "$work/b1/dent.app" "$work/Applications/dent.app"
publish 2
# What macOS's `open` would do, keeping this trial's folder and feed.
cat > "$work/open.sh" <<SH
#!/bin/sh
DENT_STATE="$work/state" DENT_FEED="file://$work/feed/latest.json" DENT_UPDATE_EVERY=3 \
  DENT_UPDATE_NOW=1 DENT_OPEN="$work/open.sh" "\$1/Contents/MacOS/dent-desktop" >>"$work/app.log" 2>&1 &
SH
chmod +x "$work/open.sh"
stop() {
  # The app first (it would start its daemon again), then all it started.
  pkill -f "$work/Applications/dent.app/Contents/MacOS/dent-desktop" 2>/dev/null || true
  # Until nothing of the trial's runs (a holder may start its daemon again).
  for _ in $(seq 20); do
    sleep 0.5
    pkill -f "$(basename "$work")/" 2>/dev/null || true
    pgrep -qf "$(basename "$work")/" || break
  done
  rm -rf "$work"
}
trap stop EXIT
runs() { cmp -s "$work/Applications/dent.app/Contents/MacOS/dent-desktop" "$work/b$1/dent.app/Contents/MacOS/dent-desktop"; }
wait_for() { # wait_for SECS what condition...
  local secs=$1 what=$2; shift 2
  for _ in $(seq $((secs * 2))); do "$@" && return 0; sleep 0.5; done
  echo "app-update-trial: $what didn't happen; log:" >&2; tail -20 "$work/app.log" >&2; exit 1
}
"$work/open.sh" "$work/Applications/dent.app"
started() { [ "$(cat "$work/state/app-updates/started" 2>/dev/null)" = "$1" ]; }
# Build 1 starts, finds build 2 and goes, in a moment.
wait_for 120 "build 2 taking build 1's place" started 2
runs 2 || { echo "the installed app isn't build 2" >&2; exit 1; }
cmp -s "$work/state/app-updates/previous/dent.app/Contents/MacOS/dent-desktop" "$work/b1/dent.app/Contents/MacOS/dent-desktop" \
  || { echo "build 1 wasn't kept" >&2; exit 1; }
echo "app-update-trial: build 1 updated to build 2"
publish 3
went_back() { [ -f "$work/state/app-updates/skipped" ] && started 2 && pgrep -qf "$work/Applications/dent.app/Contents/MacOS/dent-desktop"; }
wait_for 150 "build 3 going back to build 2" went_back
runs 2 || { echo "build 2 wasn't put back" >&2; exit 1; }
[ "$(cat "$work/state/app-updates/skipped")" = 3 ] || exit 1
sleep 8
runs 2 || { echo "build 3 was tried again" >&2; exit 1; }
echo "app-update-trial: build 3 didn't start, so build 2 came back and stays"
echo "app-update-trial: ok"
