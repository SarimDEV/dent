//! A remote daemon as a client sees one: the connection actor run by
//! [`crate::host`], with the daemon session on top. On every trusted link it
//! says hello, subscribes from the last event the client applied, and resends
//! the requests still unanswered (a resend changes nothing twice: messages,
//! forks and new sessions carry the client's own ids). Requests made while
//! the machine is away wait here and go when it's back
//! ([decision 0063](../../docs/decisions/0063-queued-prompts.md)).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dent_client_core::machine::{Machine, Note, Signal, State};
use dent_link::{Keys, Trust};
use dent_protocol::{AgentInfo, Envelope, Out, PROTOCOL, Reply, Request, encode};
use tokio::sync::mpsc;

use crate::host::{self, Ask};
use crate::store::{self, Saved};

const HELLO: u64 = 1;
const SUBSCRIBE: u64 = 2;

enum Cmd {
    /// Request `n`, already in [`Pending`], to go if the link is up.
    Send(u64),
    /// Request `n` can't go: answer it with this.
    Refuse(u64, String),
    Pair(String),
    Signal(Signal),
    Stop,
}

/// What the client hears.
#[derive(Debug, Clone)]
pub enum Event {
    State(State),
    /// The daemon answered the hello on a new link.
    Welcome {
        name: String,
        id: String,
        version: String,
        agents: Vec<AgentInfo>,
        /// The route this link took, and how this device is trusted there.
        via: String,
        trust: Trust,
    },
    /// A message from the daemon, `req` being the request it answers (0 for
    /// the feed). Session ids are the machine's own.
    Out {
        req: u64,
        out: Out,
    },
    Rtt(u64),
    /// The machine's record changed (key learned, name, routes): the client
    /// saves it.
    Saved(Saved),
    PairFailed(String),
    /// A known route answered with a different machine.
    DifferentMachine(String),
}

/// Unanswered requests, resent on each new link, each with whether it has
/// gone out on any link yet: one that has may have reached the daemon, so
/// it can't be taken back here.
#[derive(Default)]
struct Pending(BTreeMap<u64, (Request, bool)>);

impl Pending {
    /// Everything to send on a new link, now marked gone.
    fn on_link(&mut self) -> Vec<Vec<u8>> {
        self.0
            .iter_mut()
            .map(|(n, (r, gone))| {
                *gone = true;
                line(*n, r)
            })
            .collect()
    }

    /// Request `n` on a link that's up, now marked gone; none if it went
    /// already (the link came up after it was asked, and took it).
    fn go(&mut self, n: u64) -> Option<Vec<u8>> {
        let (r, gone) = self.0.get_mut(&n).filter(|(_, gone)| !*gone)?;
        *gone = true;
        Some(line(n, r))
    }

    /// Take back request `n`; only one that never went can be.
    fn cancel(&mut self, n: u64) -> bool {
        if self.0.get(&n).is_some_and(|(_, gone)| !gone) {
            self.0.remove(&n);
            return true;
        }
        false
    }
}

/// A handle on one remote machine's session.
#[derive(Clone)]
pub struct Remote {
    tx: mpsc::UnboundedSender<Cmd>,
    pending: Arc<Mutex<Pending>>,
    /// The last event the client applied from this machine; a new link
    /// subscribes from here.
    pub seq: Arc<AtomicU64>,
}

impl Remote {
    /// Send request number `n`; its reply comes back as [`Event::Out`] with
    /// the same number. While the machine is away it waits.
    pub fn send(&self, n: u64, mut r: Request) {
        if let Err(why) = carry(&mut r) {
            let _ = self.tx.send(Cmd::Refuse(n, why));
            return;
        }
        if let Ok(mut p) = self.pending.lock() {
            p.0.insert(n, (r, false));
        }
        let _ = self.tx.send(Cmd::Send(n));
    }

