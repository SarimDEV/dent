//! One remote machine's connection, as a state machine with no I/O
//! ([decision 0062]). The host does the I/O: it calls [`Machine::take_io`]
//! and does what it says (open a WebSocket to an address, send a frame,
//! close), reports back with [`Machine::opened`], [`Machine::received`] and
//! [`Machine::closed`], and calls [`Machine::tick`] whenever
//! [`Machine::next_deadline`] comes round. Time is always an argument, in
//! milliseconds from any fixed start.
//!
//! The rules ([decision 0062], [spike 12]):
//! - Any frame counts as life. Pings go every 2 s while idle. After 3 s of
//!   silence the machine reads as slow; at 5 s a replacement is dialled
//!   beside the silent link, with another dial every second while the
//!   silence lasts, and whichever proves alive first wins; at 15 s with no
//!   frame the link is torn down. Pings follow what's heard, not what's
//!   sent: the daemon only answers them.
//! - Retries wait 0.1, 0.25, 0.5 and 1 s, then double from 2 s with each
//!   wait drawn from the upper half of its step, capped at 60 s while the
//!   app is in front and the machine was online in the last 10 minutes,
//!   5 minutes otherwise. A link only resets the ladder once it has been
//!   healthy for 30 s.
//! - Foreground, a network change, the machine being seen again and an
//!   explicit retry skip the wait and start the ladder again. Refusals
//!   that only a change can cure wait for one.
//! - After an announced restart or update, redial every 0.75 s for up to
//!   2 minutes.
//! - Addresses race a quarter second apart; the first to answer with the
//!   right key wins. While on a slower route, the better ones are tried
//!   every 60 s and on every signal, and a route that answered but then
//!   failed is left alone for 5 minutes, as is one this device is trusted
//!   only elsewhere on, or where a different machine answered.
//!
//! Every connection has a number, so anything an old socket reports late is
//! ignored.
//!
//! [decision 0062]: ../../docs/decisions/0062-liveness-and-retry.md
//! [spike 12]: ../../docs/spikes/12-link-under-real-networks.md

use std::collections::{HashMap, VecDeque};

use dent_link::{Control, Goodbye, Hello, Initiator, Keys, Opened, Refusal, Transport, Trust};

/// Dialling to a finished handshake and verdict, per address.
pub const OPEN_MS: u64 = 3_000;
/// Between starting each address's attempt.
pub const STAGGER_MS: u64 = 250;
pub const PING_MS: u64 = 2_000;
/// Silence before the machine reads as slow.
pub const SLOW_MS: u64 = 3_000;
/// Silence before a replacement is dialled.
pub const REPLACE_MS: u64 = 5_000;
/// Silence before the link is torn down.
pub const SILENCE_MS: u64 = 15_000;
/// While silent, a new dial this often beside the replacement race.
const REDIAL_MS: u64 = 1_000;
const QUICK: [u64; 4] = [100, 250, 500, 1_000];
const CAP_NEAR_MS: u64 = 60_000;
const CAP_FAR_MS: u64 = 300_000;
/// Online this recently counts as "near" for the backoff cap.
const NEAR_MS: u64 = 600_000;
/// A link healthy this long resets the ladder.
const STABLE_MS: u64 = 30_000;
const HOME_EVERY_MS: u64 = 60_000;
const COOLDOWN_MS: u64 = 300_000;
const RESTART_EVERY_MS: u64 = 750;
const RESTART_FOR_MS: u64 = 120_000;
/// Each step of pairing.
const STEP_MS: u64 = 5_000;

/// What the host must do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Io {
    /// Open a WebSocket to `ws://{addr}/`, then call [`Machine::opened`].
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

/// How the machine stands, for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Not asked to connect.
    Off,
    Connecting,
    /// Connected and trusted.
    Online,
    /// Connected, but nothing heard for a few seconds.
    Slow,
    /// Lost; trying again.
    Reconnecting,
    /// Tried and failed; waiting before the next try.
    Unreachable {
        why: String,
    },
    /// Said it's going away, and why.
    Away(Goodbye),
    /// Answered, but doesn't trust this device. A code from it pairs.
    NeedsCode {
        pairing_open: bool,
    },
    /// Trusted only over other routes (for example, Tailscale).
    TrustedElsewhere {
        routes: Vec<String>,
    },
    /// No protocol in common; the named side needs an update.
    Incompatible {
        daemon: (u32, u32),
        client: (u32, u32),
    },
    /// This device was removed there. Nothing more happens.
    Removed,
}

/// What the client hears about.
#[derive(Debug, Clone, PartialEq)]
pub enum Note {
    State(State),
    /// A trusted link is up (or replaced one that was): the client sends
    /// its hello and subscriptions on it. `generation` changes each time.
    Up {
        generation: u64,
        addr: String,
        trust: Trust,
        /// The machine's public key, proven in the handshake: keep it to
        /// catch a different machine at a known address later.
        key: Vec<u8>,
    },
    /// One data message from the daemon: a JSON line.
    Data {
        generation: u64,
        line: Vec<u8>,
    },
    /// Paired: keep `key` (the machine's public key) and pass it back next
    /// time.
    Paired {
        key: Vec<u8>,
    },
    PairFailed(Refusal),
    /// A known address answered with a different key.
    DifferentMachine {
        addr: String,
    },
    /// Round trip of a ping.
    Rtt {
        ms: u64,
    },
}

/// Something that should end a wait at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Foreground,
    Background,
    NetworkChanged,
    /// The machine announced itself (Bonjour) or Tailscale sees it online.
    Seen,
    Retry,
}

enum Att {
    NotYet,
    Dialing,
    Handshake(Box<Initiator>),
    /// Handshake done, waiting for the verdict.
    Verdict(Box<Transport>),
    Failed,
}

struct Attempt {
    conn: u64,
    addr: String,
    start_at: u64,
    started: Option<u64>,
    att: Att,
}

