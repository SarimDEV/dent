//! Each remote machine's event log, kept on this device
//! ([decision 0068](../../docs/decisions/0068-files-across-machines.md)), so
//! a launch draws every machine's sessions at once (an offline machine's
//! too) and a link resumes where the last one left off instead of from the
//! start.
//!
//! It's only ever a copy: tagged with the epoch of the log it came from and
//! thrown away when the machine's log is a different one, written in
//! transactions, and deleted and begun again when it's damaged or in a
//! format this build doesn't know. Readable by this user only.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use dent_protocol::Event;
use rusqlite::{Connection, OptionalExtension, params};

/// The format of the file; another one is begun again.
const VERSION: i64 = 1;

enum Write {
    /// Events from `machine`'s log `epoch`, in order.
    Save {
        machine: String,
        epoch: String,
        events: Vec<Event>,
    },
    /// `machine`'s log is now `epoch`: what was kept from another goes.
    Reset {
        machine: String,
        epoch: String,
    },
    Forget(String),
    /// Answered once every write before it is done (tests, shutdown).
    Flush(mpsc::Sender<()>),
}

/// The kept logs. Cloned freely; writes go in order on a thread of their
/// own, so neither the window nor a link ever waits on the disk.
#[derive(Clone)]
pub struct History {
    db: Arc<Mutex<Connection>>,
    tx: mpsc::Sender<Write>,
}

/// One machine's kept log.
pub struct Kept {
    pub epoch: String,
    /// The newest event kept: a link subscribes after it.
    pub last_seq: u64,
    pub events: Vec<Event>,
}

impl History {
    /// The kept logs in `dir` (made, readable by this user only, if need
    /// be). A file that can't be read as this format is begun again.
    pub fn open(dir: &Path) -> anyhow::Result<History> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let path = dir.join("history.db");
        let db = match connect(&path) {
            Ok(db) => db,
            Err(e) => {
                eprintln!("dent: the kept history couldn't be read ({e:#}); starting it again");
                remove(&path);
                connect(&path)?
            }
        };
        let db = Arc::new(Mutex::new(db));
        let (tx, rx) = mpsc::channel();
        let writer = db.clone();
        std::thread::Builder::new()
            .name("history".into())
            .spawn(move || {
                // Machines whose history couldn't be kept this run: nothing
                // more of theirs is kept until their log starts again, so
                // no gap is ever kept as if whole.
                let mut broken: std::collections::HashSet<String> = Default::default();
                for w in rx {
                    match &w {
                        Write::Save { machine, .. } if broken.contains(machine) => continue,
                        Write::Reset { machine, .. } | Write::Forget(machine) => {
                            broken.remove(machine);
                        }
                        _ => {}
                    }
                    let machine = match &w {
                        Write::Save { machine, .. } | Write::Reset { machine, .. } => {
                            Some(machine.clone())
                        }
                        _ => None,
                    };
                    if let Ok(mut db) = writer.lock()
                        && let Err(e) = write(&mut db, w)
                    {
                        // A batch not kept would leave a gap the next link
                        // resumes past: what's kept of that machine goes, and
                        // its history comes again from the start.
                        eprintln!("dent: couldn't keep history: {e:#}");
                        if let Some(m) = machine {
                            let _ = write(&mut db, Write::Forget(m.clone()));
                            broken.insert(m);
                        }
                    }
                }
            })?;
        Ok(History { db, tx })
    }

    /// `machine`'s kept log, if there is one.
    pub fn load(&self, machine: &str) -> Option<Kept> {
        let db = self.db.lock().ok()?;
        let read = || -> rusqlite::Result<Option<Kept>> {
            let Some((epoch, last_seq)) = db
                .query_row(
                    "SELECT epoch, last_seq FROM feeds WHERE machine = ?1",
                    params![machine],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()?
            else {
                return Ok(None);
            };
            let mut q = db.prepare("SELECT body FROM events WHERE machine = ?1 ORDER BY seq")?;
            let mut events = Vec::new();
            for body in q.query_map(params![machine], |r| r.get::<_, String>(0))? {
                // One that doesn't read as an event (damage, a newer build):
                // none of it is used, rather than a history with a hole in
                // it; it comes again from the daemon.
                match serde_json::from_str::<Event>(&body?) {
                    Ok(e) => events.push(e),
                    Err(_) => return Ok(None),
                }
            }
            Ok(Some(Kept {
                epoch,
                last_seq: last_seq as u64,
                events,
            }))
        };
        match read() {
            Ok(Some(k)) => Some(k),
            Ok(None) => {
                // Not readable whole: what's kept goes, and comes again.
                drop(db);
                self.forget(machine);
                None
            }
            Err(_) => None,
        }
    }

    /// Keep `events` from `machine`'s log `epoch`, after the ones before.
    pub fn save(&self, machine: &str, epoch: &str, events: Vec<Event>) {
        if machine.is_empty() || events.is_empty() {
            return;
        }
        let _ = self.tx.send(Write::Save {
            machine: machine.into(),
            epoch: epoch.into(),
            events,
        });
    }

    /// `machine`'s log is now `epoch` (a resync): nothing from another is
    /// kept.
    pub fn reset(&self, machine: &str, epoch: &str) {
        let _ = self.tx.send(Write::Reset {
            machine: machine.into(),
            epoch: epoch.into(),
        });
    }

    /// Forget everything kept from `machine`.
    pub fn forget(&self, machine: &str) {
        let _ = self.tx.send(Write::Forget(machine.into()));
    }

    /// Wait until every write so far is on disk.
    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Write::Flush(tx)).is_ok() {
            let _ = rx.recv();
        }
    }
}

