//! Requests in flight to machines, and which have waited too long
//! ([decision 0078](../../../docs/decisions/0078-requests-that-fail-well.md)).
//!
//! Every request a client sends is noted with its machine and how long that
//! machine may take ([`Request::patience`]). Time counts only while the
//! machine is connected: a request waiting for a machine that's asleep isn't
//! late, it hasn't gone. The first answer settles it. One that runs out of
//! patience is handed back once, so the client can end the wait in words:
//! a read "didn't answer", a change "may or may not have happened".
//!
//! A request for a machine that's gone (removed meanwhile) ends at once.
//!
//! No I/O and no clock of its own: the host says what time it is (a
//! monotonic one) and how each machine stands.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use dent_protocol::{Request, Resend};

/// How long a request that's been heard working on may go quiet: its
/// machine says so every few seconds while it works (decision 0078).
pub const STALL: Duration = Duration::from_secs(60);

/// How a request's machine stands, as the host sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// Connected: the request's time counts.
    Up,
    /// Asleep, out of reach, being updated: it waits, and its time doesn't.
    Away,
    /// Not one of this device's machines any more.
    Gone,
}

/// A request that waited too long, or whose machine is gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overdue {
    pub req: u64,
    pub machine: String,
    /// It would change something: it may or may not have happened.
    pub changes: bool,
    pub patience: Duration,
    /// Its machine is gone, rather than slow.
    pub gone: bool,
    /// It was being worked on, then went quiet: stalled, not slow.
    pub stalled: bool,
    /// It was being worked on all along, and still hadn't finished.
    pub unfinished: bool,
    /// The person did it (a stop, a commit), as opposed to the client
    /// keeping things up to date: only what they did is worth a word when
    /// nothing waits on it.
    pub once: bool,
}

impl Overdue {
    /// How the wait ends, in words: who didn't answer, and what that means.
    pub fn words(&self, machine_name: &str) -> String {
        if self.gone {
            return format!("{machine_name} isn't one of your machines any more");
        }
        if self.unfinished {
            return if self.changes {
                format!(
                    "{machine_name} has been at it for {} minutes without finishing, so it may or may not have happened; look before trying again",
                    self.patience.as_secs() / 60
                )
            } else {
                format!(
                    "{machine_name} has been at it for {} minutes without finishing",
                    self.patience.as_secs() / 60
                )
            };
        }
        if self.stalled {
            return if self.changes {
                format!(
                    "{machine_name} stopped answering partway through, so that may or may not have happened; look before trying again"
                )
            } else {
                format!("{machine_name} stopped answering partway through")
            };
        }
        let secs = self.patience.as_secs();
        let when = if secs >= 120 {
            format!("{} minutes", secs / 60)
        } else {
            format!("{secs} seconds")
        };
        if self.changes {
            format!(
                "{machine_name} didn't answer in {when}, so that may or may not have happened; look before trying again"
            )
        } else {
            format!("{machine_name} didn't answer in {when}")
        }
    }
}

#[derive(Debug)]
struct Asked {
    machine: String,
    changes: bool,
    once: bool,
    patience: Duration,
    /// Time spent waiting while the machine was connected, since it was
    /// asked: never more than its patience, however busy it says it is.
    total: Duration,
    /// The same since its machine last said it's at work on it.
    waited: Duration,
    /// When it was last looked at, while connected.
    since: Option<Instant>,
    /// Its machine has said it's at work on it.
    heard: bool,
    /// It said so since the last look.
    fresh: bool,
}

/// Every request still waiting for its first answer.
#[derive(Debug, Default)]
pub struct Inflight {
    asked: BTreeMap<u64, Asked>,
}

impl Inflight {
    /// Request `req` went (or is waiting to go) to `machine`.
    pub fn asked(&mut self, req: u64, machine: &str, r: &Request) {
        self.asked.insert(
            req,
            Asked {
                machine: machine.to_string(),
                changes: r.resend() != Resend::Read,
                once: r.resend() == Resend::Once,
                patience: r.patience(),
                total: Duration::ZERO,
                waited: Duration::ZERO,
                since: None,
                heard: false,
                fresh: false,
            },
        );
    }

