//! The connection actor against a simulated daemon that speaks the real
//! link ([`dent_link`]). The simulation moves frames instantly; whether an
//! address answers, and what the daemon decides, is up to each test.

use std::collections::HashMap;

use dent_link::{
    Access, Control, Goodbye, Hello, Keys, Opened, Refusal, Responder, Transport, Trust,
};

use super::*;

/// What one address does when dialled.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Addr {
    /// Answers as the daemon.
    Up,
    /// Never answers (packets dropped).
    Silent,
    /// Refuses the TCP connection at once.
    Refused,
    /// Answers as a different machine.
    Imposter,
}

enum Peer {
    Handshaking(Box<Responder>),
    Live(Box<Transport>),
}

struct Sim {
    m: Machine,
    now: u64,
    daemon: Keys,
    imposter: Keys,
    addrs: HashMap<String, Addr>,
    /// Device keys the daemon trusts.
    trusted: Vec<Vec<u8>>,
    code: Option<String>,
    /// Refuse every connection with this instead of a verdict.
    refuse: Option<Refusal>,
    conns: HashMap<u64, (String, Peer)>,
    /// Whether the daemon answers frames on live links (false: frozen).
    responsive: bool,
    notes: Vec<Note>,
    dials: Vec<String>,
    /// New dials never complete (the network is still down), while links
    /// already up stay as they are.
    hang: bool,
}

impl Sim {
    fn new(routes: &[(&str, Addr)]) -> Self {
        let keys = Keys::generate().unwrap();
        let daemon = Keys::generate().unwrap();
        let m = Machine::new(
            "phone",
            keys,
            Some(daemon.public.clone()),
            routes.iter().map(|(a, _)| a.to_string()).collect(),
        );
        Self {
            m,
            now: 0,
            daemon,
            imposter: Keys::generate().unwrap(),
            addrs: routes.iter().map(|(a, s)| (a.to_string(), *s)).collect(),
            trusted: Vec::new(),
            code: None,
            refuse: None,
            conns: HashMap::new(),
            responsive: true,
            notes: Vec::new(),
            dials: Vec::new(),
            hang: false,
        }
    }

    fn trust_device(&mut self) {
        let k = self.m.keys.public.clone();
        self.trusted.push(k);
    }

    /// Delivers every I/O request, repeatedly, until quiet.
    fn pump(&mut self) {
        for _ in 0..200 {
            let io = self.m.take_io();
            if io.is_empty() {
                break;
            }
            for op in io {
                self.do_io(op);
            }
        }
        self.notes.extend(self.m.take_notes());
    }

    fn do_io(&mut self, op: Io) {
        match op {
            Io::Connect { conn, addr } => {
                self.dials.push(addr.clone());
                if self.hang {
                    return;
                }
                match self.addrs.get(&addr).copied().unwrap_or(Addr::Silent) {
                    Addr::Up | Addr::Imposter => {
                        self.conns
                            .insert(conn, (addr, Peer::Handshaking(Box::new(dummy_responder()))));
                        self.m.opened(conn, self.now);
                    }
                    Addr::Refused => self.m.closed(conn, "connection refused", self.now),
                    Addr::Silent => {}
                }
            }
            Io::Close { conn } => {
                self.conns.remove(&conn);
            }
            Io::Send { conn, data } => self.daemon_receives(conn, data),
        }
    }

