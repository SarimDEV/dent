//! Carrying a session from one machine to another ([decision 0064]), as a
//! client both machines trust does it: the machines never talk to each
//! other. No I/O: the host asks each machine what [`Carrier`] says to,
//! hands back each answer, and waits when told to. Every step asks where
//! things stand, so a new carrier with the same move id picks up after any
//! interruption, on either machine, the link or here.
//!
//! [decision 0064]: ../../../docs/decisions/0064-moving-sessions.md

use dent_protocol::{MoveManifest, Reply, Request, SessionId};

/// Which machine to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The machine the session leaves.
    From,
    /// The machine it goes to.
    To,
}

/// What to do next.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Ask this machine this, and hand its answer to [`Carrier::answer`].
    Ask(Side, Request),
    /// Ask the same again after this many milliseconds: the turn there is
    /// still running.
    Wait(u64, Side, Request),
    /// It's over.
    Done(Result<SessionId, Ended>),
}

/// Why a move ended without the session arriving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The machine it goes to doesn't have the session's repository. Said
    /// before anything was packed or fenced: nothing to undo.
    NoProject,
    /// The other machine looked at what arrived and didn't take it; it
    /// works where it was again.
    NotAccepted(String),
    /// A machine said no, didn't answer or went away. The same move id
    /// picks up from here.
    Stopped { side: Side, why: String },
}

/// How far along it is, for a progress line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Asking each machine what it has.
    Asking,
    /// Waiting for the turn (and work left running) to end.
    Waiting,
    /// Sending what was packed.
    Sending { sent: u64, total: u64 },
    /// The other machine is making the session from what arrived.
    Arriving,
    Done,
}

/// Bytes read and written at a time.
const CHUNK: u32 = 512 * 1024;
/// How often to ask again while the turn runs.
const WAIT_MS: u64 = 2000;

#[derive(Debug)]
enum At {
    Remotes,
    Have,
    Pack,
    In,
    Read,
    Write,
    Accept,
    Ended(Result<SessionId, String>),
    Done,
}

pub struct Carrier {
    session: SessionId,
    id: String,
    to_name: String,
    at: At,
    in_repo: bool,
    tips: Vec<String>,
    manifest: Option<Box<MoveManifest>>,
    /// The part being sent, and how much of it is there.
    part: usize,
    offset: u64,
    phase: Phase,
}

