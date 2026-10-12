#!/bin/sh
# Builds the core for iOS and the app around it (decision 0023: our own
# script). Steps: the static library per platform, the Swift bindings read
# from the library, an XCFramework, then the Xcode project from project.yml.
#
#   spikes/core-ffi/build.sh            simulator, debug
#   spikes/core-ffi/build.sh --release  simulator and device, release
set -eu
cd "$(dirname "$0")/../.."
ios=spikes/core-ffi/ios
profile=dev; dir=debug
targets="aarch64-apple-ios-sim"
if [ "${1:-}" = "--release" ]; then
  profile=release; dir=release
  targets="aarch64-apple-ios-sim aarch64-apple-ios"
fi

for t in $targets; do
  cargo build -p spike-core-ffi --lib --target "$t" --profile "$profile"
done

# Bindings come from the host build of the same code.
cargo build -p spike-core-ffi --lib
rm -rf "$ios/Generated" && mkdir -p "$ios/Generated/headers"
cargo run -q -p spike-core-ffi --features bindgen --bin uniffi-bindgen -- \
  generate --library target/debug/libdent_core.dylib --language swift \
  --out-dir "$ios/Generated/swift"
mv "$ios/Generated/swift/"*.h "$ios/Generated/headers/"
cat "$ios/Generated/swift/"*.modulemap > "$ios/Generated/headers/module.modulemap"
rm "$ios/Generated/swift/"*.modulemap

rm -rf "$ios/build/DentCore.xcframework"
args=""
for t in $targets; do
  args="$args -library target/$t/$dir/libdent_core.a -headers $ios/Generated/headers"
done
# shellcheck disable=SC2086
xcodebuild -create-xcframework $args -output "$ios/build/DentCore.xcframework" >/dev/null
(cd "$ios" && xcodegen generate --quiet)
ls -l target/aarch64-apple-ios-sim/$dir/libdent_core.a
