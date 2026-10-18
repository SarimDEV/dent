#!/usr/bin/env bash
# Walk through the real desktop app against a daemon running the fake agent.
#   scripts/walk.sh checks [--dark] [--size WxH] [--shots DIR]
#   scripts/walk.sh stills --shots DIR [--dark] [--size WxH]
#   scripts/walk.sh pickup --shots DIR [--dark] [--size WxH]   (the new-session page with many terminal sessions)
#   scripts/walk.sh polish [--shots DIR] [--dark] [--size WxH]   (a tool's picture, thinking and call times, the sidebar's marks)
#   scripts/walk.sh markdown [--shots DIR] [--dark] [--size WxH]   (only the maths, diagram and table reply)
#   scripts/walk.sh attaching [--shots DIR] [--dark] [--size WxH]   (files and pictures: chips, the large view)
#   scripts/walk.sh visuals [--shots DIR] [--dark] [--size WxH]   (coloured output, a folded paste, queued files, half-typed formatting)
#   scripts/walk.sh edges [--shots DIR] [--dark] [--size WxH]   (odd and huge content, twenty files, thirty sessions, restarts mid-flow)
#   DENT_WALK_PROMPT=... [DENT_WALK_REPO=DIR] [DENT_WALK_PERMISSION=full] scripts/walk.sh compare --shots DIR [--size WxH]
#     (the real agent on one long turn, a still in each theme every ~2s; uses your limits)
# Everything runs in a scratch home, state and project, so no account, real
# project or setting is touched. The window opens in the background and
# never takes focus. Stills need WINID (a build of crates/desktop/tools/winid.swift).
set -euo pipefail
name=${1:-checks}; shift || true
tmp=${TMPDIR:-/tmp}
root=$(mktemp -d "${tmp%/}/dent-walk.XXXXXX")
trap 'pkill -f "dent-daemon --state $root/state" 2>/dev/null || true; rm -rf "$root"' EXIT
mkdir -p "$root/home" "$root/state" "$root/proj" "$root/outside"
# Walks check approvals, so new sessions keep asking: skip the one-time
# move to Full access (decision 0048).
touch "$root/state/desktop-full-access-default"
# A page's own files, in a folder outside the project.
echo 'body{margin:40px;font:16px system-ui} h1{color:rgb(200,30,30)}' > "$root/outside/style.css"
printf 'GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\x00\x00\x00\x00\x00!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;' > "$root/outside/dot.gif"
git -C "$root/proj" init -q
echo "# Scratch project" > "$root/proj/README.md"
git -C "$root/proj" add . && git -C "$root/proj" -c user.name=walk -c user.email=walk@example.com commit -qm init
unset DENT_CLAUDE
# A session someone ran in a terminal, to pick up in dent.
mkdir -p "$root/home/.claude/projects/-proj"
cat > "$root/home/.claude/projects/-proj/22222222-3333-4444-8555-666666666666.jsonl" <<JSONL
{"type":"user","entrypoint":"cli","cwd":"$root/proj","uuid":"t1","message":{"role":"user","content":"Tidy up the README"}}
{"type":"assistant","uuid":"t2","message":{"content":[{"type":"text","text":"I tidied the README and fixed two broken links."}]}}
{"type":"user","entrypoint":"cli","cwd":"$root/proj","uuid":"t3","message":{"role":"user","content":"Now check the links in the footer too"}}
{"type":"assistant","uuid":"t4","message":{"content":[{"type":"text","text":"The footer links all work."}]}}
{"type":"ai-title","aiTitle":"README cleanup"}
JSONL
touch -t 202601010000 "$root/home/.claude/projects/-proj/"*.jsonl
# Enough terminal sessions to fill the room below a new session.
if [[ "$name" == pickup ]]; then
  for i in 1 2 3 4 5 6 7; do
    cat > "$root/home/.claude/projects/-proj/4444444$i-3333-4444-8555-666666666666.jsonl" <<JSONL
{"type":"user","entrypoint":"cli","cwd":"$root/proj","uuid":"p$i","message":{"role":"user","content":"Look into issue $i"}}
{"type":"assistant","uuid":"q$i","message":{"content":[{"type":"text","text":"Done with issue $i."}]}}
{"type":"ai-title","aiTitle":"Issue $i follow-up"}
JSONL
    touch -t 20260101000$i "$root/home/.claude/projects/-proj/4444444$i-3333-4444-8555-666666666666.jsonl"
  done
fi
# A locked or sleeping screen draws no windows, so a walk would stop dead
# with nothing to say why: say so now. While it runs, the display stays awake.
if [[ "$(uname)" == Darwin ]]; then
  session=$(ioreg -n Root -d1 || true)
  if [[ "$session" == *'"CGSSessionScreenIsLocked"=Yes'* ]]; then
    echo "walk: the screen is locked; unlock it first (windows draw nothing while it is)" >&2
    exit 2
  fi
  caffeinate -d -w $$ &
fi
# DENT_WALK_RELEASE=1 walks the optimised build, for timing what you'd really run.
profile=debug; release=()
if [[ -n "${DENT_WALK_RELEASE:-}" ]]; then profile=release; release=(--release); fi
cargo build -q ${release[@]+"${release[@]}"} -p dent-desktop -p dent-daemon -p dent-session-holder -p dent-fake-claude
bin=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/$profile
if [[ "$name" == "real" || "$name" == "compare" ]]; then
  # The real agent on its smallest model, signed in as you: your own home.
  status=0
  DENT_STATE="$root/state" DENT_WALK_PROJECT="${DENT_WALK_REPO:-$root/proj}" DENT_HOLDER_ORPHAN_MS=10000 \
    "$bin/dent-desktop" --walk "$name" "$@" || status=$?
  # Leave no trace in your Claude Code history: only this run's scratch
  # project's transcripts go.
  sleep 3
  mine="$HOME/.claude/projects/$(cd "$root/proj" && pwd -P | sed 's|[^A-Za-z0-9]|-|g')"
  [[ "$mine" == *dent-walk* && -d "$mine" ]] && rm -rf "$mine"
  exit $status
fi
# One still running in a terminal: listed, but not to be picked up. Written
# last, so it counts as running when the window looks.
now=$(date -u +%Y-%m-%dT%H:%M:%S.000Z)
cat > "$root/home/.claude/projects/-proj/33333333-4444-4555-8666-777777777777.jsonl" <<JSONL
{"type":"user","entrypoint":"cli","cwd":"$root/proj","uuid":"o1","timestamp":"$now","message":{"role":"user","content":"Profile the startup path"}}
{"type":"assistant","uuid":"o2","timestamp":"$now","message":{"content":[{"type":"text","text":"Looking at the startup path now."}]}}
JSONL
HOME="$root/home" \
DENT_STATE="$root/state" \
DENT_HOLDER_ORPHAN_MS=10000 \
DENT_CLAUDE="$bin/dent-fake-claude" \
DENT_WALK_PROJECT="$root/proj" \
DENT_WALK_OUTSIDE="$root/outside" \
FAKE_CLAUDE_SPEED="${FAKE_CLAUDE_SPEED:-0.3}" \
  "$bin/dent-desktop" --walk "$name" $([[ " $* " == *" --dark "* ]] || echo --light) "$@"