impl Carrier {
    /// Move `session` (as its own machine names it) as move `id` to the
    /// machine this device calls `to_name`, which is what the chat on the
    /// machine it leaves says it went to.
    pub fn new(session: &str, id: &str, to_name: &str) -> Self {
        Self {
            session: session.to_string(),
            id: id.to_string(),
            to_name: to_name.to_string(),
            at: At::Remotes,
            in_repo: false,
            tips: Vec::new(),
            manifest: None,
            part: 0,
            offset: 0,
            phase: Phase::Asking,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The first step.
    pub fn start(&mut self) -> Step {
        self.at = At::Remotes;
        Step::Ask(Side::From, self.pack(None))
    }

    fn pack(&self, have: Option<Vec<String>>) -> Request {
        Request::MovePack {
            session: self.session.clone(),
            id: self.id.clone(),
            to: self.to_name.clone(),
            have,
        }
    }

    fn stop(&mut self, side: Side, why: impl Into<String>) -> Step {
        self.at = At::Done;
        self.phase = Phase::Done;
        Step::Done(Err(Ended::Stopped {
            side,
            why: why.into(),
        }))
    }

    /// The answer from `side` to the last step: a reply, or what went
    /// wrong (an error it said, no answer, the link gone).
    pub fn answer(&mut self, side: Side, answer: Result<Reply, String>) -> Step {
        let reply = match answer {
            Ok(r) => r,
            Err(why) => return self.stop(side, why),
        };
        match (std::mem::replace(&mut self.at, At::Done), reply) {
            (At::Remotes, Reply::MoveRemotes { remotes }) => {
                self.in_repo = !remotes.is_empty();
                self.at = At::Have;
                Step::Ask(Side::To, Request::MoveHave { remotes })
            }
            (At::Have, Reply::MoveHave { found, tips }) => {
                if self.in_repo && !found {
                    self.phase = Phase::Done;
                    return Step::Done(Err(Ended::NoProject));
                }
                self.tips = tips;
                self.at = At::Pack;
                Step::Ask(Side::From, self.pack(Some(self.tips.clone())))
            }
            (At::Pack, Reply::MoveWaiting) => {
                self.phase = Phase::Waiting;
                self.at = At::Pack;
                Step::Wait(WAIT_MS, Side::From, self.pack(Some(self.tips.clone())))
            }
            (At::Pack, Reply::MovePacked { manifest }) => {
                let total = manifest.parts.iter().map(|p| p.bytes).sum();
                self.phase = Phase::Sending { sent: 0, total };
                self.manifest = Some(manifest.clone());
                self.at = At::In;
                Step::Ask(Side::To, Request::MoveIn { manifest })
            }
            // Already taken there: a carrier before this one got that far.
            (At::In, Reply::Created { session }) => self.ended(Ok(session)),
            (At::In, Reply::MoveParts { have }) => {
                self.part = 0;
                self.offset = 0;
                self.have(&have);
                self.next_read()
            }
            (At::Read, Reply::MoveBytes {
                offset, bytes_b64, ..
            }) => {
                let Some(part) = self.part_name() else {
                    return self.stop(Side::From, "sent a part that isn't in the move");
                };
                self.at = At::Write;
                Step::Ask(
                    Side::To,
                    Request::MoveWrite {
                        id: self.id.clone(),
                        part,
                        offset,
                        bytes_b64,
                    },
                )
            }
            (At::Write, Reply::MoveWrote { have }) => {
                if have <= self.offset {
                    let part = self.part_name().unwrap_or_default();
                    let at = self.offset;
                    return self.stop(Side::To, format!("stopped taking {part} at {at} bytes"));
                }
                self.offset = have;
                self.count();
                self.next_read()
            }
            (At::Accept, Reply::Created { session }) => self.ended(Ok(session)),
            (At::Accept, Reply::MoveNotAccepted { why }) => self.ended(Err(why)),
            (At::Ended(outcome), Reply::Ok) => {
                self.phase = Phase::Done;
                Step::Done(outcome.map_err(Ended::NotAccepted))
            }
            (at, other) => {
                let side = match at {
                    At::Remotes | At::Pack | At::Read | At::Ended(_) => Side::From,
                    _ => Side::To,
                };
                self.stop(side, format!("unexpected answer {other:?}"))
            }
        }
    }

    /// Starting a part, or carrying on with one: how much is there.
    fn have(&mut self, have: &[(String, u64)]) {
        self.offset = self
            .part_name()
            .and_then(|n| have.iter().find(|(p, _)| *p == n).map(|(_, b)| *b))
            .unwrap_or(0);
        self.have_rest = have.to_vec();
        self.count();
    }

    fn part_name(&self) -> Option<String> {
        self.manifest
            .as_ref()?
            .parts
            .get(self.part)
            .map(|p| p.name.clone())
    }

    /// The next read, or accepting once every part is there.
    fn next_read(&mut self) -> Step {
        let parts = self.manifest.as_ref().map(|m| m.parts.clone()).unwrap_or_default();
        while let Some(p) = parts.get(self.part) {
            if self.offset < p.bytes {
                self.at = At::Read;
                return Step::Ask(
                    Side::From,
                    Request::MoveRead {
                        id: self.id.clone(),
                        part: p.name.clone(),
                        offset: self.offset,
                        len: CHUNK,
                    },
                );
            }
            self.part += 1;
            let rest = std::mem::take(&mut self.have_rest);
            self.have(&rest);
        }
        self.phase = Phase::Arriving;
        self.at = At::Accept;
        Step::Ask(
            Side::To,
            Request::MoveAccept {
                id: self.id.clone(),
            },
        )
    }

    /// What's there now: every part before this one, and this one so far.
    fn count(&mut self) {
        let Some(m) = self.manifest.as_ref() else {
            return;
        };
        let total = m.parts.iter().map(|p| p.bytes).sum();
        let before: u64 = m.parts.iter().take(self.part).map(|p| p.bytes).sum();
        self.phase = Phase::Sending {
            sent: (before + self.offset).min(total),
            total,
        };
    }

    /// The machine it left hears how it ended.
    fn ended(&mut self, outcome: Result<SessionId, String>) -> Step {
        let (to_session, not_accepted) = match &outcome {
            Ok(s) => (Some(unqualified(s)), None),
            Err(why) => (None, Some(why.clone())),
        };
        self.phase = Phase::Arriving;
        self.at = At::Ended(outcome.map(|s| unqualified(&s)));
        Step::Ask(
            Side::From,
            Request::MoveEnded {
                session: self.session.clone(),
                id: self.id.clone(),
                to_session,
                not_accepted,
            },
        )
    }
}

/// A session's id as its own machine names it (a client may prefix the
/// machine).
fn unqualified(s: &str) -> String {
    s.rsplit_once('/').map_or(s, |(_, id)| id).to_string()
}

/// A new move's id: letters, digits and dashes, unique enough.
pub fn new_id(now_ms: u64, random: u64) -> String {
    format!("m{now_ms:x}-{:012x}", random & 0xffff_ffff_ffff)
}
