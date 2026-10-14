//! The daemon end to end: a real daemon, the real holder and a fake
//! `claude`, driven over the socket the way the app drives it.
//!
//! These protect sessions and data, so they run with `cargo test`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use dent_protocol::{Answer, Body, Event, Out, Outcome, Permission, Request, Status, encode};
use serde_json::Value;

static BUILD: Once = Once::new();
static N: AtomicU32 = AtomicU32::new(0);

fn bin(name: &str) -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_dent-daemon")).with_file_name(name)
}

fn build_helpers() {
    BUILD.call_once(|| {
        let ok = Command::new(env!("CARGO"))
            .args([
                "build",
                "-q",
                "-p",
                "dent-session-holder",
                "-p",
                "dent-fake-claude",
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "building the holder and fake agent failed");
    });
}

struct World {
    state: PathBuf,
    project: PathBuf,
    daemon: Option<Child>,
    env: Vec<(String, String)>,
}

impl World {
    fn new(env: &[(&str, &str)]) -> Self {
        build_helpers();
        let n = N.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("dent-t{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let state = root.join("s");
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&project)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
        };
        git(&["init", "-q"]);
        std::fs::write(project.join("README.md"), "hello\n").unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
        ]);
        let mut all: Vec<(String, String)> = vec![
            (
                "DENT_CLAUDE".into(),
                bin("dent-fake-claude").display().to_string(),
            ),
            ("FAKE_CLAUDE_SPEED".into(), "0.05".into()),
        ];
        for (k, v) in env {
            all.retain(|(a, _)| a != k);
            all.push(((*k).into(), (*v).into()));
        }
        let mut w = Self {
            state,
            project,
            daemon: None,
            env: all,
        };
        w.start();
        w
    }

    fn start(&mut self) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_dent-daemon"));
        c.arg("--state").arg(&self.state);
        for (k, v) in &self.env {
            c.env(k, v);
        }
        c.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        self.daemon = Some(c.spawn().unwrap());
        let sock = self.state.join("daemon.sock");
        let until = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(&sock).is_err() {
            assert!(Instant::now() < until, "daemon didn't start");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn kill(&mut self) {
        if let Some(mut d) = self.daemon.take() {
            let _ = d.kill();
            let _ = d.wait();
        }
    }

    fn client(&self) -> Client {
        Client::connect(&self.state.join("daemon.sock"))
    }

    fn cwd(&self) -> String {
        self.project.display().to_string()
    }

    /// The holder's pid, from its lock file.
    fn holder_pid(&self) -> Option<i32> {
        let tmp = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in self.state.as_os_str().as_encoded_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let lock = tmp.join(format!("dent-{h:016x}")).join("holder.lock");
        std::fs::read_to_string(lock).ok()?.trim().parse().ok()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        self.kill();
        if let Some(pid) = self.holder_pid() {
            // SAFETY: kill has no memory preconditions.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        if let Some(root) = self.state.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

struct Client {
    w: UnixStream,
    rx: Receiver<(u64, Out)>,
    next: u64,
    /// Every event seen, by seq.
    events: Arc<Mutex<Vec<Event>>>,
    partial: HashMap<String, String>,
    replies: HashMap<u64, Out>,
}

impl Client {
    fn connect(sock: &Path) -> Self {
        let s = UnixStream::connect(sock).unwrap();
        let r = s.try_clone().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(r).lines() {
                let Ok(line) = line else { break };
                let v: Value = serde_json::from_str(&line).unwrap();
                let req = v["req"].as_u64().unwrap_or(0);
                let out: Out = serde_json::from_value(v).unwrap();
                if tx.send((req, out)).is_err() {
                    break;
                }
            }
        });
        let mut c = Self {
            w: s,
            rx,
            next: 0,
            events: Arc::default(),
            partial: HashMap::new(),
            replies: HashMap::new(),
        };
        c.call(Request::Hello {
            proto: dent_protocol::PROTOCOL,
            client: "test".into(),
        })
        .unwrap();
        c
    }

    fn send_req(&mut self, r: Request) -> u64 {
        self.next += 1;
        self.w.write_all(&encode(self.next, &r)).unwrap();
        self.next
    }

    /// Send a request and wait for its reply.
    fn call(&mut self, r: Request) -> Result<Out, String> {
        let id = self.send_req(r);
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(out) = self.replies.remove(&id) {
                return match out {
                    Out::Error { message } => Err(message),
                    o => Ok(o),
                };
            }
            assert!(Instant::now() < until, "no reply to request {id}");
            self.pump(Duration::from_millis(50));
        }
    }

    fn pump(&mut self, wait: Duration) {
        let Ok((req, out)) = self.rx.recv_timeout(wait) else {
            return;
        };
        self.take(req, out);
        while let Ok((req, out)) = self.rx.try_recv() {
            self.take(req, out);
        }
    }

    fn take(&mut self, req: u64, out: Out) {
        match out {
            Out::Events { events } => self.events.lock().unwrap().extend(events),
            Out::Delta {
                session,
                text,
                reset,
            } => {
                let p = self.partial.entry(session).or_default();
                if reset {
                    p.clear();
                }
                p.push_str(&text);
            }
            Out::CaughtUp { .. } => {}
            o if req != 0 => {
                self.replies.insert(req, o);
            }
            _ => {}
        }
    }

    fn subscribe(&mut self, after: u64) {
        self.call(Request::Subscribe { after }).unwrap();
    }

    fn create(&mut self, cwd: &str, permission: Permission, text: &str) -> String {
        match self.call(Request::Create {
            cwd: cwd.into(),
            model: None,
            permission,
            id: format!("m-{text}"),
            text: text.into(),
            attachments: Vec::new(),
        }) {
            Ok(Out::Reply {
                reply: dent_protocol::Reply::Created { session },
            }) => session,
            other => panic!("create: {other:?}"),
        }
    }

    fn say(&mut self, session: &str, id: &str, text: &str, steer: bool) {
        self.call(Request::Send {
            session: session.into(),
            id: id.into(),
            text: text.into(),
            steer,
            attachments: Vec::new(),
        })
        .unwrap();
    }

    fn events(&self, session: &str) -> Vec<Body> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.session == session)
            .map(|e| e.body.clone())
            .collect()
    }

    /// Wait until an event matching `f` has arrived for the session.
    fn wait(&mut self, session: &str, what: &str, f: impl Fn(&Body) -> bool) -> Body {
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(b) = self.events(session).into_iter().find(|b| f(b)) {
                return b;
            }
            if Instant::now() > until {
                panic!(
                    "timed out waiting for {what}; got {:#?}",
                    self.events(session)
                );
            }
            self.pump(Duration::from_millis(50));
        }
    }

    fn count(&self, session: &str, f: impl Fn(&Body) -> bool) -> usize {
        self.events(session).iter().filter(|b| f(b)).count()
    }

    fn turns_done(&self, session: &str) -> usize {
        self.count(session, |b| {
            matches!(
                b,
                Body::TurnEnded {
                    outcome: Outcome::Done
                }
            )
        })
    }

    fn wait_turns(&mut self, session: &str, n: usize) {
        let until = Instant::now() + Duration::from_secs(20);
        while self.turns_done(session) < n {
            if Instant::now() > until {
                panic!("wanted {n} finished turns; got {:#?}", self.events(session));
            }
            self.pump(Duration::from_millis(50));
        }
    }

    fn text(&self, session: &str) -> String {
        self.events(session)
            .iter()
            .filter_map(|b| match b {
                Body::Assistant {
                    parent: None,
                    blocks,
                } => Some(
                    blocks
                        .iter()
                        .filter_map(|x| match x {
                            dent_protocol::Block::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(""),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn asked(&mut self, session: &str) -> String {
        match self.wait(session, "a question", |b| matches!(b, Body::Asked { .. })) {
            Body::Asked { id, .. } => id,
            _ => unreachable!(),
        }
    }

    fn status(&self, session: &str) -> Option<Status> {
        self.events(session).iter().rev().find_map(|b| match b {
            Body::Status { status } => Some(*status),
            _ => None,
        })
    }
}

fn is_done(b: &Body) -> bool {
    matches!(
        b,
        Body::TurnEnded {
            outcome: Outcome::Done
        }
    )
}

#[test]
fn a_reply_streams_is_kept_and_catches_up_from_any_point() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "hello there");
    c.wait(&s, "the turn to end", is_done);
    assert!(c.text(&s).contains("hello there"), "{}", c.text(&s));
    c.wait(&s, "done", |b| {
        matches!(
            b,
            Body::Status {
                status: Status::Done
            }
        )
    });
    assert!(c.count(&s, |b| matches!(b, Body::Limits { .. })) == 1);
    assert!(c.count(&s, |b| matches!(b, Body::Context { .. })) >= 1);

    // A second client sees the same history, and from a midpoint only what
    // came after.
    let all = c.events.lock().unwrap().clone();
    let mut d = w.client();
    d.subscribe(0);
    d.wait(&s, "history", is_done);
    assert_eq!(d.events.lock().unwrap().len(), all.len());
    let mid = all[all.len() / 2].seq;
    let mut e = w.client();
    e.subscribe(mid);
    e.wait(&s, "the tail", is_done);
    let got = e.events.lock().unwrap().clone();
    assert!(got.iter().all(|x| x.seq > mid));
    assert_eq!(got.len(), all.iter().filter(|x| x.seq > mid).count());
}

#[test]
fn approvals_allow_deny_and_questions() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "please run it");
    let id = c.asked(&s);
    c.pump(Duration::from_millis(100));
    assert_eq!(c.status(&s), Some(Status::NeedsYou));
    // A session waiting on you can't be closed.
    assert!(c.call(Request::Close { session: s.clone() }).is_err());
    c.call(Request::Answer {
        session: s.clone(),
        id: id.clone(),
        answer: Answer::Allow { always: false },
    })
    .unwrap();
    // Answering twice is refused, never sent twice.
    assert!(
        c.call(Request::Answer {
            session: s.clone(),
            id,
            answer: Answer::Allow { always: false },
        })
        .is_err()
    );
    c.wait_turns(&s, 1);
    assert!(c.text(&s).contains("Ran it"));

    c.say(&s, "deny", "run it again", false);
    let id = loop {
        let ids: Vec<String> = c
            .events(&s)
            .into_iter()
            .filter_map(|b| match b {
                Body::Asked { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        if ids.len() == 2 {
            break ids[1].clone();
        }
        c.pump(Duration::from_millis(50));
    };
    c.call(Request::Answer {
        session: s.clone(),
        id,
        answer: Answer::Deny {
            message: "not on main".into(),
        },
    })
    .unwrap();
    c.wait_turns(&s, 2);
    assert!(c.text(&s).contains("not on main"));

    let q = c.create(&w.cwd(), Permission::Ask, "ask me which cache");
    let id = c.asked(&q);
    c.call(Request::Answer {
        session: q.clone(),
        id,
        answer: Answer::Choose {
            answers: vec!["Redis".into()],
        },
    })
    .unwrap();
    c.wait_turns(&q, 1);
    assert!(c.text(&q).contains("Redis"), "{}", c.text(&q));
}

#[test]
fn messages_sent_while_working_wait_unless_steered() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "a long answer please");
    c.wait(&s, "the turn to start", |b| matches!(b, Body::TurnStarted));
    c.say(&s, "q1", "and then this", false);
    c.say(&s, "q2", "and this too", false);
    c.call(Request::Unqueue {
        session: s.clone(),
        id: "q2".into(),
    })
    .unwrap();
    c.wait_turns(&s, 2);
    c.pump(Duration::from_millis(300));
    assert_eq!(c.turns_done(&s), 2, "the unqueued message must not run");
    assert_eq!(
        c.count(&s, |b| matches!(b, Body::Delivered { id } if id == "q1")),
        1
    );
    assert!(c.text(&s).contains("and then this"));
    assert!(!c.text(&s).contains("and this too"));

    // Steering goes into the running turn: no interruption is reported.
    c.say(&s, "l2", "another long one", false);
    let started = c.count(&s, |b| matches!(b, Body::TurnStarted));
    let until = Instant::now() + Duration::from_secs(10);
    while c.count(&s, |b| matches!(b, Body::TurnStarted)) == started && Instant::now() < until {
        c.pump(Duration::from_millis(20));
    }
    c.pump(Duration::from_millis(200));
    c.say(&s, "st", "actually do the steer thing", true);
    c.wait_turns(&s, 3);
    assert_eq!(
        c.count(&s, |b| matches!(
            b,
            Body::TurnEnded {
                outcome: Outcome::Interrupted { .. }
            }
        )),
        0,
        "{:#?}",
        c.events(&s)
    );
    assert!(c.text(&s).contains("steer thing"));
    assert!(c.count(&s, |b| matches!(b, Body::User { steer: true, .. })) == 1);
}

#[test]
fn stop_stops_within_two_seconds_and_returns_held_messages() {
    let w = World::new(&[("FAKE_CLAUDE_SPEED", "0.5")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "a long answer please");
    c.wait(&s, "the turn to start", |b| matches!(b, Body::TurnStarted));
    c.say(&s, "held", "after that", false);
    let t = Instant::now();
    c.call(Request::Interrupt { session: s.clone() }).unwrap();
    c.wait(&s, "the interruption", |b| {
        matches!(
            b,
            Body::TurnEnded {
                outcome: Outcome::Interrupted { .. }
            }
        )
    });
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "took {:?}",
        t.elapsed()
    );
    assert_eq!(
        c.count(&s, |b| matches!(b, Body::Unqueued { id } if id == "held")),
        1
    );
    c.pump(Duration::from_millis(500));
    assert_eq!(c.status(&s), Some(Status::Interrupted));
    assert_eq!(c.turns_done(&s), 0);
    c.say(&s, "again", "short one", false);
    c.wait_turns(&s, 1);
}

#[test]
fn a_daemon_restart_mid_reply_loses_and_repeats_nothing() {
    let mut w = World::new(&[("FAKE_CLAUDE_SPEED", "0.3")]);
    let s = {
        let mut c = w.client();
        c.subscribe(0);
        let s = c.create(&w.cwd(), Permission::Ask, "a long answer please");
        c.wait(&s, "some text", |b| matches!(b, Body::TurnStarted));
        std::thread::sleep(Duration::from_millis(400));
        s
    };
    w.kill();
    std::thread::sleep(Duration::from_millis(500));
    w.start();
    let mut c = w.client();
    c.subscribe(0);
    c.wait_turns(&s, 1);
    c.pump(Duration::from_millis(300));
    assert_eq!(c.count(&s, |b| matches!(b, Body::Assistant { .. })), 1);
    assert_eq!(c.count(&s, |b| matches!(b, Body::TurnStarted)), 1);
    assert_eq!(c.count(&s, |b| matches!(b, Body::TurnEnded { .. })), 1);
    assert!(c.text(&s).contains("That's the plan"));
    let seqs: Vec<u64> = c.events.lock().unwrap().iter().map(|e| e.seq).collect();
    assert!(
        seqs.windows(2).all(|p| p[1] == p[0] + 1),
        "gaps in {seqs:?}"
    );
}

#[test]
fn a_crashed_agent_ends_the_turn_and_the_next_message_resumes_it() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "crash please");
    let end = c.wait(&s, "the interruption", |b| {
        matches!(b, Body::TurnEnded { .. })
    });
    match end {
        Body::TurnEnded {
            outcome: Outcome::Interrupted { reason: Some(r) },
        } => assert!(r.contains("exit code 3"), "{r}"),
        other => panic!("{other:?}"),
    }
    let agent = c
        .events(&s)
        .iter()
        .find_map(|b| match b {
            Body::Agent {
                agent_session: Some(a),
                ..
            } => Some(a.clone()),
            _ => None,
        })
        .expect("the agent's session is named up front");
    c.say(&s, "two", "hello again", false);
    c.wait_turns(&s, 1);
    // The same agent session carried on.
    assert!(
        c.events(&s)
            .iter()
            .all(|b| !matches!(b, Body::Agent { agent_session: Some(a), .. } if *a != agent))
    );
}