fn connect(path: &PathBuf) -> anyhow::Result<Connection> {
    use std::os::unix::fs::PermissionsExt;
    let db = Connection::open(path)?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "NORMAL")?;
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    match version {
        0 => {
            db.execute_batch(
                "CREATE TABLE feeds (
                    machine TEXT PRIMARY KEY,
                    epoch TEXT NOT NULL,
                    last_seq INTEGER NOT NULL
                );
                CREATE TABLE events (
                    machine TEXT NOT NULL,
                    seq INTEGER NOT NULL,
                    body TEXT NOT NULL,
                    PRIMARY KEY (machine, seq)
                ) WITHOUT ROWID;",
            )?;
            db.pragma_update(None, "user_version", VERSION)?;
        }
        VERSION => {}
        v => anyhow::bail!("format {v}, not {VERSION}"),
    }
    // A damaged file fails here, not halfway through a read.
    let ok: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if ok != "ok" {
        anyhow::bail!("{ok}");
    }
    Ok(db)
}

fn remove(path: &Path) {
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
    }
}

fn write(db: &mut Connection, w: Write) -> rusqlite::Result<()> {
    match w {
        Write::Save {
            machine,
            epoch,
            events,
        } => {
            let t = db.transaction()?;
            let kept: Option<String> = t
                .query_row(
                    "SELECT epoch FROM feeds WHERE machine = ?1",
                    params![machine],
                    |r| r.get(0),
                )
                .optional()?;
            // Events from another log than the one kept: that one goes.
            if kept.as_deref() != Some(epoch.as_str()) {
                t.execute("DELETE FROM events WHERE machine = ?1", params![machine])?;
                t.execute(
                    "INSERT OR REPLACE INTO feeds (machine, epoch, last_seq) VALUES (?1, ?2, 0)",
                    params![machine, epoch],
                )?;
            }
            {
                let mut ins = t.prepare_cached(
                    "INSERT OR IGNORE INTO events (machine, seq, body) VALUES (?1, ?2, ?3)",
                )?;
                for e in &events {
                    let body = serde_json::to_string(e)
                        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                    ins.execute(params![machine, e.seq as i64, body])?;
                }
            }
            let last = events.iter().map(|e| e.seq).max().unwrap_or(0) as i64;
            t.execute(
                "UPDATE feeds SET last_seq = MAX(last_seq, ?2) WHERE machine = ?1",
                params![machine, last],
            )?;
            t.commit()
        }
        Write::Reset { machine, epoch } => {
            let t = db.transaction()?;
            t.execute("DELETE FROM events WHERE machine = ?1", params![machine])?;
            t.execute(
                "INSERT OR REPLACE INTO feeds (machine, epoch, last_seq) VALUES (?1, ?2, 0)",
                params![machine, epoch],
            )?;
            t.commit()
        }
        Write::Forget(machine) => {
            let t = db.transaction()?;
            t.execute("DELETE FROM events WHERE machine = ?1", params![machine])?;
            t.execute("DELETE FROM feeds WHERE machine = ?1", params![machine])?;
            t.commit()
        }
        Write::Flush(done) => {
            let _ = done.send(());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dent_protocol::Body;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dent-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn ev(seq: u64) -> Event {
        Event {
            seq,
            session: "s1".into(),
            at: seq,
            body: Body::Delivered {
                id: format!("m{seq}"),
            },
        }
    }

    #[test]
    fn kept_in_order_and_by_log() {
        let d = dir("order");
        let h = History::open(&d).unwrap();
        assert!(h.load("m").is_none());
        h.save("m", "e1", vec![ev(1), ev(2)]);
        // Sent again after a reconnect: kept once.
        h.save("m", "e1", vec![ev(2), ev(3)]);
        h.flush();
        let k = h.load("m").unwrap();
        assert_eq!((k.epoch.as_str(), k.last_seq), ("e1", 3));
        assert_eq!(
            k.events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        // Another log: the old one's events go.
        h.save("m", "e2", vec![ev(1)]);
        h.flush();
        let k = h.load("m").unwrap();
        assert_eq!((k.epoch.as_str(), k.last_seq, k.events.len()), ("e2", 1, 1));
        h.reset("m", "e3");
        h.flush();
        let k = h.load("m").unwrap();
        assert_eq!((k.epoch.as_str(), k.last_seq, k.events.len()), ("e3", 0, 0));
        h.forget("m");
        h.flush();
        assert!(h.load("m").is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn kept_across_a_restart_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("restart");
        {
            let h = History::open(&d).unwrap();
            h.save("m", "e1", vec![ev(1)]);
            h.flush();
        }
        let h = History::open(&d).unwrap();
        assert_eq!(h.load("m").unwrap().last_seq, 1);
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&d), 0o700);
        assert_eq!(mode(&d.join("history.db")), 0o600);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_damaged_or_unknown_file_is_begun_again() {
        let d = dir("damaged");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("history.db"), b"not a database at all, just bytes").unwrap();
        let h = History::open(&d).unwrap();
        assert!(h.load("m").is_none());
        h.save("m", "e1", vec![ev(1)]);
        h.flush();
        drop(h);
        // A newer build's format.
        {
            let db = Connection::open(d.join("history.db")).unwrap();
            db.pragma_update(None, "user_version", 99).unwrap();
        }
        let h = History::open(&d).unwrap();
        assert!(h.load("m").is_none(), "not read in part");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
