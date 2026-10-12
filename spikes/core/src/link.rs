//! One machine, as a state machine: connect (racing its addresses), pair
//! with a code, follow sessions, send prompts, notice silence, reconnect.
//!
//! The host does the I/O. It calls `take_io` and does what it says (open a
//! WebSocket to an address, send bytes, close), reports back with `opened`,
//! `received` and `closed`, and calls `tick` with the time whenever
//! `next_deadline` comes round (or simply every 250 ms). Each connection has
//! its own number, so anything an old socket reports late is ignored.

use std::collections::VecDeque;

use serde::Serialize;

use crate::chat::{Applied, Chat, MessageUpdate};
use crate::keys::{Keys, fingerprint};
use crate::noise::{Initiator, LARGE, Transport};
use crate::pair::{self, Pairing};
use crate::proto::{Down, Health, Info, PROTO, PROTO_MIN, Up, new_id};

/// From dialling to knowing who answered.
pub const OPEN_MS: u64 = 3000;
/// Between starting each address's attempt.
pub const STAGGER_MS: u64 = 250;
/// Each step of pairing.
pub const STEP_MS: u64 = 5000;
pub const PING_MS: u64 = 1000;
/// No frame for this long means the link is dead (pings are answered every
/// second).
pub const SILENCE_MS: u64 = 3000;
/// Versions that don't overlap won't start to while both sides run.
pub const INCOMPATIBLE_MS: u64 = 30_000;
const BACKOFF_MIN: u64 = 100;
const BACKOFF_MAX: u64 = 2000;
/// After a goodbye the machine won't be back in a moment.
const BACKOFF_BYE: u64 = 1000;

/// What the host must do.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum Io {
    /// Open a WebSocket to `ws://{addr}/`, then call `opened`.
    Connect {
        conn: u64,
        addr: String,
    },
    /// Send one binary frame.
    Send {
        conn: u64,
        data: Vec<u8>,
    },
    Close {
        conn: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum Status {
    Idle,
    Connecting,
    Pairing,
    Online,
    /// Lost or unreachable; retrying with backoff.
    Offline,
    /// Found, but this device isn't paired with it. Waiting for a code.
    NeedsCode,
    /// The machine removed this device. Nothing more happens.
    Removed,
    /// The versions don't overlap; retrying slowly.
    Incompatible,
}

/// Why pairing didn't work, in terms a person can act on.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum PairError {
    Wrong,
    /// Three wrong: the other machine now shows a new code.
    WrongNewCode,
    Locked {
        secs: u64,
    },
    /// No code is showing there, or it expired.
    Closed,
    Unreachable {
        why: String,
    },
    Failed {
        why: String,
    },
}