#[test]
fn idle_agents_stop_quietly_and_start_again() {
    let w = World::new(&[("DENT_IDLE_MS", "300")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "hi");
    c.wait_turns(&s, 1);
    std::thread::sleep(Duration::from_millis(900));
    c.pump(Duration::from_millis(100));
    assert_eq!(
        c.count(&s, |b| matches!(b, Body::Notice { .. })),
        0,
        "{:#?}",
        c.events(&s)
    );
    c.say(&s, "2", "hi again", false);
    c.wait_turns(&s, 2);
}

#[test]
fn a_dead_holder_interrupts_the_turn_and_the_session_carries_on() {
    let w = World::new(&[("FAKE_CLAUDE_SPEED", "0.5")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "a long answer please");
    c.wait(&s, "the turn to start", |b| matches!(b, Body::TurnStarted));
    let pid = w.holder_pid().expect("holder running");
    // SAFETY: kill has no memory preconditions.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    c.wait(&s, "the interruption", |b| {
        matches!(
            b,
            Body::TurnEnded {
                outcome: Outcome::Interrupted { .. }
            }
        )
    });
    c.say(&s, "2", "short", false);
    c.wait_turns(&s, 1);
}

#[test]
fn changes_files_and_settings() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Edits, "write notes.md please");
    c.wait_turns(&s, 1);
    assert_eq!(c.count(&s, |b| matches!(b, Body::Asked { .. })), 0);
    match c.call(Request::Changes { session: s.clone() }).unwrap() {
        Out::Reply {
            reply: dent_protocol::Reply::Changes { files },
        } => {
            let f = files
                .iter()
                .find(|f| f.path == "notes.md")
                .expect("notes.md");
            assert!(f.added > 0 && f.diff.contains("+# Notes"));
        }
        other => panic!("{other:?}"),
    }
    match c
        .call(Request::ReadFile {
            session: s.clone(),
            path: "notes.md".into(),
        })
        .unwrap()
    {
        Out::Reply {
            reply: dent_protocol::Reply::File { bytes_b64 },
        } => {
            let b = dent_protocol::b64::decode(&bytes_b64).unwrap();
            assert!(String::from_utf8(b).unwrap().starts_with("# Notes"));
        }
        other => panic!("{other:?}"),
    }
    assert!(
        c.call(Request::ReadFile {
            session: s.clone(),
            path: "../../etc/passwd".into(),
        })
        .is_err()
    );
    c.call(Request::SetModel {
        session: s.clone(),
        model: "opus".into(),
    })
    .unwrap();
    c.call(Request::SetPermission {
        session: s.clone(),
        permission: Permission::Plan,
    })
    .unwrap();
    c.call(Request::Close { session: s.clone() }).unwrap();
    c.call(Request::Reopen { session: s.clone() }).unwrap();
    // A resend with the same id is recognised.
    c.say(&s, "dup", "short", false);
    c.say(&s, "dup", "short", false);
    c.wait_turns(&s, 2);
    c.pump(Duration::from_millis(300));
    assert_eq!(
        c.count(&s, |b| matches!(b, Body::User { id, .. } if id == "dup")),
        1
    );
    assert_eq!(c.turns_done(&s), 2);
    match c.call(Request::Projects).unwrap() {
        Out::Reply {
            reply: dent_protocol::Reply::Projects { projects },
        } => assert!(projects.iter().any(|p| p.path == w.cwd())),
        other => panic!("{other:?}"),
    }
    match c.call(Request::Models).unwrap() {
        Out::Reply {
            reply: dent_protocol::Reply::Models { models },
        } => assert!(models.iter().any(|m| m.id == "haiku")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn background_work_waits_for_the_follow_up_turn() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Full, "start the background job");
    c.wait_turns(&s, 1);
    c.pump(Duration::from_millis(30));
    assert_eq!(c.status(&s), Some(Status::Background));
    c.wait_turns(&s, 2);
    c.pump(Duration::from_millis(100));
    assert_eq!(c.status(&s), Some(Status::Done));
    assert!(c.text(&s).contains("finished cleanly"));
}

#[test]
fn agents_wind_down_when_no_daemon_comes_back() {
    let mut w = World::new(&[("DENT_HOLDER_ORPHAN_MS", "500")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "hi");
    c.wait_turns(&s, 1);
    let agent = c
        .events(&s)
        .iter()
        .find_map(|b| match b {
            Body::Agent {
                agent_session: Some(a),
                ..
            } => Some(a.clone()),
            _ => None,
        })
        .expect("agent named");
    let running = || {
        Command::new("pgrep")
            .args(["-f", &agent])
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false)
    };
    assert!(running(), "the agent waits for the next message");
    w.kill();
    let until = Instant::now() + Duration::from_secs(10);
    while running() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !running(),
        "the agent should exit once the holder gives up on the daemon"
    );
}
