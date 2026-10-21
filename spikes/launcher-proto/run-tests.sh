#!/usr/bin/env bash
# Runs the launcher prototype's scenarios (R-E3, R-E4) on Linux. Usage:
#   run-tests.sh DIR_WITH_launcher.py_AND_fakedaemon.py
set -uo pipefail
HERE=$1
WORK=$(mktemp -d)
pass=0; fail=0
check() { if eval "$2"; then echo "  PASS $1"; pass=$((pass+1)); else echo "  FAIL $1"; fail=$((fail+1)); fi; }
cols() { python3 -c "import sqlite3,sys; print(','.join(r[1] for r in sqlite3.connect(sys.argv[1]).execute('PRAGMA table_info(events)')))" "$1/dent.db"; }
tables() { python3 -c "import sqlite3,sys; print(','.join(r[0] for r in sqlite3.connect(sys.argv[1]).execute(\"select name from sqlite_master where type='table'\")))" "$1/dent.db"; }
state() { python3 -c "import json,sys; s=json.load(open(sys.argv[1]+'/launcher.json')); print(s['active'], (s.get('outcome') or {}).get('outcome'), ','.join(s.get('never_retry',[])))" "$1"; }

setup() {
  local r=$1
  mkdir -p "$r/versions"
  for v in v1 v2 v2bad v2slow v3; do
    mkdir -p "$r/versions/$v"
    printf '#!/bin/sh\nexec python3 %s/fakedaemon.py\n' "$HERE" > "$r/versions/$v/daemon"
    chmod +x "$r/versions/$v/daemon"
    echo '{"launcher_protocol": 1}' > "$r/versions/$v/manifest.json"
  done
  echo '{"launcher_protocol": 2}' > "$r/versions/v3/manifest.json"
  python3 - "$r/dent.db" <<'EOF'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1]); db.execute("PRAGMA journal_mode=WAL")
db.execute("CREATE TABLE events(seq INTEGER PRIMARY KEY, body TEXT)")
db.executemany("INSERT INTO events(body) VALUES (?)", [("x" * 200,)] * 20000); db.commit()
EOF
  echo '{"active": "v1", "pending": null}' > "$r/launcher.json"
}
launch() { (cd "$1" && setsid env "${@:2}" python3 "$HERE/launcher.py" run "$1" >> "$1/launcher.log" 2>&1 < /dev/null &); sleep 1.5; }
update() { python3 "$HERE/launcher.py" update "$1" "$2"; sleep "${3:-3}"; }
stop_launcher() { pkill -f "launcher.py run $1" ; pkill -f "fakedaemon.py" ; sleep 0.5; }

# A stand-in for an agent the holder runs: it must never notice an update.
AG=$WORK/agent.count
setsid bash -c "n=0; while true; do n=\$((n+1)); echo \$n > $AG; sleep 0.1; done" < /dev/null > /dev/null 2>&1 &
AGPID=$!
# However the run ends, stop what it started.
trap 'kill $AGPID 2>/dev/null; pkill -f "launcher.py run $WORK"; pkill -f fakedaemon.py; sudo umount "$WORK/small" 2>/dev/null; rm -rf "$WORK"' EXIT

echo "1. failing migration rolls back"
R=$WORK/a; setup "$R"; launch "$R"
update "$R" v2bad
check "state says rolled back, still v1" '[ "$(state $R)" = "v1 rolled_back v2bad" ]'
check "the migration's column is gone" '[ "$(cols $R)" = "seq,body" ]'
check "the half-made table is gone" '[ "$(tables $R)" = "events" ]'
check "v1 is serving again" 'grep -q "daemon v1: serving, 20000 events" $R/launcher.log'

echo "2. a rolled-back version is never retried automatically"
update "$R" v2bad 1
check "refused" 'grep -q "refused v2bad: it rolled back before" $R/launcher.log'

echo "3. a release needing a newer launcher is refused before anything stops"
before=$(pgrep -f "fakedaemon.py" | head -1)
update "$R" v3 1
check "refused" 'grep -q "refused v3: needs launcher protocol 2" $R/launcher.log'
check "daemon untouched" '[ "$(pgrep -f fakedaemon.py | head -1)" = "$before" ]'

echo "4. a good migration commits"
update "$R" v2
check "state says committed, now v2" '[ "$(state $R | cut -d" " -f1-2)" = "v2 committed" ]'
check "column added" '[ "$(cols $R)" = "seq,body,machine" ]'
check "v2 serving with every event" 'grep -q "daemon v2: serving, 20000 events" $R/launcher.log'
stop_launcher "$R"

echo "5. a stuck trial times out and rolls back"
R=$WORK/b; setup "$R"; launch "$R" DENT_PREPARE_TIMEOUT=3
update "$R" v2slow 6
check "rolled back after the deadline" '[ "$(state $R)" = "v1 rolled_back v2slow" ]'
check "v1 serving" '[ "$(grep -c "daemon v1: serving" $R/launcher.log)" = 2 ]'
stop_launcher "$R"

echo "6. a restore interrupted by a crash finishes before either version boots"
R=$WORK/c; setup "$R"; launch "$R" DENT_CRASH_IN_RESTORE=1
update "$R" v2bad 3
check "launcher died mid-restore, marker left" '[ -e $R/restore-pending ]'
check "database is still the broken one" '[ "$(cols $R)" = "seq,body,machine" ]'
pkill -f fakedaemon.py; sleep 0.3
launch "$R"; sleep 1
check "restore finished on restart" '[ ! -e $R/restore-pending ] && [ "$(cols $R)" = "seq,body" ] && [ "$(tables $R)" = "events" ]'
check "v1 serving" 'grep -q "finishing an interrupted restore" $R/launcher.log && [ "$(grep -c "daemon v1: serving" $R/launcher.log)" = 2 ]'
check "recorded as rolled back, never retried" '[ "$(state $R)" = "v1 rolled_back v2bad" ]'

echo "6b. restarting a daemon that keeps exiting doesn't leak descriptors"
L=$(pgrep -f "launcher.py run $R" | head -1)
fds() { ls "/proc/$L/fd" | wc -l; }
pkill -f fakedaemon.py; sleep 2
before=$(fds)
for _ in 1 2 3 4 5; do pkill -f fakedaemon.py; sleep 1.5; done
check "same descriptors after 5 restarts ($before, $(fds))" '[ "$(fds)" = "$before" ]'
stop_launcher "$R"

echo "7. not enough disk for the snapshot: refused, nothing stopped"
MNT=$WORK/small; mkdir -p "$MNT"
# Room for the database, not for its snapshot plus the trial's headroom.
sudo mount -t tmpfs -o size=20m tmpfs "$MNT" && sudo chown "$(id -u):$(id -g)" "$MNT"
R=$MNT/d; setup "$R"
launch "$R"; before=$(pgrep -f fakedaemon.py | head -1)
update "$R" v2 2
check "refused with sizes" 'grep -q "refused v2: .* free, need" $R/launcher.log'
check "daemon untouched" '[ "$(pgrep -f fakedaemon.py | head -1)" = "$before" ]'
grep "refused v2" "$R/launcher.log" | sed 's/^/    /'
stop_launcher "$R"; sudo umount "$MNT"

echo "8. the agent ran throughout"
a=$(cat "$AG"); sleep 0.5; b=$(cat "$AG")
check "agent still counting ($b)" '[ "$b" -gt "$a" ]'
kill "$AGPID"
echo "passed $pass, failed $fail"
