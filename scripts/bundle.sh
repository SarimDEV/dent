#!/usr/bin/env bash
# Build dent.app: the desktop app with the daemon and session holder beside
# it, so the app can start them. `--install` copies it to ~/Applications
# (outside Documents, which macOS guards from background programs).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -q -p dent-desktop -p dent-daemon -p dent-session-holder
target=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
app="$target/release/dent.app"
version=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="dent-desktop"))')
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
for b in dent-desktop dent-daemon dent-session-holder; do
  cp "$target/release/$b" "$app/Contents/MacOS/$b"
done
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>dent</string>
  <key>CFBundleDisplayName</key><string>dent</string>
  <key>CFBundleIdentifier</key><string>sh.dent.desktop</string>
  <key>CFBundleExecutable</key><string>dent-desktop</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <!-- Previews of dev servers on this machine load over plain http. -->
  <key>NSAppTransportSecurity</key>
  <dict><key>NSAllowsLocalNetworking</key><true/></dict>
</dict>
</plist>
PLIST
# Ad-hoc signature, so macOS runs it locally and remembers its permissions.
codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true
echo "built $app"
if [[ "${1:-}" == "--install" ]]; then
  mkdir -p "$HOME/Applications"
  rm -rf "$HOME/Applications/dent.app"
  cp -R "$app" "$HOME/Applications/dent.app"
  echo "installed $HOME/Applications/dent.app"
fi
