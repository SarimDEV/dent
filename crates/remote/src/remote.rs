//! A remote daemon as a client sees one: the connection actor run by
//! [`crate::host`], with the daemon session on top. On every trusted link it
//! says hello, subscribes from the last event the client applied, and resends
//! the requests still unanswered (a resend changes nothing twice: messages,
//! forks and new sessions carry the client's own ids). Requests made while
//! the machine is away wait here and go when it's back
//! ([decision 0063](../../docs/decisions/0063-queued-prompts.md)).

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dent_client_core::machine::{Machine, Note, Signal, State};
use dent_link::{Keys, Trust};
use dent_protocol::{AgentInfo, Out, PROTOCOL, Reply, Request, encode};
use tokio::sync::mpsc;

use crate::history::History;
use crate::host::{self, Ask};
use crate::store::{self, Saved};

const HELLO: u64 = 1;
const SUBSCRIBE: u64 = 2;
/// The first number a caller's request may take.
pub const FIRST: u64 = 3;

enum Cmd {
    /// Request `n`, already in [`Pending`], to go if the link is up.
    Send(u64),
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
        /// The launcher's name for the version that runs, when the service
        /// runs it, and how its last update went.
        release: String,
        update: Option<dent_protocol::UpdateOutcome>,
        /// Its number among main's builds (0: not built by CI), and a newer
        /// one it has ready.
        main_build: u64,
        available: Option<dent_protocol::Available>,
        /// The protocol it speaks.
        proto: u32,
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
    /// Request `n` to send on a new link, gone or not, now marked gone.
    fn resend(&mut self, n: u64) -> Option<Vec<u8>> {
        let (r, gone) = self.0.get_mut(&n)?;
        *gone = true;
        Some(line(n, r))
    }

    /// Request `n` on a link that's up, now marked gone; none if it went
    /// already (the link came up after it was asked, and took it).
    fn go(&mut self, n: u64) -> Option<Vec<u8>> {
        let (r, gone) = self.0.get_mut(&n).filter(|(_, gone)| !*gone)?;
        *gone = true;
        Some(line(n, r))
    }

