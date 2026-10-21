//! A remote daemon as a client sees one: the connection actor run by
//! [`crate::host`], with the daemon session on top. On every trusted link it
//! says hello, subscribes from the last event the client applied, and resends
//! the requests still unanswered (a resend changes nothing twice: messages,
//! forks and new sessions carry the client's own ids). Requests made while
//! the machine is away wait here and go when it's back
//! ([decision 0063](../../docs/decisions/0063-queued-prompts.md)).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dent_client_core::machine::{Machine, Note, Signal, State};
use dent_link::{Keys, Trust};
use dent_protocol::{AgentInfo, Envelope, Out, PROTOCOL, Reply, Request, encode};
use tokio::sync::mpsc;

use crate::host::{self, Ask};
use crate::store::{self, Saved};

const HELLO: u64 = 1;
const SUBSCRIBE: u64 = 2;

enum Cmd {
    Send(u64, Request),
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

/// A handle on one remote machine's session.
#[derive(Clone)]
pub struct Remote {
    tx: mpsc::UnboundedSender<Cmd>,
    /// The last event the client applied from this machine; a new link
    /// subscribes from here.
    pub seq: Arc<AtomicU64>,
}

impl Remote {
    /// Send request number `n`; its reply comes back as [`Event::Out`] with
    /// the same number. While the machine is away it waits.
    pub fn send(&self, n: u64, r: Request) {
        let _ = self.tx.send(Cmd::Send(n, r));
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
    let m = Machine::new(
        &store::device_name(),
        keys,
        saved.known_key(),
        saved.all_routes(),
    );
    rt.spawn(run(m, client.to_string(), saved, seq.clone(), rx, events));
    Remote { tx, seq }
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
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    mut events: impl FnMut(Event) + Send + 'static,
) {
    let (ask_tx, ask_rx) = mpsc::unbounded_channel();
    let (note_tx, mut notes) = mpsc::unbounded_channel();
    let host = tokio::spawn(host::run(m, ask_rx, note_tx));
    // Unanswered requests, resent on each new link.
    let mut pending: BTreeMap<u64, Request> = BTreeMap::new();
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
                        for (n, r) in &pending {
                            let _ = ask_tx.send(Ask::Send(line(*n, r)));
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
                        if req != 0 {
                            pending.remove(&req);
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
                Some(Cmd::Send(n, r)) => {
                    if up {
                        let _ = ask_tx.send(Ask::Send(line(n, &r)));
                    }
                    pending.insert(n, r);
                }
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
