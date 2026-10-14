#!/usr/bin/env bash
# Walk through the real desktop app against a daemon running the fake agent.
#   scripts/walk.sh checks [--dark] [--size WxH] [--shots DIR]
#   scripts/walk.sh stills --shots DIR [--dark] [--size WxH]
# Everything runs in a scratch home, state and project, so no account, real
# project or setting is touched. The window opens in the background and
# never takes focus. Stills need WINID (a build of crates/desktop/tools/winid.swift).
set -euo pipefail
name=${1:-checks}; shift || true
root=$(mktemp -d "${TMPDIR:-/tmp}/dent-walk.XXXXXX")
trap 'pkill -f "dent-daemon --state $root/state" 2>/dev/null || true; rm -rf "$root"' EXIT
mkdir -p "$root/home" "$root/state" "$root/proj"
git -C "$root/proj" init -q
echo "# Scratch project" > "$root/proj/README.md"
git -C "$root/proj" add . && git -C "$root/proj" -c user.name=walk -c user.email=walk@example.com commit -qm init
unset DENT_CLAUDE
# A session someone ran in a terminal, to pick up in dent.
mkdir -p "$root/home/.claude/projects/-proj"
cat > "$root/home/.claude/projects/-proj/22222222-3333-4444-8555-666666666666.jsonl" <<JSONL
{"type":"user","entrypoint":"cli","cwd":"$root/proj","uuid":"t1","message":{"role":"user","content":"Tidy up the README"}}
{"type":"assistant","uuid":"t2","message":{"content":[{"type":"text","text":"I tidied the README and fixed two broken links."}]}}
JSONL
touch -t 202601010000 "$root/home/.claude/projects/-proj/"*.jsonl
cargo build -q -p dent-desktop -p dent-daemon -p dent-session-holder -p dent-fake-claude
bin=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug
if [[ "$name" == "real" ]]; then
  # The real agent on its smallest model, signed in as you: your own home.
  status=0
  DENT_STATE="$root/state" DENT_WALK_PROJECT="$root/proj" DENT_HOLDER_ORPHAN_MS=2000 \
    "$bin/dent-desktop" --walk real "$@" || status=$?
  # Leave no trace in your Claude Code history: only this run's scratch
  # project's transcripts go.
  sleep 3
  mine="$HOME/.claude/projects/$(cd "$root/proj" && pwd -P | sed 's|[^A-Za-z0-9]|-|g')"
  [[ "$mine" == *dent-walk* && -d "$mine" ]] && rm -rf "$mine"
  exit $status
fi
HOME="$root/home" \
DENT_STATE="$root/state" \
DENT_HOLDER_ORPHAN_MS=2000 \
DENT_CLAUDE="$bin/dent-fake-claude" \
DENT_WALK_PROJECT="$root/proj" \
FAKE_CLAUDE_SPEED="${FAKE_CLAUDE_SPEED:-0.3}" \
  "$bin/dent-desktop" --walk "$name" $([[ " $* " == *" --dark "* ]] || echo --light) "$@"
