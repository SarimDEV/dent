//! Every machine the window talks to: this one's daemon over its socket
//! ([`crate::live`]) and each saved remote machine over `dent-remote`
//! ([remote-plan.md], stage 1). Remote sessions carry their machine in
//! their ids ([`dent_client_core::fleet`]), so a request goes to the machine
//! whose sessions it names; one that names none goes to this machine unless
//! sent to another with [`Net::send_to`]. Request numbers are shared, so a
//! reply's number says which request it answers whichever machine sent it.
//!
//! [remote-plan.md]: ../../docs/remote-plan.md

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dent_client_core::fleet;
use dent_client_core::machine::Signal;
use dent_protocol::Request;
use dent_remote::{Remote, Saved};
use futures::channel::mpsc::UnboundedSender;

use crate::live::{Incoming, Live};

/// The tag a machine being added goes by until its welcome names it.
pub fn adding_tag(addr: &str) -> String {
    format!("+{addr}")
}

struct Far {
    remote: Remote,
}

#[derive(Clone)]
pub struct Net {
    local: Live,
    next: Arc<AtomicU64>,
    ui: UnboundedSender<Incoming>,
    rt: Option<Arc<tokio::runtime::Runtime>>,
    keys: Option<dent_link::Keys>,
    far: Arc<Mutex<HashMap<String, Far>>>,
    /// Each machine's log, kept on this Mac ([decision 0068]).
    ///
    /// [decision 0068]: ../../../docs/decisions/0068-files-across-machines.md
    history: Option<dent_remote::history::History>,
    /// Every request still waiting for its answer, and for how long
    /// ([decision 0078](../../../docs/decisions/0078-requests-that-fail-well.md)).
    inflight: Arc<Mutex<dent_client_core::inflight::Inflight>>,
}

