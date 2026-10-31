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
}

impl Overdue {
    /// How the wait ends, in words: who didn't answer, and what that means.
    pub fn words(&self, machine_name: &str) -> String {
        if self.gone {
            return format!("{machine_name} isn't one of your machines any more");
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
    patience: Duration,
    /// Time spent waiting while the machine was connected.
    waited: Duration,
    /// When it was last looked at, while connected.
    since: Option<Instant>,
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
                patience: r.patience(),
                waited: Duration::ZERO,
                since: None,
            },
        );
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
                        a.waited += now.saturating_duration_since(since);
                    }
                    a.since = Some(now);
                }
                // Away: the clock stops; it starts again from when it's back.
                Link::Away | Link::Gone => a.since = None,
            }
            if state == Link::Gone || a.waited >= a.patience {
                over.push(Overdue {
                    req: *req,
                    machine: a.machine.clone(),
                    changes: a.changes,
                    patience: a.patience,
                    gone: state == Link::Gone,
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
            f.tick(t + Duration::from_secs(300), |_| Link::Up)
                .is_empty()
        );
        let over = f.tick(t + Duration::from_secs(601), |_| Link::Up);
        assert!(over[0].words("mini").contains("10 minutes"));
    }
}
