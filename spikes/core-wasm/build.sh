#!/bin/sh
# Builds the core for the browser: wasm, the JS glue, and the page's fonts.
#   spikes/core-wasm/build.sh [--release]
set -eu
cd "$(dirname "$0")/../.."
profile=dev; dir=debug
if [ "${1:-}" = "--release" ]; then profile=wasm; dir=wasm; fi
cargo build -p spike-core-wasm --target wasm32-unknown-unknown --profile "$profile"
wasm-bindgen --target web --no-typescript --out-dir spikes/core-wasm/web/pkg \
  "target/wasm32-unknown-unknown/$dir/dent_core_web.wasm"
mkdir -p spikes/core-wasm/web/fonts
cp assets/fonts/*.ttf spikes/core-wasm/web/fonts/
ls -l spikes/core-wasm/web/pkg/dent_core_web_bg.wasm