    /// Its machine says it's still at work on `req`: the clock starts
    /// again, and from now on a minute's quiet is a stall ([`STALL`]).
    pub fn alive(&mut self, req: u64) {
        if let Some(a) = self.asked.get_mut(&req) {
            a.waited = Duration::ZERO;
            // Quiet counts from here: what came before the next look isn't
            // quiet. (Its patience in all still counts it.)
            a.fresh = true;
            a.heard = true;
        }
    }

    /// An answer to `req` came: it's settled, late or not.
    pub fn answered(&mut self, req: u64) {
        self.asked.remove(&req);
    }

    /// Requests still waiting.
    pub fn len(&self) -> usize {
        self.asked.len()
    }

    pub fn is_empty(&self) -> bool {
        self.asked.is_empty()
    }

    /// Move the clocks to `now`, counting only the time each request's
    /// machine was connected, and take out what's waited past its patience.
    pub fn tick(&mut self, now: Instant, link: impl Fn(&str) -> Link) -> Vec<Overdue> {
        let mut over = Vec::new();
        for (req, a) in &mut self.asked {
            let state = link(&a.machine);
            match state {
                Link::Up => {
                    if let Some(since) = a.since {
                        let more = now.saturating_duration_since(since);
                        if !std::mem::take(&mut a.fresh) {
                            a.waited += more;
                        }
                        a.total += more;
                    }
                    a.since = Some(now);
                }
                // Away: the clock stops; it starts again from when it's back.
                Link::Away | Link::Gone => a.since = None,
            }
            // Heard at work, a minute's quiet is a stall; and however busy
            // it says it is, never past its patience in all.
            let stalled = a.heard && a.waited >= STALL;
            let unfinished = a.total >= a.patience;
            if state == Link::Gone || stalled || unfinished {
                over.push(Overdue {
                    req: *req,
                    machine: a.machine.clone(),
                    changes: a.changes,
                    patience: a.patience,
                    gone: state == Link::Gone,
                    stalled: stalled && !unfinished && state != Link::Gone,
                    unfinished: a.heard && unfinished && state != Link::Gone,
                    once: a.once,
                });
            }
        }
        for o in &over {
            self.asked.remove(&o.req);
        }
        over
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read() -> Request {
        Request::Projects
    }

    fn change() -> Request {
        Request::Interrupt {
            session: "s".into(),
            by: None,
        }
    }

    #[test]
    fn an_answer_settles_it_and_nothing_is_late() {
        let mut f = Inflight::default();
        let t = Instant::now();
        f.asked(3, "m", &read());
        f.tick(t, |_| Link::Up);
        f.answered(3);
        assert!(
            f.tick(t + Duration::from_secs(600), |_| Link::Up)
                .is_empty()
        );
        assert!(f.is_empty());
    }

    #[test]
    fn a_read_past_its_patience_is_handed_back_once() {
        let mut f = Inflight::default();
        let t = Instant::now();
        f.asked(3, "m", &read());
        f.tick(t, |_| Link::Up);
        assert!(f.tick(t + Duration::from_secs(29), |_| Link::Up).is_empty());
        let over = f.tick(t + Duration::from_secs(31), |_| Link::Up);
        assert_eq!(over.len(), 1);
        assert!(!over[0].changes);
        assert_eq!(over[0].words("mini"), "mini didn't answer in 30 seconds");
        assert!(
            f.tick(t + Duration::from_secs(90), |_| Link::Up).is_empty(),
            "once"
        );
    }

    #[test]
    fn a_change_past_its_patience_may_or_may_not_have_happened() {
        let mut f = Inflight::default();
        let t = Instant::now();
        f.asked(4, "m", &change());
        f.tick(t, |_| Link::Up);
        let over = f.tick(t + Duration::from_secs(31), |_| Link::Up);
        assert!(over[0].changes);
        assert!(
            over[0]
                .words("mini")
                .contains("may or may not have happened")
        );
    }

    /// Asleep or out of reach, the machine hasn't had it: the clock stops,
    /// and starts again from when it's back, not from when it was asked.
    #[test]
    fn time_away_doesnt_count() {
        let mut f = Inflight::default();
        let t = Instant::now();
        f.asked(3, "m", &read());
        f.tick(t, |_| Link::Up);
        f.tick(t + Duration::from_secs(20), |_| Link::Up);
        // Away for an hour.
        f.tick(t + Duration::from_secs(21), |_| Link::Away);
        assert!(
            f.tick(t + Duration::from_secs(3600), |_| Link::Away)
                .is_empty()
        );
        // Back: 20 s waited so far, 10 more to go.
        f.tick(t + Duration::from_secs(3601), |_| Link::Up);
        assert!(
            f.tick(t + Duration::from_secs(3610), |_| Link::Up)
                .is_empty()
        );
        assert_eq!(f.tick(t + Duration::from_secs(3612), |_| Link::Up).len(), 1);
    }

    #[test]
    fn each_machine_keeps_its_own_clock() {
        let mut f = Inflight::default();
        let t = Instant::now();
        f.asked(3, "a", &read());
        f.asked(4, "b", &read());
        f.tick(t, |_| Link::Up);
        let over = f.tick(t + Duration::from_secs(31), |m| {
            if m == "a" { Link::Up } else { Link::Away }
        });
        assert_eq!(over.iter().map(|o| o.req).collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn a_request_for_a_machine_thats_gone_ends_at_once() {
        let mut f = Inflight::default();
        let t = Instant::now();
        f.asked(3, "old", &change());
        let over = f.tick(t, |_| Link::Gone);
        assert!(over[0].gone);
        assert_eq!(
            over[0].words("mini"),
            "mini isn't one of your machines any more"
        );
        assert!(f.is_empty());
    }

    /// A long job that keeps saying it's at work never runs out, however
    /// long it takes; one that goes quiet for a minute is stalled.
    #[test]
    fn a_job_at_work_runs_on_and_one_gone_quiet_is_stalled() {
        let mut f = Inflight::default();
        let t = Instant::now();
        f.asked(5, "m", &Request::CloneProject { url: "u".into() });
        f.tick(t, |_| Link::Up);
        // Twenty minutes of work, heard from every 5 s.
        for i in 1..=240 {
            f.alive(5);
            assert!(
                f.tick(t + Duration::from_secs(i * 5), |_| Link::Up)
                    .is_empty()
            );
        }
        // Then quiet.
        let quiet = t + Duration::from_secs(1200);
        assert!(
            f.tick(quiet + Duration::from_secs(59), |_| Link::Up)
                .is_empty()
        );
        let over = f.tick(quiet + Duration::from_secs(61), |_| Link::Up);
        assert!(over[0].stalled);
        assert!(
            over[0]
                .words("mini")
                .starts_with("mini stopped answering partway through")
        );
    }

    /// One that says it's at work for ever still ends: at its patience in
    /// all, and says it never finished.
    #[test]
    fn busy_for_ever_still_ends() {
        let mut f = Inflight::default();
        let t = Instant::now();
        f.asked(5, "m", &Request::CloneProject { url: "u".into() });
        f.tick(t, |_| Link::Up);
        let mut over = Vec::new();
        for i in 1..=400 {
            f.alive(5);
            over = f.tick(t + Duration::from_secs(i * 5), |_| Link::Up);
            if !over.is_empty() {
                assert_eq!(i * 5, 1800, "ends at 30 minutes");
                break;
            }
        }
        assert!(over[0].unfinished && !over[0].stalled);
        assert!(
            over[0]
                .words("mini")
                .contains("at it for 30 minutes without finishing")
        );
    }

    #[test]
    fn long_jobs_get_long_patience() {
        let mut f = Inflight::default();
        let t = Instant::now();
        f.asked(
            5,
            "m",
            &Request::CloneProject {
                url: "https://example.com/a.git".into(),
            },
        );
        f.tick(t, |_| Link::Up);
        assert!(
            f.tick(t + Duration::from_secs(1200), |_| Link::Up)
                .is_empty()
        );
        let over = f.tick(t + Duration::from_secs(1801), |_| Link::Up);
        assert!(over[0].words("mini").contains("30 minutes"));
    }
}
