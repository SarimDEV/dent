//! Carrying a session from one machine to another ([decision 0064]), as a
//! client both machines trust does it: the machines never talk to each
//! other. No I/O: the host asks each machine what [`Carrier`] says to,
//! hands back each answer, and waits when told to. Every step asks where
//! things stand, so a new carrier with the same move id picks up after any
//! interruption, on either machine, the link or here.
//!
//! [decision 0064]: ../../../docs/decisions/0064-moving-sessions.md

use dent_protocol::{Bring, GitRemote, MoveManifest, Reply, Request, SessionId};

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
    /// The machine it goes to is cloning the project for it.
    Cloning,
    /// Waiting for the turn (and work left running) to end.
    Waiting,
    /// Sending what was packed.
    Sending {
        sent: u64,
        total: u64,
    },
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
    /// Reading the ignored files the clone takes, on the machine it leaves.
    Files,
    /// The machine it goes to cloning the project.
    Clone,
    /// Calling the move off on the machine it leaves: the other has no
    /// project.
    NoProject,
    Done,
}

/// A project to clone on the machine it goes to, when that has none:
/// from these remotes, with these ignored files read from `dir` on the
/// machine it leaves.
#[derive(Debug, Clone, PartialEq)]
pub struct CloneThere {
    pub dir: String,
    pub paths: Vec<String>,
    pub bring: Bring,
}

