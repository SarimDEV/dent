#!/bin/bash
# Edge fixtures for the `break-live` walk scene, after files-fixtures.sh:
# files that can't be read, a link loop, odd names, an empty and a sparse
# 3 GB file, sixty pictures, and sessions on the VM that name them.
set -euo pipefail
VM=${VM:-dent-linux}
NAME=${NAME:-lima-dent-linux}
LIMA=${LIMA:-/opt/homebrew/bin/limactl}
here=$(cd "$(dirname "$0")/.." && pwd)
"$LIMA" shell "$VM" -- bash -lc 'cd ~/fx && rm -f pipe.png && mkfifo pipe.png; echo secret > locked.png && chmod 000 locked.png; ln -sf loop1 loop2; ln -sf loop2 loop1; cp pics/a.png "ünïcødé 名前.png"; cp pics/a.png "$(printf "new\nline.png")"; mkdir -p dir.png; : > empty.png; truncate -s 3G sparse.mov; mkdir -p many && for i in $(seq -w 0 59); do cp pics/a.png many/p$i.png; printf "%s" $i >> many/p$i.png; done'
dent="$here/target/debug/dent"
cwd="/home/$USER.guest/fx"
"$dent" ask "$NAME" 'say: Edge files: ![pipe](pipe.png) ![locked](locked.png) ![dir](dir.png) ![uni](<ünïcødé 名前.png>) ![empty](empty.png) ![loop](loop1) and [sparse.mov](sparse.mov) [big.bin](big.bin)' --cwd "$cwd"
many=$(for i in $(seq -w 0 59); do printf '![p%s](many/p%s.png)\n\n' "$i" "$i"; done)
"$dent" ask "$NAME" "say: Many pictures:

$many" --cwd "$cwd"
