#!/usr/bin/env bash
# Logic crates must never depend on GPUI; only dent-desktop (and spikes) may.
set -euo pipefail
status=0
for crate in dent-protocol dent-client-core dent-daemon dent-session-holder dent-cli dent-web dent-hub; do
  if cargo tree -p "$crate" -e normal --prefix none 2>/dev/null | grep -qE '^gpui(-component)? '; then
    echo "boundary: $crate depends on GPUI" >&2
    status=1
  fi
done
exit $status