pub struct Carrier {
    session: SessionId,
    clone: Option<CloneThere>,
    /// Cloned already this time: not cloned again.
    cloned: bool,
    remotes: Vec<GitRemote>,
    id: String,
    to_name: String,
    to_machine: String,
    at: At,
    in_repo: bool,
    tips: Vec<String>,
    manifest: Option<Box<MoveManifest>>,
    /// How much of each part the other machine said it had.
    arrived: Vec<(String, u64)>,
    /// The part being sent, and how much of it is there.
    part: usize,
    offset: u64,
    phase: Phase,
    chunk: u32,
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
            to_machine: String::new(),
            clone: None,
            cloned: false,
            remotes: Vec::new(),
            at: At::Remotes,
            in_repo: false,
            tips: Vec::new(),
            manifest: None,
            arrived: Vec::new(),
            part: 0,
            offset: 0,
            phase: Phase::Asking,
            chunk: CHUNK,
        }
    }

    /// Reads and writes `bytes` at a time (tests use small ones).
    pub fn with_chunk(mut self, bytes: u32) -> Self {
        self.chunk = bytes.max(1);
        self
    }

    /// The machine it goes to by id too, so every device finds it
    /// whatever it calls it.
    pub fn to_machine(mut self, id: &str) -> Self {
        self.to_machine = id.to_string();
        self
    }

    /// Clone the project there first if it hasn't one.
    pub fn clone_there(mut self, c: CloneThere) -> Self {
        self.clone = Some(c);
        self
    }

    fn bring_project(&self, with_files: bool) -> Request {
        Request::BringProject {
            id: self.id.clone(),
            bring: self
                .clone
                .as_ref()
                .filter(|_| with_files)
                .map(|c| c.bring.clone()),
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
            to_machine: self.to_machine.clone(),
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
        // Called off, or there was nothing to call off: no project there.
        // Refused, it may be there already (an earlier carrier sent it),
        // and the machine it leaves says what to do instead.
        if matches!(self.at, At::NoProject) {
            self.at = At::Done;
            self.phase = Phase::Done;
            return match answer {
                Err(why) if !why.contains("no such move") => Step::Done(Err(Ended::Stopped {
                    side: Side::From,
                    why: format!("{} doesn't have the project now, and {why}", self.to_name),
                })),
                _ => Step::Done(Err(Ended::NoProject)),
            };
        }
        // A clone there forgotten (its daemon restarted) is begun again;
        // one too old to clone says so plainly.
        if matches!(self.at, At::Clone)
            && let Err(why) = &answer
        {
            if why.contains("no such clone") {
                return Step::Ask(Side::To, self.bring_project(true));
            }
            if why.contains("unknown variant") {
                let to = self.to_name.clone();
                return self.stop(
                    Side::To,
                    format!("{to} runs an older dent, which can't clone it: update dent there"),
                );
            }
        }
        let reply = match answer {
            Ok(r) => r,
            Err(why) => return self.stop(side, why),
        };
        match (std::mem::replace(&mut self.at, At::Done), reply) {
            (At::Remotes, Reply::MoveRemotes { remotes }) => {
                self.in_repo = !remotes.is_empty();
                self.remotes = remotes.clone();
                self.at = At::Have;
                Step::Ask(Side::To, Request::MoveHave { remotes })
            }
            // Not there, and to be cloned: its files first, then the clone.
            (At::Have, Reply::MoveHave { found: false, .. })
                if self.in_repo && !self.cloned && self.clone.is_some() =>
            {
                self.phase = Phase::Cloning;
                let c = self.clone.as_ref().expect("checked");
                if c.paths.is_empty() {
                    self.at = At::Clone;
                    Step::Ask(Side::To, self.bring_project(true))
                } else {
                    self.at = At::Files;
                    Step::Ask(
                        Side::From,
                        Request::ReadFiles {
                            dir: c.dir.clone(),
                            paths: c.paths.clone(),
                        },
                    )
                }
            }
            (At::Files, Reply::Carried { files }) => {
                if let Some(c) = self.clone.as_mut() {
                    c.bring.files = files;
                }
                self.at = At::Clone;
                Step::Ask(Side::To, self.bring_project(true))
            }
            (At::Clone, Reply::Cloning) => {
                self.at = At::Clone;
                Step::Wait(WAIT_MS, Side::To, self.bring_project(false))
            }
            // Cloned: asked again, it's found, and the move goes on.
            (At::Clone, Reply::ProjectMade { .. }) => {
                self.cloned = true;
                self.phase = Phase::Asking;
                self.at = At::Have;
                Step::Ask(
                    Side::To,
                    Request::MoveHave {
                        remotes: self.remotes.clone(),
                    },
                )
            }
            (At::Have, Reply::MoveHave { found, tips }) => {
                // Gone there since an earlier carrier began, the session may
                // be packed and waiting: it's called off, which the machine
                // it leaves refuses once anything may have been sent.
                if self.in_repo && !found {
                    self.at = At::NoProject;
                    return Step::Ask(
                        Side::From,
                        Request::MoveStop {
                            session: self.session.clone(),
                            id: self.id.clone(),
                            continue_here: false,
                        },
                    );
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
                self.arrived = have;
                self.begin_part(0);
                self.next_read()
            }
            (
                At::Read,
                Reply::MoveBytes {
                    offset, bytes_b64, ..
                },
            ) => {
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
            // Heard: whatever it answers, the move is over.
            (At::Ended(outcome), _) => {
                self.phase = Phase::Done;
                Step::Done(outcome.map_err(Ended::NotAccepted))
            }
            (at, other) => {
                let side = match at {
                    At::Remotes | At::Pack | At::Read | At::Files | At::Ended(_) => Side::From,
                    _ => Side::To,
                };
                self.stop(side, format!("unexpected answer {other:?}"))
            }
        }
    }

    /// On to part `n`, from as much of it as had arrived.
    fn begin_part(&mut self, n: usize) {
        self.part = n;
        self.offset = self
            .part_name()
            .and_then(|name| {
                self.arrived
                    .iter()
                    .find(|(p, _)| *p == name)
                    .map(|(_, b)| *b)
            })
            .unwrap_or(0);
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
        let parts = self
            .manifest
            .as_ref()
            .map(|m| m.parts.clone())
            .unwrap_or_default();
        while let Some(p) = parts.get(self.part) {
            if self.offset < p.bytes {
                self.at = At::Read;
                return Step::Ask(
                    Side::From,
                    Request::MoveRead {
                        id: self.id.clone(),
                        part: p.name.clone(),
                        offset: self.offset,
                        len: self.chunk,
                    },
                );
            }
            self.begin_part(self.part + 1);
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

#[cfg(test)]
mod tests {
    use super::*;
    use dent_protocol::{GitRemote, MovePart, Permission, b64};
    use std::collections::HashMap;

    /// Two machines as the carrier sees them, answering as the daemons do.
    struct Pair {
        /// The machine it leaves: its parts' bytes, and turns still to wait.
        data: Vec<(String, Vec<u8>)>,
        waits: usize,
        in_repo: bool,
        told: Option<(Option<String>, Option<String>)>,
        packs_with_have: usize,
        reads: usize,
        /// The machine it goes to.
        found: bool,
        got: HashMap<String, Vec<u8>>,
        accept: Result<String, String>,
        accepted: bool,
        /// Fail the nth write (1-based) with this.
        fail_write: Option<(usize, String)>,
        writes: usize,
        /// Cloning there: how many times it says it's still cloning, the
        /// asks it had, whether it forgets one once (a restart), whether
        /// it's too old to clone, and whether the clone is the repository.
        clone_polls: usize,
        clone_asks: Vec<Option<Bring>>,
        forget_clone: bool,
        old: bool,
        clone_finds: bool,
        files_read: Option<Vec<String>>,
    }

    impl Pair {
        fn new(parts: &[(&str, usize)]) -> Self {
            let data = parts
                .iter()
                .map(|(n, len)| (n.to_string(), (0..*len).map(|i| (i % 251) as u8).collect()))
                .collect();
            Self {
                data,
                waits: 0,
                in_repo: true,
                told: None,
                packs_with_have: 0,
                reads: 0,
                found: true,
                got: HashMap::new(),
                accept: Ok("s9".into()),
                accepted: false,
                fail_write: None,
                writes: 0,
                clone_polls: 0,
                clone_asks: Vec::new(),
                forget_clone: false,
                old: false,
                clone_finds: true,
                files_read: None,
            }
        }

        fn manifest(&self) -> Box<MoveManifest> {
            Box::new(MoveManifest {
                id: "m1".into(),
                from_machine: "studio".into(),
                from_name: String::new(),
                from_session: "s1".into(),
                agent: "claude".into(),
                agent_session: "a".into(),
                cwd: "/w".into(),
                title: "t".into(),
                model: None,
                permission: Permission::default(),
                effort: None,
                repo: None,
                files_from: String::new(),
                parts: self
                    .data
                    .iter()
                    .map(|(n, b)| MovePart {
                        name: n.clone(),
                        bytes: b.len() as u64,
                        sha256: String::new(),
                    })
                    .collect(),
            })
        }

        fn ask(&mut self, side: Side, req: Request) -> Result<Reply, String> {
            match (side, req) {
                (Side::From, Request::MovePack { have: None, .. }) => Ok(Reply::MoveRemotes {
                    remotes: if self.in_repo {
                        vec![GitRemote {
                            name: "origin".into(),
                            url: "git@h:o/r".into(),
                        }]
                    } else {
                        vec![]
                    },
                }),
                (Side::To, Request::MoveHave { .. }) => Ok(Reply::MoveHave {
                    found: self.found,
                    tips: vec!["abc".into()],
                }),
                (Side::From, Request::MovePack { have: Some(_), .. }) => {
                    self.packs_with_have += 1;
                    if self.waits > 0 {
                        self.waits -= 1;
                        return Ok(Reply::MoveWaiting);
                    }
                    Ok(Reply::MovePacked {
                        manifest: self.manifest(),
                    })
                }
                (Side::To, Request::MoveIn { manifest }) => {
                    if self.accepted {
                        return Ok(Reply::Created {
                            session: "s9".into(),
                        });
                    }
                    Ok(Reply::MoveParts {
                        have: manifest
                            .parts
                            .iter()
                            .map(|p| {
                                (
                                    p.name.clone(),
                                    self.got.get(&p.name).map_or(0, |b| b.len() as u64),
                                )
                            })
                            .collect(),
                    })
                }
                (
                    Side::From,
                    Request::MoveRead {
                        part, offset, len, ..
                    },
                ) => {
                    self.reads += 1;
                    let all = &self.data.iter().find(|(n, _)| *n == part).unwrap().1;
                    let end = (offset as usize + len as usize).min(all.len());
                    Ok(Reply::MoveBytes {
                        offset,
                        total: all.len() as u64,
                        bytes_b64: b64::encode(&all[offset as usize..end]),
                    })
                }
                (
                    Side::To,
                    Request::MoveWrite {
                        part,
                        offset,
                        bytes_b64,
                        ..
                    },
                ) => {
                    self.writes += 1;
                    if let Some((n, why)) = &self.fail_write
                        && *n == self.writes
                    {
                        return Err(why.clone());
                    }
                    let have = self.got.entry(part).or_default();
                    assert_eq!(have.len() as u64, offset, "written where it stands");
                    have.extend(b64::decode(&bytes_b64).unwrap());
                    Ok(Reply::MoveWrote {
                        have: have.len() as u64,
                    })
                }
                (Side::To, Request::MoveAccept { .. }) => {
                    self.accepted = self.accept.is_ok();
                    Ok(match &self.accept {
                        Ok(s) => Reply::Created {
                            session: format!("dent-linux/{s}"),
                        },
                        Err(why) => Reply::MoveNotAccepted { why: why.clone() },
                    })
                }
                (
                    Side::From,
                    Request::MoveEnded {
                        to_session,
                        not_accepted,
                        ..
                    },
                ) => {
                    self.told = Some((to_session, not_accepted));
                    // Nothing packed: the machine it leaves has no such move.
                    if self.packs_with_have == 0 {
                        return Err("no such move here".into());
                    }
                    Ok(Reply::Ok)
                }
                (Side::From, Request::MoveStop { .. }) => {
                    self.told = Some((None, Some("called off".into())));
                    if self.packs_with_have == 0 {
                        return Err("no such move here".into());
                    }
                    if self.reads > 0 {
                        return Err("the other machine may have it already; keep it here with Continue here, or retry the move".into());
                    }
                    Ok(Reply::Ok)
                }
                (Side::From, Request::ReadFiles { paths, .. }) => {
                    self.files_read = Some(paths.clone());
                    Ok(Reply::Carried {
                        files: paths
                            .iter()
                            .map(|p| dent_protocol::Carried {
                                name: p.clone(),
                                bytes_b64: b64::encode(b"KEY=1"),
                            })
                            .collect(),
                    })
                }
                (Side::To, Request::BringProject { bring, .. }) => {
                    if self.old {
                        return Err("unknown variant `bring_project`".into());
                    }
                    self.clone_asks.push(bring.clone());
                    if bring.is_none() && self.forget_clone {
                        self.forget_clone = false;
                        return Err("no such clone here".into());
                    }
                    if self.clone_polls > 0 {
                        self.clone_polls -= 1;
                        return Ok(Reply::Cloning);
                    }
                    self.found = self.clone_finds;
                    Ok(Reply::ProjectMade {
                        path: "/p/app".into(),
                        warning: None,
                    })
                }
                (side, other) => panic!("{side:?} asked {other:?}"),
            }
        }

        /// Runs a carrier to its end; the waits it was told to make.
        fn run(&mut self, c: &mut Carrier) -> (Result<SessionId, Ended>, usize) {
            let mut step = c.start();
            let mut waits = 0;
            for _ in 0..10_000 {
                match step {
                    Step::Ask(side, req) => {
                        let a = self.ask(side, req);
                        step = c.answer(side, a);
                    }
                    Step::Wait(_, side, req) => {
                        waits += 1;
                        let a = self.ask(side, req);
                        step = c.answer(side, a);
                    }
                    Step::Done(r) => return (r, waits),
                }
            }
            panic!("never ended");
        }
    }

    fn clone(paths: &[&str]) -> CloneThere {
        CloneThere {
            dir: "/w".into(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            bring: Bring {
                name: "app".into(),
                remotes: vec![GitRemote {
                    name: "origin".into(),
                    url: "git@h:o/r".into(),
                }],
                branch: None,
                files: Vec::new(),
            },
        }
    }

    /// Not there: the ticked files are read where it leaves, the project
    /// is cloned there with them (asked once with them, then only how
    /// it's going), and the move goes on as usual.
    #[test]
    fn a_project_cloned_there_first_then_it_moves() {
        let mut p = Pair::new(&[("events.jsonl", 2000)]);
        p.found = false;
        p.clone_polls = 2;
        let mut c = Carrier::new("s1", "m1", "dent-linux")
            .clone_there(clone(&[".env", "config/.env.local"]))
            .with_chunk(512);
        let (r, waits) = p.run(&mut c);
        assert_eq!(r, Ok("s9".into()));
        assert_eq!(waits, 2);
        assert_eq!(
            p.files_read.as_deref(),
            Some(&[".env".to_string(), "config/.env.local".to_string()][..])
        );
        let first = p.clone_asks[0].as_ref().expect("the first ask carries it");
        assert_eq!(first.files.len(), 2);
        assert!(p.clone_asks[1..].iter().all(Option::is_none));
        assert_eq!(p.got["events.jsonl"].len(), 2000);
    }

    /// Nothing ticked: nothing read; a clone forgotten there (its daemon
    /// restarted) is asked for again, whole.
    #[test]
    fn a_clone_forgotten_there_is_begun_again() {
        let mut p = Pair::new(&[("events.jsonl", 10)]);
        p.found = false;
        p.clone_polls = 1;
        p.forget_clone = true;
        let mut c = Carrier::new("s1", "m1", "dent-linux").clone_there(clone(&[]));
        let (r, _) = p.run(&mut c);
        assert_eq!(r, Ok("s9".into()));
        assert!(p.files_read.is_none());
        let whole = p.clone_asks.iter().filter(|a| a.is_some()).count();
        assert_eq!(whole, 2, "{:?}", p.clone_asks);
    }

    /// Too old there to clone: it says so, before anything was packed. A
    /// clone that isn't the repository after all ends as no project, once.
    #[test]
    fn a_clone_that_cant_be_had_ends_plainly() {
        let mut p = Pair::new(&[("events.jsonl", 10)]);
        p.found = false;
        p.old = true;
        let mut c = Carrier::new("s1", "m1", "dent-linux").clone_there(clone(&[]));
        let (r, _) = p.run(&mut c);
        let Err(Ended::Stopped { why, .. }) = r else {
            panic!("{r:?}")
        };
        assert!(why.contains("older dent"), "{why}");
        assert_eq!(p.packs_with_have, 0);

        let mut p = Pair::new(&[("events.jsonl", 10)]);
        p.found = false;
        p.clone_finds = false;
        let mut c = Carrier::new("s1", "m1", "dent-linux").clone_there(clone(&[]));
        let (r, _) = p.run(&mut c);
        assert_eq!(r, Err(Ended::NoProject));
        assert_eq!(p.clone_asks.len(), 1);
    }

    #[test]
    fn every_byte_arrives_once_and_the_machine_it_left_hears_where() {
        let mut p = Pair::new(&[
            ("events.jsonl", 900_000),
            ("git.bundle", 1_300_000),
            ("empty", 0),
        ]);
        let mut c = Carrier::new("s1", "m1", "dent-linux");
        let (r, _) = p.run(&mut c);
        assert_eq!(r, Ok("s9".into()));
        for (name, bytes) in &p.data {
            assert_eq!(
                p.got.get(name).map_or(&[][..], |b| &b[..]),
                &bytes[..],
                "{name}"
            );
        }
        assert_eq!(p.told, Some((Some("s9".into()), None)));
        assert_eq!(c.phase(), Phase::Done);
    }

    #[test]
    fn it_waits_for_the_turn_then_carries_on() {
        let mut p = Pair::new(&[("events.jsonl", 10)]);
        p.waits = 2;
        let mut c = Carrier::new("s1", "m1", "dent-linux");
        let (r, waits) = p.run(&mut c);
        assert_eq!(r, Ok("s9".into()));
        assert_eq!(waits, 2);
    }

    #[test]
    fn a_machine_without_the_project_is_found_out_before_anything_is_packed() {
        let mut p = Pair::new(&[("events.jsonl", 10)]);
        p.found = false;
        let (r, _) = p.run(&mut Carrier::new("s1", "m1", "dent-linux"));
        assert_eq!(r, Err(Ended::NoProject));
        assert_eq!(p.packs_with_have, 0, "nothing packed or fenced");
        // Gone there after an earlier carrier packed it: called off, so it
        // works where it was again.
        let mut p = Pair::new(&[("events.jsonl", 10)]);
        p.packs_with_have = 1;
        p.found = false;
        assert_eq!(
            p.run(&mut Carrier::new("s1", "m1", "dent-linux")).0,
            Err(Ended::NoProject)
        );
        assert_eq!(p.told, Some((None, Some("called off".into()))));
        // Gone after some was sent: it may be there already, so it isn't
        // called off; the machine it leaves says what to do.
        p.reads = 1;
        let Err(Ended::Stopped {
            side: Side::From,
            why,
        }) = p.run(&mut Carrier::new("s1", "m1", "dent-linux")).0
        else {
            panic!("called off a move that may have arrived")
        };
        assert!(why.contains("Continue here"), "{why}");
        // Dent's scratch folder has no repository: it moves anywhere.
        let mut p = Pair::new(&[("events.jsonl", 10)]);
        p.found = false;
        p.in_repo = false;
        assert!(p.run(&mut Carrier::new("s1", "m1", "dent-linux")).0.is_ok());
    }

    #[test]
    fn refused_there_it_says_why_on_both_machines() {
        let mut p = Pair::new(&[("events.jsonl", 10)]);
        p.accept = Err("its agent isn't installed there".into());
        let (r, _) = p.run(&mut Carrier::new("s1", "m1", "dent-linux"));
        assert_eq!(
            r,
            Err(Ended::NotAccepted("its agent isn't installed there".into()))
        );
        assert_eq!(
            p.told,
            Some((None, Some("its agent isn't installed there".into())))
        );
    }

    #[test]
    fn stopped_partway_a_new_carrier_picks_up_where_it_was() {
        let mut p = Pair::new(&[("events.jsonl", 700_000), ("git.bundle", 1_500_000)]);
        p.fail_write = Some((3, "the link to dent-linux ended".into()));
        let (r, _) = p.run(&mut Carrier::new("s1", "m1", "dent-linux"));
        assert_eq!(
            r,
            Err(Ended::Stopped {
                side: Side::To,
                why: "the link to dent-linux ended".into()
            })
        );
        let reads = p.reads;
        let partway: u64 = p.got.values().map(|b| b.len() as u64).sum();
        assert!(partway > 0);
        // Again with the same id: only what's missing is read again.
        p.fail_write = None;
        let mut c = Carrier::new("s1", "m1", "dent-linux");
        let (r, _) = p.run(&mut c);
        assert_eq!(r, Ok("s9".into()));
        let again = p.reads - reads;
        let fresh =
            (700_000u64.div_ceil(CHUNK as u64) + 1_500_000u64.div_ceil(CHUNK as u64)) as usize;
        assert!(
            again < fresh,
            "read {again} chunks again, {fresh} from nothing"
        );
        for (name, bytes) in &p.data {
            assert_eq!(&p.got[name][..], &bytes[..], "{name}");
        }
        // And once it's there, one more carrier just finishes.
        let (r, _) = p.run(&mut Carrier::new("s1", "m1", "dent-linux"));
        assert_eq!(r, Ok("s9".into()));
    }

    #[test]
    fn progress_counts_what_arrived_including_what_was_there() {
        let mut p = Pair::new(&[("a", 1_000_000)]);
        p.got.insert("a".into(), vec![0; 600_000]);
        // The bytes there already are this machine's own: it reads on from them.
        p.data[0].1[..600_000].fill(0);
        let mut c = Carrier::new("s1", "m1", "dent-linux");
        let mut step = c.start();
        let mut seen = Vec::new();
        let mut arriving = false;
        while let Step::Ask(side, req) | Step::Wait(_, side, req) = step {
            let a = p.ask(side, req);
            step = c.answer(side, a);
            match c.phase() {
                Phase::Sending { sent, total } => seen.push((sent, total)),
                Phase::Arriving => arriving = true,
                _ => {}
            }
        }
        assert_eq!(seen.first(), Some(&(0, 1_000_000)));
        assert!(seen.contains(&(600_000, 1_000_000)), "{seen:?}");
        assert!(
            seen.windows(2).all(|w| w[0].0 <= w[1].0),
            "only ever up: {seen:?}"
        );
        // Every byte there, it reads as arriving rather than still sending.
        assert!(arriving);
    }
}
