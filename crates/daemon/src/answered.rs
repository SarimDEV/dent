//! Changes answered once ([decision 0078]). A client sends a change with a
//! key ([`dent_protocol::Envelope::key`]), the same each time it sends it.
//! The first copy runs; its answer is kept. A copy that comes after (its
//! answer was lost on a link that dropped, or as this daemon restarted)
//! gets that answer again and does nothing; one that comes while the first
//! still runs waits for it and gets the same.
//!
//! What can't be done twice (`Resend::Once`: a commit, a new project) is
//! written down before it runs, and its answer after, on disk for a day: a
//! daemon that stopped partway through one answers a copy with "may or may
//! not have happened" rather than doing it again. What can be repeated is
//! kept in memory only. A job that ends without answering (it panicked, or
//! the daemon stopped) is answered for by its [`Job`] guard, so no copy
//! waits for ever.
//!
//! [decision 0078]: ../../../docs/decisions/0078-requests-that-fail-well.md

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dent_protocol::Out;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

/// How many answers are kept, and for how long.
const KEPT: usize = 2000;
const KEPT_FOR: Duration = Duration::from_secs(24 * 3600);

/// What a copy of a change hears when the daemon stopped partway through it.
pub const STOPPED_PARTWAY: &str = "dent stopped partway through that, so it may or may not have happened; look before trying again";

#[derive(Clone, Serialize, Deserialize)]
struct Kept {
    key: String,
    /// Unix milliseconds.
    at: u64,
    /// Can't be done twice: kept on disk, from before it runs.
    #[serde(default)]
    once: bool,
    /// None while it runs (or if the daemon stopped partway through it).
    #[serde(default)]
    out: Option<Out>,
}

/// What's known of a key.
pub enum Seen {
    /// Never seen: run it, then answer through its [`Job`].
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
    /// What's kept in `path` (`answered.json` in the state folder). One
    /// begun and never answered was cut off by a stop: it says so.
    pub fn open(path: PathBuf) -> Self {
        let mut done: VecDeque<Kept> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let old = now_ms().saturating_sub(KEPT_FOR.as_millis() as u64);
        done.retain(|k| k.at >= old);
        for k in &mut done {
            if k.out.is_none() {
                k.out = Some(Out::Error {
                    message: STOPPED_PARTWAY.into(),
                });
            }
        }
        Self {
            path,
            done: Mutex::new(done),
            running: Mutex::default(),
        }
    }

    /// Take `key` on: new, answered, or running. A new one that can't be
    /// done twice (`once`) is written down before it runs.
    pub fn begin(&self, key: &str, once: bool) -> Seen {
        let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(out) = done
            .iter()
            .rev()
            .find(|k| k.key == key)
            .and_then(|k| k.out.clone())
        {
            return Seen::Done(Box::new(out));
        }
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = running.get(key) {
            return Seen::Running(tx.subscribe());
        }
        running.insert(key.to_string(), watch::channel(None).0);
        if once {
            done.push_back(Kept {
                key: key.to_string(),
                at: now_ms(),
                once,
                out: None,
            });
            self.save(&mut done);
        }
        Seen::New
    }

    /// `key` answered `out`: kept, and said to any copy waiting.
    pub fn finish(&self, key: &str, out: &Out) {
        {
            let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
            let once = match done.iter_mut().rev().find(|k| k.key == key) {
                Some(k) => {
                    k.out = Some(out.clone());
                    k.once
                }
                None => {
                    done.push_back(Kept {
                        key: key.to_string(),
                        at: now_ms(),
                        once: false,
                        out: Some(out.clone()),
                    });
                    false
                }
            };
            if once {
                self.save(&mut done);
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

    /// Trimmed, and what can't be done twice written whole, by temp file
    /// and rename: a crash keeps the old.
    fn save(&self, done: &mut VecDeque<Kept>) {
        while done.len() > KEPT {
            done.pop_front();
        }
        let once: Vec<&Kept> = done.iter().filter(|k| k.once).collect();
        if let Ok(bytes) = serde_json::to_vec(&once) {
            let tmp = self.path.with_extension("json.new");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

/// A keyed change at work: answers for it if it ends without answering
/// (it panicked, or the daemon is stopping), so no copy waits for ever.
pub struct Job {
    answered: Option<(Arc<crate::daemon::Daemon>, String)>,
}

impl Job {
    pub fn new(d: &Arc<crate::daemon::Daemon>, key: Option<&String>) -> Self {
        Self {
            answered: key.map(|k| (d.clone(), k.clone())),
        }
    }

    /// It answered `out`.
    pub fn finish(mut self, out: &Out) {
        if let Some((d, key)) = self.answered.take() {
            d.answered.finish(&key, out);
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        if let Some((d, key)) = self.answered.take() {
            d.answered.finish(
                &key,
                &Out::Error {
                    message: STOPPED_PARTWAY.into(),
                },
            );
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
        assert!(matches!(a.begin("k1", false), Seen::New));
        a.finish("k1", &ok());
        assert!(matches!(a.begin("k1", false), Seen::Done(o) if *o == ok()));
        assert!(
            matches!(a.begin("k2", false), Seen::New),
            "another key is its own"
        );
    }

    #[tokio::test]
    async fn a_copy_while_the_first_runs_waits_for_its_answer() {
        let a = Answered::open(scratch("running"));
        assert!(matches!(a.begin("k", false), Seen::New));
        let Seen::Running(mut rx) = a.begin("k", false) else {
            panic!("the second copy waits");
        };
        a.finish("k", &ok());
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow().clone(), Some(ok()));
    }

    /// A restart is when answers get lost: what can't be done twice and
    /// was answered before it is still known after.
    #[test]
    fn answers_that_cant_be_repeated_outlive_a_restart() {
        let path = scratch("restart");
        let a = Answered::open(path.clone());
        a.begin("once", true);
        a.finish("once", &ok());
        a.begin("again", false);
        a.finish("again", &ok());
        drop(a);
        let b = Answered::open(path);
        assert!(matches!(b.begin("once", true), Seen::Done(o) if *o == ok()));
        assert!(
            matches!(b.begin("again", false), Seen::New),
            "a repeatable change isn't written to disk"
        );
    }

    /// Stopped partway through something that can't be done twice: after
    /// the restart, a copy hears it may or may not have happened, and it
    /// isn't done again.
    #[test]
    fn one_cut_off_by_a_stop_is_never_done_again() {
        let path = scratch("cut");
        let a = Answered::open(path.clone());
        assert!(matches!(a.begin("commit", true), Seen::New));
        drop(a);
        let b = Answered::open(path);
        assert!(matches!(
            b.begin("commit", true),
            Seen::Done(o) if matches!(&*o, Out::Error { message } if message == STOPPED_PARTWAY)
        ));
    }
}
