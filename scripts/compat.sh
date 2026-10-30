#!/usr/bin/env bash
# Old and new dent still work together (decision 0077): a daemon from one
# build, driven over the remote link by the CLI from another. Pairs with a
# code, starts a session with the stand-in agent, and waits for its answer.
#
#   compat.sh DAEMON_DIR CLI_DIR FAKE_CLAUDE
#
# DAEMON_DIR holds dent-daemon and dent-session-holder, CLI_DIR holds dent.
set -euo pipefail
daemon_dir=$(cd "${1:?usage: compat.sh DAEMON_DIR CLI_DIR FAKE_CLAUDE}" && pwd)
cli=$(cd "${2:?}" && pwd)/dent
fake=$(cd "$(dirname "${3:?}")" && pwd)/$(basename "$3")
work=$(mktemp -d)
port=$((20000 + RANDOM % 20000))
export DENT_STATE=$work/state DENT_CLIENT_STATE=$work/client DENT_REMOTE_LAN=1 \
  DENT_REMOTE_PORT=$port DENT_CLAUDE=$fake DENT_UPDATES_OFF=1
mkdir -p "$work/project"
"$daemon_dir/dent-daemon" >"$work/daemon.log" 2>&1 &
pid=$!
stop() {
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  pkill -f "dent-session-holder.*$work" 2>/dev/null || true
  rm -rf "$work"
}
trap stop EXIT
fail() { echo "compat: $*" >&2; echo "--- daemon log" >&2; tail -40 "$work/daemon.log" >&2; exit 1; }
for _ in $(seq 100); do
  "$daemon_dir/dent-daemon" pair >"$work/pair.txt" 2>/dev/null && break
  sleep 0.2
done
code=$(sed -n 's/^Pairing code: //p' "$work/pair.txt" | tr -d ' ')
[ -n "$code" ] || fail "no pairing code"
"$cli" machine add "127.0.0.1:$port" --code "$code" >"$work/add.txt" 2>&1 || { cat "$work/add.txt" >&2; fail "pairing failed"; }
name=$("$cli" machines | awk 'NR==1 {print $1}')
[ -n "$name" ] || fail "no machine saved"
limit=(); command -v timeout >/dev/null && limit=(timeout 120)
out=$(${limit[@]+"${limit[@]}"} "$cli" ask "$name" "say hello" --cwd "$work/project" 2>&1) || { echo "$out" >&2; fail "ask failed"; }
session=$(echo "$out" | tail -1)
# The agent's turn, as the daemon tells it: events past where catching up
# ended, with none missing.
"$cli" watch "$name" --secs 10 >"$work/watch.txt" 2>&1 || true
caught=$(sed -n 's/.*caught up at \([0-9]*\).*/\1/p' "$work/watch.txt" | tail -1)
events=$(sed -n 's/.*events \([0-9]*\), last seq.*/\1/p' "$work/watch.txt" | tail -1)
grep -q "gaps 0" "$work/watch.txt" || { cat "$work/watch.txt" >&2; fail "events went missing"; }
[ "${events:-0}" -gt "${caught:-0}" ] || { cat "$work/watch.txt" >&2; fail "the agent never answered ($session)"; }
echo "compat: $session answered ($events events)"