/// What the UI hears about. Not boxed: UniFFI can't carry boxes, and these
/// are moved a few at a time.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum Note {
    Status {
        status: Status,
        detail: String,
    },
    /// Who answered, once trusted (or once found, for a machine to add).
    Welcome {
        name: String,
        id: String,
        via: String,
        addr: String,
        info: Info,
    },
    /// Paired: store `key` (hex) and pass it back next time.
    Paired {
        key: String,
        name: String,
        id: String,
    },
    PairFailed {
        error: PairError,
    },
    Message {
        update: MessageUpdate,
    },
    /// A prompt is waiting to be sent or acknowledged.
    Queued {
        id: String,
        session: String,
        text: String,
    },
    /// The daemon has logged the prompt and will run it.
    Accepted {
        id: String,
        session: String,
        dup: bool,
    },
    Health {
        health: Health,
    },
    Rtt {
        ms: u64,
    },
    /// The machine said why it's going: "sleep", "quit" or "restart".
    Bye {
        reason: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct Stats {
    pub events: u64,
    pub dups: u64,
    pub gaps: u64,
    pub reconnects: u64,
    pub frames: u64,
    pub bytes: u64,
}

struct Prompt {
    id: String,
    session: String,
    text: String,
}

struct Welcome {
    name: String,
    id: String,
    via: String,
    trusted: bool,
    proto: u32,
    min: u32,
    info: Info,
}

enum Att {
    /// Not started yet (staggered).
    NotYet,
    /// Waiting for the host to say the socket is open.
    Dialing,
    Handshake(Box<Initiator>),
    /// Encrypted; waiting for the welcome.
    Hello(Box<Transport>),
    /// Answered but doesn't trust this device; kept in case nothing better
    /// answers.
    Untrusted(Box<Welcome>),
    Failed(String),
}

struct Attempt {
    conn: u64,
    addr: String,
    started: u64,
    att: Att,
}

enum Step {
    Spake(Box<Pairing>),
    Confirm(Vec<u8>),
    Paired,
}

enum Phase {
    Idle,
    Waiting {
        until: u64,
    },
    Connecting {
        started: u64,
        attempts: Vec<Attempt>,
    },
    Pairing {
        conn: u64,
        t: Box<Transport>,
        step: Step,
        deadline: u64,
        name: String,
        id: String,
    },
    Open {
        conn: u64,
        t: Box<Transport>,
        last_rx: u64,
        last_ping: u64,
    },
    /// Removed, or waiting for a code: nothing happens until told.
    Stopped,
}

pub struct Link {
    name: String,
    keys: Keys,
    /// The daemon's key once paired.
    known: Option<Vec<u8>>,
    addrs: Vec<String>,
    phase: Phase,
    next_conn: u64,
    backoff: u64,
    code: Option<String>,
    chat: Chat,
    pending: Vec<Prompt>,
    push: Option<(String, String)>,
    io: VecDeque<Io>,
    notes: Vec<Note>,
    status: (Status, String),
    was_online: bool,
    stats: Stats,
    /// Follow the machine's health too.
    health: bool,
}

/// Encrypt and queue one message.
fn emit(io: &mut VecDeque<Io>, t: &mut Transport, conn: u64, msg: &Up) {
    let Ok(plain) = serde_json::to_vec(msg) else {
        return;
    };
    match t.seal(&plain) {
        Ok(frames) => io.extend(frames.into_iter().map(|data| Io::Send { conn, data })),
        // Only fails if the nonce space runs out; the link is unusable.
        Err(_) => io.push_back(Io::Close { conn }),
    }
}

fn denied(reason: &str) -> PairError {
    match reason {
        "wrong" => PairError::Wrong,
        "wrong-new-code" => PairError::WrongNewCode,
        "closed" | "pair first" => PairError::Closed,
        r => match r.strip_prefix("locked:").and_then(|s| s.parse().ok()) {
            Some(secs) => PairError::Locked { secs },
            None => PairError::Failed { why: r.into() },
        },
    }
}

impl Link {
    /// `known` is the daemon's key from an earlier pairing (None to add a
    /// new machine); `addrs` are "host:port" to try, best first.
    pub fn new(name: &str, keys: Keys, known: Option<Vec<u8>>, addrs: Vec<String>) -> Self {
        Self {
            name: name.into(),
            keys,
            known,
            addrs,
            phase: Phase::Idle,
            next_conn: 1,
            backoff: BACKOFF_MIN,
            code: None,
            chat: Chat::default(),
            pending: Vec::new(),
            push: None,
            io: VecDeque::new(),
            notes: Vec::new(),
            status: (Status::Idle, String::new()),
            was_online: false,
            stats: Stats::default(),
            health: true,
        }
    }

    // ---- What the host or UI asks for ----

    pub fn start(&mut self, now: u64) {
        if matches!(
            self.phase,
            Phase::Idle | Phase::Stopped | Phase::Waiting { .. }
        ) {
            self.connect(now);
        }
    }

    /// Stop and close everything.
    pub fn stop(&mut self) {
        self.close_all();
        self.phase = Phase::Idle;
        self.set_status(Status::Idle, "");
    }

    pub fn set_addrs(&mut self, addrs: Vec<String>) {
        self.addrs = addrs;
    }

    /// Pair using the code shown on the machine. Starts over on a fresh
    /// connection.
    pub fn pair(&mut self, code: &str, now: u64) {
        self.code = Some(code.trim().to_string());
        self.close_all();
        self.connect(now);
    }

    /// Follow a session from what's already held.
    pub fn follow(&mut self, session: &str) {
        self.chat.touch(session);
        let after = self.chat.last(session);
        self.send(&Up::Subscribe {
            session: session.into(),
            after,
        });
    }

    /// Send a prompt; it's kept and resent on every reconnect until the
    /// daemon acknowledges it, and runs once whatever happens.
    pub fn prompt(&mut self, session: &str, text: &str) -> String {
        let id = new_id();
        self.chat.touch(session);
        self.notes.push(Note::Queued {
            id: id.clone(),
            session: session.into(),
            text: text.into(),
        });
        let up = Up::Prompt {
            id: id.clone(),
            session: session.into(),
            text: text.into(),
        };
        self.pending.push(Prompt {
            id: id.clone(),
            session: session.into(),
            text: text.into(),
        });
        self.send(&up);
        id
    }

    /// Tell the machine where to send this phone's notifications.
    pub fn set_push(&mut self, handle: &str, key_hex: &str) {
        self.push = Some((handle.into(), key_hex.into()));
        self.send(&Up::Push {
            handle: handle.into(),
            key: key_hex.into(),
        });
    }

    /// Ask the daemon for `n` events at once (throughput checks).
    pub fn bulk(&mut self, session: &str, n: u64) {
        self.send(&Up::Bulk {
            session: session.into(),
            n: n as usize,
        });
    }

    /// Everything in a session again, for a UI that has just opened it.
    pub fn resend(&mut self, session: &str) {
        self.chat.resend(session);
    }

    pub fn take_io(&mut self) -> Vec<Io> {
        self.io.drain(..).collect()
    }

    /// Notes, then an update for each message that changed.
    pub fn take_notes(&mut self) -> Vec<Note> {
        let mut out = std::mem::take(&mut self.notes);
        out.extend(
            self.chat
                .drain()
                .into_iter()
                .map(|update| Note::Message { update }),
        );
        out
    }

    pub fn status(&self) -> Status {
        self.status.0
    }

    pub fn known_key(&self) -> Option<String> {
        self.known.as_ref().map(hex::encode)
    }

    pub fn stats(&self) -> Stats {
        self.stats.clone()
    }

    pub fn chat(&self) -> &Chat {
        &self.chat
    }

    /// When `tick` next has something to do.
    pub fn next_deadline(&self) -> Option<u64> {
        match &self.phase {
            Phase::Idle | Phase::Stopped => None,
            Phase::Waiting { until } => Some(*until),
            Phase::Connecting { started, attempts } => attempts
                .iter()
                .enumerate()
                .filter_map(|(i, a)| match a.att {
                    Att::NotYet => Some(started + STAGGER_MS * i as u64),
                    Att::Dialing | Att::Handshake(_) | Att::Hello(_) => Some(a.started + OPEN_MS),
                    _ => None,
                })
                .min(),
            Phase::Pairing { deadline, .. } => Some(*deadline),
            Phase::Open {
                last_rx, last_ping, ..
            } => Some((last_ping + PING_MS).min(last_rx + SILENCE_MS)),
        }
    }

    // ---- What the host reports ----

    pub fn opened(&mut self, conn: u64, now: u64) {
        let Phase::Connecting { attempts, .. } = &mut self.phase else {
            return;
        };
        let Some(a) = attempts.iter_mut().find(|a| a.conn == conn) else {
            return;
        };
        if !matches!(a.att, Att::Dialing) {
            return;
        }
        match Initiator::start(&self.keys.private) {
            Ok((hs, msg)) => {
                a.att = Att::Handshake(Box::new(hs));
                self.io.push_back(Io::Send { conn, data: msg });
            }
            Err(e) => {
                a.att = Att::Failed(format!("{e:#}"));
                self.io.push_back(Io::Close { conn });
            }
        }
        let _ = now;
    }

    pub fn received(&mut self, conn: u64, data: &[u8], now: u64) {
        self.stats.frames += 1;
        self.stats.bytes += data.len() as u64;
        match &self.phase {
            Phase::Connecting { .. } => self.on_connecting(conn, data, now),
            Phase::Pairing { conn: c, .. } if *c == conn => self.on_pairing(data, now),
            Phase::Open { conn: c, .. } if *c == conn => self.on_open(data, now),
            // A late frame from a socket we've moved on from.
            _ => {}
        }
    }

    pub fn closed(&mut self, conn: u64, reason: &str, now: u64) {
        match &mut self.phase {
            Phase::Connecting { attempts, .. } => {
                if let Some(a) = attempts.iter_mut().find(|a| a.conn == conn) {
                    match a.att {
                        Att::Failed(_) | Att::NotYet => {}
                        // An untrusted answer can close; the decision stands.
                        Att::Untrusted(..) => {}
                        _ => a.att = Att::Failed(reason.into()),
                    }
                }
                self.check_attempts(now);
            }
            Phase::Pairing { conn: c, step, .. } if *c == conn => {
                // The daemon closes once paired; anything else is a failure.
                if matches!(step, Step::Paired) {
                    self.phase = Phase::Waiting { until: now };
                } else {
                    self.pair_failed(PairError::Unreachable { why: reason.into() }, now);
                }
            }
            Phase::Open { conn: c, .. } if *c == conn => self.lost(reason, BACKOFF_MIN, now),
            _ => {}
        }
    }

    pub fn tick(&mut self, now: u64) {
        match &mut self.phase {
            Phase::Waiting { until } if now >= *until => self.connect(now),
            Phase::Connecting { started, attempts } => {
                let started = *started;
                for (i, a) in attempts.iter_mut().enumerate() {
                    match a.att {
                        Att::NotYet if now >= started + STAGGER_MS * i as u64 => {
                            a.att = Att::Dialing;
                            a.started = now;
                            self.io.push_back(Io::Connect {
                                conn: a.conn,
                                addr: a.addr.clone(),
                            });
                        }
                        Att::Dialing | Att::Handshake(_) | Att::Hello(_)
                            if now >= a.started + OPEN_MS =>
                        {
                            a.att = Att::Failed(format!("no answer within {} s", OPEN_MS / 1000));
                            self.io.push_back(Io::Close { conn: a.conn });
                        }
                        _ => {}
                    }
                }
                self.check_attempts(now);
            }
            Phase::Pairing { deadline, .. } if now >= *deadline => self.pair_failed(
                PairError::Unreachable {
                    why: "stopped answering".into(),
                },
                now,
            ),
            Phase::Open {
                conn,
                t,
                last_rx,
                last_ping,
            } => {
                if now >= *last_rx + SILENCE_MS {
                    self.lost(
                        &format!("silent for {} s", SILENCE_MS / 1000),
                        BACKOFF_MIN,
                        now,
                    );
                } else if now >= *last_ping + PING_MS {
                    *last_ping = now;
                    emit(&mut self.io, t, *conn, &Up::Ping { t: now });
                }
            }
            _ => {}
        }
    }

    // ---- Inside ----

    fn set_status(&mut self, status: Status, detail: &str) {
        if self.status.0 != status || self.status.1 != detail {
            self.status = (status, detail.into());
            self.notes.push(Note::Status {
                status,
                detail: detail.into(),
            });
        }
    }

    fn send(&mut self, msg: &Up) {
        if let Phase::Open { conn, t, .. } = &mut self.phase {
            emit(&mut self.io, t, *conn, msg);
        }
    }

    fn close_all(&mut self) {
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Connecting { attempts, .. } => {
                for a in attempts {
                    if !matches!(a.att, Att::NotYet | Att::Failed(_)) {
                        self.io.push_back(Io::Close { conn: a.conn });
                    }
                }
            }
            Phase::Pairing { conn, .. } | Phase::Open { conn, .. } => {
                self.io.push_back(Io::Close { conn })
            }
            _ => {}
        }
    }

    fn connect(&mut self, now: u64) {
        self.close_all();
        if self.addrs.is_empty() {
            self.set_status(Status::Offline, "no address");
            self.phase = Phase::Waiting {
                until: now + BACKOFF_MAX,
            };
            return;
        }
        let attempts = self
            .addrs
            .iter()
            .map(|addr| {
                let conn = self.next_conn;
                self.next_conn += 1;
                Attempt {
                    conn,
                    addr: addr.clone(),
                    started: 0,
                    att: Att::NotYet,
                }
            })
            .collect();
        self.phase = Phase::Connecting {
            started: now,
            attempts,
        };
        let status = if self.code.is_some() {
            Status::Pairing
        } else {
            Status::Connecting
        };
        self.set_status(status, "");
        self.tick(now);
    }

    fn on_connecting(&mut self, conn: u64, data: &[u8], now: u64) {
        let Phase::Connecting { attempts, .. } = &mut self.phase else {
            return;
        };
        let Some(a) = attempts.iter_mut().find(|a| a.conn == conn) else {
            return;
        };
        match std::mem::replace(&mut a.att, Att::Failed(String::new())) {
            Att::Handshake(hs) => match hs.finish(data) {
                Ok((mut t, msg)) => {
                    self.io.push_back(Io::Send { conn, data: msg });
                    emit(
                        &mut self.io,
                        &mut t,
                        conn,
                        &Up::Hello {
                            name: self.name.clone(),
                            proto: PROTO,
                            probe: false,
                        },
                    );
                    a.att = Att::Hello(Box::new(t));
                }
                Err(e) => {
                    a.att = Att::Failed(format!("{e:#}"));
                    self.io.push_back(Io::Close { conn });
                }
            },
            Att::Hello(mut t) => {
                let msg = match t.open(data) {
                    Ok(Some(m)) => m,
                    Ok(None) => {
                        a.att = Att::Hello(t);
                        return;
                    }
                    Err(e) => {
                        a.att = Att::Failed(format!("{e:#}"));
                        self.io.push_back(Io::Close { conn });
                        self.check_attempts(now);
                        return;
                    }
                };
                match serde_json::from_slice::<Down>(&msg) {
                    Ok(Down::Welcome {
                        name,
                        id,
                        trusted,
                        via,
                        proto,
                        min,
                        info,
                    }) => {
                        let w = Welcome {
                            name,
                            id,
                            via,
                            trusted,
                            proto,
                            min,
                            info: *info,
                        };
                        self.on_welcome(conn, t, w, now);
                        return;
                    }
                    _ => {
                        a.att = Att::Failed("expected a welcome".into());
                        self.io.push_back(Io::Close { conn });
                    }
                }
            }
            other => a.att = other,
        }
        self.check_attempts(now);
    }

    fn on_welcome(&mut self, conn: u64, t: Box<Transport>, w: Welcome, now: u64) {
        let fail = |link: &mut Link, why: String| {
            if let Phase::Connecting { attempts, .. } = &mut link.phase
                && let Some(a) = attempts.iter_mut().find(|a| a.conn == conn)
            {
                a.att = Att::Failed(why);
            }
            link.io.push_back(Io::Close { conn });
            link.check_attempts(now);
        };
        if let Some(k) = &self.known
            && &t.remote != k
        {
            return fail(self, "answered with a different key".into());
        }
        if w.proto < PROTO_MIN || PROTO < w.min {
            self.close_all();
            self.set_status(
                Status::Incompatible,
                &format!(
                    "it speaks {}, oldest {}; this speaks {PROTO}",
                    w.proto, w.min
                ),
            );
            self.phase = Phase::Waiting {
                until: now + INCOMPATIBLE_MS,
            };
            return;
        }
        if w.trusted {
            return self.go_online(conn, t, w, now);
        }
        if w.via == "tailnet-only" {
            return fail(self, "only trusts this device over Tailscale".into());
        }
        if let Some(code) = self.code.clone() {
            return self.start_pairing(conn, t, w, &code, now);
        }
        if let Phase::Connecting { attempts, .. } = &mut self.phase
            && let Some(a) = attempts.iter_mut().find(|a| a.conn == conn)
        {
            drop(t);
            a.att = Att::Untrusted(Box::new(w));
        }
        self.check_attempts(now);
    }

    /// When every attempt has an outcome: settle for an untrusted answer,
    /// or wait and try again.
    fn check_attempts(&mut self, now: u64) {
        let Phase::Connecting { attempts, .. } = &mut self.phase else {
            return;
        };
        let settled = attempts
            .iter()
            .all(|a| matches!(a.att, Att::Failed(_) | Att::Untrusted(..)));
        if !settled {
            return;
        }
        let mut attempts = std::mem::take(attempts);
        let found = attempts
            .iter()
            .position(|a| matches!(a.att, Att::Untrusted(..)));
        if let Some(i) = found {
            let a = attempts.swap_remove(i);
            for other in attempts {
                if matches!(other.att, Att::Untrusted(..)) {
                    self.io.push_back(Io::Close { conn: other.conn });
                }
            }
            let Att::Untrusted(w) = a.att else {
                return;
            };
            self.io.push_back(Io::Close { conn: a.conn });
            self.phase = Phase::Stopped;
            if self.known.is_some() {
                // It knew this device and no longer does.
                self.set_status(Status::Removed, "removed on that machine");
            } else {
                self.notes.push(Note::Welcome {
                    name: w.name.clone(),
                    id: w.id.clone(),
                    via: w.via.clone(),
                    addr: a.addr.clone(),
                    info: w.info.clone(),
                });
                self.set_status(Status::NeedsCode, &w.name);
            }
            return;
        }
        let why = attempts
            .iter()
            .rev()
            .find_map(|a| match &a.att {
                Att::Failed(w) if !w.is_empty() => Some(w.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "unreachable".into());
        self.set_status(Status::Offline, &why);
        self.phase = Phase::Waiting {
            until: now + self.backoff,
        };
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
    }

    /// Close every other attempt.
    fn take_winner(&mut self, conn: u64) -> Option<String> {
        let Phase::Connecting { attempts, .. } = std::mem::replace(&mut self.phase, Phase::Idle)
        else {
            return None;
        };
        let mut addr = None;
        for a in attempts {
            if a.conn == conn {
                addr = Some(a.addr);
            } else if !matches!(a.att, Att::NotYet | Att::Failed(_)) {
                self.io.push_back(Io::Close { conn: a.conn });
            }
        }
        addr
    }

    fn go_online(&mut self, conn: u64, mut t: Box<Transport>, w: Welcome, now: u64) {
        let addr = self.take_winner(conn).unwrap_or_default();
        t.limit = LARGE;
        if self.was_online {
            self.stats.reconnects += 1;
        }
        self.was_online = true;
        self.backoff = BACKOFF_MIN;
        if self.known.is_none() {
            // Trusted with no code (your own Tailscale account).
            self.known = Some(t.remote.clone());
            self.notes.push(Note::Paired {
                key: hex::encode(&t.remote),
                name: w.name.clone(),
                id: fingerprint(&t.remote),
            });
        }
        self.notes.push(Note::Welcome {
            name: w.name,
            id: w.id,
            via: w.via,
            addr: addr.clone(),
            info: w.info,
        });
        self.phase = Phase::Open {
            conn,
            t,
            last_rx: now,
            last_ping: now,
        };
        self.set_status(Status::Online, &addr);
        let sessions: Vec<String> = self.chat.sessions().cloned().collect();
        for s in sessions {
            self.follow(&s);
        }
        if self.health {
            self.send(&Up::Subscribe {
                session: "_status".into(),
                after: 0,
            });
        }
        let prompts: Vec<Up> = self
            .pending
            .iter()
            .map(|p| Up::Prompt {
                id: p.id.clone(),
                session: p.session.clone(),
                text: p.text.clone(),
            })
            .collect();
        for p in prompts {
            self.send(&p);
        }
        if let Some((handle, key)) = self.push.clone() {
            self.send(&Up::Push { handle, key });
        }
    }

    fn start_pairing(
        &mut self,
        conn: u64,
        mut t: Box<Transport>,
        w: Welcome,
        code: &str,
        now: u64,
    ) {
        self.take_winner(conn);
        let p = Pairing::start(code);
        emit(
            &mut self.io,
            &mut t,
            conn,
            &Up::Pair {
                spake: hex::encode(&p.outbound),
            },
        );
        self.phase = Phase::Pairing {
            conn,
            t,
            step: Step::Spake(Box::new(p)),
            deadline: now + STEP_MS,
            name: w.name,
            id: w.id,
        };
        self.set_status(Status::Pairing, "");
    }

    fn pair_failed(&mut self, error: PairError, _now: u64) {
        self.code = None;
        self.close_all();
        self.phase = Phase::Stopped;
        self.notes.push(Note::PairFailed { error });
        self.set_status(Status::NeedsCode, "");
    }

    fn on_pairing(&mut self, data: &[u8], now: u64) {
        let Phase::Pairing {
            conn,
            t,
            step,
            deadline,
            name,
            id,
        } = &mut self.phase
        else {
            return;
        };
        let conn = *conn;
        let msg = match t.open(data) {
            Ok(Some(m)) => m,
            Ok(None) => return,
            Err(e) => {
                return self.pair_failed(
                    PairError::Failed {
                        why: format!("{e:#}"),
                    },
                    now,
                );
            }
        };
        let down = match serde_json::from_slice::<Down>(&msg) {
            Ok(d) => d,
            Err(e) => {
                return self.pair_failed(PairError::Failed { why: e.to_string() }, now);
            }
        };
        *deadline = now + STEP_MS;
        let fail = |why: &str| PairError::Failed { why: why.into() };
        let next = match (std::mem::replace(step, Step::Paired), down) {
            (_, Down::Denied { reason }) => Err(denied(&reason)),
            (Step::Spake(mut p), Down::Pair { spake }) => match hex::decode(&spake)
                .map_err(|e| e.to_string())
                .and_then(|b| p.finish(&b).map_err(|e| e.to_string()))
            {
                Ok(key) => {
                    let mac = pair::confirm(&key, &t.hash, b"client");
                    emit(
                        &mut self.io,
                        t,
                        conn,
                        &Up::PairConfirm {
                            mac: hex::encode(mac),
                        },
                    );
                    Ok(Step::Confirm(key))
                }
                Err(e) => Err(fail(&e)),
            },
            (Step::Confirm(key), Down::PairConfirm { mac })
                if hex::decode(&mac).is_ok_and(|m| pair::verify(&key, &t.hash, b"daemon", &m)) =>
            {
                Ok(Step::Paired)
            }
            (Step::Confirm(_), _) => {
                Err(fail("the other machine couldn't prove it knows the code"))
            }
            (Step::Paired, Down::Paired) => {
                let key = t.remote.clone();
                let (name, id) = (name.clone(), id.clone());
                self.known = Some(key.clone());
                self.code = None;
                self.notes.push(Note::Paired {
                    key: hex::encode(&key),
                    name,
                    id,
                });
                // The daemon closes this connection; the next one is
                // trusted. Don't wait for the close to arrive.
                self.io.push_back(Io::Close { conn });
                self.phase = Phase::Waiting { until: now };
                self.set_status(Status::Connecting, "");
                self.tick(now);
                return;
            }
            (_, other) => Err(fail(&format!("unexpected {other:?}"))),
        };
        match next {
            Ok(s) => *step = s,
            Err(e) => self.pair_failed(e, now),
        }
    }

    fn on_open(&mut self, data: &[u8], now: u64) {
        let Phase::Open { t, last_rx, .. } = &mut self.phase else {
            return;
        };
        *last_rx = now;
        let msg = match t.open(data) {
            Ok(Some(m)) => m,
            Ok(None) => return,
            Err(e) => return self.lost(&format!("{e:#}"), BACKOFF_MIN, now),
        };
        let down = match serde_json::from_slice::<Down>(&msg) {
            Ok(d) => d,
            // Something newer than this build understands: skip it.
            Err(_) => return,
        };
        match down {
            Down::Event(ev) => match self.chat.apply(ev) {
                Applied::Dup => self.stats.dups += 1,
                Applied::Gap => {
                    self.stats.gaps += 1;
                    self.stats.events += 1;
                }
                Applied::New => self.stats.events += 1,
            },
            Down::Accepted { id, session, dup } => {
                self.pending.retain(|p| p.id != id);
                self.notes.push(Note::Accepted { id, session, dup });
            }
            Down::Health(health) => self.notes.push(Note::Health { health }),
            Down::Pong { t } if t > 0 => self.notes.push(Note::Rtt {
                ms: now.saturating_sub(t),
            }),
            Down::Bye { reason } => {
                self.notes.push(Note::Bye {
                    reason: reason.clone(),
                });
                self.lost(&format!("said goodbye: {reason}"), BACKOFF_BYE, now);
            }
            Down::Denied { reason } if reason == "removed" => {
                self.close_all();
                self.phase = Phase::Stopped;
                self.set_status(Status::Removed, "removed on that machine");
            }
            Down::Denied { reason } => self.lost(&reason, BACKOFF_MIN, now),
            _ => {}
        }
    }

    fn lost(&mut self, reason: &str, backoff: u64, now: u64) {
        self.close_all();
        self.set_status(Status::Offline, reason);
        self.backoff = backoff;
        self.phase = Phase::Waiting {
            until: now + backoff,
        };
        self.backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> Link {
        Link::new(
            "phone",
            Keys::generate(),
            None,
            vec!["a:1".into(), "b:2".into()],
        )
    }

    #[test]
    fn races_addresses_and_times_out() {
        let mut l = link();
        l.start(0);
        assert_eq!(
            l.take_io(),
            [Io::Connect {
                conn: 1,
                addr: "a:1".into()
            }]
        );
        l.tick(250);
        assert_eq!(
            l.take_io(),
            [Io::Connect {
                conn: 2,
                addr: "b:2".into()
            }]
        );
        l.tick(3000);
        assert_eq!(l.take_io(), [Io::Close { conn: 1 }]);
        l.tick(3250);
        assert_eq!(l.take_io(), [Io::Close { conn: 2 }]);
        assert_eq!(l.status(), Status::Offline);
        assert_eq!(l.next_deadline(), Some(3350));
    }

    #[test]
    fn stale_and_garbage_are_ignored() {
        let mut l = link();
        l.start(0);
        l.take_io();
        l.opened(1, 1);
        let io = l.take_io();
        assert!(matches!(io[..], [Io::Send { conn: 1, .. }]));
        // Frames for a connection that doesn't exist, and opened twice.
        l.received(99, b"junk", 2);
        l.closed(98, "gone", 2);
        l.opened(1, 3);
        assert!(l.take_io().is_empty());
        // Garbage instead of a handshake reply fails that attempt only.
        l.received(1, b"not noise", 4);
        assert_eq!(l.take_io(), [Io::Close { conn: 1 }]);
        assert_eq!(l.status(), Status::Connecting);
    }

    #[test]
    fn prompts_wait_for_a_link() {
        let mut l = link();
        let id = l.prompt("s", "hello");
        assert!(l.take_io().is_empty());
        let notes = l.take_notes();
        assert!(matches!(&notes[0], Note::Queued { id: i, .. } if *i == id));
        assert_eq!(l.pending.len(), 1);
    }
}