/// A set of racing attempts.
struct Race {
    attempts: Vec<Attempt>,
    /// Untrusted answers, kept in case nothing trusts us.
    untrusted: Option<(u64, String, Box<Transport>, bool)>,
    /// Why the race's attempts failed, last first.
    why: Option<String>,
    /// A blocking refusal heard during the race.
    refused: Option<Refusal>,
}

enum PairStep {
    Spake(Box<dent_link::pair::Pairing>),
    Confirm(Vec<u8>),
    Done,
}

enum Phase {
    Off,
    Waiting {
        until: u64,
    },
    Racing(Race),
    /// Answered but untrusted: the connection stays open for pairing,
    /// kept alive by pings like a trusted one.
    Untrusted {
        conn: u64,
        addr: String,
        t: Box<Transport>,
        pairing: Option<(PairStep, u64)>,
        last_rx: u64,
        last_ping: u64,
    },
    Open(Box<Open>),
    /// Waiting for a signal: blocked by a refusal, or removed.
    Blocked,
}

struct Open {
    conn: u64,
    addr: String,
    t: Transport,
    generation: u64,
    opened: u64,
    last_rx: u64,
    /// When this side last pinged. A ping goes when nothing has been heard
    /// for a while (the daemon only answers, so a busy sender must still
    /// ping), and when nothing has been sent for a while (the daemon judges
    /// this side's liveness by what it hears, so a busy listener must still
    /// ping).
    last_ping: u64,
    /// When this side last sent any frame.
    last_tx: u64,
    slow: bool,
    /// A replacement racing beside the silent link.
    replace: Option<Race>,
    /// When the next extra dial joins the replacement race, and how many
    /// have so far (they cycle through the routes).
    redial_at: u64,
    redials: usize,
    /// Better routes being tried while on a slower one.
    home: Option<Race>,
    last_home: u64,
    /// A signal asked for a home check at the next tick.
    home_due: bool,
}

pub struct Machine {
    keys: Keys,
    name: String,
    /// The daemon's key once paired; None to add a new machine.
    known: Option<Vec<u8>>,
    routes: Vec<String>,
    phase: Phase,
    next_conn: u64,
    generation: u64,
    attempt: usize,
    io: VecDeque<Io>,
    notes: Vec<Note>,
    state: State,
    foreground: bool,
    last_online: Option<u64>,
    /// Redial fast until then, after an announced restart.
    restart_until: Option<u64>,
    /// An announced restart that didn't come back in time.
    missed_restart: bool,
    /// Routes that answered then failed, until when they're left alone.
    cooldown: HashMap<String, u64>,
    /// The code to pair with, kept until an outcome: a dropped connection
    /// resumes the same pairing on the next one.
    code: Option<String>,
    rng: u64,
}