    /// Take back request `n` if it hasn't gone out on any link: it won't be
    /// sent when the machine is back. Whether it was taken back; one that
    /// went may have reached the daemon.
    pub fn cancel(&self, n: u64) -> bool {
        self.pending.lock().is_ok_and(|mut p| p.cancel(n))
    }

    /// Pair with the code shown on the machine.
    pub fn pair(&self, code: &str) {
        let _ = self.tx.send(Cmd::Pair(code.to_string()));
    }

    pub fn signal(&self, s: Signal) {
        let _ = self.tx.send(Cmd::Signal(s));
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

/// Start following `saved` on `rt`: from its known key and routes, or (a
/// machine being added, with no key yet) from its routes alone. `events`
/// hears everything, on the runtime's thread.
pub fn spawn(
    rt: &tokio::runtime::Handle,
    keys: Keys,
    client: &str,
    saved: Saved,
    events: impl FnMut(Event) + Send + 'static,
) -> Remote {
    let (tx, rx) = mpsc::unbounded_channel();
    let seq = Arc::new(AtomicU64::new(0));
    let pending = Arc::new(Mutex::new(Pending::default()));
    let m = Machine::new(
        &store::device_name(),
        keys,
        saved.known_key(),
        saved.all_routes(),
    );
    rt.spawn(run(
        m,
        client.to_string(),
        saved,
        seq.clone(),
        pending.clone(),
        rx,
        events,
    ));
    Remote { tx, pending, seq }
}

/// Where a new link subscribes from: the client's cursor and the log epoch
/// it counts in.
struct Cursor {
    /// What the client says it applied ([`Remote::seq`]).
    applied: Arc<AtomicU64>,
    /// The newest event passed to the client from this log. The client
    /// can't have applied more, so a cursor it stored from before a resync
    /// (its window still on older events) never resumes in the new log.
    passed: u64,
    /// The log epoch goes with the cursor: a new one is taken only from
    /// the resync that resets it, or a link would resume an old cursor in a
    /// new log.
    epoch: Option<String>,
}

impl Cursor {
    fn subscribe(&self) -> Request {
        Request::Subscribe {
            after: self.applied.load(Ordering::Relaxed).min(self.passed),
            epoch: self.epoch.clone(),
        }
    }

    fn welcome(&mut self, epoch: String) {
        self.epoch.get_or_insert(epoch);
    }

    /// A message from the daemon, before the client hears it.
    fn out(&mut self, out: &Out) {
        match out {
            Out::Resync { epoch } => {
                self.epoch = Some(epoch.clone());
                self.passed = 0;
                self.applied.store(0, Ordering::Relaxed);
            }
            Out::Events { events } => {
                if let Some(e) = events.last() {
                    self.passed = self.passed.max(e.seq);
                }
            }
            _ => {}
        }
    }
}

/// A message's files that are on this device go inside it: the machine
/// it's for can't read this device's paths. A path not here is left as it
/// is: it's one of that machine's own (a message of its, put back to edit).
/// Read here, on the asking thread, once; a resend carries what was read.
fn carry(r: &mut Request) -> Result<(), String> {
    let (Request::Send {
        attachments, files, ..
    }
    | Request::Create {
        attachments, files, ..
    }) = r
    else {
        return Ok(());
    };
    let mut total = files
        .iter()
        .map(|f| f.bytes_b64.len() / 4 * 3)
        .sum::<usize>();
    let mut keep = Vec::new();
    for path in std::mem::take(attachments) {
        let p = std::path::Path::new(&path);
        if !p.is_file() {
            keep.push(path);
            continue;
        }
        let name = p
            .file_name()
            .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
        let bytes = std::fs::read(p).map_err(|e| format!("couldn't read {name}: {e}"))?;
        total += bytes.len();
        if total > dent_protocol::CARRIED_MAX {
            return Err(format!(
                "these files come to more than {} MB, too many to send to another machine at once",
                dent_protocol::CARRIED_MAX >> 20
            ));
        }
        files.push(dent_protocol::Carried {
            name,
            bytes_b64: dent_protocol::b64::encode(&bytes),
        });
    }
    *attachments = keep;
    Ok(())
}

fn line(n: u64, r: &Request) -> Vec<u8> {
    let mut l = encode(n, r);
    l.pop();
    l
}

async fn run(
    m: Machine,
    client: String,
    mut saved: Saved,
    seq: Arc<AtomicU64>,
    pending: Arc<Mutex<Pending>>,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    mut events: impl FnMut(Event) + Send + 'static,
) {
    let (ask_tx, ask_rx) = mpsc::unbounded_channel();
    let (note_tx, mut notes) = mpsc::unbounded_channel();
    let host = tokio::spawn(host::run(m, ask_rx, note_tx));
    let mut up = false;
    // The link the last Up named, and how it was trusted.
    let mut via = (
        String::new(),
        Trust::Untrusted {
            pairing_open: false,
        },
    );
    let mut cursor = Cursor {
        applied: seq,
        passed: 0,
        epoch: None,
    };
    loop {
        tokio::select! {
            n = notes.recv() => {
                let Some(n) = n else { break };
                match n {
                    Note::Up { addr, trust, key, .. } => {
                        up = true;
                        via = (addr, trust);
                        if saved.key.is_empty() {
                            saved.key = hex::encode(&key);
                        }
                        let hello = Request::Hello { proto: PROTOCOL, client: client.clone() };
                        let sub = cursor.subscribe();
                        let _ = ask_tx.send(Ask::Send(line(HELLO, &hello)));
                        let _ = ask_tx.send(Ask::Send(line(SUBSCRIBE, &sub)));
                        let resend = pending.lock().map(|mut p| p.on_link()).unwrap_or_default();
                        for l in resend {
                            let _ = ask_tx.send(Ask::Send(l));
                        }
                    }
                    Note::State(s) => {
                        up = matches!(s, State::Online | State::Slow) && up;
                        events(Event::State(s));
                    }
                    Note::Data { line, .. } => {
                        let Ok(env) = serde_json::from_slice::<Envelope<Out>>(&line) else {
                            continue;
                        };
                        let (req, out) = (env.req, env.body);
                        if req == HELLO {
                            match out {
                                Out::Reply { reply: Reply::Welcome { machine, id, version, agents, addresses, epoch: e, .. } } => {
                                    if !saved.id.is_empty() && !id.is_empty() && id != saved.id {
                                        events(Event::DifferentMachine(via.0.clone()));
                                        continue;
                                    }
                                    cursor.welcome(e);
                                    let before = saved.clone();
                                    saved.name = machine.clone();
                                    if !id.is_empty() {
                                        saved.id = id.clone();
                                    }
                                    saved.learned = addresses;
                                    if saved != before && !saved.id.is_empty() {
                                        events(Event::Saved(saved.clone()));
                                    }
                                    events(Event::Welcome {
                                        name: machine,
                                        id,
                                        version,
                                        agents,
                                        via: via.0.clone(),
                                        trust: via.1.clone(),
                                    });
                                }
                                Out::Error { message } => events(Event::State(State::Unreachable { why: message })),
                                _ => {}
                            }
                            continue;
                        }
                        if req == SUBSCRIBE {
                            continue;
                        }
                        cursor.out(&out);
                        if req != 0
                            && let Ok(mut p) = pending.lock()
                        {
                            p.0.remove(&req);
                        }
                        events(Event::Out { req, out });
                    }
                    Note::Rtt { ms } => events(Event::Rtt(ms)),
                    Note::Paired { key } => saved.key = hex::encode(&key),
                    Note::PairFailed(r) => events(Event::PairFailed(dent_client_core::machine::refusal_words(&r))),
                    Note::DifferentMachine { addr } => events(Event::DifferentMachine(addr)),
                }
            }
            c = cmds.recv() => match c {
                Some(Cmd::Send(n)) => {
                    if up && let Some(l) = pending.lock().ok().and_then(|mut p| p.go(n)) {
                        let _ = ask_tx.send(Ask::Send(l));
                    }
                }
                Some(Cmd::Refuse(req, message)) => events(Event::Out { req, out: Out::Error { message } }),
                Some(Cmd::Pair(code)) => { let _ = ask_tx.send(Ask::Pair(code)); }
                Some(Cmd::Signal(s)) => { let _ = ask_tx.send(Ask::Signal(s)); }
                Some(Cmd::Stop) | None => break,
            },
        }
    }
    let _ = ask_tx.send(Ask::Stop);
    let _ = host.await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use dent_protocol::{Body, Event};

    fn events(seqs: &[u64]) -> Out {
        Out::Events {
            events: seqs
                .iter()
                .map(|&seq| Event {
                    seq,
                    session: "s1".into(),
                    at: 0,
                    body: Body::Delivered { id: "m".into() },
                })
                .collect(),
        }
    }

    #[test]
    fn only_a_request_that_never_went_out_is_taken_back() {
        let mut p = Pending::default();
        let r = || Request::Projects;
        p.0.insert(3, (r(), false));
        p.0.insert(4, (r(), false));
        // Away: 3 waits and is taken back; it never goes.
        assert!(p.cancel(3));
        assert!(p.go(3).is_none());
        // The link comes up first: 4 goes, and may have reached the daemon.
        assert_eq!(p.on_link().len(), 1);
        assert!(!p.cancel(4));
        assert!(p.go(4).is_none(), "the link took it: it doesn't go twice");
        // Sent while up, it went at once.
        p.0.insert(5, (r(), false));
        assert!(p.go(5).is_some());
        assert!(!p.cancel(5));
        assert!(!p.cancel(9), "answered or never asked");
    }

    #[test]
    fn files_here_travel_inside_the_message_and_the_machines_own_stay_paths() {
        let dir = std::env::temp_dir().join(format!("dent-carry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let here = dir.join("shot.png");
        std::fs::write(&here, b"png bytes").unwrap();
        let theirs = "/home/ada/.local/state/dent/attachments/carried/m1/plan.md".to_string();
        let mut r = Request::Send {
            session: "s1".into(),
            id: "m2".into(),
            text: "look".into(),
            steer: false,
            attachments: vec![here.display().to_string(), theirs.clone()],
            files: Vec::new(),
        };
        carry(&mut r).unwrap();
        let Request::Send {
            attachments, files, ..
        } = &r
        else {
            unreachable!()
        };
        assert_eq!(attachments, &vec![theirs]);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "shot.png");
        assert_eq!(
            dent_protocol::b64::decode(&files[0].bytes_b64).unwrap(),
            b"png bytes"
        );
        // Carried once: sent again, nothing's read twice.
        let before = r.clone();
        carry(&mut r).unwrap();
        assert_eq!(r, before);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_cursor_stored_from_before_a_resync_isnt_resumed_in_the_new_log() {
        let applied = Arc::new(AtomicU64::new(0));
        let mut c = Cursor {
            applied: applied.clone(),
            passed: 0,
            epoch: None,
        };
        c.welcome("e1".into());
        c.out(&events(&[1, 2, 7]));
        applied.store(7, Ordering::Relaxed);
        let sub = |after: u64, epoch: &str| Request::Subscribe {
            after,
            epoch: Some(epoch.into()),
        };
        assert_eq!(c.subscribe(), sub(7, "e1"));
        // The log started again. The window, not there yet, stores where it
        // was in the old one.
        c.out(&Out::Resync { epoch: "e2".into() });
        applied.store(7, Ordering::Relaxed);
        assert_eq!(c.subscribe(), sub(0, "e2"));
        // Events passed on but not yet applied are still the window's to
        // apply: a link resumes from what it applied.
        c.out(&events(&[1, 2]));
        applied.store(1, Ordering::Relaxed);
        assert_eq!(c.subscribe(), sub(1, "e2"));
        c.welcome("e3".into());
        assert_eq!(c.subscribe(), sub(1, "e2"));
    }
}
