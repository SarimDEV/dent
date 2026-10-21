#!/usr/bin/env python3
"""Remote plan stage 0, R-E3/R-E4 (docs/remote-tests.md): a prototype of the
launcher in decision 0065, which owns which daemon version runs.

    launcher.py run STATE_DIR           supervise the active version
    launcher.py update STATE_DIR VER    ask the running launcher to update

Versions live in STATE_DIR/versions/<ver>/daemon (any executable) with a
manifest.json {"launcher_protocol": N}. The daemon gets DENT_DB, DENT_TRIAL
(1 while on trial) and two pipes, named by DENT_PREPARED_FD and
DENT_COMMITTED_FD: it writes "prepared\n" once migrated and bound, then waits
for "committed\n" before taking work.

State is written by temp file, fsync, rename and directory fsync. A crash at
any point leaves either the old or the new record, never half of one.
"""
import json, os, shutil, signal, subprocess, sys, time

PROTOCOL = 1
PREPARE_TIMEOUT = float(os.environ.get("DENT_PREPARE_TIMEOUT", "1800"))


def log(*a):
    print("launcher:", *a, flush=True)


def fsync_path(path):
    fd = os.open(path, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def fsync_dir(path):
    fsync_path(os.path.dirname(path) or ".")


def write_atomic(path, data):
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)
    fsync_dir(path)


def clone(src, dst_dir):
    """Clone where the filesystem can (APFS, reflink), copy otherwise."""
    if sys.platform == "darwin":
        cmd = ["cp", "-c", src, dst_dir]  # clonefile; macOS cp has no --reflink
    else:
        cmd = ["cp", "--reflink=auto", src, dst_dir]
    if subprocess.run(cmd).returncode != 0:
        shutil.copy2(src, dst_dir)


class State:
    def __init__(self, root):
        self.root = root
        self.path = os.path.join(root, "launcher.json")
        self.db = os.path.join(root, "dent.db")

    def load(self):
        with open(self.path) as f:
            return json.load(f)  # unreadable state stops startup: no guessing

    def save(self, s):
        write_atomic(self.path, json.dumps(s, indent=1))


def db_files(db):
    return [db, db + "-wal", db + "-shm"]


def snapshot(st, uid):
    snap = os.path.join(st.root, "snapshots", uid)
    if os.path.isdir(snap):
        return snap  # once per update, never overwritten by a retry
    stage = snap + ".staging"
    shutil.rmtree(stage, ignore_errors=True)
    os.makedirs(stage)
    for f in db_files(st.db):
        if os.path.exists(f):
            clone(f, stage)
    for f in os.listdir(stage):
        fsync_path(os.path.join(stage, f))
    fsync_path(stage)
    os.replace(stage, snap)
    fsync_dir(snap)
    return snap


def restore(st, uid):
    """Finish (or do) a restore. A marker makes an interrupted one finish
    before either version boots."""
    snap = os.path.join(st.root, "snapshots", uid)
    marker = os.path.join(st.root, "restore-pending")
    write_atomic(marker, uid)
    if os.environ.get("DENT_CRASH_IN_RESTORE") == "1":
        os.environ.pop("DENT_CRASH_IN_RESTORE")
        log("simulated crash halfway through the restore")
        os._exit(9)
    for f in db_files(st.db):
        if os.path.exists(f):
            os.remove(f)
    for f in os.listdir(snap):
        dst = os.path.join(st.root, f)
        shutil.copy2(os.path.join(snap, f), dst)
        fsync_path(dst)
    # The restored files must be on disk before the marker goes, or a power
    # cut could leave a half-copied database and no marker to finish it.
    fsync_dir(st.db)
    os.remove(marker)
    fsync_dir(marker)


def free_enough(st):
    size = sum(os.path.getsize(f) for f in db_files(st.db) if os.path.exists(f))
    free = shutil.disk_usage(st.root).free
    need = int(size * 1.05) + 16 * 1024 * 1024  # snapshot plus headroom for the trial
    return free >= need, size, free, need


class Daemon:
    """A daemon process and the launcher's ends of its two pipes. The
    committed pipe stays open while the daemon runs, so it never reads EOF
    as the launcher going away; both ends are closed when it stops, so
    restarts don't leak descriptors."""

    def __init__(self, p, prepared, committed):
        self.p, self.prepared, self.committed = p, prepared, committed

    def commit(self):
        os.write(self.committed, b"committed\n")

    def close(self):
        for fd in (self.prepared, self.committed):
            if fd is not None:
                os.close(fd)
        self.prepared = self.committed = None


