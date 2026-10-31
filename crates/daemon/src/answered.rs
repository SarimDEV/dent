//! Changes answered once ([decision 0078]). A client sends a change with a
//! key ([`dent_protocol::Envelope::key`]), the same each time it sends it.
//! The first copy runs; its answer is kept, here and on disk, for a day. A
//! copy that comes after (its answer was lost on a link that dropped, or as
//! this daemon restarted) gets that answer again and does nothing; one that
//! comes while the first still runs waits for it and gets the same.
//!
//! [decision 0078]: ../../../docs/decisions/0078-requests-that-fail-well.md

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use dent_protocol::Out;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

/// How many answers are kept, and for how long.
const KEPT: usize = 2000;
const KEPT_FOR: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone, Serialize, Deserialize)]
struct Kept {
    key: String,
    /// Unix milliseconds.
    at: u64,
    out: Out,
}

/// What's known of a key.
pub enum Seen {
    /// Never seen: run it, then [`Answered::finish`].
    New,
    /// Answered before: this is that answer.
    Done(Box<Out>),
    /// Running now: its answer comes on this.
    Running(watch::Receiver<Option<Out>>),
}

pub struct Answered {
    path: PathBuf,
    done: Mutex<VecDeque<Kept>>,
    running: Mutex<HashMap<String, watch::Sender<Option<Out>>>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Answered {
    /// What's kept in `path` (`answered.json` in the state folder).
    pub fn open(path: PathBuf) -> Self {
        let mut done: VecDeque<Kept> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let old = now_ms().saturating_sub(KEPT_FOR.as_millis() as u64);
        done.retain(|k| k.at >= old);
        Self {
            path,
            done: Mutex::new(done),
            running: Mutex::default(),
        }
    }

    /// Take `key` on: new, answered, or running.
    pub fn begin(&self, key: &str) -> Seen {
        let done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(k) = done.iter().rev().find(|k| k.key == key) {
            return Seen::Done(Box::new(k.out.clone()));
        }
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = running.get(key) {
            return Seen::Running(tx.subscribe());
        }
        running.insert(key.to_string(), watch::channel(None).0);
        Seen::New
    }

    /// `key` ran and answered `out`: kept, and said to any copy waiting.
    pub fn finish(&self, key: &str, out: &Out) {
        {
            let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
            done.push_back(Kept {
                key: key.to_string(),
                at: now_ms(),
                out: out.clone(),
            });
            while done.len() > KEPT {
                done.pop_front();
            }
            // Written whole, by temp file and rename: a crash keeps the old.
            if let Ok(bytes) = serde_json::to_vec(&*done) {
                let tmp = self.path.with_extension("json.new");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, &self.path);
                }
            }
        }
        if let Some(tx) = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key)
        {
            let _ = tx.send(Some(out.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dent-answered-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("answered.json")
    }

    fn ok() -> Out {
        Out::Error {
            message: "first answer".into(),
        }
    }

    #[test]
    fn a_key_runs_once_and_its_copies_get_the_first_answer() {
        let a = Answered::open(scratch("once"));
        assert!(matches!(a.begin("k1"), Seen::New));
        a.finish("k1", &ok());
        assert!(matches!(a.begin("k1"), Seen::Done(o) if *o == ok()));
        assert!(matches!(a.begin("k2"), Seen::New), "another key is its own");
    }

    #[tokio::test]
    async fn a_copy_while_the_first_runs_waits_for_its_answer() {
        let a = Answered::open(scratch("running"));
        assert!(matches!(a.begin("k"), Seen::New));
        let Seen::Running(mut rx) = a.begin("k") else {
            panic!("the second copy waits");
        };
        a.finish("k", &ok());
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow().clone(), Some(ok()));
    }

    /// A restart is when answers get lost: what was answered before it is
    /// still known after.
    #[test]
    fn answers_outlive_a_restart() {
        let path = scratch("restart");
        let a = Answered::open(path.clone());
        a.begin("k");
        a.finish("k", &ok());
        drop(a);
        let again = Answered::open(path);
        assert!(matches!(again.begin("k"), Seen::Done(o) if *o == ok()));
    }
}