impl Machine {
    /// `known` is the daemon's key from an earlier pairing (None to add a new
    /// machine); `routes` are `host:port`, best first.
    pub fn new(name: &str, keys: Keys, known: Option<Vec<u8>>, routes: Vec<String>) -> Self {
        let rng = keys.public.iter().fold(0x9e37_79b9_7f4a_7c15u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
        }) | 1;
        Self {
            keys,
            name: name.into(),
            known,
            routes,
            phase: Phase::Off,
            next_conn: 1,
            generation: 0,
            attempt: 0,
            io: VecDeque::new(),
            notes: Vec::new(),
            state: State::Off,
            foreground: true,
            last_online: None,
            restart_until: None,
            missed_restart: false,
            cooldown: HashMap::new(),
            code: None,
            rng,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn take_io(&mut self) -> Vec<Io> {
        self.io.drain(..).collect()
    }

    pub fn take_notes(&mut self) -> Vec<Note> {
        std::mem::take(&mut self.notes)
    }

    pub fn known_key(&self) -> Option<&[u8]> {
        self.known.as_deref()
    }

    /// Replaces the routes (for example, with ones the daemon reported).
    /// A live link is never replaced for this alone.
    pub fn set_routes(&mut self, routes: Vec<String>) {
        self.routes = routes;
    }

    pub fn routes(&self) -> &[String] {
        &self.routes
    }

    /// Start connecting, and keep at it.
    pub fn connect(&mut self, now: u64) {
        if matches!(
            self.phase,
            Phase::Off | Phase::Blocked | Phase::Waiting { .. }
        ) {
            self.attempt = 0;
            self.race(now);
        }
    }

    /// Stop and stay stopped.
    pub fn disconnect(&mut self) {
        self.code = None;
        self.close_all();
        self.phase = Phase::Off;
        self.set_state(State::Off);
    }

    /// Pair with the code shown on the machine. Uses the open untrusted
    /// connection if there is one, else connects first. The code is kept
    /// until pairing succeeds or is refused, so a dropped connection
    /// resumes it.
    pub fn pair(&mut self, code: &str, now: u64) {
        self.code = Some(code.to_string());
        match self.phase {
            // An exchange in flight finishes first: a second one would
            // break it on the machine's side.
            Phase::Untrusted {
                pairing: Some(_), ..
            } => {}
            Phase::Untrusted { .. } => self.start_pair(now),
            // Its outcome decides; an untrusted answer pairs then.
            Phase::Racing(_) => {}
            // Already trusted: nothing to pair.
            Phase::Open(_) => self.code = None,
            Phase::Off | Phase::Waiting { .. } | Phase::Blocked => {
                self.attempt = 0;
                self.race(now);
            }
        }
    }

    fn start_pair(&mut self, now: u64) {
        let Some(code) = self.code.clone() else {
            return;
        };
        let Phase::Untrusted {
            conn, t, pairing, ..
        } = &mut self.phase
        else {
            return;
        };
        let p = dent_link::pair::Pairing::start(&code);
        let msg = Control::Pair {
            spake: hex::encode(&p.outbound),
        };
        if let Ok(f) = t.seal_control(&msg) {
            self.io.push_back(Io::Send {
                conn: *conn,
                data: f,
            });
        }
        *pairing = Some((PairStep::Spake(Box::new(p)), now + STEP_MS));
    }

    /// Something changed that's worth acting on at once.
    pub fn signal(&mut self, s: Signal, now: u64) {
        match s {
            Signal::Background => {
                self.foreground = false;
                return;
            }
            Signal::Foreground => self.foreground = true,
            _ => {}
        }
        let removed = self.state == State::Removed;
        match &mut self.phase {
            Phase::Off => {}
            Phase::Waiting { .. } => {
                self.attempt = 0;
                self.race(now);
            }
            Phase::Blocked if !removed => {
                self.attempt = 0;
                self.race(now);
            }
            // An attempt in flight carries on, but the ladder starts again.
            Phase::Racing(_) => self.attempt = 0,
            Phase::Open(o) => {
                // Probe the live link rather than replace it; check better
                // routes too.
                if let Ok(f) = o.t.seal_control(&Control::Ping { t: now }) {
                    self.io.push_back(Io::Send {
                        conn: o.conn,
                        data: f,
                    });
                    o.last_ping = now;
                    o.last_tx = now;
                }
                o.home_due = true;
            }
            _ => {}
        }
        self.tick(now);
    }

    /// Sends one data message (a JSON line) on the live link. Returns
    /// false when there's none; the caller keeps it for later.
    pub fn send(&mut self, line: &[u8], now: u64) -> bool {
        let Phase::Open(o) = &mut self.phase else {
            return false;
        };
        match o.t.seal_data(line) {
            Ok(frames) => {
                for data in frames {
                    self.io.push_back(Io::Send { conn: o.conn, data });
                }
                o.last_tx = now;
                true
            }
            Err(_) => false,
        }
    }

    /// When the host should next call [`Machine::tick`].
    pub fn next_deadline(&self) -> Option<u64> {
        match &self.phase {
            Phase::Off | Phase::Blocked => None,
            Phase::Waiting { until } => Some(*until),
            Phase::Racing(r) => race_deadline(r),
            Phase::Untrusted {
                pairing,
                last_rx,
                last_ping,
                ..
            } => {
                let d = ((*last_rx).max(*last_ping) + PING_MS).min(last_rx + SILENCE_MS);
                Some(pairing.as_ref().map_or(d, |(_, p)| d.min(*p)))
            }
            Phase::Open(o) => {
                // Only deadlines still ahead: one already acted on (slow
                // shown, replacement running) would wake the host in a loop.
                let mut d = (o.last_rx.max(o.last_ping) + PING_MS)
                    .min(o.last_tx + PING_MS)
                    .min(o.last_rx + SILENCE_MS);
                if !o.slow {
                    d = d.min(o.last_rx + SLOW_MS);
                }
                if o.replace.is_none() {
                    d = d.min(o.last_rx + REPLACE_MS);
                } else {
                    d = d.min(o.redial_at);
                }
                for r in [&o.replace, &o.home].into_iter().flatten() {
                    if let Some(rd) = race_deadline(r) {
                        d = d.min(rd);
                    }
                }
                if o.home.is_none() && self.better_routes(&o.addr, o.last_rx).next().is_some() {
                    d = d.min(o.last_home + HOME_EVERY_MS);
                }
                Some(d)
            }
        }
    }

    pub fn tick(&mut self, now: u64) {
        match &mut self.phase {
            Phase::Waiting { until } if now >= *until => self.race(now),
            Phase::Racing(_) => self.step_race(now, Slot::Main),
            Phase::Untrusted {
                conn,
                t,
                pairing,
                last_rx,
                last_ping,
                ..
            } => {
                // A stalled or silent link is dropped; the code is kept, so
                // the next connection resumes the pairing.
                if pairing.as_ref().is_some_and(|(_, d)| now >= *d) {
                    self.after_loss(now, "pairing stalled".into());
                } else if now >= *last_rx + SILENCE_MS {
                    self.after_loss(now, format!("silent for {} s", SILENCE_MS / 1000));
                } else if now >= (*last_rx).max(*last_ping) + PING_MS
                    && let Ok(f) = t.seal_control(&Control::Ping { t: now })
                {
                    self.io.push_back(Io::Send {
                        conn: *conn,
                        data: f,
                    });
                    *last_ping = now;
                }
            }
            Phase::Open(_) => self.tick_open(now),
            _ => {}
        }
    }

    pub fn opened(&mut self, conn: u64, now: u64) {
        let keys = self.keys.clone();
        let Some(a) = self.attempt_mut(conn) else {
            return;
        };
        if !matches!(a.att, Att::Dialing) {
            return;
        }
        match Initiator::start(&keys) {
            Ok((ini, m1)) => {
                a.att = Att::Handshake(Box::new(ini));
                a.started.get_or_insert(now);
                self.io.push_back(Io::Send { conn, data: m1 });
            }
            Err(_) => {
                a.att = Att::Failed;
                self.io.push_back(Io::Close { conn });
            }
        }
        self.tick(now);
    }

    pub fn closed(&mut self, conn: u64, why: &str, now: u64) {
        // An untrusted answer kept in a race: it's gone.
        if let Some(slot) = self.untrusted_slot(conn) {
            if let Some(r) = self.race_ref(slot) {
                r.untrusted = None;
                r.why = Some(why.to_string());
            }
            self.settle_race(now, slot);
            return;
        }
        if let Some(slot) = self.slot_of(conn) {
            if let Some(a) = self.attempt_mut(conn) {
                a.att = Att::Failed;
            }
            if let Some(r) = self.race_ref(slot) {
                r.why = Some(why.to_string());
            }
            self.tick(now);
            return;
        }
        match &self.phase {
            Phase::Open(o) if o.conn == conn => self.lose_open(now, why.to_string()),
            Phase::Untrusted { conn: c, .. } if *c == conn => self.after_loss(now, why.to_string()),
            _ => {}
        }
    }

    pub fn received(&mut self, conn: u64, data: &[u8], now: u64) {
        // An untrusted answer kept in a race? Its frames are opened in
        // order, or the pairing that may follow would fail.
        if let Some(slot) = self.untrusted_slot(conn) {
            self.race_untrusted_received(slot, conn, data, now);
            return;
        }
        // An attempt in progress?
        if self.attempt_mut(conn).is_some() {
            self.attempt_received(conn, data, now);
            return;
        }
        match &mut self.phase {
            Phase::Open(o) if o.conn == conn => {
                o.last_rx = now;
                // The old link proved alive first: it wins.
                if let Some(r) = o.replace.take() {
                    for c in race_conns(&r) {
                        self.io.push_back(Io::Close { conn: c });
                    }
                }
                let Phase::Open(o) = &mut self.phase else {
                    return;
                };
                if o.slow {
                    o.slow = false;
                    self.set_state(State::Online);
                }
                let Phase::Open(o) = &mut self.phase else {
                    return;
                };
                match o.t.open(data) {
                    Err(_) => self.lose_open(now, "the link was garbled".into()),
                    Ok(None) => {}
                    Ok(Some(Opened::Data(line))) => {
                        let generation = o.generation;
                        self.notes.push(Note::Data { generation, line });
                    }
                    Ok(Some(Opened::Control(c))) => self.open_control(c, now),
                }
            }
            Phase::Untrusted {
                conn: c, last_rx, ..
            } if *c == conn => {
                *last_rx = now;
                self.untrusted_received(data, now);
            }
            _ => {}
        }
    }

    // ---- racing ----

    fn race(&mut self, now: u64) {
        self.close_all();
        let race = self.new_race(self.routes.clone(), now);
        if race.attempts.is_empty() {
            self.phase = Phase::Blocked;
            self.set_state(State::Unreachable {
                why: "no address to try".into(),
            });
            return;
        }
        self.phase = Phase::Racing(race);
        if !matches!(self.state, State::Reconnecting | State::Away(_)) {
            self.set_state(State::Connecting);
        }
        self.step_race(now, Slot::Main);
    }

    fn new_race(&mut self, addrs: Vec<String>, now: u64) -> Race {
        let attempts = addrs
            .into_iter()
            .enumerate()
            .map(|(i, addr)| {
                let conn = self.next_conn;
                self.next_conn += 1;
                Attempt {
                    conn,
                    addr,
                    start_at: now + STAGGER_MS * i as u64,
                    started: None,
                    att: Att::NotYet,
                }
            })
            .collect();
        Race {
            attempts,
            untrusted: None,
            why: None,
            refused: None,
        }
    }

    fn race_ref(&mut self, slot: Slot) -> Option<&mut Race> {
        match (&mut self.phase, slot) {
            (Phase::Racing(r), Slot::Main) => Some(r),
            (Phase::Open(o), Slot::Replace) => o.replace.as_mut(),
            (Phase::Open(o), Slot::Home) => o.home.as_mut(),
            _ => None,
        }
    }

    fn attempt_mut(&mut self, conn: u64) -> Option<&mut Attempt> {
        let slot = self.slot_of(conn)?;
        self.race_ref(slot)?
            .attempts
            .iter_mut()
            .find(|a| a.conn == conn)
    }

    fn slot_of(&self, conn: u64) -> Option<Slot> {
        let has = |r: &Race| r.attempts.iter().any(|a| a.conn == conn);
        match &self.phase {
            Phase::Racing(r) if has(r) => Some(Slot::Main),
            Phase::Open(o) if o.replace.as_ref().is_some_and(has) => Some(Slot::Replace),
            Phase::Open(o) if o.home.as_ref().is_some_and(has) => Some(Slot::Home),
            _ => None,
        }
    }

    fn step_race(&mut self, now: u64, slot: Slot) {
        let mut opens = Vec::new();
        let mut closes = Vec::new();
        {
            let Some(r) = self.race_ref(slot) else { return };
            for a in &mut r.attempts {
                match a.att {
                    Att::NotYet if now >= a.start_at => {
                        a.att = Att::Dialing;
                        a.started = Some(now);
                        opens.push((a.conn, a.addr.clone()));
                    }
                    Att::Dialing | Att::Handshake(_) | Att::Verdict(_)
                        if a.started.is_some_and(|s| now >= s + OPEN_MS) =>
                    {
                        a.att = Att::Failed;
                        r.why = Some(format!(
                            "{} didn't answer within {} s",
                            a.addr,
                            OPEN_MS / 1000
                        ));
                        closes.push(a.conn);
                    }
                    _ => {}
                }
            }
        }
        for (conn, addr) in opens {
            self.io.push_back(Io::Connect { conn, addr });
        }
        for conn in closes {
            self.io.push_back(Io::Close { conn });
        }
        self.settle_race(now, slot);
    }

    /// Whether the race is over, and what follows.
    fn settle_race(&mut self, now: u64, slot: Slot) {
        let Some(r) = self.race_ref(slot) else { return };
        let all_failed = r.attempts.iter().all(|a| matches!(a.att, Att::Failed));
        if !all_failed {
            return;
        }
        let untrusted = r.untrusted.take();
        let refused = r.refused.take();
        let why = r.why.take().unwrap_or_else(|| "no address answered".into());
        match slot {
            Slot::Replace | Slot::Home => {
                if let Phase::Open(o) = &mut self.phase {
                    if slot == Slot::Replace {
                        o.replace = None;
                    } else {
                        o.home = None;
                    }
                }
                if let Some((conn, ..)) = untrusted {
                    self.io.push_back(Io::Close { conn });
                }
            }
            Slot::Main => {
                if let Some((conn, addr, t, pairing_open)) = untrusted {
                    self.phase = Phase::Untrusted {
                        conn,
                        addr,
                        t,
                        pairing: None,
                        last_rx: now,
                        last_ping: now,
                    };
                    self.set_state(State::NeedsCode { pairing_open });
                    self.start_pair(now);
                    return;
                }
                match refused {
                    Some(r) => self.blocked_by(r, now),
                    None => self.after_loss(now, why),
                }
            }
        }
    }

    fn untrusted_slot(&self, conn: u64) -> Option<Slot> {
        let has = |r: &Race| r.untrusted.as_ref().is_some_and(|u| u.0 == conn);
        match &self.phase {
            Phase::Racing(r) if has(r) => Some(Slot::Main),
            Phase::Open(o) if o.replace.as_ref().is_some_and(has) => Some(Slot::Replace),
            Phase::Open(o) if o.home.as_ref().is_some_and(has) => Some(Slot::Home),
            _ => None,
        }
    }

    fn race_untrusted_received(&mut self, slot: Slot, conn: u64, data: &[u8], now: u64) {
        let Some(r) = self.race_ref(slot) else {
            return;
        };
        let Some((_, _, t, _)) = r.untrusted.as_mut() else {
            return;
        };
        let (keep, reply) = match t.open(data) {
            Ok(Some(Opened::Control(Control::Ping { t: n }))) => {
                (true, t.seal_control(&Control::Pong { t: n }).ok())
            }
            Ok(Some(Opened::Control(Control::Refuse(_)))) | Err(_) => (false, None),
            _ => (true, None),
        };
        if !keep {
            r.untrusted = None;
            self.io.push_back(Io::Close { conn });
            self.settle_race(now, slot);
        } else if let Some(data) = reply {
            self.io.push_back(Io::Send { conn, data });
        }
    }

    fn attempt_received(&mut self, conn: u64, data: &[u8], now: u64) {
        let Some(slot) = self.slot_of(conn) else {
            return;
        };
        let known = self.known.clone();
        let hello = Hello {
            name: self.name.clone(),
            proto_min: dent_protocol::PROTOCOL_MIN,
            proto: dent_protocol::PROTOCOL,
            probe: false,
        };
        let mut done: Option<(String, Box<Transport>, Trust)> = None;
        let mut diff_machine: Option<String> = None;
        let mut stale: Option<String> = None;
        {
            let Some(a) = self.attempt_mut(conn) else {
                return;
            };
            let addr = a.addr.clone();
            let att = std::mem::replace(&mut a.att, Att::Failed);
            match att {
                Att::Handshake(ini) => match ini.read(data, known.as_deref()) {
                    Ok(ans) => match ans.finish(&hello) {
                        Ok((t, m3)) => {
                            a.att = Att::Verdict(Box::new(t));
                            self.io.push_back(Io::Send { conn, data: m3 });
                        }
                        Err(_) => self.io.push_back(Io::Close { conn }),
                    },
                    Err(e) => {
                        if matches!(e, dent_link::Error::WrongKey(_)) {
                            diff_machine = Some(a.addr.clone());
                            stale = Some(a.addr.clone());
                        }
                        self.io.push_back(Io::Close { conn });
                    }
                },
                Att::Verdict(mut t) => match t.open(data) {
                    Ok(Some(Opened::Control(Control::Trust(v)))) => {
                        done = Some((a.addr.clone(), t, v));
                    }
                    Ok(Some(Opened::Control(Control::Refuse(r)))) => {
                        self.io.push_back(Io::Close { conn });
                        if matches!(r, Refusal::TrustedOn { .. }) {
                            // Not usable for this device: demote it rather
                            // than ask again at every home check.
                            stale = Some(addr.clone());
                        }
                        if let Some(race) = self.race_ref(slot) {
                            race.why = Some(refusal_words(&r));
                            if !r.transient() {
                                race.refused = Some(r);
                            }
                        }
                    }
                    Ok(_) => {
                        // Something else first: keep waiting for the verdict.
                        if let Some(a) = self.attempt_mut(conn) {
                            a.att = Att::Verdict(t);
                        }
                    }
                    Err(_) => self.io.push_back(Io::Close { conn }),
                },
                other => {
                    if let Some(a) = self.attempt_mut(conn) {
                        a.att = other;
                    }
                }
            }
        }
        if let Some(addr) = stale {
            self.cooldown.insert(addr, now + COOLDOWN_MS);
        }
        if let Some(addr) = diff_machine {
            self.notes
                .push(Note::DifferentMachine { addr: addr.clone() });
            if let Some(r) = self.race_ref(slot) {
                r.why = Some(format!("a different machine answered at {addr}"));
            }
        }
        if let Some((addr, t, verdict)) = done {
            match verdict {
                Trust::Untrusted { pairing_open } => {
                    if let Some(r) = self.race_ref(slot) {
                        // Keep the first untrusted answer; close later ones.
                        if r.untrusted.is_none() {
                            r.untrusted = Some((conn, addr, t, pairing_open));
                        } else {
                            self.io.push_back(Io::Close { conn });
                        }
                    }
                }
                trust => {
                    self.won(slot, conn, addr, *t, trust, now);
                    return;
                }
            }
        }
        self.settle_race(now, slot);
    }

    /// An attempt came up trusted: it becomes the live link.
    fn won(
        &mut self,
        slot: Slot,
        conn: u64,
        addr: String,
        mut t: Transport,
        trust: Trust,
        now: u64,
    ) {
        t.limit = dent_link::LARGE;
        // Close every other attempt in this race (and the old link when
        // this replaces it).
        let mut close = Vec::new();
        if let Some(r) = self.race_ref(slot) {
            for a in &r.attempts {
                if a.conn != conn && !matches!(a.att, Att::Failed) {
                    close.push(a.conn);
                }
            }
            if let Some((c, ..)) = r.untrusted.as_ref() {
                close.push(*c);
            }
        }
        if let Phase::Open(o) = &self.phase {
            close.push(o.conn);
            for r in [&o.replace, &o.home].into_iter().flatten() {
                close.extend(race_conns(r));
            }
            // A healthy link swapped out still counts toward a stable run.
            if now.saturating_sub(o.opened) >= STABLE_MS {
                self.attempt = 0;
            }
        }
        for c in close {
            if c != conn {
                self.io.push_back(Io::Close { conn: c });
            }
        }
        self.generation += 1;
        self.cooldown.remove(&addr);
        let key = t.remote.clone();
        if self.known.is_none() || matches!(trust, Trust::Paired { .. }) {
            self.known = Some(t.remote.clone());
        }
        self.restart_until = None;
        self.missed_restart = false;
        self.code = None;
        self.last_online = Some(now);
        self.phase = Phase::Open(Box::new(Open {
            conn,
            addr: addr.clone(),
            t,
            generation: self.generation,
            opened: now,
            last_rx: now,
            last_ping: now,
            last_tx: now,
            slow: false,
            replace: None,
            redial_at: now,
            redials: 0,
            home: None,
            last_home: now,
            home_due: false,
        }));
        self.notes.push(Note::Up {
            generation: self.generation,
            addr,
            trust,
            key,
        });
        self.set_state(State::Online);
    }

    // ---- open ----

    fn tick_open(&mut self, now: u64) {
        let better: Vec<String> = match &self.phase {
            Phase::Open(o) => self.better_routes(&o.addr, now).collect(),
            _ => return,
        };
        let mut to_replace = false;
        let mut redial: Option<usize> = None;
        let mut to_home = false;
        {
            let Phase::Open(o) = &mut self.phase else {
                return;
            };
            let quiet = now.saturating_sub(o.last_rx);
            if quiet >= SILENCE_MS {
                let why = format!("silent for {} s", SILENCE_MS / 1000);
                self.lose_open(now, why);
                return;
            }
            if quiet >= SLOW_MS && !o.slow {
                o.slow = true;
                self.set_state(State::Slow);
            }
            let Phase::Open(o) = &mut self.phase else {
                return;
            };
            let unheard = now >= o.last_rx.max(o.last_ping) + PING_MS;
            let unsent = now >= o.last_tx + PING_MS;
            if (unheard || unsent)
                && let Ok(f) = o.t.seal_control(&Control::Ping { t: now })
            {
                self.io.push_back(Io::Send {
                    conn: o.conn,
                    data: f,
                });
                o.last_ping = now;
                o.last_tx = now;
            }
            if quiet >= REPLACE_MS && o.replace.is_none() {
                to_replace = true;
            } else if o.replace.is_some() && now >= o.redial_at {
                o.redial_at = now + REDIAL_MS;
                redial = Some(o.redials);
                o.redials += 1;
            }
            let due = o.home_due || now >= o.last_home + HOME_EVERY_MS;
            o.home_due = false;
            if o.home.is_none() && !better.is_empty() && due {
                o.last_home = now;
                to_home = true;
            }
        }
        let replace_routes = match &self.phase {
            Phase::Open(o) if self.routes.is_empty() => vec![o.addr.clone()],
            _ => self.routes.clone(),
        };
        if to_replace {
            let r = self.new_race(replace_routes.clone(), now);
            if let Phase::Open(o) = &mut self.phase {
                o.replace = Some(r);
                o.redial_at = now + REDIAL_MS;
                o.redials = 0;
            }
        } else if let Some(i) = redial {
            // Still silent: one more dial joins the race every second.
            let addr = replace_routes[i % replace_routes.len()].clone();
            let mut extra = self.new_race(vec![addr], now);
            if let Phase::Open(o) = &mut self.phase
                && let Some(r) = o.replace.as_mut()
            {
                r.attempts.append(&mut extra.attempts);
            }
        }
        if to_home {
            let r = self.new_race(better, now);
            if let Phase::Open(o) = &mut self.phase {
                o.home = Some(r);
            }
        }
        self.step_race(now, Slot::Replace);
        self.step_race(now, Slot::Home);
    }

    /// Routes ahead of `current` that aren't cooling down.
    fn better_routes<'a>(
        &'a self,
        current: &'a str,
        now: u64,
    ) -> impl Iterator<Item = String> + 'a {
        self.routes
            .iter()
            .take_while(move |a| a.as_str() != current)
            .filter(move |a| self.cooldown.get(*a).is_none_or(|until| now >= *until))
            .cloned()
    }

    fn open_control(&mut self, c: Control, now: u64) {
        match c {
            Control::Ping { t } => {
                if let Phase::Open(o) = &mut self.phase
                    && let Ok(f) = o.t.seal_control(&Control::Pong { t })
                {
                    self.io.push_back(Io::Send {
                        conn: o.conn,
                        data: f,
                    });
                }
            }
            Control::Pong { t } if t > 0 && t <= now => self.notes.push(Note::Rtt { ms: now - t }),
            // Awake only matters to a client waiting on backoff; this link
            // is live.
            Control::Goodbye(Goodbye::Awake) => {}
            Control::Goodbye(g) => {
                let back_soon = matches!(
                    g,
                    Goodbye::Restart { .. } | Goodbye::Update { .. } | Goodbye::Awake
                );
                self.close_all();
                self.set_state(State::Away(g));
                if back_soon {
                    self.restart_until = Some(now + RESTART_FOR_MS);
                    self.missed_restart = false;
                    self.phase = Phase::Waiting {
                        until: now + RESTART_EVERY_MS,
                    };
                } else {
                    // Asleep or quit: wait for it to be seen again, with the
                    // ladder as a fallback.
                    self.attempt = self.attempt.max(QUICK.len());
                    let wait = self.wait(now);
                    self.phase = Phase::Waiting { until: now + wait };
                }
            }
            Control::Refuse(r) => {
                if r.transient() {
                    self.lose_open(now, refusal_words(&r));
                } else {
                    self.close_all();
                    self.blocked_by(r, now);
                }
            }
            _ => {}
        }
    }

    fn lose_open(&mut self, now: u64, why: String) {
        let Phase::Open(o) = std::mem::replace(&mut self.phase, Phase::Waiting { until: u64::MAX })
        else {
            return;
        };
        let healthy = now.saturating_sub(o.opened) >= STABLE_MS;
        if healthy {
            self.attempt = 0;
        } else if now < self.last_online.unwrap_or(0) + STABLE_MS {
            // Answered, then failed fast: leave that route alone for a while.
            self.cooldown.insert(o.addr.clone(), now + COOLDOWN_MS);
        }
        self.io.push_back(Io::Close { conn: o.conn });
        if let Some(h) = &o.home {
            for c in race_conns(h) {
                self.io.push_back(Io::Close { conn: c });
            }
        }
        self.set_state(State::Reconnecting);
        // A replacement already dialling carries on as the race.
        if let Some(mut r) = o.replace {
            r.why = Some(why);
            self.phase = Phase::Racing(r);
            self.step_race(now, Slot::Main);
            return;
        }
        if healthy {
            // A lost healthy link tries again at once, through every route;
            // if that fails, `why` is what the person sees.
            self.race(now);
            if let Phase::Racing(r) = &mut self.phase {
                r.why = Some(why);
            }
        } else {
            // One that opened and died keeps climbing the ladder, or a
            // machine that accepts and drops would be redialled in a loop.
            let wait = self.wait(now);
            self.attempt += 1;
            self.phase = Phase::Waiting { until: now + wait };
        }
    }

    // ---- untrusted and pairing ----

    fn untrusted_received(&mut self, data: &[u8], now: u64) {
        let Phase::Untrusted {
            conn, t, pairing, ..
        } = &mut self.phase
        else {
            return;
        };
        let conn = *conn;
        let opened = match t.open(data) {
            Ok(Some(Opened::Control(c))) => c,
            Ok(_) => return,
            Err(_) => {
                self.io.push_back(Io::Close { conn });
                self.after_loss(now, "the link was garbled".into());
                return;
            }
        };
        match (opened, pairing.as_mut()) {
            (Control::Ping { t: n }, _) => {
                if let Ok(f) = t.seal_control(&Control::Pong { t: n }) {
                    self.io.push_back(Io::Send { conn, data: f });
                }
            }
            // Only in answer to ours: a second one must not move pairing on
            // past the machine's proof of the code.
            (Control::Pair { spake }, Some((step, deadline)))
                if matches!(step, PairStep::Spake(_)) =>
            {
                let PairStep::Spake(p) = std::mem::replace(step, PairStep::Done) else {
                    return;
                };
                let mut p = *p;
                let key = match hex::decode(&spake).ok().and_then(|s| p.finish(&s).ok()) {
                    Some(k) => k,
                    None => return self.pair_failed(Refusal::PairWrong),
                };
                let mac = dent_link::pair::confirm(&key, &t.hash, dent_link::pair::Role::Client);
                if let Ok(f) = t.seal_control(&Control::PairConfirm {
                    mac: hex::encode(mac),
                }) {
                    self.io.push_back(Io::Send { conn, data: f });
                }
                *step = PairStep::Confirm(key);
                *deadline = now + STEP_MS;
            }
            (Control::PairConfirm { mac }, Some((PairStep::Confirm(key), _))) => {
                let ok = hex::decode(&mac).is_ok_and(|m| {
                    dent_link::pair::verify(key, &t.hash, dent_link::pair::Role::Daemon, &m)
                });
                if !ok {
                    // The machine couldn't prove it knows the code: not the
                    // machine the code came from.
                    self.pair_failed(Refusal::PairWrong);
                    return self.after_loss(now, "the machine couldn't prove the code".into());
                }
                *pairing = Some((PairStep::Done, now + STEP_MS));
            }
            // Only once the machine has proved it knows the code.
            (Control::Paired { access }, Some((PairStep::Done, _))) => {
                let Phase::Untrusted { conn, addr, t, .. } =
                    std::mem::replace(&mut self.phase, Phase::Off)
                else {
                    return;
                };
                self.known = Some(t.remote.clone());
                self.code = None;
                self.notes.push(Note::Paired {
                    key: t.remote.clone(),
                });
                self.phase = Phase::Racing(Race {
                    attempts: vec![Attempt {
                        conn,
                        addr: addr.clone(),
                        start_at: now,
                        started: Some(now),
                        att: Att::Failed,
                    }],
                    untrusted: None,
                    why: None,
                    refused: None,
                });
                self.won(Slot::Main, conn, addr, *t, Trust::Paired { access }, now);
            }
            (Control::Refuse(r), _) => self.pair_failed(r),
            _ => {}
        }
    }

    /// Pairing didn't work; the connection stays open for another try.
    fn pair_failed(&mut self, r: Refusal) {
        self.code = None;
        self.notes.push(Note::PairFailed(r));
        if let Phase::Untrusted { pairing, .. } = &mut self.phase {
            *pairing = None;
        }
    }

    // ---- waiting ----

    fn after_loss(&mut self, now: u64, why: String) {
        self.close_all();
        if let Some(until) = self.restart_until
            && now < until
        {
            self.phase = Phase::Waiting {
                until: now + RESTART_EVERY_MS,
            };
            return;
        }
        // The restart window ran out: back to the ladder, and say so until
        // it's back.
        self.missed_restart |= self.restart_until.take().is_some();
        let wait = self.wait(now);
        self.attempt += 1;
        self.phase = Phase::Waiting { until: now + wait };
        if self.missed_restart {
            self.set_state(State::Unreachable {
                why: format!("didn't come back ({why})"),
            });
        } else if !matches!(self.state, State::Away(_)) {
            self.set_state(State::Unreachable { why });
        }
    }

    fn blocked_by(&mut self, r: Refusal, now: u64) {
        self.close_all();
        self.phase = Phase::Blocked;
        let state = match r {
            Refusal::Removed => {
                self.known = None;
                State::Removed
            }
            Refusal::Incompatible { daemon, client } => State::Incompatible { daemon, client },
            Refusal::TrustedOn { routes } => State::TrustedElsewhere { routes },
            Refusal::NotTrusted => State::NeedsCode {
                pairing_open: false,
            },
            Refusal::LanOff => State::Unreachable {
                why: refusal_words(&r),
            },
            other => {
                // Anything else blocking: back off like a failure.
                self.after_loss(now, refusal_words(&other));
                return;
            }
        };
        self.set_state(state);
    }

    /// The wait before the next attempt.
    fn wait(&mut self, now: u64) -> u64 {
        if let Some(w) = QUICK.get(self.attempt) {
            return *w;
        }
        let near = self.foreground && self.last_online.is_some_and(|t| now < t + NEAR_MS);
        let cap = if near { CAP_NEAR_MS } else { CAP_FAR_MS };
        // Doubling from the last quick wait: steps of 2, 4, 8 s and on.
        let steps = (self.attempt + 1 - QUICK.len()).min(16) as u32;
        let ceiling = (QUICK[QUICK.len() - 1] << steps).min(cap);
        let half = ceiling / 2;
        half + self.random() % (half + 1)
    }

    fn random(&mut self) -> u64 {
        // xorshift: jitter only needs to spread clients apart.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    fn close_all(&mut self) {
        let mut close = Vec::new();
        match &self.phase {
            Phase::Racing(r) => close.extend(race_conns(r)),
            Phase::Untrusted { conn, .. } => close.push(*conn),
            Phase::Open(o) => {
                close.push(o.conn);
                for r in [&o.replace, &o.home].into_iter().flatten() {
                    close.extend(race_conns(r));
                }
            }
            _ => {}
        }
        for conn in close {
            self.io.push_back(Io::Close { conn });
        }
        if !matches!(self.phase, Phase::Off | Phase::Blocked) {
            self.phase = Phase::Waiting { until: u64::MAX };
        }
    }

    fn set_state(&mut self, s: State) {
        if self.state != s {
            self.state = s.clone();
            self.notes.push(Note::State(s));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Main,
    Replace,
    Home,
}

/// A race's sockets that are open or opening.
fn race_conns(r: &Race) -> Vec<u64> {
    let mut v: Vec<u64> = r
        .attempts
        .iter()
        .filter(|a| !matches!(a.att, Att::Failed | Att::NotYet))
        .map(|a| a.conn)
        .collect();
    if let Some((c, ..)) = r.untrusted.as_ref() {
        v.push(*c);
    }
    v
}

fn race_deadline(r: &Race) -> Option<u64> {
    r.attempts
        .iter()
        .filter_map(|a| match a.att {
            Att::NotYet => Some(a.start_at),
            Att::Dialing | Att::Handshake(_) | Att::Verdict(_) => a.started.map(|s| s + OPEN_MS),
            Att::Failed => None,
        })
        .min()
}

/// A refusal in words a person can act on.
/// A machine's address as typed, made whole: dent's port (7480) when none
/// is given, brackets around a bare IPv6 address. `None` when it can't be
/// an address at all, so it's said at once rather than tried.
pub fn address(typed: &str) -> Option<String> {
    let a = typed.trim();
    let whole = if a.starts_with('[') {
        if a.contains("]:") {
            a.to_string()
        } else {
            format!("{a}:7480")
        }
    } else if a.matches(':').count() > 1 {
        // A bare IPv6 address.
        format!("[{a}]:7480")
    } else if a.contains(':') {
        a.to_string()
    } else {
        format!("{a}:7480")
    };
    looks_like_address(&whole).then_some(whole)
}

/// Whether `addr` could be a machine's address: a host (a name, an IPv4
/// address or a bracketed IPv6 one) and a port.
pub fn looks_like_address(addr: &str) -> bool {
    let Some((host, port)) = addr.rsplit_once(':') else {
        return false;
    };
    if port.parse::<u16>().map_or(true, |p| p == 0) {
        return false;
    }
    if let Some(v6) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return v6.parse::<std::net::Ipv6Addr>().is_ok();
    }
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

pub fn refusal_words(r: &Refusal) -> String {
    match r {
        Refusal::Incompatible { daemon, client } => format!(
            "no protocol in common: the machine speaks {}–{}, this app {}–{}; update the {}",
            daemon.0,
            daemon.1,
            client.0,
            client.1,
            if daemon.1 < client.0 {
                "machine"
            } else {
                "app"
            }
        ),
        Refusal::NotTrusted => "this device isn't paired with that machine".into(),
        Refusal::TrustedOn { routes } => format!("trusted only over {}", routes.join(", ")),
        Refusal::Removed => "the machine removed this device".into(),
        Refusal::Starting { .. } => "the machine is starting".into(),
        Refusal::Busy { .. } => "the machine is busy".into(),
        Refusal::RateLimited { .. } => "too many connections from here".into(),
        Refusal::PairWrong => "wrong code".into(),
        Refusal::PairWrongNewCode => "wrong code three times; the machine shows a new one".into(),
        Refusal::PairClosed => "no pairing code is showing on that machine".into(),
        Refusal::PairLocked { secs } => format!("pairing paused; try again in {secs} s"),
        Refusal::LanOff => "LAN connections are off on that machine".into(),
    }
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;

#[cfg(test)]
mod address_tests {
    use super::address;

    #[test]
    fn an_address_is_made_whole_or_refused_at_once() {
        assert_eq!(address(" mini ").as_deref(), Some("mini:7480"));
        assert_eq!(
            address("192.168.1.20").as_deref(),
            Some("192.168.1.20:7480")
        );
        assert_eq!(
            address("mini.tail12.ts.net:9000").as_deref(),
            Some("mini.tail12.ts.net:9000")
        );
        assert_eq!(address("fe80::1").as_deref(), Some("[fe80::1]:7480"));
        assert_eq!(address("[::1]").as_deref(), Some("[::1]:7480"));
        assert_eq!(address("[::1]:7481").as_deref(), Some("[::1]:7481"));
        for bad in [
            "",
            "not a host",
            "host:0",
            "host:99999",
            "host:",
            "a..b",
            "[nope]:1",
            "hö.st",
        ] {
            assert_eq!(address(bad), None, "{bad:?}");
        }
    }
}