    fn daemon_receives(&mut self, conn: u64, data: Vec<u8>) {
        let Some((addr, peer)) = self.conns.remove(&conn) else {
            return;
        };
        let kind = self.addrs.get(&addr).copied().unwrap_or(Addr::Silent);
        if kind == Addr::Silent {
            return;
        }
        match peer {
            Peer::Handshaking(_) if is_msg1(&data) => {
                let keys = if kind == Addr::Imposter {
                    &self.imposter
                } else {
                    &self.daemon
                };
                let hello = Hello {
                    name: "studio".into(),
                    proto_min: 1,
                    proto: 1,
                    probe: false,
                };
                let (res, m2) = Responder::accept(keys, &data, &hello).unwrap();
                self.conns
                    .insert(conn, (addr, Peer::Handshaking(Box::new(res))));
                self.m.received(conn, &m2, self.now);
            }
            Peer::Handshaking(res) => {
                let (mut t, _hello) = res.finish(&data).unwrap();
                let verdict = if let Some(r) = self.refuse.clone() {
                    Control::Refuse(r)
                } else if self.trusted.contains(&t.remote) {
                    Control::Trust(Trust::Paired {
                        access: Access::Full,
                    })
                } else {
                    Control::Trust(Trust::Untrusted {
                        pairing_open: self.code.is_some(),
                    })
                };
                let f = t.seal_control(&verdict).unwrap();
                self.conns.insert(conn, (addr, Peer::Live(Box::new(t))));
                self.m.received(conn, &f, self.now);
            }
            Peer::Live(mut t) => {
                if !self.responsive {
                    self.conns.insert(conn, (addr, Peer::Live(t)));
                    return;
                }
                let replies = match t.open(&data) {
                    Ok(Some(Opened::Control(Control::Ping { t: n }))) => {
                        vec![Control::Pong { t: n }]
                    }
                    Ok(Some(Opened::Control(Control::Pair { spake }))) => {
                        self.daemon_pair_step1(&mut t, spake)
                    }
                    Ok(Some(Opened::Control(Control::PairConfirm { mac }))) => {
                        self.daemon_pair_step2(&mut t, mac)
                    }
                    _ => vec![],
                };
                let mut frames = Vec::new();
                for c in &replies {
                    frames.push(t.seal_control(c).unwrap());
                }
                self.conns.insert(conn, (addr, Peer::Live(t)));
                for f in frames {
                    self.m.received(conn, &f, self.now);
                }
            }
        }
    }

    fn daemon_pair_step1(&mut self, _t: &mut Transport, spake: String) -> Vec<Control> {
        let Some(code) = self.code.clone() else {
            return vec![Control::Refuse(Refusal::PairClosed)];
        };
        let mut p = dent_link::pair::Pairing::start(&code);
        let out = p.outbound.clone();
        let key = p.finish(&hex::decode(spake).unwrap()).unwrap();
        PAIR_KEY.with(|k| *k.borrow_mut() = Some(key));
        vec![Control::Pair {
            spake: hex::encode(out),
        }]
    }

    fn daemon_pair_step2(&mut self, t: &mut Transport, mac: String) -> Vec<Control> {
        let key = PAIR_KEY.with(|k| k.borrow_mut().take()).unwrap();
        let ok = dent_link::pair::verify(
            &key,
            &t.hash,
            dent_link::pair::Role::Client,
            &hex::decode(mac).unwrap(),
        );
        if !ok {
            return vec![Control::Refuse(Refusal::PairWrong)];
        }
        self.trusted.push(t.remote.clone());
        self.code = None;
        let mine = dent_link::pair::confirm(&key, &t.hash, dent_link::pair::Role::Daemon);
        vec![
            Control::PairConfirm {
                mac: hex::encode(mine),
            },
            Control::Paired {
                access: Access::Full,
            },
        ]
    }

    /// Sends a control message from the daemon on every live link.
    fn daemon_says(&mut self, c: Control) {
        let conns: Vec<u64> = self.conns.keys().copied().collect();
        for conn in conns {
            if let Some((addr, Peer::Live(mut t))) = self.conns.remove(&conn) {
                let f = t.seal_control(&c).unwrap();
                self.conns.insert(conn, (addr, Peer::Live(t)));
                self.m.received(conn, &f, self.now);
            }
        }
        self.pump();
    }

    /// Advances time in small steps, ticking and pumping as a host would.
    fn run_for(&mut self, ms: u64) {
        let end = self.now + ms;
        while self.now < end {
            let next = self
                .m
                .next_deadline()
                .unwrap_or(u64::MAX)
                .min(end)
                .max(self.now + 1);
            self.now = next.min(self.now + 250);
            self.m.tick(self.now);
            self.pump();
        }
    }

