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
  <!-- What macOS shows when dent first opens a project in a guarded folder. -->
  <key>NSDocumentsFolderUsageDescription</key><string>dent runs your agents in the projects you open, and some of them are in Documents.</string>
  <key>NSDesktopFolderUsageDescription</key><string>dent runs your agents in the projects you open, and some of them are on the Desktop.</string>
  <key>NSDownloadsFolderUsageDescription</key><string>dent runs your agents in the projects you open, and some of them are in Downloads.</string>
  <!-- Previews of dev servers on this machine load over plain http. -->
  <key>NSAppTransportSecurity</key>
  <dict><key>NSAllowsLocalNetworking</key><true/></dict>
</dict>
</plist>
PLIST
# Signed with the same certificate every build, macOS sees one app and keeps
# the folder access you gave it. An ad-hoc signature is new on every build,
# so macOS would ask again each time. DENT_SIGN picks the identity; by
# default it's this Mac's Apple Development certificate, if it has one.
id=${DENT_SIGN:-$(security find-identity -v -p codesigning 2>/dev/null | awk -F'"' '/Apple Development/ {print $2; exit}')}
sign() {
  codesign --force --sign "${id:--}" "$@" >/dev/null
}
if [[ -n "$id" ]] && sign --identifier sh.dent.daemon "$app/Contents/MacOS/dent-daemon" \
  && sign --identifier sh.dent.holder "$app/Contents/MacOS/dent-session-holder" \
  && sign "$app"; then
  echo "signed as $id"
else
  [[ -n "$id" ]] && echo "couldn't sign as $id; signing ad hoc (macOS will ask for folder access again)"
  id=""
  sign --identifier sh.dent.daemon "$app/Contents/MacOS/dent-daemon"
  sign --identifier sh.dent.holder "$app/Contents/MacOS/dent-session-holder"
  sign "$app"
fi
echo "built $app"
if [[ "${1:-}" == "--install" ]]; then
  mkdir -p "$HOME/Applications"
  rm -rf "$HOME/Applications/dent.app"
  cp -R "$app" "$HOME/Applications/dent.app"
  echo "installed $HOME/Applications/dent.app"
fi