def start(st, ver, trial):
    exe = os.path.join(st.root, "versions", ver, "daemon")
    r3, w3 = os.pipe()  # child -> launcher: prepared
    r4, w4 = os.pipe()  # launcher -> child: committed
    env = dict(os.environ, DENT_DB=st.db, DENT_TRIAL="1" if trial else "0", DENT_VERSION=ver,
               DENT_PREPARED_FD=str(w3), DENT_COMMITTED_FD=str(r4))
    try:
        p = subprocess.Popen([exe], env=env, pass_fds=(w3, r4))
    except OSError:
        for fd in (r3, w3, r4, w4):
            os.close(fd)
        raise
    os.close(w3)
    os.close(r4)
    return Daemon(p, r3, w4)


def wait_prepared(d, timeout):
    p, r3 = d.p, d.prepared
    deadline = time.monotonic() + timeout
    buf = b""
    os.set_blocking(r3, False)
    while time.monotonic() < deadline:
        if p.poll() is not None:
            return "crashed"
        try:
            chunk = os.read(r3, 64)
            if chunk:
                buf += chunk
            if b"prepared\n" in buf:
                return "prepared"
        except BlockingIOError:
            pass
        time.sleep(0.05)
    return "timeout"


def stop(d):
    if d is None:
        return
    p = d.p
    if p.poll() is None:
        p.terminate()
        try:
            p.wait(5)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait()
    d.close()


def rolled_back(s, uid):
    """Record a rollback of the pending version, which is then never
    retried automatically, however the rollback was reached."""
    target = s.get("pending")
    never = s.get("never_retry", [])
    if target and target not in never:
        never = never + [target]
    s.update(pending=None, never_retry=never,
             outcome={"id": uid, "from": s.get("rollback"), "to": target, "outcome": "rolled_back"})


def run(root):
    st = State(root)
    s = st.load()
    marker = os.path.join(root, "restore-pending")
    if os.path.exists(marker):
        with open(marker) as f:
            uid = f.read().strip()
        log(f"finishing an interrupted restore of {uid}")
        restore(st, uid)
        rolled_back(s, uid)
        st.save(s)
    if s.get("pending"):
        # Stopped mid-trial: the pending version never committed.
        log(f"found uncommitted {s['pending']}; restoring")
        restore(st, s["update_id"])
        rolled_back(s, s["update_id"])
        st.save(s)
    p = start(st, s["active"], trial=False)
    p.commit()
    log(f"running {s['active']}")
    req = os.path.join(root, "update-request")
    while True:
        time.sleep(0.2)
        if p.p.poll() is not None:
            log(f"daemon exited {p.p.returncode}; restarting in 1 s")
            stop(p)
            time.sleep(1)
            p = start(st, s["active"], trial=False)
            p.commit()
        if not os.path.exists(req):
            continue
        with open(req) as f:
            target = f.read().strip()
        os.remove(req)
        s = st.load()
        if target in s.get("never_retry", []):
            log(f"refused {target}: it rolled back before")
            continue
        try:
            with open(os.path.join(root, "versions", target, "manifest.json")) as f:
                man = json.load(f)
        except (OSError, ValueError) as e:
            log(f"refused {target}: can't read its manifest ({e})")
            continue
        if man.get("launcher_protocol", 1) > PROTOCOL:
            log(f"refused {target}: needs launcher protocol {man['launcher_protocol']}")
            continue
        ok, size, free, need = free_enough(st)
        if not ok:
            log(f"refused {target}: {free} bytes free, need {need} (database {size}); nothing was stopped")
            continue
        uid = f"u{int(time.time() * 1000)}"
        s.update(pending=target, update_id=uid, rollback=s["active"])
        st.save(s)  # recorded before anything stops
        log(f"update {uid}: {s['active']} -> {target}")
        stop(p)
        snapshot(st, uid)
        t = start(st, target, trial=True)
        got = wait_prepared(t, PREPARE_TIMEOUT)
        if got == "prepared":
            s.update(active=target, pending=None, outcome={"id": uid, "from": s["rollback"], "to": target, "outcome": "committed"})
            st.save(s)  # committed durably before the daemon is told
            t.commit()
            p = t
            log(f"committed {target}")
        else:
            stop(t)
            log(f"trial {got}; restoring the snapshot")
            restore(st, uid)
            rolled_back(s, uid)
            st.save(s)
            p = start(st, s["active"], trial=False)
            p.commit()
            log(f"back on {s['active']}")


if __name__ == "__main__":
    cmd, root = sys.argv[1], sys.argv[2]
    if cmd == "run":
        signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
        run(root)
    elif cmd == "update":
        write_atomic(os.path.join(root, "update-request"), sys.argv[3])