    fn states(&self) -> Vec<State> {
        self.notes
            .iter()
            .filter_map(|n| match n {
                Note::State(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    fn ups(&self) -> Vec<(u64, String)> {
        self.notes
            .iter()
            .filter_map(|n| match n {
                Note::Up {
                    generation, addr, ..
                } => Some((*generation, addr.clone())),
                _ => None,
            })
            .collect()
    }
}

thread_local! {
    static PAIR_KEY: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
}

/// A placeholder until message 1 arrives (a responder needs it to start).
fn dummy_responder() -> Responder {
    let k = Keys::generate().unwrap();
    let (ini, m1) = dent_link::Initiator::start(&k).unwrap();
    let _ = ini;
    let hello = Hello::default();
    Responder::accept(&Keys::generate().unwrap(), &m1, &hello)
        .unwrap()
        .0
}

/// Message 1 of XX is a bare ephemeral key: 32 bytes.
fn is_msg1(data: &[u8]) -> bool {
    data.len() == 32
}

#[test]
fn connects_over_the_first_route_that_trusts_it() {
    let mut s = Sim::new(&[("lan:7480", Addr::Silent), ("ts:7480", Addr::Up)]);
    s.trust_device();
    s.m.connect(0);
    s.pump();
    s.run_for(1_000);
    assert_eq!(s.m.state(), &State::Online);
    assert_eq!(s.ups(), vec![(1, "ts:7480".to_string())]);
    // The dead LAN address cost a quarter second, not its 3 s limit.
    assert!(s.dials.contains(&"lan:7480".to_string()));
}

#[test]
fn an_untrusted_device_pairs_with_the_code() {
    let mut s = Sim::new(&[("lan:7480", Addr::Up)]);
    s.m.known = None;
    s.m.connect(0);
    s.pump();
    s.run_for(500);
    assert_eq!(
        s.m.state(),
        &State::NeedsCode {
            pairing_open: false
        }
    );
    // No code showing there.
    s.m.pair("123456", s.now);
    s.pump();
    assert!(s.notes.contains(&Note::PairFailed(Refusal::PairClosed)));
    // A code shows; a wrong guess, then the right one.
    s.code = Some("424242".into());
    s.m.pair("111111", s.now);
    s.pump();
    assert!(s.notes.contains(&Note::PairFailed(Refusal::PairWrong)));
    s.code = Some("424242".into());
    s.m.pair("424242", s.now);
    s.pump();
    assert_eq!(s.m.state(), &State::Online);
    assert!(
        s.notes
            .iter()
            .any(|n| matches!(n, Note::Paired { key } if *key == s.daemon.public))
    );
    assert_eq!(s.m.known_key(), Some(s.daemon.public.as_slice()));
}

#[test]
fn silence_reads_slow_then_a_replacement_takes_over() {
    let mut s = Sim::new(&[("lan:7480", Addr::Up)]);
    s.trust_device();
    s.m.connect(0);
    s.pump();
    s.run_for(500);
    // The daemon stops answering on the live link, but new connections work
    // (the old socket is stuck behind TCP's backoff).
    s.responsive = false;
    s.run_for(3_200);
    assert_eq!(s.m.state(), &State::Slow);
    s.responsive = true;
    s.run_for(2_500);
    assert_eq!(s.m.state(), &State::Online);
    assert_eq!(s.ups().len(), 2, "moved to a fresh link: {:?}", s.ups());
    assert!(
        !s.states().contains(&State::Reconnecting),
        "never dropped: {:?}",
        s.states()
    );
}

#[test]
fn long_silence_tears_down_and_reconnects() {
    let mut s = Sim::new(&[("lan:7480", Addr::Up)]);
    s.trust_device();
    s.m.connect(0);
    s.pump();
    s.run_for(500);
    // The whole machine goes quiet: nothing answers at all.
    s.addrs.insert("lan:7480".into(), Addr::Silent);
    s.run_for(15_500);
    assert!(
        s.states().contains(&State::Reconnecting),
        "{:?}",
        s.states()
    );
    s.addrs.insert("lan:7480".into(), Addr::Up);
    s.run_for(10_000);
    assert_eq!(s.m.state(), &State::Online);
}

#[test]
fn backoff_climbs_with_jitter_and_a_signal_skips_it() {
    let mut s = Sim::new(&[("lan:7480", Addr::Refused)]);
    s.trust_device();
    s.m.connect(0);
    s.pump();
    // Count dials over ten minutes of refusals.
    s.run_for(600_000);
    let n = s.dials.len();
    assert!(n > 10, "keeps trying: {n}");
    assert!(n < 40, "capped and jittered, not hammering: {n}");
    // A network change dials at once.
    let before = s.dials.len();
    s.addrs.insert("lan:7480".into(), Addr::Up);
    s.m.signal(Signal::NetworkChanged, s.now);
    s.pump();
    assert!(s.dials.len() > before);
    assert_eq!(s.m.state(), &State::Online);
}

#[test]
fn an_announced_restart_redials_quickly() {
    let mut s = Sim::new(&[("lan:7480", Addr::Up)]);
    s.trust_device();
    s.m.connect(0);
    s.pump();
    s.run_for(500);
    s.addrs.insert("lan:7480".into(), Addr::Refused);
    s.daemon_says(Control::Goodbye(Goodbye::Restart {
        expected_down_ms: 3000,
    }));
    assert_eq!(
        s.m.state(),
        &State::Away(Goodbye::Restart {
            expected_down_ms: 3000
        })
    );
    let before = s.dials.len();
    s.run_for(4_000);
    let tries = s.dials.len() - before;
    assert!(tries >= 4, "redials about every 0.75 s: {tries}");
    s.addrs.insert("lan:7480".into(), Addr::Up);
    s.run_for(1_000);
    assert_eq!(s.m.state(), &State::Online);
}

#[test]
fn removal_is_final_even_after_signals() {
    let mut s = Sim::new(&[("lan:7480", Addr::Up)]);
    s.trust_device();
    s.m.connect(0);
    s.pump();
    s.run_for(500);
    s.daemon_says(Control::Refuse(Refusal::Removed));
    assert_eq!(s.m.state(), &State::Removed);
    assert_eq!(s.m.known_key(), None);
    let before = s.dials.len();
    s.m.signal(Signal::Retry, s.now);
    s.run_for(120_000);
    assert_eq!(s.dials.len(), before, "no more dialling once removed");
}

#[test]
fn incompatible_waits_for_a_signal_not_a_timer() {
    let mut s = Sim::new(&[("lan:7480", Addr::Up)]);
    s.trust_device();
    s.refuse = Some(Refusal::Incompatible {
        daemon: (1, 1),
        client: (2, 3),
    });
    s.m.connect(0);
    s.pump();
    s.run_for(1_000);
    assert!(matches!(s.m.state(), State::Incompatible { .. }));
    let before = s.dials.len();
    s.run_for(600_000);
    assert_eq!(s.dials.len(), before, "blocked: no timer retries");
    s.refuse = None;
    s.m.signal(Signal::Seen, s.now);
    s.pump();
    s.run_for(500);
    assert_eq!(s.m.state(), &State::Online);
}

#[test]
fn a_different_machine_at_a_known_address_is_flagged() {
    let mut s = Sim::new(&[("old:7480", Addr::Imposter), ("ts:7480", Addr::Up)]);
    s.trust_device();
    s.m.connect(0);
    s.pump();
    s.run_for(1_000);
    assert!(s.notes.contains(&Note::DifferentMachine {
        addr: "old:7480".into()
    }));
    assert_eq!(s.ups(), vec![(1, "ts:7480".to_string())]);
}

#[test]
fn comes_home_to_the_better_route() {
    let mut s = Sim::new(&[("lan:7480", Addr::Refused), ("ts:7480", Addr::Up)]);
    s.trust_device();
    s.m.connect(0);
    s.pump();
    s.run_for(1_000);
    assert_eq!(s.ups().last().map(|u| u.1.clone()), Some("ts:7480".into()));
    s.addrs.insert("lan:7480".into(), Addr::Up);
    s.run_for(61_000);
    assert_eq!(
        s.ups().last().map(|u| u.1.clone()),
        Some("lan:7480".into()),
        "{:?}",
        s.ups()
    );
    assert_eq!(s.m.state(), &State::Online);
    assert!(!s.states().contains(&State::Reconnecting));
}

#[test]
fn data_goes_out_only_on_a_live_link() {
    let mut s = Sim::new(&[("lan:7480", Addr::Refused)]);
    s.trust_device();
    s.m.connect(0);
    s.pump();
    assert!(!s.m.send(b"{}", s.now), "nothing live: the caller keeps it");
    s.addrs.insert("lan:7480".into(), Addr::Up);
    s.m.signal(Signal::Retry, s.now);
    s.pump();
    assert!(s.m.send(br#"{"req":1,"op":"hello","proto":1,"client":"t"}"#, s.now));
}

/// Brings `s` online over its routes.
fn online(s: &mut Sim) {
    s.m.connect(s.now);
    s.pump();
    s.run_for(500);
    assert_eq!(s.m.state(), &State::Online);
}

#[test]
fn a_busy_sender_still_pings() {
    // The daemon only answers pings. A client sending data every half
    // second must still ping, or the link would read as silent.
    let mut s = Sim::new(&[("lan:7480", Addr::Up)]);
    s.trust_device();
    online(&mut s);
    for _ in 0..40 {
        assert!(s.m.send(b"{}", s.now));
        s.pump();
        s.run_for(500);
    }
    assert!(!s.states().contains(&State::Slow), "{:?}", s.states());
    assert_eq!(s.ups().len(), 1);
}

#[test]
fn silence_never_asks_the_host_to_wake_in_a_loop() {
    let mut s = Sim::new(&[("lan:7480", Addr::Up)]);
    s.trust_device();
    online(&mut s);
    s.responsive = false;
    s.hang = true;
    for _ in 0..60 {
        s.run_for(250);
        if let Some(d) = s.m.next_deadline() {
            assert!(d > s.now, "deadline {d} not after {}", s.now);
        }
    }
}

#[test]
fn the_old_link_answering_first_cancels_the_replacement() {
    let mut s = Sim::new(&[("lan:7480", Addr::Up)]);
    s.trust_device();
    online(&mut s);
    s.responsive = false;
    s.hang = true;
    s.run_for(5_500);
    assert!(matches!(&s.m.phase, Phase::Open(o) if o.replace.is_some()));
    // The old socket's backlog clears and a frame gets through.
    s.daemon_says(Control::Ping { t: 1 });
    assert!(matches!(&s.m.phase, Phase::Open(o) if o.replace.is_none()));
    assert_eq!(s.m.state(), &State::Online);
    assert_eq!(s.ups().len(), 1);
}

#[test]
fn while_silent_a_new_dial_starts_every_second() {
    let mut s = Sim::new(&[("lan:7480", Addr::Up)]);
    s.trust_device();
    online(&mut s);
    s.responsive = false;
    s.hang = true;
    let before = s.dials.len();
    // From the replacement at 5 s to the teardown at 15 s.
    s.run_for(14_000);
    let n = s.dials.len() - before;
    assert!(n >= 9, "dials while silent: {n}");
}

#[test]
fn the_replacement_carries_on_when_the_old_link_closes() {
    let mut s = Sim::new(&[("lan:7480", Addr::Up)]);
    s.trust_device();
    online(&mut s);
    s.responsive = false;
    s.hang = true;
    s.run_for(5_500);
    let Phase::Open(o) = &s.m.phase else {
        panic!("not open")
    };
    let (old, racing) = (o.conn, o.replace.as_ref().unwrap().attempts[0].conn);
    s.m.closed(old, "reset", s.now);
    s.pump();
    assert_eq!(s.m.state(), &State::Reconnecting);
    assert!(
        matches!(&s.m.phase, Phase::Racing(r) if r.attempts.iter().any(|a| a.conn == racing)),
        "the dial in flight was kept"
    );
}