    /// Request `n`, if it has files on this device still to read in.
    fn to_carry(&self, n: u64, theirs: &HashSet<String>) -> Option<Request> {
        let (r, _) = self.0.get(&n)?;
        local_files(r, theirs).then(|| r.clone())
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
    /// Paths the machine's own events named: its files, which stay paths.
    theirs: Arc<Mutex<HashSet<String>>>,
}

impl Remote {
    /// Send request number `n`; its reply comes back as [`Event::Out`] with
    /// the same number. While the machine is away it waits.
    /// Files on this device go inside it, read in on the way out (see
    /// [`carry`]).
    ///
    /// Numbers up to [`FIRST`] are the link's own (its hello and
    /// subscribe); a reply to one of those would never reach the caller.
    pub fn send(&self, n: u64, r: Request) {
        assert!(
            n >= FIRST,
            "request numbers below {FIRST} are the link's own"
        );
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

    /// Paths the machine named in its own events (files of its messages):
    /// sent back, they stay paths, never read here.
    pub fn theirs(&self, paths: impl IntoIterator<Item = String>) {
        if let Ok(mut t) = self.theirs.lock() {
            t.extend(paths);
        }
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
    spawn_kept(rt, keys, client, saved, None, events)
}

/// The machine's log as this device keeps it ([`History`]): where a link
/// resumes, and where what comes is kept.
pub struct Keeping {
    pub history: History,
    /// The kept log's epoch and its newest event, already applied by the
    /// client; `None` when nothing's kept.
    pub resume: Option<(String, u64)>,
}

/// Like [`spawn`], keeping the machine's log on this device: a link
/// subscribes after what's kept, and every event that comes is kept, in
/// order, before the client hears it.
pub fn spawn_kept(
    rt: &tokio::runtime::Handle,
    keys: Keys,
    client: &str,
    saved: Saved,
    keeping: Option<Keeping>,
    events: impl FnMut(Event) + Send + 'static,
) -> Remote {
    spawn_with(rt, keys, client, saved, keeping, true, events)
}

/// Like [`spawn`], for asking only: no link subscribes to the machine's
/// log, so nothing of it comes, only answers (`dent call`, `dent get`).
pub fn spawn_asking(
    rt: &tokio::runtime::Handle,
    keys: Keys,
    client: &str,
    saved: Saved,
    events: impl FnMut(Event) + Send + 'static,
) -> Remote {
    spawn_with(rt, keys, client, saved, None, false, events)
}

fn spawn_with(
    rt: &tokio::runtime::Handle,
    keys: Keys,
    client: &str,
    saved: Saved,
    keeping: Option<Keeping>,
    follow: bool,
    events: impl FnMut(Event) + Send + 'static,
) -> Remote {
    let (tx, rx) = mpsc::unbounded_channel();
    let resumed = keeping.as_ref().and_then(|k| k.resume.clone());
    let seq = Arc::new(AtomicU64::new(resumed.as_ref().map_or(0, |r| r.1)));
    let pending = Arc::new(Mutex::new(Pending::default()));
    let theirs = Arc::new(Mutex::new(HashSet::new()));
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
        keeping,
        follow,
        seq.clone(),
        pending.clone(),
        theirs.clone(),
        rx,
        events,
    ));
    Remote {
        tx,
        pending,
        seq,
        theirs,
    }
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

/// The oldest protocol whose daemons take files inside a message.
const CARRIES: u32 = 2;

/// The oldest protocol whose daemons can bring a project from another
/// machine (`Request::Create`'s `bring`). An older one would read the
/// request as a session in its scratch folder.
const BRINGS: u32 = 3;

/// The paths a message attaches, if it's one that carries files.
fn attached(r: &Request) -> Option<&Vec<String>> {
    match r {
        Request::Send { attachments, .. } | Request::Create { attachments, .. } => {
            Some(attachments)
        }
        _ => None,
    }
}

/// Whether `r` attaches a file of this device's: any path but the
/// machine's own.
fn local_files(r: &Request, theirs: &HashSet<String>) -> bool {
    attached(r).is_some_and(|a| a.iter().any(|p| !theirs.contains(p)))
}

/// A message's files from this device go inside it: the machine it's for
/// can't read this device's paths. Its own files (paths its events named)
/// stay paths. A file of this device's that's gone or unreadable stops the
/// message, rather than sending a path that means nothing there.
fn carry(r: &mut Request, theirs: &HashSet<String>) -> Result<(), String> {
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
        if theirs.contains(&path) {
            keep.push(path);
            continue;
        }
        let p = std::path::Path::new(&path);
        let name = p
            .file_name()
            .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
        let too_many = || {
            format!(
                "these files come to more than {} MB, too many to send to another machine at once",
                dent_protocol::CARRIED_MAX >> 20
            )
        };
        // Opened without waiting (a pipe would hold this machine's link up
        // for good), only a plain file, and its size known before any of it
        // is read: a video of gigabytes is refused, not loaded first.
        let f = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(p)
                .map_err(|e| format!("couldn't read {name}: {e}"))?
        };
        let meta = f
            .metadata()
            .map_err(|e| format!("couldn't read {name}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("{name} isn't a file that can be sent"));
        }
        if total + meta.len() as usize > dent_protocol::CARRIED_MAX {
            return Err(too_many());
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        {
            use std::io::Read;
            f.take((dent_protocol::CARRIED_MAX - total) as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| format!("couldn't read {name}: {e}"))?;
        }
        total += bytes.len();
        // Grown while it was read.
        if total > dent_protocol::CARRIED_MAX {
            return Err(too_many());
        }
        files.push(dent_protocol::Carried {
            name,
            bytes_b64: dent_protocol::b64::encode(&bytes),
        });
    }
    *attachments = keep;
    Ok(())
}

/// Read request `n`'s files in, once, off the async threads, before it
/// first goes; the carried request replaces it, so a resend reads nothing
/// again. `Err`: it can't go, and why.
async fn ready_up(
    pending: &Arc<Mutex<Pending>>,
    theirs: &Arc<Mutex<HashSet<String>>>,
    proto: u32,
    name: &str,
    n: u64,
) -> Result<(), String> {
    let brings = pending.lock().ok().is_some_and(|p| {
        p.0.get(&n)
            .is_some_and(|(r, _)| matches!(r, Request::Create { bring: Some(_), .. }))
    });
    if brings && proto < BRINGS {
        return Err(format!(
            "{name} runs an older dent, which can't bring a project from another machine: update dent there"
        ));
    }
    let known = theirs.lock().map(|t| t.clone()).unwrap_or_default();
    let Some(mut r) = pending.lock().ok().and_then(|p| p.to_carry(n, &known)) else {
        return Ok(());
    };
    if proto < CARRIES {
        return Err(format!(
            "{name} runs an older dent, which can't take files from another machine: update dent there"
        ));
    }
    let r = tokio::task::spawn_blocking(move || carry(&mut r, &known).map(|()| r))
        .await
        .map_err(|e| e.to_string())??;
    if let Ok(mut p) = pending.lock()
        && let Some((was, false)) = p.0.get_mut(&n)
    {
        *was = r;
    }
    Ok(())
}

fn line(n: u64, r: &Request) -> Vec<u8> {
    let mut l = encode(n, r);
    l.pop();
    l
}

// The actor's state, each piece shared with the handle or the host.
#[allow(clippy::too_many_arguments)]
async fn run(
    m: Machine,
    client: String,
    mut saved: Saved,
    keeping: Option<Keeping>,
    follow: bool,
    seq: Arc<AtomicU64>,
    pending: Arc<Mutex<Pending>>,
    theirs: Arc<Mutex<HashSet<String>>>,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    mut events: impl FnMut(Event) + Send + 'static,
) {
    let (ask_tx, ask_rx) = mpsc::unbounded_channel();
    let (note_tx, mut notes) = mpsc::unbounded_channel();
    let host = tokio::spawn(host::run(m, ask_rx, note_tx));
    // The daemon's protocol, once it has answered the hello on this link:
    // requests wait for it, since whether a message's files can go inside
    // it depends on it.
    let mut ready: Option<u32> = None;
    // Request `n` can't go: it's answered with why, and dropped.
    let refuse = |n: u64, message: String, events: &mut dyn FnMut(Event)| {
        if let Ok(mut p) = pending.lock() {
            p.0.remove(&n);
        }
        events(Event::Out {
            req: n,
            out: Out::Error { message },
        });
    };
    // The link the last Up named, and how it was trusted.
    let mut via = (
        String::new(),
        Trust::Untrusted {
            pairing_open: false,
        },
    );
    // Resumed from the kept log: the client has applied it, so a link
    // subscribes after it, in its epoch.
    let (history, resume) = match keeping {
        Some(k) => (Some(k.history), k.resume),
        None => (None, None),
    };
    let mut cursor = Cursor {
        applied: seq,
        passed: resume.as_ref().map_or(0, |r| r.1),
        epoch: resume.map(|r| r.0),
    };
    loop {
        tokio::select! {
            n = notes.recv() => {
                let Some(n) = n else { break };
                match n {
                    Note::Up { addr, trust, key, .. } => {
                        ready = None;
                        via = (addr, trust);
                        if saved.key.is_empty() {
                            saved.key = hex::encode(&key);
                        }
                        let hello = Request::Hello { proto: PROTOCOL, client: client.clone() };
                        let sub = cursor.subscribe();
                        let _ = ask_tx.send(Ask::Send(line(HELLO, &hello)));
                        if follow {
                            let _ = ask_tx.send(Ask::Send(line(SUBSCRIBE, &sub)));
                        }
                    }
                    Note::State(s) => {
                        if !matches!(s, State::Online | State::Slow) {
                            ready = None;
                        }
                        events(Event::State(s));
                    }
                    Note::Data { line, .. } => {
                        let Some(env) = dent_protocol::read_out(&line) else {
                            continue;
                        };
                        let (req, out) = (env.req, env.body);
                        if req == HELLO {
                            match out {
                                Out::Reply { reply: Reply::Welcome { proto, machine, id, version, agents, addresses, wake, epoch: e, release, update, main_build, available, .. } } => {
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
                                    // Kept from before when it says none: a
                                    // machine waking up on another network
                                    // still has its cards.
                                    if !wake.is_empty() {
                                        saved.wake = wake;
                                    }
                                    if saved != before && !saved.id.is_empty() {
                                        events(Event::Saved(saved.clone()));
                                    }
                                    events(Event::Welcome {
                                        name: machine.clone(),
                                        id,
                                        version,
                                        release,
                                        update: update.map(|u| *u),
                                        main_build,
                                        available,
                                        proto,
                                        agents,
                                        via: via.0.clone(),
                                        trust: via.1.clone(),
                                    });
                                    // What's still unanswered goes again, in
                                    // order: a resend changes nothing twice.
                                    ready = Some(proto);
                                    let all: Vec<u64> = pending.lock().map(|p| p.0.keys().copied().collect()).unwrap_or_default();
                                    for n in all {
                                        if let Err(why) = ready_up(&pending, &theirs, proto, &machine, n).await {
                                            refuse(n, why, &mut events);
                                        } else if let Some(l) = pending.lock().ok().and_then(|mut p| p.resend(n)) {
                                            let _ = ask_tx.send(Ask::Send(l));
                                        }
                                    }
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
                        // Kept before the client hears it, in the order it
                        // came, tagged with the log it's from.
                        if let (Some(h), Some(epoch)) = (&history, &cursor.epoch)
                            && !saved.id.is_empty()
                        {
                            match &out {
                                Out::Resync { epoch } => h.reset(&saved.id, epoch),
                                Out::Events { events } => {
                                    h.save(&saved.id, epoch, events.clone())
                                }
                                _ => {}
                            }
                        }
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
                    if let Some(proto) = ready {
                        if let Err(why) = ready_up(&pending, &theirs, proto, &saved.name, n).await {
                            refuse(n, why, &mut events);
                        } else if let Some(l) = pending.lock().ok().and_then(|mut p| p.go(n)) {
                            let _ = ask_tx.send(Ask::Send(l));
                        }
                    }
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
    fn only_a_request_that_never_went_out_is_taken_back() {
        let mut p = Pending::default();
        let r = || Request::Projects;
        p.0.insert(3, (r(), false));
        p.0.insert(4, (r(), false));
        // Away: 3 waits and is taken back; it never goes.
        assert!(p.cancel(3));
        assert!(p.go(3).is_none());
        // The link comes up first: 4 goes, and may have reached the daemon.
        assert!(p.resend(4).is_some());
        assert!(p.resend(3).is_none());
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
        let known: HashSet<String> = [theirs.clone()].into();
        assert!(local_files(&r, &known));
        carry(&mut r, &known).unwrap();
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
        // Carried once: nothing's left to read.
        assert!(!local_files(&r, &known));
        // A file of this device's that's gone stops the message.
        let mut gone = Request::Send {
            session: "s1".into(),
            id: "m3".into(),
            text: "look".into(),
            steer: false,
            attachments: vec![dir.join("moved.png").display().to_string()],
            files: Vec::new(),
        };
        assert!(carry(&mut gone, &known).unwrap_err().contains("moved.png"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_project_is_brought_only_to_a_daemon_that_can_take_it() {
        let bring = Request::Create {
            agent: None,
            cwd: String::new(),
            worktree: None,
            model: None,
            permission: Default::default(),
            effort: None,
            id: "m1".into(),
            text: "hi".into(),
            attachments: Vec::new(),
            files: Vec::new(),
            bring: Some(Default::default()),
        };
        let pending = Arc::new(Mutex::new(Pending::default()));
        pending.lock().unwrap().0.insert(1, (bring, false));
        let theirs = Arc::new(Mutex::new(HashSet::new()));
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let old = rt.block_on(ready_up(&pending, &theirs, 2, "vm", 1));
        assert!(old.unwrap_err().contains("can't bring a project"));
        assert!(rt.block_on(ready_up(&pending, &theirs, 3, "vm", 1)).is_ok());
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

#[cfg(test)]
#[path = "break_b_tests.rs"]
mod break_b_tests;
