#!/bin/sh
# Install dent from main's newest build (decision 0077). From then on it
# updates itself: Update in the app runs every newer build.
#
#   curl -fsSL https://raw.githubusercontent.com/SarimDEV/dent/main/scripts/install.sh | sh
#
# On a Mac this installs dent.app in ~/Applications. `sh -s -- --service`
# (always, on Linux) also runs dent's daemon as a service, for a machine
# other devices drive: a systemd user unit on Linux, a LaunchAgent on macOS.
set -eu

feed=${DENT_FEED:-https://github.com/SarimDEV/dent/releases/download/edge/latest.json}
service=no
[ "${1:-}" = "--service" ] && service=yes

os=$(uname -s)
arch=$(uname -m)
[ "$arch" = arm64 ] && arch=aarch64
case "$os" in
  Darwin) os=macos ;;
  Linux) os=linux; service=yes ;;
  *) echo "dent runs on macOS and Linux" >&2; exit 1 ;;
esac
if [ "$os" = macos ] && [ "$arch" != aarch64 ]; then
  echo "dent's builds are for Macs with Apple silicon; on this Mac, build it from source." >&2
  exit 1
fi
if [ "$os" = linux ] && ! command -v python3 >/dev/null; then
  echo "Installing dent here needs python3 (to read the build list); install it and run this again." >&2
  exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$feed" -o "$tmp/latest.json"

# A field of the feed, without needing anything installed.
field() {
  if [ "$os" = macos ]; then
    plutil -extract "$1" raw -o - "$tmp/latest.json"
  else
    python3 -c 'import json, sys
v = json.load(open(sys.argv[1]))
for k in sys.argv[2].split("."):
    v = v[k]
print(v)' "$tmp/latest.json" "$1"
  fi
}

# Download a part and check it against the feed.
get() {
  url=$(field "parts.$1.url")
  want=$(field "parts.$1.sha256")
  curl -fSL --progress-bar "$url" -o "$tmp/$1.tar.gz"
  if [ "$os" = macos ]; then
    have=$(shasum -a 256 "$tmp/$1.tar.gz" | cut -d' ' -f1)
  else
    have=$(sha256sum "$tmp/$1.tar.gz" | cut -d' ' -f1)
  fi
  [ "$have" = "$want" ] || { echo "$1 doesn't match its checksum" >&2; exit 1; }
  mkdir -p "$tmp/$1"
  tar -xzf "$tmp/$1.tar.gz" -C "$tmp/$1"
}

build=$(field build)
echo "dent build $build"

if [ "$os" = macos ]; then
  get "app-$os-$arch"
  mkdir -p "$HOME/Applications"
  if pgrep -qf "$HOME/Applications/dent.app/Contents/MacOS/dent-desktop"; then
    echo "Quit dent first, then run this again." >&2
    exit 1
  fi
  rm -rf "$HOME/Applications/dent.app"
  mv "$tmp/app-$os-$arch/dent.app" "$HOME/Applications/dent.app"
  echo "Installed ~/Applications/dent.app"
fi

if [ "$service" = yes ]; then
  get "daemon-$os-$arch"
  "$tmp/daemon-$os-$arch/dent-daemon" install
  # The CLI, where a shell finds it.
  mkdir -p "$HOME/.local/bin"
  cp "$tmp/daemon-$os-$arch/dent" "$HOME/.local/bin/.dent.new"
  mv "$HOME/.local/bin/.dent.new" "$HOME/.local/bin/dent"
  echo "dent's daemon now runs as a service, and updates with Update in the app."
fi

[ "$os" = macos ] && echo "Open dent from ~/Applications."
exit 0
