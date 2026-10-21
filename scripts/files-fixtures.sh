#!/bin/bash
# Fixtures for the `files-live` walk scene: a project on the test VM with
# pictures, a note, a page with its own styles, a video and a big file, the
# same path holding a different picture on each machine, and sessions there
# that name them all (decision 0068). Needs the VM paired in
# DENT_CLIENT_STATE and running the fake agent.
set -euo pipefail
VM=${VM:-dent-linux}
NAME=${NAME:-lima-dent-linux}
LIMA=${LIMA:-/opt/homebrew/bin/limactl}
here=$(cd "$(dirname "$0")/.." && pwd)
assets="$here/crates/fake-claude/assets"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/fx/pics"
cp "$assets/page.png" "$work/fx/pics/a.png"
cp "$assets/page-hero.png" "$work/fx/pics/big shot#1.png"
cp "$assets/demo.mp4" "$work/fx/take.mp4"
printf '# Notes\n\nA picture beside me:\n\n![a](pics/a.png)\n' > "$work/fx/doc.md"
printf 'body{background:#fdf6e3;font:16px system-ui}h1{color:#b58900}' > "$work/fx/style.css"
printf '<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="style.css"></head><body><h1>From the VM</h1><img src="pics/a.png" width="200"><img src="/pics/a.png" width="100"></body></html>' > "$work/fx/page.html"

"$LIMA" shell "$VM" -- bash -lc 'rm -rf ~/fx; mkdir -p ~/Pictures ~/Movies'
"$LIMA" copy -r "$work/fx" "$VM:/home/$USER.guest/fx"
"$LIMA" copy "$assets/page-tablet.png" "$VM:/home/$USER.guest/Pictures/h.png"
"$LIMA" copy "$assets/demo.mp4" "$VM:/home/$USER.guest/Movies/take.mp4"
# The same path, a different picture on each machine.
"$LIMA" copy "$assets/page.png" "$VM:/tmp/dent-same.png"
cp "$assets/page-phone.png" /tmp/dent-same.png
"$LIMA" shell "$VM" -- bash -lc 'head -c 50000000 /dev/urandom > ~/fx/big.bin'

dent="$here/target/debug/dent"
cwd="/home/$USER.guest/fx"
"$dent" ask "$NAME" 'say: Files test. Here: ![a](pics/a.png) and ![spaced](<pics/big shot#1.png>) and home ![h](~/Pictures/h.png) and the trap ![same](/tmp/dent-same.png) and gone ![x](nope.png). Notes: [doc.md](doc.md), page: [page.html](page.html), folder [pics](pics), movie ~/Movies/take.mp4 and take.mp4, big [big.bin](big.bin).' --cwd "$cwd"
"$dent" ask "$NAME" "take a screenshot of the page" --cwd "$cwd"
"$dent" ask "$NAME" "make a video of it" --cwd "$cwd"
"$dent" ask "$NAME" "look at this picture" --file "$assets/page-hero.png" --cwd "$cwd"
