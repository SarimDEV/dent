//! The event log: one SQLite file per daemon, WAL, one writer.
//!
//! Every event goes in `events` with the next sequence number. `sessions`
//! is a projection of those events kept in the same transaction, so listing
//! sessions never replays logs. When an event came from agent output, the
//! same write records how far into that process's output the daemon has
//! read, so after a restart it resumes from exactly there.

use std::path::Path;

use anyhow::{Context, Result};
use dent_protocol::{Body, Event, Permission, Status};
use rusqlite::{Connection, OptionalExtension, params};

pub struct Store {
    db: Connection,
}

/// A session as the daemon needs it to carry on after a restart.
#[derive(Debug, Clone)]
pub struct Row {
    pub id: String,
    pub cwd: String,
    pub model: Option<String>,
    pub permission: Permission,
    pub agent_session: Option<String>,
    pub status: Status,
    pub proc: Option<String>,
    pub proc_off: u64,
    pub updated: u64,
}

/// Where in a process's output an event came from.
pub struct Source<'a> {
    pub proc: &'a str,
    pub off: u64,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;
             CREATE TABLE IF NOT EXISTS events (
               seq INTEGER PRIMARY KEY AUTOINCREMENT,
               session TEXT NOT NULL,
               at INTEGER NOT NULL,
               body TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS events_by_session ON events (session, seq);
             CREATE TABLE IF NOT EXISTS sessions (
               id TEXT PRIMARY KEY,
               cwd TEXT NOT NULL,
               model TEXT,
               permission TEXT NOT NULL,
               title TEXT NOT NULL,
               agent_session TEXT,
               status TEXT NOT NULL,
               closed INTEGER NOT NULL DEFAULT 0,
               created INTEGER NOT NULL,
               updated INTEGER NOT NULL,
               proc TEXT,
               proc_off INTEGER NOT NULL DEFAULT 0,
               procs INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        Ok(Self { db })
    }

    #[cfg(test)]
    pub fn last_seq(&self) -> Result<u64> {
        Ok(self
            .db
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?)
    }

    /// Append one event and update the session's row to match, in one
    /// transaction.
    pub fn append(
        &mut self,
        session: &str,
        at: u64,
        body: &Body,
        src: Option<Source<'_>>,
    ) -> Result<Event> {
        let json = serde_json::to_string(body)?;
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO events (session, at, body) VALUES (?1, ?2, ?3)",
            params![session, at as i64, json],
        )?;
        let seq = tx.last_insert_rowid() as u64;
        project(&tx, session, at, body)?;
        tx.execute(
            "UPDATE sessions SET updated = ?2 WHERE id = ?1",
            params![session, at as i64],
        )?;
        if let Some(src) = src {
            tx.execute(
                "UPDATE sessions SET proc = ?2, proc_off = ?3 WHERE id = ?1",
                params![session, src.proc, src.off as i64],
            )?;
        }
        tx.commit()?;
        Ok(Event {
            seq,
            session: session.into(),
            at,
            body: body.clone(),
        })
    }

    /// Name the next agent process for a session and make it current.
    pub fn next_proc(&self, session: &str) -> Result<String> {
        let n: i64 = self.db.query_row(
            "UPDATE sessions SET procs = procs + 1 WHERE id = ?1 RETURNING procs",
            params![session],
            |r| r.get(0),
        )?;
        let proc = format!("{session}-{n}");
        self.db.execute(
            "UPDATE sessions SET proc = ?2, proc_off = 0 WHERE id = ?1",
            params![session, proc],
        )?;
        Ok(proc)
    }

    /// The session's agent process has exited and been handled.
    pub fn clear_proc(&self, session: &str) -> Result<()> {
        self.db.execute(
            "UPDATE sessions SET proc = NULL, proc_off = 0 WHERE id = ?1",
            params![session],
        )?;
        Ok(())
    }

    pub fn sessions(&self) -> Result<Vec<Row>> {
        let mut st = self.db.prepare(
            "SELECT id, cwd, model, permission, agent_session, status, proc, proc_off, updated
             FROM sessions ORDER BY updated DESC",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    cwd: r.get(1)?,
                    model: r.get(2)?,
                    permission: from_text(&r.get::<_, String>(3)?),
                    agent_session: r.get(4)?,
                    status: from_text(&r.get::<_, String>(5)?),
                    proc: r.get(6)?,
                    proc_off: r.get::<_, i64>(7)? as u64,
                    updated: r.get::<_, i64>(8)? as u64,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn session(&self, id: &str) -> Result<Option<Row>> {
        Ok(self.sessions()?.into_iter().find(|r| r.id == id))
    }

    /// Events after `after`, up to about `max_bytes` of them (at least one),
    /// and the last seq looked at: rows this daemon can't read are skipped,
    /// so the batch can be empty while the log goes on.
    pub fn after(&self, after: u64, max_bytes: usize) -> Result<(Vec<Event>, u64)> {
        let mut st = self.db.prepare_cached(
            "SELECT seq, session, at, body FROM events WHERE seq > ?1 ORDER BY seq LIMIT 2000",
        )?;
        let mut rows = st.query(params![after as i64])?;
        let mut out = Vec::new();
        let mut bytes = 0;
        let mut upto = after;
        while let Some(r) = rows.next()? {
            let seq = r.get::<_, i64>(0)? as u64;
            upto = seq;
            let body: String = r.get(3)?;
            bytes += body.len() + 64;
            let Ok(parsed) = serde_json::from_str::<Body>(&body) else {
                // Written by a newer daemon and since downgraded: skip it.
                continue;
            };
            out.push(Event {
                seq,
                session: r.get(1)?,
                at: r.get::<_, i64>(2)? as u64,
                body: parsed,
            });
            if bytes >= max_bytes {
                break;
            }
        }
        Ok((out, upto))
    }

    /// Every event of one session, oldest first.
    pub fn session_events(&self, session: &str) -> Result<Vec<Event>> {
        let mut st = self
            .db
            .prepare_cached("SELECT seq, at, body FROM events WHERE session = ?1 ORDER BY seq")?;
        let rows = st
            .query_map(params![session], |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(seq, at, body)| {
                Some(Event {
                    seq,
                    session: session.into(),
                    at,
                    body: serde_json::from_str(&body).ok()?,
                })
            })
            .collect())
    }

    pub fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn put(&self, key: &str, value: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Every key starting with `prefix`, with its value.
    pub fn with_prefix(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        let mut q = self
            .db
            .prepare("SELECT key, value FROM kv WHERE substr(key, 1, ?2) = ?1")?;
        let rows = q
            .query_map(params![prefix, prefix.len() as i64], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn del(&self, key: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM kv WHERE key = ?1", params![key])?;
        Ok(())
    }

    /// Fold the write-ahead log back into the database.
    pub fn checkpoint(&self) -> Result<()> {
        self.db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

fn to_text<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn from_text<T: serde::de::DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_value(serde_json::Value::String(s.into())).unwrap_or_default()
}

fn project(tx: &rusqlite::Transaction<'_>, session: &str, at: u64, body: &Body) -> Result<()> {
    match body {
        Body::Created {
            cwd,
            model,
            permission,
            title,
            ..
        } => {
            tx.execute(
                "INSERT INTO sessions (id, cwd, model, permission, title, status, created, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    session,
                    cwd,
                    model,
                    to_text(permission),
                    title,
                    to_text(&Status::New),
                    at as i64
                ],
            )?;
        }
        Body::Settings {
            model,
            permission,
            title,
            ..
        } => {
            if let Some(m) = model {
                tx.execute(
                    "UPDATE sessions SET model = ?2 WHERE id = ?1",
                    params![session, m],
                )?;
            }
            if let Some(p) = permission {
                tx.execute(
                    "UPDATE sessions SET permission = ?2 WHERE id = ?1",
                    params![session, to_text(p)],
                )?;
            }
            if let Some(t) = title {
                tx.execute(
                    "UPDATE sessions SET title = ?2 WHERE id = ?1",
                    params![session, t],
                )?;
            }
        }
        Body::Status { status } => {
            tx.execute(
                "UPDATE sessions SET status = ?2 WHERE id = ?1",
                params![session, to_text(status)],
            )?;
        }
        Body::Agent {
            agent_session: Some(a),
            ..
        } => {
            tx.execute(
                "UPDATE sessions SET agent_session = ?2 WHERE id = ?1",
                params![session, a],
            )?;
        }
        Body::Closed | Body::Reopened => {
            tx.execute(
                "UPDATE sessions SET closed = ?2 WHERE id = ?1",
                params![session, matches!(body, Body::Closed) as i64],
            )?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created() -> Body {
        Body::Created {
            agent: "claude".into(),
            cwd: "/tmp".into(),
            model: None,
            permission: Permission::Ask,
            title: "t".into(),
        }
    }

    #[test]
    fn appends_project_and_survive_reopening() {
        let dir = std::env::temp_dir().join(format!("dent-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dent.db");
        {
            let mut s = Store::open(&path).unwrap();
            s.append("a", 1, &created(), None).unwrap();
            s.append(
                "a",
                2,
                &Body::Status {
                    status: Status::Working,
                },
                Some(Source {
                    proc: "a-1",
                    off: 120,
                }),
            )
            .unwrap();
            s.append("a", 3, &Body::Closed, None).unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.last_seq().unwrap(), 3);
        let row = s.session("a").unwrap().unwrap();
        assert_eq!(row.status, Status::Working);
        assert_eq!(row.proc.as_deref(), Some("a-1"));
        assert_eq!(row.proc_off, 120);
        assert_eq!(s.after(1, 1 << 20).unwrap().0.len(), 2);
        assert_eq!(s.after(0, 1).unwrap().0.len(), 1);
        // A row this daemon can't read is passed over, not the end.
        s.db.execute(
            "INSERT INTO events (session, at, body) VALUES ('a', 4, '{\"type\":\"from_the_future\"}')",
            [],
        )
        .unwrap();
        let mut s = s;
        s.append("a", 5, &Body::Reopened, None).unwrap();
        let (got, upto) = s.after(3, 1 << 20).unwrap();
        assert_eq!((got.len(), got[0].seq, upto), (1, 5, 5));
        // Nothing readable left in a batch still says how far it got.
        s.db.execute(
            "INSERT INTO events (session, at, body) VALUES ('a', 6, '{\"type\":\"from_the_future\"}')",
            [],
        )
        .unwrap();
        assert_eq!(s.after(5, 1 << 20).unwrap(), (Vec::new(), 6));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
