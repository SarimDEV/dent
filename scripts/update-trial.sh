#!/usr/bin/env bash
# A daemon updating itself from a feed, end to end, with real builds
# (decision 0077). Build 1 runs under the launcher; build 2 is published to a
# local feed. Build 1 must find it, stage it and say so in its welcome; asked
# to update, it must come back as build 2 with its sessions still there.
#
#   scripts/update-trial.sh            (on Linux; it builds twice)
#
# Leaves nothing running. Takes a few minutes, most of it building.
set -euo pipefail
cd "$(dirname "$0")/.."
work=$(mktemp -d)
target=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
for n in 1 2; do
  DENT_MAIN_BUILD=$n DENT_COMMIT=trial$n cargo build -q -p dent-daemon -p dent-session-holder -p dent-launcher -p dent-fake-claude
  mkdir -p "$work/b$n"
  cp "$target/debug/"{dent-daemon,dent-session-holder,dent-launcher} "$work/b$n/"
done
cp "$target/debug/dent-fake-claude" "$work/"

# Build 2, published.
mkdir -p "$work/parts" "$work/feed"
os=$(uname -s | tr '[:upper:]' '[:lower:]'); [[ $os == darwin ]] && os=macos
arch=$(uname -m); [[ $arch == arm64 ]] && arch=aarch64
tar -czf "$work/parts/daemon-$os-$arch.tar.gz" -C "$work/b2" dent-daemon dent-session-holder dent-launcher
cp "$work/parts/daemon-$os-$arch.tar.gz" "$work/feed/daemon-$os-$arch-2.tar.gz"
python3 scripts/feed.py "$work/parts" 2 trial2 "file://$work/feed" > "$work/feed/latest.json"

# Build 1, run by the launcher as the service would.
state=$work/state
mkdir -p "$state/versions/b1"
cp "$work/b1/"{dent-daemon,dent-session-holder} "$state/versions/b1/"
"$work/b1/dent-daemon" manifest > "$state/versions/b1/manifest"
echo "active=b1" > "$state/launcher.state"
export DENT_FEED="file://$work/feed/latest.json" DENT_UPDATE_EVERY=2 \
  DENT_CLAUDE="$work/dent-fake-claude" DENT_REMOTE_PORT=$((20000 + RANDOM % 20000))
"$work/b1/dent-launcher" run --state "$state" >"$work/launcher.log" 2>&1 &
launcher=$!
done_() {
  kill "$launcher" 2>/dev/null || true
  wait "$launcher" 2>/dev/null || true
  pkill -f "dent-session-holder.*$work" 2>/dev/null || true
  pkill -f "dent-session-holder.*$(basename "$work")" 2>/dev/null || true
}
trap done_ EXIT
python3 - "$state" "$work" <<'PY'
import json, os, socket, sys, time
state, work = sys.argv[1], sys.argv[2]

def sock_path():
    p = os.path.join(state, "daemon.sock")
    return p

def talk(reqs, want, secs=30):
    """Send requests on a fresh connection; read until want(out) is true."""
    end = time.time() + secs
    while time.time() < end:
        try:
            s = socket.socket(socket.AF_UNIX)
            s.connect(sock_path())
            break
        except OSError:
            time.sleep(0.2)
    else:
        sys.exit("trial: no daemon socket")
    f = s.makefile("rwb")
    for r in reqs:
        f.write((json.dumps(r) + "\n").encode()); f.flush()
    s.settimeout(max(1, end - time.time()))
    for line in f:
        out = json.loads(line)
        if want(out):
            s.close()
            return out
    sys.exit("trial: connection ended")

hello = {"req": 1, "op": "hello", "proto": 4, "client": "trial"}
def welcome():
    return talk([hello], lambda o: o.get("reply") == "welcome")

w = welcome()
assert w.get("main_build") == 1, w
assert w.get("managed"), w
# A session made on build 1, to find again on build 2.
made = talk([hello, {"req": 3, "op": "create", "cwd": work, "id": "", "text": "hi"}],
            lambda o: o.get("req") == 3)
print("trial: build 1 runs; made", made.get("reply") or made)
end = time.time() + 60
while time.time() < end:
    w = welcome()
    if w.get("available"):
        break
    time.sleep(1)
a = w.get("available")
assert a and a["main_build"] == 2 and "-b2-" in a["version"], w
print("trial: build 2 ready as", a["version"])
press = {"req": 3, "op": "update", "version": a["version"], "once": "trial-press-1"}
done = talk([hello, press], lambda o: o.get("req") == 3)
assert done.get("reply") == "ok", done
end = time.time() + 90
while time.time() < end:
    try:
        w = welcome()
        if w.get("main_build") == 2:
            break
    except SystemExit:
        pass
    time.sleep(1)
assert w.get("main_build") == 2, w
assert w.get("update") and not w["update"].get("rolled_back"), w
assert not w.get("available"), w
print("trial: build 2 runs; update", w["update"])
# The same press sent again after a reconnect changes nothing.
again = talk([hello, press], lambda o: o.get("req") == 3)
assert again.get("reply") == "ok", again
time.sleep(3)
w2 = welcome()
assert w2.get("main_build") == 2 and w2.get("update", {}).get("id") == w["update"]["id"], w2
print("trial: the press sent again changed nothing")
# The session made on build 1 is there on build 2.
sid = made.get("session") or made.get("id")
seen = talk([hello, {"req": 2, "op": "subscribe", "after": 0}],
            lambda o: o.get("op") == "events" and any(e.get("session") == sid for e in o.get("events", [])))
print("trial: session", sid, "still there")
PY
echo "update-trial: ok"
