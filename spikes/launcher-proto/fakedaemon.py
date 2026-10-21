#!/usr/bin/env python3
"""A stand-in daemon for launcher.py. Its version directory's name picks its
behaviour: a migration that adds a column (v2), or one that fails halfway
after changing the database (v2bad), or one that hangs (v2slow)."""
import os, sqlite3, sys, time

ver = os.environ["DENT_VERSION"]
db = sqlite3.connect(os.environ["DENT_DB"])
db.execute("PRAGMA journal_mode=WAL")
db.execute("CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY, body TEXT)")
cols = [r[1] for r in db.execute("PRAGMA table_info(events)")]
if ver.startswith("v2") and "machine" not in cols:
    if ver == "v2slow":
        time.sleep(3600)
    db.execute("ALTER TABLE events ADD COLUMN machine TEXT")
    db.execute("UPDATE events SET machine='studio'")
    db.commit()
    if ver == "v2bad":
        db.execute("CREATE TABLE half_done(x)")
        db.commit()
        sys.exit("migration failed halfway")
if ver == "v1" and "machine" in cols:
    sys.exit("v1 can't read a v2 database")
os.write(int(os.environ["DENT_PREPARED_FD"]), b"prepared\n")
os.read(int(os.environ["DENT_COMMITTED_FD"]), 64)  # parked until committed
n = db.execute("SELECT count(*) FROM events").fetchone()[0]
print(f"daemon {ver}: serving, {n} events, columns {cols if not ver.startswith('v2') else cols + ['machine']}", flush=True)
while True:
    time.sleep(0.5)