impl Net {
    /// This machine's daemon, and every saved machine.
    pub fn start(local: Live, ui: UnboundedSender<Incoming>) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("dent-remote")
            .enable_all()
            .build()
            .map_err(|e| eprintln!("remote machines are off: {e}"))
            .ok()
            .map(Arc::new);
        let keys = dent_remote::store::device_keys()
            .map_err(|e| eprintln!("remote machines are off: {e}"))
            .ok();
        let history = dent_remote::history::History::open(&dent_remote::store::state_dir())
            .map_err(|e| eprintln!("history won't be kept: {e:#}"))
            .ok();
        let net = Self {
            local,
            next: Arc::new(AtomicU64::new(100)),
            ui,
            rt,
            keys,
            far: Arc::new(Mutex::new(HashMap::new())),
            history,
            inflight: Arc::default(),
        };
        // Off the window's thread: reading each machine's kept history
        // can take a while, and each link starts only once its history is
        // handed over, so nothing it says comes before what's kept.
        let n = net.clone();
        std::thread::spawn(move || {
            for s in dent_remote::store::load() {
                // One forgotten meanwhile isn't followed again.
                if dent_remote::store::load().iter().any(|x| x.id == s.id) {
                    n.follow(s);
                }
            }
        });
        net
    }

    fn number(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// Send a request to the machine whose sessions it names; its reply
    /// comes back with the returned number.
    pub fn send(&self, mut r: Request) -> u64 {
        let n = self.number();
        match fleet::route(&mut r) {
            Some(m) => self.deliver(&m, n, r),
            None => eprintln!("a request named sessions on different machines: {r:?}"),
        }
        n
    }

    /// Send a request that names no session to `machine` ("" is this one).
    pub fn send_to(&self, machine: &str, r: Request) -> u64 {
        let n = self.number();
        self.deliver(machine, n, r);
        n
    }

    /// The first answer to request `n` came: it's settled.
    pub fn answered(&self, n: u64) {
        if let Ok(mut f) = self.inflight.lock() {
            f.answered(n);
        }
    }

    /// Request `n` is still being worked on, its machine says.
    pub fn alive(&self, n: u64) {
        if let Ok(mut f) = self.inflight.lock() {
            f.alive(n);
        }
    }

    /// Requests that have waited past their patience while their machine
    /// was connected (`connected`), taken out to be ended in words.
    pub fn overdue(
        &self,
        now: std::time::Instant,
        link: impl Fn(&str) -> dent_client_core::inflight::Link,
    ) -> Vec<dent_client_core::inflight::Overdue> {
        self.inflight
            .lock()
            .map(|mut f| f.tick(now, link))
            .unwrap_or_default()
    }

    fn deliver(&self, machine: &str, n: u64, r: Request) {
        if let Ok(mut f) = self.inflight.lock() {
            f.asked(n, machine, &r);
        }
        if machine.is_empty() {
            self.local.send_as(n, r);
        } else if let Some(f) = self
            .far
            .lock()
            .ok()
            .and_then(|f| f.get(machine).map(|f| f.remote.clone()))
        {
            f.send(n, r);
        }
        // A machine no longer followed: its request ends at once, as gone
        // (`Dent::end_overdue`).
    }

    /// Paths `machine` named in its own events: its files, which a
    /// message sent back there names rather than carries.
    pub fn theirs(&self, machine: &str, paths: Vec<String>) {
        if let Ok(f) = self.far.lock()
            && let Some(f) = f.get(machine)
        {
            f.remote.theirs(paths);
        }
    }

    /// Take back request `n` to `machine` if it never went out: whether it
    /// was. A machine not followed was never sent anything.
    pub fn cancel(&self, machine: &str, n: u64) -> bool {
        self.far
            .lock()
            .is_ok_and(|f| f.get(machine).is_none_or(|f| f.remote.cancel(n)))
    }

    /// Where this machine's feed is: reconnects subscribe from here.
    pub fn set_seq(&self, machine: &str, seq: u64) {
        if machine.is_empty() {
            self.local.seq.store(seq, Ordering::Relaxed);
        } else if let Ok(f) = self.far.lock()
            && let Some(f) = f.get(machine)
        {
            f.remote.seq.store(seq, Ordering::Relaxed);
        }
    }

    /// The remote machines being followed, by tag.
    pub fn tags(&self) -> Vec<String> {
        self.far
            .lock()
            .map(|f| f.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Follow a saved machine, under its id.
    pub fn follow(&self, s: Saved) {
        // "" is this machine: a record with no id (hand-edited, or very old)
        // would mix its sessions into this machine's.
        if s.id.is_empty() {
            return;
        }
        let tag = s.id.clone();
        self.spawn(tag, s);
    }

    /// Start adding the machine at `addr`: events come tagged
    /// [`adding_tag`] until its welcome names it, then by its id.
    pub fn add(&self, addr: &str) {
        let s = Saved {
            routes: vec![addr.to_string()],
            ..Saved::default()
        };
        self.spawn(adding_tag(addr), s);
    }

    fn spawn(&self, tag: String, s: Saved) {
        let (Some(rt), Some(keys)) = (&self.rt, &self.keys) else {
            return;
        };
        // What's kept from its log goes to the window first, as if it had
        // just come, so its sessions are there before any link (and while
        // it's away); the link then resumes after it. A machine being
        // added has nothing kept yet.
        // Held while its kept history goes to the window and its link is
        // made: a request for one of its sessions waits for the link
        // instead of being dropped.
        let Ok(mut far) = self.far.lock() else { return };
        let keeping = self.history.clone().map(|history| {
            let kept = (!s.id.is_empty()).then(|| history.load(&s.id)).flatten();
            let resume = kept.map(|k| {
                let _ = self.ui.unbounded_send(Incoming::Remote {
                    id: tag.clone(),
                    event: dent_remote::Event::Out {
                        req: 0,
                        out: dent_protocol::Out::Events { events: k.events },
                    },
                });
                (k.epoch, k.last_seq)
            });
            dent_remote::remote::Keeping { history, resume }
        });
        if let Some(old) = far.remove(&tag) {
            old.remote.stop();
        }
        // What its events are tagged with: a machine being added is
        // retagged with its id once the welcome names it.
        let t = Arc::new(Mutex::new(tag.clone()));
        let ui = self.ui.clone();
        let all = self.far.clone();
        let remote = dent_remote::remote::spawn_kept(
            rt.handle(),
            keys.clone(),
            "dent desktop",
            s,
            keeping,
            move |event| {
                if let dent_remote::Event::Saved(s) = &event
                    && let Err(e) = dent_remote::store::upsert(s)
                {
                    eprintln!("couldn't save {}: {e}", s.name);
                }
                // Named at last: from now on it goes by its id.
                if let dent_remote::Event::Saved(s) = &event
                    && let Ok(mut tag) = t.lock()
                    && *tag != s.id
                    && let Ok(mut far) = all.lock()
                    && let Some(f) = far.remove(&*tag)
                {
                    // A machine already saved, added again by another
                    // address: this link takes over from the old one.
                    if let Some(old) = far.insert(s.id.clone(), f) {
                        old.remote.stop();
                    }
                    let _ = ui.unbounded_send(Incoming::Renamed {
                        from: tag.clone(),
                        to: s.id.clone(),
                    });
                    *tag = s.id.clone();
                }
                let id = t.lock().map(|t| t.clone()).unwrap_or_default();
                let _ = ui.unbounded_send(Incoming::Remote { id, event });
            },
        );
        far.insert(tag, Far { remote });
    }

    /// Stop following a machine; with `forget`, it's no longer saved.
    pub fn stop(&self, id: &str, forget: bool) {
        if let Ok(mut far) = self.far.lock()
            && let Some(f) = far.remove(id)
        {
            f.remote.stop();
        }
        if forget {
            let _ = dent_remote::store::forget(id);
            if let Some(h) = &self.history {
                h.forget(id);
            }
        }
    }

    /// Pair the machine with the code shown on it.
    pub fn pair(&self, id: &str, code: &str) {
        if let Ok(far) = self.far.lock()
            && let Some(f) = far.get(id)
        {
            f.remote.pair(code);
        }
    }

    /// Tell `id` (or, with none, every machine) something that should end
    /// its wait: the window coming forward, the network changing, Retry.
    pub fn signal(&self, id: Option<&str>, s: Signal) {
        if let Ok(far) = self.far.lock() {
            for (tag, f) in far.iter() {
                if id.is_none_or(|i| i == tag) {
                    f.remote.signal(s);
                }
            }
        }
    }
}
