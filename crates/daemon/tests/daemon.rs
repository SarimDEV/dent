//! The daemon end to end: a real daemon, the real holder and a fake
//! `claude`, driven over the socket the way the app drives it.
//!
//! These protect sessions and data, so they run with `cargo test`.

mod common;

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
            // These tests are about the local socket: no network listener,
            // so test daemons never hold the real daemon's port.
            ("DENT_REMOTE_PORT".into(), "0".into()),
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
        let sock = dent_protocol::daemon_socket(&self.state);
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
        Client::connect(&dent_protocol::daemon_socket(&self.state))
    }

    fn cwd(&self) -> String {
        self.project.display().to_string()
    }

    fn agent_pids(&self) -> Vec<i32> {
        common::agent_pids(&self.state)
    }

    fn holder_pid(&self) -> Option<i32> {
        common::holder_pid(&self.state)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        self.kill();
        common::stop_all(&self.state);
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

    /// Takes in what has arrived, waiting up to `wait` for something: for
    /// loops that poll. To wait a set time, [`Client::pump_for`].
    fn pump(&mut self, wait: Duration) {
        let Ok((req, out)) = self.rx.recv_timeout(wait) else {
            return;
        };
        self.take(req, out);
        while let Ok((req, out)) = self.rx.try_recv() {
            self.take(req, out);
        }
    }

    /// Takes in everything that arrives for all of `wait`: a test that
    /// then asserts nothing more happened has really waited.
    fn pump_for(&mut self, wait: Duration) {
        let until = Instant::now() + wait;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match self.rx.recv_timeout(left) {
                Ok((req, out)) => self.take(req, out),
                Err(_) => return,
            }
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
        self.call(Request::Subscribe { after, epoch: None })
            .unwrap();
    }

    fn create(&mut self, cwd: &str, permission: Permission, text: &str) -> String {
        match self.call(Request::Create {
            agent: None,
            cwd: cwd.into(),
            worktree: None,
            model: None,
            permission,
            effort: None,
            id: format!("m-{text}"),
            text: text.into(),
            attachments: Vec::new(),
            files: Vec::new(),
            bring: None,
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
            files: Vec::new(),
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
                    ..
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
    // The agent's own count, with its window size, follows the turn.
    c.wait(&s, "the context count", |b| {
        matches!(b, Body::Context { max: Some(_), .. })
    });

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
fn agents_say_how_they_stand_and_sessions_keep_theirs() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    match c.call(Request::Agents { fresh: true }).unwrap() {
        Out::Reply {
            reply: dent_protocol::Reply::Agents { agents, default },
        } => {
            assert_eq!(default, "claude");
            let a = agents.iter().find(|a| a.id == "claude").expect("claude");
            assert!(a.installed, "{a:?}");
            assert_eq!(a.version.as_deref(), Some("2.1.301 (Claude Code)"));
            assert!(!a.untested(), "{a:?}");
            assert!(matches!(
                &a.account,
                dent_protocol::Account::SignedIn { plan: Some(p), .. } if p == "max"
            ));
            assert!(a.caps.steer && a.caps.rewind && !a.caps.permissions.is_empty());
        }
        other => panic!("{other:?}"),
    }
    // A session names its agent, and one this machine has no driver for
    // is refused before anything is made.
    let made = c.call(Request::Create {
        agent: Some("no-such-agent".into()),
        cwd: w.cwd(),
        worktree: None,
        model: None,
        permission: Permission::Ask,
        effort: None,
        id: "m-nobody".into(),
        text: "hello".into(),
        attachments: Vec::new(),
        files: Vec::new(),
        bring: None,
    });
    assert!(
        matches!(&made, Err(e) if e.contains("no agent called no-such-agent")),
        "{made:?}"
    );
    let s = match c.call(Request::Create {
        agent: Some("claude".into()),
        cwd: w.cwd(),
        worktree: None,
        model: None,
        permission: Permission::Ask,
        effort: None,
        id: "m-named".into(),
        text: "hello by name".into(),
        attachments: Vec::new(),
        files: Vec::new(),
        bring: None,
    }) {
        Ok(Out::Reply {
            reply: dent_protocol::Reply::Created { session },
        }) => session,
        other => panic!("create: {other:?}"),
    };
    c.wait(&s, "the turn to end", is_done);
    assert_eq!(
        c.count(
            &s,
            |b| matches!(b, Body::Created { agent, .. } if agent == "claude")
        ),
        1
    );
    assert!(
        c.call(Request::Models {
            agent: Some("no-such-agent".into()),
        })
        .is_err()
    );
}

#[test]
fn an_agent_that_can_do_less_is_never_asked_to() {
    let w = World::new(&[(
        "DENT_CAPS_OFF",
        "steer,rewind,fork,effort,restart,stop_task,set_model,mode:plan",
    )]);
    let mut c = w.client();
    c.subscribe(0);
    // What it can't do says so where clients look.
    match c.call(Request::Agents { fresh: false }).unwrap() {
        Out::Reply {
            reply: dent_protocol::Reply::Agents { agents, .. },
        } => {
            let caps = &agents[0].caps;
            assert!(!caps.steer && !caps.rewind && !caps.fork && caps.aside);
            assert!(!caps.permissions.contains(&Permission::Plan));
        }
        other => panic!("{other:?}"),
    }
    let s = c.create(&w.cwd(), Permission::Ask, "a long answer please");
    c.wait(&s, "the turn to start", |b| matches!(b, Body::TurnStarted));
    // A steer waits for the turn to end, as any message sent meanwhile.
    c.say(&s, "st", "steer this in", true);
    c.wait(
        &s,
        "it waits",
        |b| matches!(b, Body::User { id, queued: true, steer: false, .. } if id == "st"),
    );
    // A model change it can't take as it runs is kept for the next process.
    c.call(Request::SetModel {
        session: s.clone(),
        model: "haiku".into(),
    })
    .unwrap();
    c.wait(
        &s,
        "the model is kept",
        |b| matches!(b, Body::Settings { model: Some(m), .. } if m == "haiku"),
    );
    c.wait_turns(&s, 2);
    let refused = |r: Result<Out, String>, why: &str| match r {
        Err(e) => assert!(e.contains(why), "{e}"),
        other => panic!("{why}: {other:?}"),
    };
    let first = c
        .events(&s)
        .iter()
        .find_map(|b| match b {
            Body::User { id, .. } => Some(id.clone()),
            _ => None,
        })
        .unwrap();
    refused(
        c.call(Request::Rewind {
            session: s.clone(),
            to: first,
            files: false,
        }),
        "can't go back",
    );
    refused(
        c.call(Request::Fork {
            session: s.clone(),
            before: None,
            until: None,
            id: None,
        }),
        "can't carry a conversation on",
    );
    refused(
        c.call(Request::SetEffort {
            session: s.clone(),
            effort: "high".into(),
        }),
        "no effort levels",
    );
    refused(
        c.call(Request::RestartAgent { session: s.clone() }),
        "can't start again",
    );
    refused(
        c.call(Request::SetPermission {
            session: s.clone(),
            permission: Permission::Plan,
        }),
        "no Plan mode",
    );
    refused(
        c.call(Request::StopTask {
            session: s.clone(),
            task: "t".into(),
        }),
        "can't stop one piece",
    );
    // What it can do still works.
    c.call(Request::SetPermission {
        session: s.clone(),
        permission: Permission::Edits,
    })
    .unwrap();
    // And the machine says how it stands for Git and GitHub.
    match c.call(Request::Machine).unwrap() {
        Out::Reply {
            reply: dent_protocol::Reply::Machine { machine },
        } => assert!(machine.git.is_some(), "{machine:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_command_the_agent_runs_itself_shows_its_answer_once() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "/context");
    c.wait(&s, "the command to end", is_done);
    // Its answer is the command's, with the meter as data, and only once.
    let commands = |c: &Client| {
        c.count(&s, |b| {
            matches!(b, Body::Command { name, view: Some(dent_protocol::View::Meter { .. }), text, .. }
                if name == "context" && text.contains("Context Usage"))
        })
    };
    assert_eq!(commands(&c), 1, "{:#?}", c.events(&s));
    assert_eq!(c.count(&s, |b| matches!(b, Body::Assistant { .. })), 0);
    // An ordinary reply isn't repeated from its result.
    let replies = |c: &Client| c.count(&s, |b| matches!(b, Body::Assistant { parent: None, .. }));
    let before = replies(&c);
    c.say(&s, "m2", "hello there", false);
    c.wait_turns(&s, 2);
    assert!(replies(&c) > before);
    assert_eq!(
        c.text(&s).matches("hello there").count(),
        1,
        "{}",
        c.text(&s)
    );
}

#[test]
fn a_new_sessions_folder_lists_its_files() {
    let w = World::new(&[]);
    let mut c = w.client();
    match c.call(Request::FilesIn { dir: w.cwd() }).unwrap() {
        Out::Reply {
            reply: dent_protocol::Reply::Files { paths },
        } => assert!(paths.iter().any(|p| p == "README.md"), "{paths:?}"),
        o => panic!("{o:?}"),
    }
    assert!(
        c.call(Request::FilesIn {
            dir: "relative".into()
        })
        .is_err()
    );
}

#[test]
fn files_a_message_carries_are_written_here_and_attached() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "first");
    c.wait(&s, "the turn to end", is_done);
    let file = |name: &str, bytes: &[u8]| dent_protocol::Carried {
        name: name.into(),
        bytes_b64: dent_protocol::b64::encode(bytes),
    };
    c.call(Request::Send {
        session: s.clone(),
        id: "m-carry".into(),
        text: "look".into(),
        steer: false,
        attachments: Vec::new(),
        files: vec![
            file("../../escape.txt", b"one"),
            file("a/Escape.txt", b"two"),
            file(&format!("{}.png", "x".repeat(300)), b"three"),
        ],
    })
    .unwrap();
    c.wait(
        &s,
        "the message",
        |b| matches!(b, Body::User { id, .. } if id == "m-carry"),
    );
    let paths = c
        .events(&s)
        .into_iter()
        .find_map(|b| match b {
            Body::User {
                id, attachments, ..
            } if id == "m-carry" => Some(attachments),
            _ => None,
        })
        .unwrap();
    assert_eq!(paths.len(), 3, "{paths:?}");
    // Kept to their message's folder, whatever the names say, and two
    // files of one name both kept.
    for p in &paths {
        assert!(p.contains("/attachments/carried/m-carry/"), "{p}");
    }
    assert!(!paths[0].eq_ignore_ascii_case(&paths[1]));
    assert_eq!(std::fs::read(&paths[0]).unwrap(), b"one");
    assert_eq!(std::fs::read(&paths[1]).unwrap(), b"two");
    // A name too long for a disk is cut, keeping its extension.
    assert!(paths[2].ends_with(".png"), "{}", paths[2]);
    assert_eq!(std::fs::read(&paths[2]).unwrap(), b"three");
}

#[test]
fn a_project_is_brought_from_another_machine_with_its_ignored_files() {
    let home = std::env::temp_dir().join(format!("dent-bring-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\n\tname = t\n\temail = t@t\n",
    )
    .unwrap();
    let h = home.display().to_string();
    let git = |dir: &Path, args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("HOME", &h)
            .output()
            .unwrap();
        assert!(
            ok.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&ok.stderr)
        );
    };
    // The machine that has it: a checkout on a branch, pushed to a bare
    // repository, with ignored files a clone won't have.
    let there = home.join("there/app");
    let bare = home.join("app.git");
    std::fs::create_dir_all(&there).unwrap();
    git(&home, &["init", "-q", "--bare", bare.to_str().unwrap()]);
    git(&there, &["init", "-q", "-b", "main"]);
    std::fs::write(
        there.join(".gitignore"),
        ".env*\n!.env.example\nnode_modules/\n",
    )
    .unwrap();
    // Ignored by this checkout alone: the clone won't have the rule.
    std::fs::write(there.join(".git/info/exclude"), "*.bin\n").unwrap();
    std::fs::write(there.join(".env.example"), "KEY=\n").unwrap();
    git(&there, &["add", "."]);
    git(&there, &["commit", "-q", "-m", "first"]);
    git(&there, &["checkout", "-q", "-b", "feature"]);
    git(
        &there,
        &[
            "remote",
            "add",
            "origin",
            &format!("file://{}", bare.display()),
        ],
    );
    git(
        &there,
        &[
            "remote",
            "add",
            "upstream",
            "https://example.com/up/app.git",
        ],
    );
    git(&there, &["push", "-q", "origin", "main", "feature"]);
    std::fs::write(there.join(".env"), "SECRET=1\n").unwrap();
    std::fs::create_dir_all(there.join("web")).unwrap();
    std::fs::write(there.join("web/.env.local"), "B=2\n").unwrap();
    let odd: Vec<u8> = (0..=255u8).rev().collect();
    std::fs::write(there.join("blob.bin"), &odd).unwrap();
    std::fs::create_dir_all(there.join("node_modules/x")).unwrap();
    std::fs::write(there.join("node_modules/x/i.js"), "1").unwrap();

    let w = World::new(&[("HOME", h.as_str())]);
    let mut c = w.client();
    c.subscribe(0);
    let pack = match c
        .call(Request::Pack {
            dir: there.display().to_string(),
        })
        .unwrap()
    {
        Out::Reply {
            reply: dent_protocol::Reply::Pack { pack: Some(p) },
        } => p,
        o => panic!("{o:?}"),
    };
    assert_eq!(pack.name, "app");
    assert_eq!(pack.branch.as_deref(), Some("feature"));
    // A remote that's a path on this machine (file://) is no use elsewhere.
    let names: Vec<_> = pack.remotes.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["upstream"]);
    // Environment files ticked and first; ignored folders not listed.
    let listed: Vec<_> = pack
        .ignored
        .iter()
        .map(|i| (i.path.as_str(), i.carry))
        .collect();
    assert_eq!(
        listed,
        [
            (".env", true),
            ("web/.env.local", true),
            ("blob.bin", false)
        ]
    );
    // All pushed, nothing changed: a clone has everything.
    assert_eq!((pack.unpushed, pack.changed, pack.missing()), (0, 0, None));
    // A commit on no remote and a change not committed: said, since a
    // clone won't have them.
    std::fs::write(
        there.join(".env.example"),
        "KEY=1
",
    )
    .unwrap();
    git(&there, &["commit", "-q", "-am", "here only"]);
    std::fs::write(there.join(".gitignore"), "changed\n").unwrap();
    let again = match c
        .call(Request::Pack {
            dir: there.display().to_string(),
        })
        .unwrap()
    {
        Out::Reply {
            reply: dent_protocol::Reply::Pack { pack: Some(p) },
        } => p,
        o => panic!("{o:?}"),
    };
    assert_eq!((again.unpushed, again.changed), (1, 1));
    assert_eq!(
        again.missing().as_deref(),
        Some(
            "1 commit here isn't pushed, and 1 file has changes not committed: the copy there won't have them"
        )
    );
    git(&there, &["reset", "-q", "--hard", "origin/feature"]);
    // Only files inside the checkout are read.
    assert!(
        c.call(Request::ReadFiles {
            dir: there.display().to_string(),
            paths: vec!["../app.git/HEAD".into()],
        })
        .is_err()
    );
    let files = match c
        .call(Request::ReadFiles {
            dir: there.display().to_string(),
            paths: vec![".env".into(), "web/.env.local".into(), "blob.bin".into()],
        })
        .unwrap()
    {
        Out::Reply {
            reply: dent_protocol::Reply::Carried { files },
        } => files,
        o => panic!("{o:?}"),
    };

    // The machine it goes to (here the same daemon, standing in): the
    // session exists at once and works in the clone once it's made.
    let bring = |files: Vec<dent_protocol::Carried>, url: String| dent_protocol::Bring {
        name: pack.name.clone(),
        remotes: vec![
            dent_protocol::GitRemote {
                name: "origin".into(),
                url,
            },
            pack.remotes[0].clone(),
        ],
        branch: pack.branch.clone(),
        files,
    };
    let create = |c: &mut Client, id: &str, b: dent_protocol::Bring| match c
        .call(Request::Create {
            agent: None,
            cwd: String::new(),
            worktree: None,
            model: None,
            permission: Permission::Full,
            effort: None,
            id: id.into(),
            text: "hello".into(),
            attachments: Vec::new(),
            files: Vec::new(),
            bring: Some(b),
        })
        .unwrap()
    {
        Out::Reply {
            reply: dent_protocol::Reply::Created { session },
        } => session,
        o => panic!("{o:?}"),
    };
    let s = create(
        &mut c,
        "m-bring",
        bring(files, format!("file://{}", bare.display())),
    );
    c.wait(&s, "the turn to end", is_done);
    let notice = c
        .events(&s)
        .into_iter()
        .find_map(|b| match b {
            Body::Notice { text } => Some(text),
            _ => None,
        })
        .unwrap();
    assert!(
        notice.starts_with("Cloned app from origin on feature and brought 3 ignored files"),
        "{notice}"
    );
    let here = home.join("dent/projects/app");
    let cwd = c
        .events(&s)
        .into_iter()
        .find_map(|b| match b {
            Body::Created { cwd, .. } => Some(cwd),
            _ => None,
        })
        .unwrap();
    assert_eq!(cwd, here.display().to_string());
    assert_eq!(std::fs::read(here.join(".env")).unwrap(), b"SECRET=1\n");
    assert_eq!(
        std::fs::read(here.join("web/.env.local")).unwrap(),
        b"B=2\n"
    );
    assert_eq!(std::fs::read(here.join("blob.bin")).unwrap(), odd);
    assert!(!here.join("node_modules").exists());
    // Every file brought stays out of Git, even one only that checkout
    // ignored.
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&here)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&status.stdout), "");
    let remotes = std::process::Command::new("git")
        .args(["remote"])
        .current_dir(&here)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&remotes.stdout),
        "origin\nupstream\n"
    );
    // Its staging is gone, and the project's on the list.
    assert!(!w.state.join("bring").join(&s).exists());
    match c.call(Request::Projects).unwrap() {
        Out::Reply {
            reply: dent_protocol::Reply::Projects { projects },
        } => assert!(projects.iter().any(|p| Path::new(&p.path) == here)),
        o => panic!("{o:?}"),
    }
    // Sent again, the same session; no second clone.
    assert_eq!(
        create(&mut c, "m-bring", bring(Vec::new(), String::new())),
        s
    );
    assert!(!home.join("dent/projects/app-2").exists());

    // One that can't be cloned says why, and what was sent waits.
    let bad = create(
        &mut c,
        "m-bad",
        bring(Vec::new(), "file:///nowhere/at/all".into()),
    );
    c.wait(
        &bad,
        "the failure",
        |b| matches!(b, Body::Notice { text } if text.starts_with("Couldn't bring app here")),
    );
    // It's given back, never sent to an agent in an empty folder.
    c.wait(
        &bad,
        "what was sent comes back",
        |b| matches!(b, Body::Unqueued { id, returned: Some(r) } if id == "m-bad" && r.text == "hello"),
    );
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(c.count(&bad, |b| matches!(b, Body::Delivered { .. })), 0);
    assert_eq!(c.count(&bad, |b| matches!(b, Body::TurnStarted)), 0);
    assert!(home.join("dent/projects/app-2").is_dir());
}

#[test]
fn the_lan_switch_holds_across_a_restart() {
    let mut w = World::new(&[]);
    let lan = |c: &mut Client, r: Request| match c.call(r).unwrap() {
        Out::Reply {
            reply: dent_protocol::Reply::Machine { machine },
        } => machine.lan,
        o => panic!("{o:?}"),
    };
    let mut c = w.client();
    assert!(!lan(&mut c, Request::Machine));
    assert!(lan(&mut c, Request::SetLan { on: true }));
    drop(c);
    w.kill();
    w.start();
    let mut c = w.client();
    assert!(lan(&mut c, Request::Machine), "kept");
    assert!(!lan(&mut c, Request::SetLan { on: false }));
}

#[test]
fn a_sessions_files_are_stat_and_read_by_what_it_wrote() {
    use dent_protocol::{FileRef, FileStat};
    let home = std::env::temp_dir().join(format!("dent-files-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join("Movies")).unwrap();
    std::fs::write(home.join("Movies/take.mov"), b"movie bytes").unwrap();
    let h = home.canonicalize().unwrap().display().to_string();
    let w = World::new(&[("HOME", h.as_str())]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "first");
    c.wait(&s, "the turn to end", is_done);
    let cwd = Path::new(&w.cwd()).canonicalize().unwrap();
    std::fs::write(
        cwd.join("chart.png"),
        b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\x02\x80\0\0\x01\xe0",
    )
    .unwrap();
    std::fs::create_dir_all(cwd.join("out")).unwrap();
    let outside = std::env::temp_dir().join(format!("dent-outside-{}.txt", std::process::id()));
    std::fs::write(&outside, b"outside").unwrap();
    let state = w.state.canonicalize().unwrap();
    std::fs::create_dir_all(state.join("attachments/x")).unwrap();
    std::fs::write(state.join("attachments/x/pic.png"), b"pic").unwrap();
    let r = |p: &str| FileRef {
        session: s.clone(),
        path: p.into(),
        base: None,
    };
    let stats = match c
        .call(Request::Stat {
            files: vec![
                r("chart.png"),
                r("~/Movies/take.mov"),
                r(&outside.display().to_string()),
                r(&state.join("dent.db").display().to_string()),
                r(&state.join("attachments/x/pic.png").display().to_string()),
                r("missing.png"),
                r("out"),
                FileRef {
                    session: "no-such-session".into(),
                    path: "x".into(),
                    base: None,
                },
            ],
        })
        .unwrap()
    {
        Out::Reply {
            reply: dent_protocol::Reply::Stats { stats },
        } => stats,
        o => panic!("{o:?}"),
    };
    assert_eq!(stats.len(), 8);
    match &stats[0] {
        FileStat::File {
            path,
            width,
            height,
            ..
        } => {
            assert_eq!(Path::new(path), cwd.join("chart.png"));
            assert_eq!((*width, *height), (Some(640), Some(480)));
        }
        s => panic!("{s:?}"),
    }
    // `~` is the daemon's home, not the asker's.
    assert!(matches!(&stats[1], FileStat::File { path, .. } if path.starts_with(&h)));
    // Outside the session's folder: readable, by a trusted device.
    assert!(matches!(&stats[2], FileStat::File { .. }), "{:?}", stats[2]);
    // The daemon's own state isn't, but the files it wrote for sessions are.
    assert!(
        matches!(&stats[3], FileStat::Refused { .. }),
        "{:?}",
        stats[3]
    );
    assert!(matches!(&stats[4], FileStat::File { .. }), "{:?}", stats[4]);
    assert!(
        matches!(&stats[5], FileStat::Missing { checked } if Path::new(checked) == Path::new(&w.cwd()).join("missing.png"))
    );
    assert!(matches!(&stats[6], FileStat::Dir { .. }));
    assert!(matches!(&stats[7], FileStat::Unreadable { .. }));

    // A range, pinned to its version; rewritten, it says so.
    let read = |c: &mut Client, p: &str, offset: u64, expect| {
        c.call(Request::Read {
            file: r(p),
            offset,
            len: 5,
            expect,
        })
        .unwrap()
    };
    let stamp = match read(&mut c, "~/Movies/take.mov", 0, None) {
        Out::Reply {
            reply:
                dent_protocol::Reply::Bytes {
                    stamp,
                    total,
                    bytes_b64,
                    ..
                },
        } => {
            assert_eq!(total, 11);
            assert_eq!(dent_protocol::b64::decode(&bytes_b64).unwrap(), b"movie");
            stamp
        }
        o => panic!("{o:?}"),
    };
    match read(&mut c, "~/Movies/take.mov", 6, Some(stamp)) {
        Out::Reply {
            reply: dent_protocol::Reply::Bytes { bytes_b64, .. },
        } => assert_eq!(dent_protocol::b64::decode(&bytes_b64).unwrap(), b"bytes"),
        o => panic!("{o:?}"),
    }
    std::thread::sleep(std::time::Duration::from_millis(5));
    std::fs::write(home.join("Movies/take.mov"), b"other movie").unwrap();
    assert!(matches!(
        read(&mut c, "~/Movies/take.mov", 6, Some(stamp)),
        Out::Reply {
            reply: dent_protocol::Reply::FileGone { changed: true, .. }
        }
    ));
    assert!(matches!(
        read(
            &mut c,
            &state.join("identity.key").display().to_string(),
            0,
            None
        ),
        Out::Reply {
            reply: dent_protocol::Reply::FileGone {
                stat: FileStat::Refused { .. } | FileStat::Missing { .. },
                ..
            }
        }
    ));
    // The viewer's whole-file read follows the same rule.
    assert!(
        c.call(Request::ReadFile {
            session: s.clone(),
            path: outside.display().to_string(),
        })
        .is_ok()
    );
    assert!(
        c.call(Request::ReadFile {
            session: s.clone(),
            path: state.join("dent.db").display().to_string(),
        })
        .is_err()
    );

    // Many reads at once never hold up the rest.
    let big = cwd.join("big.bin");
    std::fs::write(&big, vec![7u8; 8 << 20]).unwrap();
    // More than a client may queue: the rest are told to ask again, and
    // nothing else waits behind them.
    let mut asked = Vec::new();
    for i in 0..100u64 {
        asked.push(c.send_req(Request::Read {
            file: r("big.bin"),
            offset: i * dent_protocol::READ_MAX as u64,
            len: dent_protocol::READ_MAX,
            expect: None,
        }));
    }
    let t = std::time::Instant::now();
    assert!(c.call(Request::Projects).is_ok());
    assert!(
        t.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        t.elapsed()
    );
    let _ = std::fs::remove_file(&outside);
}

#[test]
fn projects_are_made_added_taken_off_and_put_back() {
    // Its own home: another test clears `dent-home-<pid>` while running.
    let home = std::env::temp_dir().join(format!("dent-projects-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let h = home.display().to_string();
    // A Git identity for the first commit, inside the test's own home.
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\n\tname = t\n\temail = t@t\n",
    )
    .unwrap();
    let w = World::new(&[("HOME", h.as_str())]);
    let mut c = w.client();
    let paths = |out: Out| match out {
        Out::Reply {
            reply: dent_protocol::Reply::Projects { projects },
        } => projects.into_iter().map(|p| p.path).collect::<Vec<_>>(),
        o => panic!("{o:?}"),
    };

    // From a name: a repository with a first commit, on the list.
    let made = match c
        .call(Request::NewProject {
            name: "My App".into(),
        })
        .unwrap()
    {
        Out::Reply {
            reply: dent_protocol::Reply::ProjectMade { path, warning },
        } => {
            assert_eq!(warning, None, "{warning:?}");
            path
        }
        o => panic!("{o:?}"),
    };
    assert_eq!(made, format!("{h}/dent/projects/my-app"));
    assert!(Path::new(&made).join(".git").is_dir());
    assert!(paths(c.call(Request::Projects).unwrap()).contains(&made));
    // The same name again gets its own folder.
    match c
        .call(Request::NewProject {
            name: "my app".into(),
        })
        .unwrap()
    {
        Out::Reply {
            reply: dent_protocol::Reply::ProjectMade { path, .. },
        } => assert_eq!(path, format!("{h}/dent/projects/my-app-2")),
        o => panic!("{o:?}"),
    }

    // A typed folder, made on the way.
    let typed = format!("{h}/code/new-thing");
    let list = paths(
        c.call(Request::AddProject {
            path: typed.clone(),
            create: true,
        })
        .unwrap(),
    );
    assert!(list.contains(&typed) && Path::new(&typed).is_dir());
    assert!(
        c.call(Request::AddProject {
            path: format!("{h}/nope"),
            create: false,
        })
        .is_err()
    );

    // Off the list and back; the folder stays.
    let off = paths(
        c.call(Request::HideProject {
            path: typed.clone(),
            hidden: true,
        })
        .unwrap(),
    );
    assert!(!off.contains(&typed) && Path::new(&typed).is_dir());
    let back = paths(
        c.call(Request::HideProject {
            path: typed.clone(),
            hidden: false,
        })
        .unwrap(),
    );
    assert!(back.contains(&typed));

    // Folders for a typed path, `~` kept.
    match c
        .call(Request::Folders {
            partial: "~/dent/projects/my".into(),
        })
        .unwrap()
    {
        Out::Reply {
            reply: dent_protocol::Reply::Folders { parent, folders },
        } => {
            assert_eq!(parent, "~/dent/projects");
            assert_eq!(
                folders,
                ["~/dent/projects/my-app", "~/dent/projects/my-app-2"]
            );
        }
        o => panic!("{o:?}"),
    }
    // A clone of a local repository is refused: remote URLs only.
    assert!(c.call(Request::CloneProject { url: w.cwd() }).is_err());
    drop(w);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn approvals_allow_deny_and_questions() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "please run it");
    let id = c.asked(&s);
    c.pump_for(Duration::from_millis(100));
    assert_eq!(c.status(&s), Some(Status::NeedsYou));
    // A session waiting on you can't be closed.
    assert!(c.call(Request::Close { session: s.clone() }).is_err());
    c.call(Request::Answer {
        session: s.clone(),
        id: id.clone(),
        answer: Answer::Allow {
            always: false,
            project: false,
        },
    })
    .unwrap();
    // Answering twice is refused, never sent twice.
    assert!(
        c.call(Request::Answer {
            session: s.clone(),
            id,
            answer: Answer::Allow {
                always: false,
                project: false,
            },
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
    c.pump_for(Duration::from_millis(300));
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
    c.say(&s, "st", "actually do the steer thing", true);
    // Wait for the steered reply itself: on a busy machine the turn count
    // can reach three before it arrives.
    c.wait(&s, "the steered reply", |b| {
        matches!(b, Body::Assistant { parent: None, blocks, .. }
            if blocks.iter().any(|x| matches!(x, dent_protocol::Block::Text { text } if text.contains("steer thing"))))
    });
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
    assert!(c.text(&s).contains("steer thing"), "{}", c.text(&s));
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
    c.call(Request::Interrupt {
        session: s.clone(),
        by: None,
    })
    .unwrap();
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
        c.count(
            &s,
            |b| matches!(b, Body::Unqueued { id, .. } if id == "held")
        ),
        1
    );
    // Stopped before anything came back: the first message goes back too,
    // and the session is as it was before it.
    let first = c
        .events(&s)
        .iter()
        .find_map(|b| match b {
            Body::User { id, .. } => Some(id.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        c.count(
            &s,
            |b| matches!(b, Body::Withdrawn { ids } if *ids == [first.clone()])
        ),
        1
    );
    c.pump_for(Duration::from_millis(500));
    // Nothing left of it: stopped, not a session still starting.
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
    c.pump_for(Duration::from_millis(300));
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
    c.pump_for(Duration::from_millis(100));
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
    // Quiet, as an agent deep in a long command is: nothing it writes
    // fails to make it notice the holder has gone.
    let s = c.create(&w.cwd(), Permission::Ask, "work quietly please");
    c.wait(&s, "the turn to start", |b| matches!(b, Body::TurnStarted));
    let agents = w.agent_pids();
    assert_eq!(agents.len(), 1);
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
    // The agent the holder left running is stopped: it would go on
    // working beside the one that carries on.
    let until = Instant::now() + Duration::from_secs(5);
    // SAFETY: kill with signal 0 only asks whether the process is there.
    while unsafe { libc::kill(agents[0], 0) } == 0 {
        assert!(Instant::now() < until, "the old agent is still running");
        std::thread::sleep(Duration::from_millis(100));
    }
    c.say(&s, "2", "short", false);
    c.wait_turns(&s, 1);
}

/// Messages a session sent itself to carry on after being cut off.
fn carry_ons(c: &Client, s: &str) -> usize {
    c.count(
        s,
        |b| matches!(b, Body::User { text, .. } if text.starts_with("You were cut off")),
    )
}

fn interruptions(c: &Client, s: &str) -> usize {
    c.count(s, |b| {
        matches!(
            b,
            Body::TurnEnded {
                outcome: Outcome::Interrupted { .. }
            }
        )
    })
}

fn kill_holder(w: &World) {
    let pid = w.holder_pid().expect("holder running");
    // SAFETY: kill has no memory preconditions.
    unsafe { libc::kill(pid, libc::SIGKILL) };
}

#[test]
fn a_turn_a_reboot_cut_off_carries_on_by_itself_once() {
    let mut w = World::new(&[("FAKE_CLAUDE_SPEED", "0.5")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Full, "a long answer please");
    c.wait(&s, "the turn to start", |b| matches!(b, Body::TurnStarted));
    // The machine goes down: the daemon and everything it held.
    w.kill();
    kill_holder(&w);
    drop(c);
    w.start();
    let mut c = w.client();
    c.subscribe(0);
    c.wait(&s, "the interruption", |b| {
        matches!(
            b,
            Body::TurnEnded {
                outcome: Outcome::Interrupted { .. }
            }
        )
    });
    // Down again before it carried on: the next daemon still does, once.
    w.kill();
    drop(c);
    w.start();
    let mut c = w.client();
    c.subscribe(0);
    c.wait(
        &s,
        "carrying on",
        |b| matches!(b, Body::User { text, .. } if text.starts_with("You were cut off")),
    );
    c.wait_turns(&s, 1);
    c.pump_for(Duration::from_secs(6));
    assert_eq!(carry_ons(&c, &s), 1);
    assert_eq!(interruptions(&c, &s), 1);
}

#[test]
fn a_turn_waiting_on_you_or_answered_first_never_carries_on_by_itself() {
    let w = World::new(&[("FAKE_CLAUDE_SPEED", "0.5")]);
    let mut c = w.client();
    c.subscribe(0);
    // Waiting on an approval when the holder dies: yours to carry on.
    let s = c.create(&w.cwd(), Permission::Ask, "write notes.md please");
    c.asked(&s);
    kill_holder(&w);
    c.wait(&s, "the interruption", |b| {
        matches!(
            b,
            Body::TurnEnded {
                outcome: Outcome::Interrupted { .. }
            }
        )
    });
    c.pump_for(Duration::from_secs(7));
    assert_eq!(carry_ons(&c, &s), 0);
    // Cut off mid-reply, but you write first: yours goes, and only yours.
    let t = c.create(&w.cwd(), Permission::Full, "a long answer please");
    c.wait(&t, "the turn to start", |b| matches!(b, Body::TurnStarted));
    kill_holder(&w);
    c.wait(&t, "the interruption", |b| {
        matches!(
            b,
            Body::TurnEnded {
                outcome: Outcome::Interrupted { .. }
            }
        )
    });
    c.say(&t, "mine", "short", false);
    c.wait_turns(&t, 1);
    c.pump_for(Duration::from_secs(6));
    assert_eq!(carry_ons(&c, &t), 0);
    // Cut off, then you press Stop: it stays stopped.
    let u = c.create(&w.cwd(), Permission::Full, "one more long answer please");
    c.wait(&u, "the turn to start", |b| matches!(b, Body::TurnStarted));
    kill_holder(&w);
    c.wait(&u, "the interruption", |b| {
        matches!(
            b,
            Body::TurnEnded {
                outcome: Outcome::Interrupted { .. }
            }
        )
    });
    c.call(Request::Interrupt {
        session: u.clone(),
        by: None,
    })
    .unwrap();
    c.pump_for(Duration::from_secs(7));
    assert_eq!(carry_ons(&c, &u), 0);
}

#[test]
fn cut_off_again_and_again_it_stops_carrying_on_and_waits() {
    let w = World::new(&[("FAKE_CLAUDE_SPEED", "3")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Full, "a long answer please");
    for n in 1..=4 {
        let started = n;
        let until = Instant::now() + Duration::from_secs(15);
        while c.count(&s, |b| matches!(b, Body::TurnStarted)) < started {
            assert!(Instant::now() < until, "turn {n} never started");
            c.pump(Duration::from_millis(50));
        }
        kill_holder(&w);
        while interruptions(&c, &s) < n {
            assert!(Instant::now() < until, "cut {n} never noticed");
            c.pump(Duration::from_millis(50));
        }
    }
    c.wait(
        &s,
        "it giving up",
        |b| matches!(b, Body::Notice { text } if text.starts_with("Cut off 3 times")),
    );
    c.pump_for(Duration::from_secs(7));
    assert_eq!(carry_ons(&c, &s), 3);
}

#[test]
fn changes_files_and_settings() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Edits, "write notes.md please");
    c.wait_turns(&s, 1);
    assert_eq!(c.count(&s, |b| matches!(b, Body::Asked { .. })), 0);
    match c
        .call(Request::Changes {
            session: s.clone(),
            turn: None,
        })
        .unwrap()
    {
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
    c.pump_for(Duration::from_millis(300));
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
    match c.call(Request::Usage { agent: None }).unwrap() {
        Out::Reply {
            reply: dent_protocol::Reply::Usage { windows, .. },
        } => {
            assert_eq!(windows.len(), 2, "{windows:?}");
            assert!((windows[0].used - 0.18).abs() < 1e-9);
        }
        other => panic!("{other:?}"),
    }
    match c.call(Request::Models { agent: None }).unwrap() {
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
    c.wait(&s, "background work", |b| {
        matches!(
            b,
            Body::Status {
                status: Status::Background
            }
        )
    });
    c.wait_turns(&s, 2);
    c.pump_for(Duration::from_millis(100));
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
    // And the holder itself goes, with nothing left to hand over.
    // SAFETY: kill with signal 0 only checks that the process exists.
    let alive = |p: i32| unsafe { libc::kill(p, 0) } == 0;
    let until = Instant::now() + Duration::from_secs(10);
    while w.holder_pid().is_some_and(alive) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !w.holder_pid().is_some_and(alive),
        "the holder should exit once its finished output has nobody to collect it"
    );
}

#[test]
fn rewinding_restores_files_and_picks_the_conversation_up_there() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Edits, "write notes.md please");
    c.wait_turns(&s, 1);
    c.say(&s, "second", "write other.md please", false);
    c.wait_turns(&s, 2);
    let proj = std::path::Path::new(&w.cwd()).to_path_buf();
    assert!(proj.join("other.md").exists());
    assert_eq!(c.count(&s, |b| matches!(b, Body::Checkpoint { .. })), 2);

    // Going back while it works is refused.
    c.say(&s, "long", "a long answer please", false);
    c.wait(
        &s,
        "the long turn",
        |b| matches!(b, Body::User { id, .. } if id == "long"),
    );
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        c.call(Request::Rewind {
            session: s.clone(),
            to: "second".into(),
            files: true,
        })
        .is_err()
    );
    c.call(Request::Interrupt {
        session: s.clone(),
        by: None,
    })
    .unwrap();
    c.wait(&s, "stopped", |b| {
        matches!(
            b,
            Body::TurnEnded {
                outcome: Outcome::Interrupted { .. }
            }
        )
    });
    c.pump(Duration::from_millis(200));

    c.call(Request::Rewind {
        session: s.clone(),
        to: "second".into(),
        files: true,
    })
    .unwrap();
    c.wait(&s, "the rewind", |b| matches!(b, Body::Rewound { .. }));
    assert!(proj.join("notes.md").exists(), "files from before stay");
    assert!(!proj.join("other.md").exists(), "files made after go");

    // The next process picks the conversation up at the reply before it.
    c.say(&s, "args", "tell me your args", false);
    c.wait_turns(&s, 3);
    let t = c.text(&s);
    assert!(
        t.contains("--resume-session-at") && t.contains("--fork-session"),
        "{t}"
    );

    // Back to the very first message: a fresh conversation.
    c.call(Request::Rewind {
        session: s.clone(),
        to: "m-write notes.md please".into(),
        files: false,
    })
    .unwrap();
    c.say(&s, "args2", "tell me your args again", false);
    c.wait_turns(&s, 4);
    let last = c.text(&s);
    let last = last.lines().last().unwrap_or_default();
    assert!(
        last.contains("--session-id") && !last.contains("--resume"),
        "{last}"
    );
}

#[test]
fn effort_reaches_the_agent_and_changing_it_starts_a_new_process() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = match c.call(Request::Create {
        agent: None,
        cwd: w.cwd(),
        worktree: None,
        model: None,
        permission: Permission::Ask,
        effort: Some("low".into()),
        id: "e1".into(),
        text: "tell me your args".into(),
        attachments: Vec::new(),
        files: Vec::new(),
        bring: None,
    }) {
        Ok(Out::Reply {
            reply: dent_protocol::Reply::Created { session },
        }) => session,
        other => panic!("{other:?}"),
    };
    c.wait_turns(&s, 1);
    assert!(c.text(&s).contains("--effort low"), "{}", c.text(&s));
    c.call(Request::SetEffort {
        session: s.clone(),
        effort: "max".into(),
    })
    .unwrap();
    c.say(&s, "e2", "your args again", false);
    c.wait_turns(&s, 2);
    let last = c.text(&s);
    let last = last.lines().last().unwrap_or_default();
    assert!(
        last.contains("--effort max") && last.contains("--resume"),
        "{last}"
    );
}

#[test]
fn restarting_the_agent_resumes_the_conversation_in_a_new_process() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "tell me your args");
    c.wait_turns(&s, 1);
    assert!(!c.text(&s).contains("--resume"), "{}", c.text(&s));
    c.call(Request::RestartAgent { session: s.clone() })
        .unwrap();
    c.wait(
        &s,
        "the notice",
        |b| matches!(b, Body::Notice { text } if text.contains("starts again")),
    );
    c.say(&s, "r2", "your args again", false);
    c.wait_turns(&s, 2);
    let last = c.text(&s);
    let last = last.lines().last().unwrap_or_default();
    assert!(last.contains("--resume"), "{last}");
}

#[test]
fn a_fork_copies_the_conversation_and_resumes_it_as_a_new_agent_session() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "hello there");
    c.wait_turns(&s, 1);
    c.say(&s, "two", "and a second thing", false);
    c.wait_turns(&s, 2);
    let parent_agent = c
        .events(&s)
        .iter()
        .find_map(|b| match b {
            Body::Agent {
                agent_session: Some(a),
                ..
            } => Some(a.clone()),
            _ => None,
        })
        .unwrap();

    // Everything so far.
    let f = match c.call(Request::Fork {
        until: None,
        id: None,
        session: s.clone(),
        before: None,
    }) {
        Ok(Out::Reply {
            reply: dent_protocol::Reply::Created { session },
        }) => session,
        other => panic!("{other:?}"),
    };
    c.wait(&f, "the copy", |b| matches!(b, Body::Forked { .. }));
    assert_eq!(c.count(&f, |b| matches!(b, Body::User { .. })), 2);
    assert!(c.text(&f).contains("hello there"));
    c.say(&f, "fa", "your args please", false);
    c.wait_turns(&f, 3);
    let args = c.text(&f);
    let args = args.lines().last().unwrap_or_default();
    assert!(
        args.contains(&format!("--resume {parent_agent}")) && args.contains("--fork-session"),
        "{args}"
    );
    // The original is untouched.
    assert_eq!(c.count(&s, |b| matches!(b, Body::User { .. })), 2);

    // Up to just before the second message.
    let g = match c.call(Request::Fork {
        until: None,
        id: None,
        session: s.clone(),
        before: Some("two".into()),
    }) {
        Ok(Out::Reply {
            reply: dent_protocol::Reply::Created { session },
        }) => session,
        other => panic!("{other:?}"),
    };
    c.wait(&g, "the shorter copy", |b| matches!(b, Body::Forked { .. }));
    assert_eq!(c.count(&g, |b| matches!(b, Body::User { .. })), 1);
}

#[test]
fn terminal_sessions_are_listed_and_carried_on_with_their_history() {
    let home = std::env::temp_dir().join(format!("dent-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let w0 = World::new(&[]);
    let cwd = w0.cwd();
    drop(w0);
    std::fs::create_dir_all(&cwd).unwrap();
    let dir = home.join(".claude/projects/-some-project");
    std::fs::create_dir_all(&dir).unwrap();
    let id = "11111111-2222-4333-8444-555555555555";
    let lines = [
        format!(r#"{{"type":"user","entrypoint":"cli","cwd":"{cwd}","uuid":"u1","message":{{"role":"user","content":"Make the parser faster"}}}}"#),
        r#"{"type":"assistant","uuid":"a1","message":{"content":[{"type":"text","text":"I made it faster."}]}}"#.to_string(),
        r#"{"type":"user","uuid":"m1","isMeta":true,"message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#.to_string(),
        r#"{"type":"last-prompt","lastPrompt":"Make the parser faster"}"#.to_string(),
    ];
    let file = dir.join(format!("{id}.jsonl"));
    std::fs::write(&file, lines.join("\n") + "\n").unwrap();
    // Not touched in the last minute, so not open in a terminal.
    Command::new("touch")
        .args(["-t", "202001010000"])
        .arg(&file)
        .status()
        .unwrap();

    let w = World::new(&[("HOME", home.to_str().unwrap())]);
    let mut c = w.client();
    c.subscribe(0);
    let listed = match c.call(Request::Recent).unwrap() {
        Out::Reply {
            reply: dent_protocol::Reply::Recent { sessions },
        } => sessions,
        other => panic!("{other:?}"),
    };
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].title, "Make the parser faster");
    let s = match c.call(Request::Adopt {
        agent: None,
        agent_session: id.into(),
    }) {
        Ok(Out::Reply {
            reply: dent_protocol::Reply::Created { session },
        }) => session,
        other => panic!("{other:?}"),
    };
    c.wait(&s, "the import", |b| matches!(b, Body::Status { .. }));
    assert!(c.text(&s).contains("I made it faster."));
    assert_eq!(
        c.count(&s, |b| matches!(b, Body::User { .. })),
        1,
        "meta prompts stay out"
    );
    // Listed no more, and adopting twice is refused.
    match c.call(Request::Recent).unwrap() {
        Out::Reply {
            reply: dent_protocol::Reply::Recent { sessions },
        } => assert!(sessions.is_empty()),
        other => panic!("{other:?}"),
    }
    assert!(
        c.call(Request::Adopt {
            agent: None,
            agent_session: id.into()
        })
        .is_err()
    );
    c.say(&s, "next", "tell me your args", false);
    c.wait_turns(&s, 2);
    let t = c.text(&s);
    assert!(t.contains(&format!("--resume {id}")), "{t}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn requests_dent_doesnt_handle_are_declined_so_the_agent_never_waits() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "elicit a project");
    c.wait(&s, "the turn to end", is_done);
    assert!(
        c.text(&s).contains("The tickets server got: decline"),
        "{}",
        c.text(&s)
    );
    assert_eq!(
        c.count(
            &s,
            |b| matches!(b, Body::Notice { text } if text.starts_with("tickets asked"))
        ),
        1
    );
}

#[test]
fn a_conversation_the_agent_forgot_carries_on_in_a_new_one() {
    let w = World::new(&[("DENT_IDLE_MS", "300"), ("FAKE_CLAUDE_FORGET", "1")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "hi");
    c.wait_turns(&s, 1);
    // The agent stops when idle, so the next message resumes, and the
    // resume finds nothing.
    std::thread::sleep(Duration::from_millis(900));
    c.say(&s, "2", "tell me your args", false);
    c.wait_turns(&s, 2);
    let text = c.text(&s);
    let last = text.lines().last().unwrap_or_default();
    assert!(
        last.contains("--session-id") && !last.contains("--resume"),
        "{last}"
    );
    assert!(
        c.events(&s)
            .iter()
            .any(|b| matches!(b, Body::Notice { text } if text.contains("started a new one")))
    );
    assert_eq!(c.count(&s, |b| matches!(b, Body::TurnEnded { .. })), 2);
    assert_eq!(c.count(&s, is_done), 2);
}

/// The conversation the agent lost goes into its new one as text, from
/// dent's own record: it starts knowing what was said.
#[test]
fn a_lost_conversation_is_given_back_as_text() {
    let w = World::new(&[("DENT_IDLE_MS", "300"), ("FAKE_CLAUDE_FORGET", "1")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "say: the plan is ready");
    c.wait_turns(&s, 1);
    std::thread::sleep(Duration::from_millis(900));
    c.say(&s, "2", "recall what I said", false);
    c.wait_turns(&s, 2);
    let text = c.text(&s);
    let last = text.lines().last().unwrap_or_default();
    // The first message and its reply, not the message being asked now.
    assert!(last.contains("Given back: 1 messages."), "{last}");
    assert!(c.events(&s).iter().any(
        |b| matches!(b, Body::Notice { text } if text.contains("given what was said so far as text"))
    ));
}

#[test]
fn an_agent_stuck_starting_ends_the_turn_with_why() {
    let w = World::new(&[("DENT_START_MS", "500"), ("FAKE_CLAUDE_HANG", "1")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "hi");
    c.wait(&s, "the turn to fail", |b| {
        matches!(b, Body::TurnEnded { outcome: Outcome::Error { message } }
            if message.starts_with("Claude Code didn't start") && message.contains("waiting for a lock"))
    });
}

#[test]
fn a_side_question_stays_out_of_the_conversation_and_can_become_a_fork() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "hi");
    c.wait_turns(&s, 1);
    let turns = c.count(&s, |b| matches!(b, Body::TurnStarted));
    c.call(Request::Aside {
        session: s.clone(),
        id: "q1".into(),
        text: "what does main do?".into(),
    })
    .unwrap();
    c.wait(&s, "the side answer", |b| {
        matches!(b, Body::AsideReply { error: false, .. })
    });
    let (text, agent) = c
        .events(&s)
        .into_iter()
        .find_map(|b| match b {
            Body::AsideReply {
                text,
                agent_session,
                ..
            } => Some((text, agent_session)),
            _ => None,
        })
        .unwrap();
    assert!(text.contains("what does main do?"), "{text}");
    let agent = agent.expect("the forked conversation's id");
    // The conversation itself didn't move.
    assert_eq!(c.count(&s, |b| matches!(b, Body::TurnStarted)), turns);
    assert!(!c.text(&s).contains("what does main do?"));

    let fork = match c.call(Request::PromoteAside {
        session: s.clone(),
        id: "q1".into(),
    }) {
        Ok(Out::Reply {
            reply: dent_protocol::Reply::Created { session },
        }) => session,
        other => panic!("{other:?}"),
    };
    c.wait(&fork, "the fork", |b| matches!(b, Body::Status { .. }));
    assert!(c.text(&fork).contains("what does main do?"));
    let before = c.turns_done(&fork);
    c.say(&fork, "f1", "tell me your args", false);
    c.wait_turns(&fork, before + 1);
    let last = c.text(&fork);
    let last = last.lines().last().unwrap_or_default();
    assert!(
        last.contains(&format!("--resume {agent}")) && !last.contains("--fork-session"),
        "{last}"
    );
}

#[test]
fn a_side_question_about_no_session_is_refused() {
    let w = World::new(&[]);
    let mut c = w.client();
    let refused = matches!(
        c.call(Request::Aside {
            session: "nope".into(),
            id: "q".into(),
            text: "?".into(),
        }),
        Ok(Out::Error { .. }) | Err(_)
    );
    assert!(refused);
}

#[test]
fn commands_compaction_retries_and_dialogs_arrive_by_kind() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    // A terminal-only command and an unknown one say which they are.
    let s = c.create(&w.cwd(), Permission::Ask, "/theme");
    c.wait(&s, "the refusal", |b| {
        matches!(b, Body::Command { name, outcome: dent_protocol::CommandOutcome::TerminalOnly, .. } if name == "theme")
    });
    c.wait_turns(&s, 1);
    c.say(&s, "m2", "/fooo", false);
    c.wait(&s, "the suggestion", |b| {
        matches!(b, Body::Command { name, outcome: dent_protocol::CommandOutcome::Unknown { suggestion: Some(x) }, .. }
            if name == "fooo" && x == "compact")
    });
    c.wait_turns(&s, 2);
    // Compacting: the marker with what it freed, and the new count after.
    c.say(&s, "m3", "/compact", false);
    c.wait(&s, "the marker", |b| {
        matches!(b, Body::Compacted { auto: false, before, after: Some(2_100) } if *before > 4_200)
    });
    c.wait_turns(&s, 3);
    c.wait(&s, "the new count", |b| {
        matches!(b, Body::Context { used: 4_200, .. })
    });
    // Usage comes as labelled bars.
    c.say(&s, "m4", "/usage", false);
    c.wait(&s, "the limits", |b| {
        matches!(b, Body::Command { view: Some(dent_protocol::View::Limits { bars }), .. }
            if bars.len() == 3 && bars[2].label == "This week (Fable)")
    });
    c.wait_turns(&s, 4);
    // Claude Code's own question is answered like the agent's.
    c.say(&s, "m5", "declined", false);
    let id = c.asked(&s);
    c.call(Request::Answer {
        session: s.clone(),
        id,
        answer: Answer::Choose {
            answers: vec!["Try with Opus 5.5".into()],
        },
    })
    .unwrap();
    c.wait_turns(&s, 5);
    assert!(
        c.text(&s).contains("Answered with Opus 5.5"),
        "{}",
        c.text(&s)
    );
    // Commands carry what they take.
    let commands = match c.call(Request::Commands {
        session: Some(s.clone()),
    }) {
        Ok(Out::Reply {
            reply: dent_protocol::Reply::Commands { commands },
        }) => commands,
        other => panic!("{other:?}"),
    };
    let args = |n: &str| {
        commands
            .iter()
            .find(|c| c.name == n)
            .map(|c| c.args.clone())
    };
    assert!(matches!(
        args("autocompact"),
        Some(dent_protocol::Args::Choice { optional: true, .. })
    ));
    match args("output-style") {
        Some(dent_protocol::Args::Choice { options, .. }) => {
            assert_eq!(options.len(), 4);
            assert!(options[0].current);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn clients_joining_mid_reply_see_it_once_and_events_never_skip() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    // Two sessions writing at once: the feed still carries every seq, in
    // order.
    let a = c.create(&w.cwd(), Permission::Ask, "a long answer please");
    let b = c.create(&w.cwd(), Permission::Ask, "another long answer please");
    c.wait(&a, "the turn to start", |x| matches!(x, Body::TurnStarted));
    // Clients joining while the reply streams get what streamed so far
    // once, then the rest.
    let mut late: Vec<Client> = (0..6)
        .map(|_| {
            std::thread::sleep(Duration::from_millis(15));
            let mut l = w.client();
            l.subscribe(0);
            l
        })
        .collect();
    c.wait_turns(&a, 1);
    c.wait_turns(&b, 1);
    let want = c.text(&a);
    for l in &mut late {
        l.wait_turns(&a, 1);
        // Under load a client can join after the reply finished: then it
        // streamed nothing, and has the reply whole from the log instead.
        let streamed = l.partial.get(&a).cloned().unwrap_or_default();
        if streamed.is_empty() {
            assert_eq!(l.text(&a), want);
        } else {
            assert_eq!(streamed, want);
        }
    }
    let seqs: Vec<u64> = c.events.lock().unwrap().iter().map(|e| e.seq).collect();
    assert!(
        seqs.windows(2).all(|p| p[1] == p[0] + 1),
        "gaps in {seqs:?}"
    );
}

#[test]
fn a_session_whose_folder_was_deleted_says_so() {
    let w = World::new(&[("DENT_IDLE_MS", "200")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "hi");
    c.wait_turns(&s, 1);
    // Let the agent stop, then take its folder away.
    std::thread::sleep(Duration::from_millis(800));
    std::fs::remove_dir_all(&w.project).unwrap();
    let _ = c.call(Request::Send {
        session: s.clone(),
        id: "2".into(),
        text: "hi again".into(),
        steer: false,
        attachments: Vec::new(),
        files: Vec::new(),
    });
    c.wait(
        &s,
        "the reason",
        |b| matches!(b, Body::Notice { text } if text.contains("no longer exists")),
    );
}

#[test]
fn always_allow_for_the_project_survives_a_daemon_restart() {
    let mut w = World::new(&[]);
    let s = {
        let mut c = w.client();
        c.subscribe(0);
        let s = c.create(&w.cwd(), Permission::Ask, "please run it");
        c.asked(&s);
        s
    };
    w.kill();
    w.start();
    let mut c = w.client();
    c.subscribe(0);
    let id = c.asked(&s);
    c.call(Request::Answer {
        session: s.clone(),
        id,
        answer: Answer::Allow {
            always: true,
            project: true,
        },
    })
    .unwrap();
    c.wait_turns(&s, 1);
    // The rule the agent offered, kept in the project: not all of Bash,
    // and not only for this session.
    let saved =
        std::fs::read_to_string(w.project.join(".claude/settings.local.json")).unwrap_or_default();
    assert!(
        saved.contains("Bash(") && !saved.contains("Bash()"),
        "{saved}"
    );
}

#[test]
fn background_work_outlives_a_setting_change_and_ends_with_its_agent() {
    // Slow enough (1.5 s of background work) to act while it runs.
    let w = World::new(&[("FAKE_CLAUDE_SPEED", "1")]);
    let mut c = w.client();
    c.subscribe(0);
    let background = |b: &Body| {
        matches!(
            b,
            Body::Status {
                status: Status::Background
            }
        )
    };
    // Changing effort waits for the background work instead of stopping it.
    let s = c.create(&w.cwd(), Permission::Full, "start the background job");
    c.wait(&s, "the background work", background);
    c.call(Request::SetEffort {
        session: s.clone(),
        effort: "max".into(),
    })
    .unwrap();
    c.wait_turns(&s, 2);
    assert!(c.text(&s).contains("finished cleanly"));
    c.pump_for(Duration::from_millis(100));
    assert_eq!(c.status(&s), Some(Status::Done));

    // When the agent goes (here for a rewind), so does its background work.
    // (Its own first message: the same id would be the same session.)
    let t = c.create(&w.cwd(), Permission::Full, "start another background job");
    c.wait(&t, "the background work", background);
    c.call(Request::Rewind {
        session: t.clone(),
        to: "m-start another background job".into(),
        files: false,
    })
    .unwrap();
    c.wait(
        &t,
        "the work to be let go",
        |b| matches!(b, Body::Notice { text } if text.starts_with("Background work stopped")),
    );
    let until = Instant::now() + Duration::from_secs(10);
    while c.status(&t) == Some(Status::Background) && Instant::now() < until {
        c.pump_for(Duration::from_millis(50));
    }
    assert_eq!(c.status(&t), Some(Status::Done), "{:#?}", c.events(&t));
}

#[test]
fn a_message_sent_as_an_idle_agent_winds_down_still_runs() {
    let w = World::new(&[("DENT_IDLE_MS", "200"), ("FAKE_CLAUDE_LINGER_MS", "2000")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "hi");
    c.wait_turns(&s, 1);
    // Its input is closed and it hasn't exited yet.
    std::thread::sleep(Duration::from_millis(600));
    c.say(&s, "2", "hi again", false);
    c.wait_turns(&s, 2);
    // And a model change in that window is taken by the next agent.
    std::thread::sleep(Duration::from_millis(600));
    c.call(Request::SetModel {
        session: s.clone(),
        model: "opus".into(),
    })
    .unwrap();
    c.wait(
        &s,
        "the model",
        |b| matches!(b, Body::Settings { model: Some(m), .. } if m == "opus"),
    );
}

#[test]
fn a_setting_the_agent_refuses_is_not_kept() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "hi");
    c.wait_turns(&s, 1);
    c.call(Request::SetModel {
        session: s.clone(),
        model: "unavailable-model".into(),
    })
    .unwrap();
    c.wait(
        &s,
        "the refusal",
        |b| matches!(b, Body::Notice { text } if text.contains("not available")),
    );
    c.call(Request::SetModel {
        session: s.clone(),
        model: "haiku".into(),
    })
    .unwrap();
    c.wait(
        &s,
        "the model",
        |b| matches!(b, Body::Settings { model: Some(m), .. } if m == "haiku"),
    );
    assert_eq!(
        c.count(
            &s,
            |b| matches!(b, Body::Settings { model: Some(m), .. } if m.starts_with("unavailable"))
        ),
        0
    );
}

#[test]
fn a_stop_or_a_stuck_start_is_seen_through_across_a_daemon_restart() {
    // Stop asked, the agent ignoring it, and the daemon restarting before
    // the grace is up: the turn still ends.
    let mut w = World::new(&[("FAKE_CLAUDE_SPEED", "0.5"), ("FAKE_CLAUDE_DEAF", "1")]);
    let s = {
        let mut c = w.client();
        c.subscribe(0);
        // Slowly: the answer outlasts the stop's grace after the restart
        // (at full length it takes about as long, and the two raced).
        let s = c.create(&w.cwd(), Permission::Ask, "a long answer, slowly please");
        c.wait(&s, "the turn to start", |b| matches!(b, Body::TurnStarted));
        c.call(Request::Interrupt {
            session: s.clone(),
            by: None,
        })
        .unwrap();
        s
    };
    w.kill();
    w.start();
    let mut c = w.client();
    c.subscribe(0);
    c.wait(&s, "the interruption", |b| {
        matches!(
            b,
            Body::TurnEnded {
                outcome: Outcome::Interrupted { .. }
            }
        )
    });
    drop(c);
    drop(w);

    // An agent stuck starting when the daemon restarts still times out.
    let mut w = World::new(&[("DENT_START_MS", "1000"), ("FAKE_CLAUDE_HANG", "1")]);
    let s = {
        let mut c = w.client();
        c.subscribe(0);
        c.create(&w.cwd(), Permission::Ask, "hi")
    };
    w.kill();
    w.start();
    let mut c = w.client();
    c.subscribe(0);
    c.wait(&s, "the turn to fail", |b| {
        matches!(b, Body::TurnEnded { outcome: Outcome::Error { message } }
            if message.starts_with("Claude Code didn't start"))
    });
}

#[test]
fn resent_creates_forks_and_rewinds_happen_once() {
    let mut w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let create = Request::Create {
        agent: None,
        cwd: w.cwd(),
        worktree: None,
        model: None,
        permission: Permission::Edits,
        effort: None,
        id: "first".into(),
        text: "hi".into(),
        attachments: Vec::new(),
        files: Vec::new(),
        bring: None,
    };
    let made = |o: Result<Out, String>| match o {
        Ok(Out::Reply {
            reply: dent_protocol::Reply::Created { session },
        }) => session,
        other => panic!("{other:?}"),
    };
    let s = made(c.call(create.clone()));
    c.wait_turns(&s, 1);
    c.say(&s, "second", "write notes.md please", false);
    c.wait_turns(&s, 2);
    let fork = Request::Fork {
        until: None,
        session: s.clone(),
        before: None,
        id: Some("f1".into()),
    };
    let f = made(c.call(fork.clone()));
    // A restart in between: the same requests again.
    drop(c);
    w.kill();
    w.start();
    let mut c = w.client();
    c.subscribe(0);
    assert_eq!(made(c.call(create)), s);
    assert_eq!(made(c.call(fork)), f);
    c.pump_for(Duration::from_millis(300));
    assert_eq!(c.count(&s, |b| matches!(b, Body::Created { .. })), 1);
    assert_eq!(
        c.count(&s, |b| matches!(b, Body::User { id, .. } if id == "first")),
        1
    );
    let sessions = c
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e.body, Body::Created { .. }))
        .count();
    assert_eq!(sessions, 2);

    // Rewound once, files and all; a file changed by hand since stays.
    let rewind = Request::Rewind {
        session: s.clone(),
        to: "second".into(),
        files: true,
    };
    c.call(rewind.clone()).unwrap();
    assert!(!w.project.join("notes.md").exists());
    std::fs::write(w.project.join("README.md"), "mine\n").unwrap();
    c.call(rewind).unwrap();
    assert_eq!(
        std::fs::read_to_string(w.project.join("README.md")).unwrap(),
        "mine\n"
    );
    c.pump_for(Duration::from_millis(100));
    assert_eq!(c.count(&s, |b| matches!(b, Body::Rewound { .. })), 1);
}

#[test]
fn messages_sent_while_a_new_worktree_is_set_up_wait_and_go_in_order() {
    // Worktrees go under HOME: a scratch one, not yours.
    let home = std::env::temp_dir().join(format!("dent-th{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let w = World::new(&[("HOME", home.to_str().unwrap())]);
    std::fs::create_dir_all(w.project.join(".dent")).unwrap();
    std::fs::write(
        w.project.join(".dent/setup"),
        "sleep 1\necho ok > ready.txt\n",
    )
    .unwrap();
    for args in [
        &["add", ".dent/setup"][..],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "setup",
        ][..],
    ] {
        Command::new("git")
            .args(args)
            .current_dir(&w.project)
            .status()
            .unwrap();
    }
    let mut c = w.client();
    c.subscribe(0);
    let s = match c.call(Request::Create {
        agent: None,
        cwd: w.cwd(),
        worktree: Some(dent_protocol::NewWorktree::default()),
        model: None,
        permission: Permission::Full,
        effort: None,
        id: "w1".into(),
        text: "first".into(),
        attachments: Vec::new(),
        files: Vec::new(),
        bring: None,
    }) {
        Ok(Out::Reply {
            reply: dent_protocol::Reply::Created { session },
        }) => session,
        other => panic!("{other:?}"),
    };
    // Sent while setup still runs: it waits behind the first.
    c.say(&s, "w2", "second", false);
    c.wait_turns(&s, 2);
    let events = c.events(&s);
    let at = |f: &dyn Fn(&Body) -> bool| events.iter().position(f);
    let setup = at(&|b| matches!(b, Body::Notice { text } if text.contains("Set up the worktree")))
        .expect("the setup notice");
    let first = at(&|b| matches!(b, Body::Delivered { id } if id == "w1")).expect("w1 delivered");
    let second = at(&|b| matches!(b, Body::Delivered { id } if id == "w2")).expect("w2 delivered");
    let reply = at(&|b| matches!(b, Body::Assistant { .. })).expect("a reply");
    assert!(
        setup < first && first < reply && first < second,
        "{events:#?}"
    );
    // Both waited, shown, rather than vanishing until setup ended.
    assert!(
        events
            .iter()
            .any(|b| matches!(b, Body::User { id, queued: true, .. } if id == "w1"))
    );
    let cwd = events
        .iter()
        .find_map(|b| match b {
            Body::Created { cwd, .. } => Some(cwd.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        cwd.starts_with(home.to_str().unwrap()) || cwd.contains("/dent/worktrees/"),
        "{cwd}"
    );
    assert!(std::path::Path::new(&cwd).join("ready.txt").exists());
}

// ---------------------------------------------------------------------------
// Adversarial tests (break-b): carried files, bringing a project, messages
// that wait, the LAN switch. Each fails only when a user would see wrong
// data, a hang, or a hole.

fn carried_file(name: &str, bytes: &[u8]) -> dent_protocol::Carried {
    dent_protocol::Carried {
        name: name.into(),
        bytes_b64: dent_protocol::b64::encode(bytes),
    }
}

fn user_attachments(c: &Client, s: &str, msg: &str) -> Vec<String> {
    c.events(s)
        .into_iter()
        .find_map(|b| match b {
            Body::User {
                id, attachments, ..
            } if id == msg => Some(attachments),
            _ => None,
        })
        .unwrap_or_default()
}

fn send_files(c: &mut Client, s: &str, id: &str, files: Vec<dent_protocol::Carried>) {
    c.call(Request::Send {
        session: s.into(),
        id: id.into(),
        text: "look".into(),
        steer: false,
        attachments: Vec::new(),
        files,
    })
    .unwrap();
}

/// A thousand files in one message, all named alike, plus empty files and
/// names that try to climb out or are only dots, unicode or very long:
/// each arrives with its own bytes, at its own path inside its message's
/// folder; and the same message sent again (a reconnect) is taken once.
#[test]
fn bb_many_oddly_named_files_arrive_intact_and_a_resend_is_taken_once() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "first");
    c.wait(&s, "the turn to end", is_done);
    let mut files: Vec<(String, Vec<u8>)> = (0..1000)
        .map(|i| ("notes.txt".to_string(), format!("file {i}").into_bytes()))
        .collect();
    for (name, bytes) in [
        ("empty", &b""[..]),
        ("", b"no name"),
        ("..", b"dots"),
        ("...", b"three dots"),
        ("a\\..\\..\\win.txt", b"backslashes"),
        ("résumé 📄.pdf", b"unicode"),
    ] {
        files.push((name.into(), bytes.to_vec()));
    }
    files.push((format!("{}.pdf", "é".repeat(150)), b"long".to_vec()));
    let req: Vec<_> = files.iter().map(|(n, b)| carried_file(n, b)).collect();
    send_files(&mut c, &s, "m-many", req.clone());
    c.wait(
        &s,
        "the message",
        |b| matches!(b, Body::User { id, .. } if id == "m-many"),
    );
    let paths = user_attachments(&c, &s, "m-many");
    assert_eq!(paths.len(), files.len());
    let distinct: std::collections::HashSet<_> = paths.iter().map(|p| p.to_lowercase()).collect();
    assert_eq!(distinct.len(), paths.len(), "two files landed on one path");
    for (p, (name, bytes)) in paths.iter().zip(&files) {
        assert!(p.contains("/attachments/carried/m-many/"), "{p}");
        assert_eq!(&std::fs::read(p).unwrap(), bytes, "{name:?} at {p}");
    }
    // Sent again after a reconnect: one message, the same files.
    send_files(&mut c, &s, "m-many", req);
    c.pump_for(Duration::from_millis(800));
    assert_eq!(
        c.count(&s, |b| matches!(b, Body::User { id, .. } if id == "m-many")),
        1
    );
    let dir = w.state.join("attachments/carried/m-many");
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), files.len());
}

/// Message ids are chosen by clients. Two different messages must never
/// share a folder: the second would overwrite the first's attachment, and
/// the agent (or the user opening it) would see the wrong file.
#[test]
fn bb_two_messages_never_overwrite_each_others_files() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "first");
    c.wait(&s, "the turn to end", is_done);
    send_files(
        &mut c,
        &s,
        "web/m1",
        vec![carried_file("plan.md", b"from web")],
    );
    c.wait(
        &s,
        "the first",
        |b| matches!(b, Body::User { id, .. } if id == "web/m1"),
    );
    send_files(
        &mut c,
        &s,
        "cli/m1",
        vec![carried_file("plan.md", b"from cli")],
    );
    c.wait(
        &s,
        "the second",
        |b| matches!(b, Body::User { id, .. } if id == "cli/m1"),
    );
    let first = user_attachments(&c, &s, "web/m1");
    let second = user_attachments(&c, &s, "cli/m1");
    assert_ne!(first, second, "both messages point at one file");
    assert_eq!(
        std::fs::read(&first[0]).unwrap(),
        b"from web",
        "the first message's attachment was overwritten by the second"
    );
}

/// Many messages for a session, then all of them sent again (the app
/// resends everything still waiting after a restart): each is taken
/// exactly once, in the order written.
#[test]
fn bb_many_waiting_messages_are_taken_once_in_order_through_a_resend() {
    let w = World::new(&[]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Ask, "first");
    c.wait(&s, "the turn to end", is_done);
    let ids: Vec<String> = (0..15).map(|i| format!("q{i:02}")).collect();
    for round in 0..2 {
        for id in &ids {
            c.send_req(Request::Send {
                session: s.clone(),
                id: id.clone(),
                text: format!("say {id}"),
                steer: false,
                attachments: Vec::new(),
                files: Vec::new(),
            });
        }
        if round == 0 {
            c.pump(Duration::from_millis(100));
        }
    }
    let until = Instant::now() + Duration::from_secs(40);
    let users = |c: &Client| -> Vec<String> {
        c.events(&s)
            .into_iter()
            .filter_map(|b| match b {
                Body::User { id, .. } if id.starts_with('q') => Some(id),
                _ => None,
            })
            .collect()
    };
    while users(&c).len() < ids.len() && Instant::now() < until {
        c.pump(Duration::from_millis(100));
    }
    c.pump_for(Duration::from_secs(2));
    assert_eq!(users(&c), ids);
}

struct BringHome {
    home: PathBuf,
    h: String,
}

impl BringHome {
    fn new(tag: &str) -> Self {
        let home = std::env::temp_dir().join(format!("dent-bb-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join(".gitconfig"),
            "[user]\n\tname = t\n\temail = t@t\n[init]\n\tdefaultBranch = main\n",
        )
        .unwrap();
        let home = home.canonicalize().unwrap();
        let h = home.display().to_string();
        Self { home, h }
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("HOME", &self.h)
            .output()
            .unwrap();
        assert!(
            ok.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&ok.stderr)
        );
        String::from_utf8_lossy(&ok.stdout).into_owned()
    }

    /// A checkout `there/app` on `branch`, pushed to the bare `app.git`.
    fn repo(&self, branch: &str, gitignore: &str) -> (PathBuf, PathBuf) {
        let there = self.home.join("there/app");
        let bare = self.home.join("app.git");
        std::fs::create_dir_all(&there).unwrap();
        self.git(
            &self.home,
            &["init", "-q", "--bare", bare.to_str().unwrap()],
        );
        self.git(&there, &["init", "-q", "-b", "main"]);
        std::fs::write(there.join(".gitignore"), gitignore).unwrap();
        std::fs::write(there.join("README.md"), "hi\n").unwrap();
        self.git(&there, &["add", "."]);
        self.git(&there, &["commit", "-q", "-m", "first"]);
        if branch != "main" {
            self.git(&there, &["checkout", "-q", "-b", branch]);
        }
        self.git(
            &there,
            &[
                "remote",
                "add",
                "origin",
                &format!("file://{}", bare.display()),
            ],
        );
        let mut push = vec!["push", "-q", "origin", "main"];
        if branch != "main" {
            push.push(branch);
        }
        self.git(&there, &push);
        (there, bare)
    }

    fn projects(&self) -> PathBuf {
        self.home.join("dent/projects")
    }
}

impl Drop for BringHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn bring_req(
    id: &str,
    url: &str,
    branch: Option<&str>,
    files: Vec<dent_protocol::Carried>,
) -> Request {
    Request::Create {
        agent: None,
        cwd: String::new(),
        worktree: None,
        model: None,
        permission: Permission::Full,
        effort: None,
        id: id.into(),
        text: "hello".into(),
        attachments: Vec::new(),
        files: Vec::new(),
        bring: Some(dent_protocol::Bring {
            name: "app".into(),
            remotes: vec![dent_protocol::GitRemote {
                name: "origin".into(),
                url: url.into(),
            }],
            branch: branch.map(str::to_string),
            files,
        }),
    }
}

fn created(out: Result<Out, String>) -> String {
    match out {
        Ok(Out::Reply {
            reply: dent_protocol::Reply::Created { session },
        }) => session,
        o => panic!("{o:?}"),
    }
}

fn cwd_of(c: &Client, s: &str) -> PathBuf {
    c.events(s)
        .into_iter()
        .find_map(|b| match b {
            Body::Created { cwd, .. } => Some(PathBuf::from(cwd)),
            _ => None,
        })
        .unwrap()
}

fn notices(c: &Client, s: &str) -> Vec<String> {
    c.events(s)
        .into_iter()
        .filter_map(|b| match b {
            Body::Notice { text } => Some(text),
            _ => None,
        })
        .collect()
}

/// Carried files must never land in the repository (`.git`). On a disk
/// that ignores case (every Mac's default), `.GIT` *is* `.git`: a file sent
/// as `.GIT/hooks/pre-commit` must not end up as the clone's hook.
#[test]
fn bb_a_carried_file_never_lands_in_git_by_another_case() {
    let b = BringHome::new("case");
    let (_there, bare) = b.repo("main", ".env\n");
    let w = World::new(&[("HOME", b.h.as_str())]);
    let mut c = w.client();
    c.subscribe(0);
    let s = created(c.call(bring_req(
        "m-case",
        &format!("file://{}", bare.display()),
        None,
        vec![
            carried_file(".env", b"A=1\n"),
            carried_file(".GIT/hooks/pre-commit", b"#!/bin/sh\necho owned\n"),
            carried_file(".Git/info/attributes", b"* filter=evil\n"),
        ],
    )));
    // Refused whole (the message given back) or brought without them:
    // either way nothing lands in the repository.
    c.wait(&s, "the turn to end or the message given back", |b| {
        is_done(b)
            || matches!(
                b,
                Body::Unqueued {
                    returned: Some(_),
                    ..
                }
            )
    });
    let here = cwd_of(&c, &s);
    let git = here.join(".git");
    assert!(
        !git.join("hooks/pre-commit").exists() && !git.join("info/attributes").exists(),
        "carried files were written inside .git (pre-commit: {}, info/attributes: {}); notices {:?}",
        git.join("hooks/pre-commit").exists(),
        git.join("info/attributes").exists(),
        notices(&c, &s)
    );
}

/// Files ignored only by the other checkout's own rules are kept out of
/// Git here through `.git/info/exclude`. A name ending in a space, or with
/// a newline in it, must still be excluded, or the secret shows up in
/// `git status` and is one `git add -A` from a commit (and a newline lets
/// a name add an ignore rule of its own).
#[test]
fn bb_brought_secrets_with_awkward_names_stay_out_of_git() {
    let b = BringHome::new("exclude");
    let (there, bare) = b.repo("main", "");
    std::fs::write(there.join(".git/info/exclude"), "secret*\n").unwrap();
    let names = ["secret key ", "secret\nREADME.md", "secret [1]"];
    for n in names {
        std::fs::write(there.join(n), b"token").unwrap();
    }
    let w = World::new(&[("HOME", b.h.as_str())]);
    let mut c = w.client();
    c.subscribe(0);
    let files = match c
        .call(Request::ReadFiles {
            dir: there.display().to_string(),
            paths: names.iter().map(|n| n.to_string()).collect(),
        })
        .unwrap()
    {
        Out::Reply {
            reply: dent_protocol::Reply::Carried { files },
        } => files,
        o => panic!("{o:?}"),
    };
    let s = created(c.call(bring_req(
        "m-ex",
        &format!("file://{}", bare.display()),
        None,
        files,
    )));
    c.wait(&s, "the turn to end", is_done);
    let here = cwd_of(&c, &s);
    for n in names {
        if n.contains('\n') {
            // No exclude rule can hold a line break: left out, not left
            // where it could be committed.
            assert!(!here.join(n).exists(), "{n:?} brought");
        } else {
            assert_eq!(std::fs::read(here.join(n)).unwrap(), b"token", "{n:?}");
        }
    }
    let status = b.git(
        &here,
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    );
    assert_eq!(
        status,
        "",
        "brought secrets aren't ignored in the clone; exclude is:\n{}",
        std::fs::read_to_string(here.join(".git/info/exclude")).unwrap_or_default()
    );
}

/// A static HTTP server for a bare repository (Git's "dumb" protocol). The
/// first `stall` connections are accepted and never answered, as a link
/// that stalls mid-clone.
fn dumb_http(
    root: PathBuf,
    stall: usize,
) -> (
    u16,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<Mutex<Vec<std::net::TcpStream>>>,
) {
    use std::io::Read;
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let held = Arc::new(Mutex::new(Vec::new()));
    let (n2, held2) = (n.clone(), held.clone());
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            if n2.fetch_add(1, Ordering::SeqCst) < stall {
                held2.lock().unwrap().push(s);
                continue;
            }
            let root = root.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut b = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut b) {
                        Ok(0) | Err(_) => return,
                        Ok(k) => buf.extend_from_slice(&b[..k]),
                    }
                }
                let head = String::from_utf8_lossy(&buf);
                let path = head.split_whitespace().nth(1).unwrap_or("/");
                let path = path.split('?').next().unwrap_or("/");
                let file = root.join(path.trim_start_matches('/'));
                let resp = match std::fs::read(&file) {
                    Ok(body) if file.is_file() => {
                        let mut r = format!(
                            "HTTP/1.0 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        r.extend(body);
                        r
                    }
                    _ => b"HTTP/1.0 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
                };
                let _ = s.write_all(&resp);
            });
        }
    });
    (port, n, held)
}

/// The clone is a durable job: the daemon restarting mid-clone, twice,
/// starts it over each time; in the end the project is there once, whole,
/// on its slashed branch, with its files, no half-made folders left, and
/// the first message reaches the agent exactly once.
#[test]
fn bb_a_clone_survives_two_restarts_mid_clone() {
    let b = BringHome::new("restart");
    let (_there, bare) = b.repo("feature/x/y", ".env\n");
    b.git(&bare, &["update-server-info"]);
    let (port, accepted, held) = dumb_http(b.home.clone(), 2);
    let url = format!("http://127.0.0.1:{port}/app.git");
    let mut w = World::new(&[("HOME", b.h.as_str())]);
    let mut c = w.client();
    let s = created(c.call(bring_req(
        "m-dur",
        &url,
        Some("feature/x/y"),
        vec![carried_file(".env", b"K=v\n")],
    )));
    for k in 1..=2 {
        let until = Instant::now() + Duration::from_secs(15);
        while accepted.load(Ordering::SeqCst) < k {
            assert!(Instant::now() < until, "clone try {k} never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(c);
        w.kill();
        w.start();
        c = w.client();
    }
    c.subscribe(0);
    c.wait(&s, "the turn to end", is_done);
    held.lock().unwrap().clear();
    let here = b.projects().join("app");
    assert_eq!(cwd_of(&c, &s), here);
    assert!(here.join("README.md").is_file(), "{:?}", notices(&c, &s));
    assert_eq!(std::fs::read(here.join(".env")).unwrap(), b"K=v\n");
    assert_eq!(
        b.git(&here, &["symbolic-ref", "--short", "HEAD"]).trim(),
        "feature/x/y"
    );
    let left: Vec<_> = std::fs::read_dir(b.projects())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["app"], "half-made clones or second folders left");
    c.pump_for(Duration::from_millis(500));
    assert_eq!(
        c.count(&s, |b| matches!(b, Body::User { id, .. } if id == "m-dur")),
        1
    );
    assert_eq!(c.count(&s, |b| matches!(b, Body::TurnStarted)), 1);
}

/// The same project brought several times at once, with a folder of that
/// name already there holding someone's work: each bring gets a folder of
/// its own, none is cloned over another, and the existing folder is
/// untouched.
#[test]
fn bb_brings_of_one_project_at_once_never_share_a_folder() {
    let b = BringHome::new("twice");
    let (_there, bare) = b.repo("main", ".env\n");
    std::fs::create_dir_all(b.projects().join("app")).unwrap();
    std::fs::write(b.projects().join("app/mine.txt"), "my work").unwrap();
    let w = World::new(&[("HOME", b.h.as_str())]);
    let url = format!("file://{}", bare.display());
    let mut clients: Vec<Client> = (0..4).map(|_| w.client()).collect();
    let reqs: Vec<u64> = clients
        .iter_mut()
        .enumerate()
        .map(|(i, c)| {
            c.send_req(bring_req(
                &format!("m-at-once-{i}"),
                &url,
                None,
                vec![carried_file(".env", format!("N={i}\n").as_bytes())],
            ))
        })
        .collect();
    let mut sessions = Vec::new();
    for (c, req) in clients.iter_mut().zip(reqs) {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(o) = c.replies.remove(&req) {
                sessions.push(created(Ok(o)));
                break;
            }
            assert!(Instant::now() < until, "no reply");
            c.pump(Duration::from_millis(50));
        }
    }
    let mut c = w.client();
    c.subscribe(0);
    let mut dirs = std::collections::HashSet::new();
    for (i, s) in sessions.iter().enumerate() {
        c.wait(s, "the turn to end", is_done);
        let here = cwd_of(&c, s);
        assert!(
            dirs.insert(here.clone()),
            "two brings share {}",
            here.display()
        );
        assert!(here.join("README.md").is_file(), "{:?}", notices(&c, s));
        assert_eq!(
            std::fs::read_to_string(here.join(".env")).unwrap(),
            format!("N={i}\n")
        );
    }
    assert_eq!(
        std::fs::read_to_string(b.projects().join("app/mine.txt")).unwrap(),
        "my work"
    );
}

/// The saved LAN setting wins over DENT_REMOTE_LAN across restarts, and
/// what the daemon does (in memory) always matches what it will come back
/// with (on disk), even when two devices flip the switch at once.
#[test]
fn bb_the_lan_switch_wins_over_the_env_and_disk_matches_memory() {
    let mut w = World::new(&[("DENT_REMOTE_LAN", "1")]);
    let lan_of = |o: Out| match o {
        Out::Reply {
            reply: dent_protocol::Reply::Machine { machine },
        } => machine.lan,
        o => panic!("{o:?}"),
    };
    let mut c = w.client();
    assert!(lan_of(c.call(Request::Machine).unwrap()), "env turns it on");
    assert!(!lan_of(c.call(Request::SetLan { on: false }).unwrap()));
    drop(c);
    w.kill();
    w.start();
    let mut c = w.client();
    assert!(
        !lan_of(c.call(Request::Machine).unwrap()),
        "turned off, it stays off though DENT_REMOTE_LAN=1"
    );
    drop(c);
    for round in 0..5 {
        let mut a = w.client();
        let mut b = w.client();
        let (mut ra, mut rb) = (Vec::new(), Vec::new());
        // Each SetLan answers with the whole machine (~150 ms here).
        for _ in 0..20 {
            ra.push(a.send_req(Request::SetLan { on: true }));
            rb.push(b.send_req(Request::SetLan { on: false }));
        }
        for (c, rs) in [(&mut a, &ra), (&mut b, &rb)] {
            let until = Instant::now() + Duration::from_secs(60);
            while !rs.iter().all(|r| c.replies.contains_key(r)) {
                assert!(Instant::now() < until, "SetLan replies didn't all come");
                c.pump(Duration::from_millis(20));
            }
        }
        let memory = lan_of(a.call(Request::Machine).unwrap());
        drop((a, b));
        w.kill();
        w.start();
        let mut c = w.client();
        let disk = lan_of(c.call(Request::Machine).unwrap());
        assert_eq!(
            memory, disk,
            "round {round}: on before the restart was {memory}, after {disk}"
        );
    }
}

/// The id of the last reply in `s` so far.
fn last_reply(c: &Client, s: &str) -> String {
    c.events(s)
        .iter()
        .rev()
        .find_map(|b| match b {
            Body::Assistant {
                parent: None,
                uuid: Some(u),
                ..
            } => Some(u.clone()),
            _ => None,
        })
        .expect("a reply with an id")
}

/// Wait for the `n`th event matching `f`.
fn wait_nth(c: &mut Client, s: &str, n: usize, f: impl Fn(&Body) -> bool) {
    let until = Instant::now() + Duration::from_secs(15);
    while c.count(s, &f) < n {
        assert!(Instant::now() < until, "waited; got {:#?}", c.events(s));
        c.pump(Duration::from_millis(20));
    }
}

fn stop(c: &mut Client, s: &str) {
    let stopped = |b: &Body| {
        matches!(
            b,
            Body::TurnEnded {
                outcome: Outcome::Interrupted { .. }
            }
        )
    };
    let n = c.count(s, stopped);
    c.call(Request::Interrupt {
        session: s.into(),
        by: Some("Esc".into()),
    })
    .unwrap();
    wait_nth(c, s, n + 1, stopped);
}

#[test]
fn a_stop_before_any_reply_takes_the_message_out_of_the_conversation() {
    let mut w = World::new(&[("FAKE_CLAUDE_SPEED", "0.5")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Full, "hi");
    c.wait_turns(&s, 1);
    let r1 = last_reply(&c, &s);
    c.say(&s, "b", "a long answer please", false);
    wait_nth(&mut c, &s, 2, |b| matches!(b, Body::TurnStarted));
    stop(&mut c, &s);
    assert_eq!(
        c.count(
            &s,
            |b| matches!(b, Body::Withdrawn { ids } if *ids == ["b"])
        ),
        1,
        "{:#?}",
        c.events(&s)
    );
    // As it was before: the stop leaves no mark of its own.
    c.pump_for(Duration::from_millis(300));
    assert_eq!(c.status(&s), Some(Status::Done));
    // The next agent carries on from the reply before it, in a copy, so
    // the message is gone from the agent's conversation too.
    c.say(&s, "c", "your args", false);
    c.wait_turns(&s, 2);
    let args = c.text(&s);
    assert!(
        args.contains(&format!("--resume-session-at {r1}")) && args.contains("--fork-session"),
        "{args}"
    );

    // Taken back again, then dent restarts with the agent still running:
    // the next message still starts one from before it.
    let r2 = last_reply(&c, &s);
    c.say(&s, "d", "a long answer please", false);
    wait_nth(&mut c, &s, 4, |b| matches!(b, Body::TurnStarted));
    stop(&mut c, &s);
    assert_eq!(
        c.count(
            &s,
            |b| matches!(b, Body::Withdrawn { ids } if *ids == ["d"])
        ),
        1
    );
    drop(c);
    w.kill();
    w.start();
    let mut c = w.client();
    c.subscribe(0);
    c.pump_for(Duration::from_millis(300));
    assert_eq!(c.status(&s), Some(Status::Done));
    c.say(&s, "e", "your args again", false);
    c.wait_turns(&s, 3);
    let args = c.text(&s);
    assert!(
        args.contains(&format!("--resume-session-at {r2}")),
        "{args}"
    );
}

#[test]
fn a_first_message_stopped_before_a_reply_starts_a_new_conversation() {
    let w = World::new(&[("FAKE_CLAUDE_SPEED", "0.5")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Full, "a long answer please");
    c.wait(&s, "its turn", |b| matches!(b, Body::TurnStarted));
    stop(&mut c, &s);
    assert_eq!(c.count(&s, |b| matches!(b, Body::Withdrawn { .. })), 1);
    c.pump_for(Duration::from_millis(300));
    assert_eq!(c.status(&s), Some(Status::Interrupted));
    let first = c
        .events(&s)
        .iter()
        .find_map(|b| match b {
            Body::Agent {
                agent_session: Some(a),
                ..
            } => Some(a.clone()),
            _ => None,
        })
        .unwrap();
    c.say(&s, "b", "your args", false);
    c.wait_turns(&s, 1);
    let args = c.text(&s);
    assert!(
        args.contains("--session-id") && !args.contains(&first) && !args.contains("--resume"),
        "{args}"
    );
}

#[test]
fn a_stop_after_something_came_back_keeps_the_message() {
    let w = World::new(&[("FAKE_CLAUDE_SPEED", "0.5")]);
    let mut c = w.client();
    c.subscribe(0);
    // A call waiting for approval.
    let s = c.create(&w.cwd(), Permission::Ask, "run it");
    c.wait(&s, "the approval", |b| matches!(b, Body::Asked { .. }));
    stop(&mut c, &s);
    assert_eq!(c.count(&s, |b| matches!(b, Body::Withdrawn { .. })), 0);
    c.pump_for(Duration::from_millis(300));
    assert_eq!(c.status(&s), Some(Status::Interrupted));
    // A reply partway written.
    c.say(&s, "b", "a long answer please", false);
    wait_nth(&mut c, &s, 2, |b| matches!(b, Body::TurnStarted));
    std::thread::sleep(Duration::from_millis(800));
    stop(&mut c, &s);
    assert_eq!(c.count(&s, |b| matches!(b, Body::Withdrawn { .. })), 0);
    assert!(!c.text(&s).is_empty());
}

#[test]
fn a_steer_sent_while_a_turn_stops_waits_for_it_to_end() {
    // Nothing goes into a turn being stopped, so a stop before any reply
    // never takes back a steer the person didn't stop.
    let w = World::new(&[("FAKE_CLAUDE_SPEED", "0.5"), ("FAKE_CLAUDE_DEAF", "1")]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Full, "work quietly");
    wait_nth(&mut c, &s, 1, |b| matches!(b, Body::TurnStarted));
    c.call(Request::Interrupt {
        session: s.clone(),
        by: Some("Esc".into()),
    })
    .unwrap();
    c.say(&s, "steer", "and this now", true);
    wait_nth(
        &mut c,
        &s,
        1,
        |b| matches!(b, Body::User { id, .. } if id == "steer"),
    );
    assert_eq!(
        c.count(
            &s,
            |b| matches!(b, Body::User { id, queued: true, .. } if id == "steer")
        ),
        1
    );
    wait_nth(&mut c, &s, 1, |b| matches!(b, Body::Withdrawn { .. }));
    assert!(
        c.count(
            &s,
            |b| matches!(b, Body::Withdrawn { ids } if ids.iter().any(|i| i == "steer"))
        ) == 0
    );
    c.wait_turns(&s, 1);
}

// ---- Moving a session between machines (decision 0064) ----

/// Two machines: each a daemon with a home of its own, the same project
/// cloned on both from one origin, the fake agent keeping transcripts.
struct Moves {
    root: PathBuf,
    a: World,
    b: World,
    /// The project's checkout on each.
    on_a: PathBuf,
    on_b: PathBuf,
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

impl Moves {
    fn new(tag: &str, b_has_project: bool) -> Self {
        let root = std::env::temp_dir().join(format!("dent-move-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home_a, home_b) = (root.join("home-a"), root.join("home-b"));
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();
        let origin = root.join("origin.git");
        git_in(&root, &["init", "-q", "--bare", origin.to_str().unwrap()]);
        // A URL, as a real remote is (paths on one machine aren't carried).
        let url = format!("file://{}", origin.display());
        let on_a = home_a.join("app");
        git_in(&root, &["clone", "-q", &url, on_a.to_str().unwrap()]);
        std::fs::write(on_a.join("README.md"), "app\n").unwrap();
        git_in(&on_a, &["checkout", "-q", "-b", "work"]);
        git_in(&on_a, &["add", "."]);
        git_in(&on_a, &["commit", "-qm", "base"]);
        git_in(&on_a, &["push", "-q", "origin", "work"]);
        let on_b = home_b.join("app");
        if b_has_project {
            git_in(&root, &["clone", "-q", &url, on_b.to_str().unwrap()]);
        }
        let env = |home: &Path| {
            vec![
                ("HOME".to_string(), home.display().to_string()),
                ("FAKE_CLAUDE_TRANSCRIPTS".to_string(), "1".to_string()),
                ("FAKE_CLAUDE_STRICT_RESUME".to_string(), "1".to_string()),
            ]
        };
        let ea = env(&home_a);
        let eb = env(&home_b);
        let a = World::new(
            &ea.iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect::<Vec<_>>(),
        );
        let b = World::new(
            &eb.iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect::<Vec<_>>(),
        );
        Moves {
            root,
            a,
            b,
            on_a,
            on_b,
        }
    }
}

impl Drop for Moves {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn reply(out: Result<Out, String>) -> Result<dent_protocol::Reply, String> {
    match out? {
        Out::Reply { reply } => Ok(reply),
        o => Err(format!("{o:?}")),
    }
}

/// Carry move `id` of `session` from `a` to `b`, `chunk` bytes at a time,
/// with the carrier `dent move` and the desktop use; `stop_after` chunks,
/// it stops (an interruption; `usize::MAX` stops once everything's sent,
/// before asking to accept). Returns the session it became, or why it
/// wasn't taken.
fn carry(
    a: &mut Client,
    b: &mut Client,
    session: &str,
    id: &str,
    chunk: u32,
    stop_after: Option<usize>,
) -> Result<Result<String, String>, String> {
    carry_then(a, b, session, id, chunk, stop_after, || {})
}

/// [`carry`], with `before_accept` run just before it asks to accept.
fn carry_then(
    a: &mut Client,
    b: &mut Client,
    session: &str,
    id: &str,
    chunk: u32,
    stop_after: Option<usize>,
    before_accept: impl FnMut(),
) -> Result<Result<String, String>, String> {
    let c = dent_client_core::carry::Carrier::new(session, id, "b")
        .to_machine("b-key")
        .with_chunk(chunk);
    drive(a, b, c, stop_after, before_accept)
}

/// Run carrier `c` between the two machines to its end.
fn drive(
    a: &mut Client,
    b: &mut Client,
    mut c: dent_client_core::carry::Carrier,
    stop_after: Option<usize>,
    mut before_accept: impl FnMut(),
) -> Result<Result<String, String>, String> {
    use dent_client_core::carry::{Ended, Side, Step};
    let mut step = c.start();
    let mut reads = 0;
    loop {
        let (side, req) = match step {
            Step::Done(Ok(s)) => return Ok(Ok(s)),
            Step::Done(Err(Ended::NotAccepted(why))) => return Ok(Err(why)),
            Step::Done(Err(e)) => return Err(format!("{e:?}")),
            Step::Ask(side, req) => (side, req),
            Step::Wait(_, side, req) => {
                std::thread::sleep(Duration::from_millis(200));
                (side, req)
            }
        };
        match &req {
            Request::MoveRead { .. } => {
                if stop_after.is_some_and(|n| reads >= n) {
                    return Err("stopped".into());
                }
                reads += 1;
            }
            Request::MoveAccept { .. } if stop_after == Some(usize::MAX) => {
                return Err("all sent".into());
            }
            Request::MoveAccept { .. } => before_accept(),
            _ => {}
        }
        let answer = match side {
            Side::From => reply(a.call(req)),
            Side::To => reply(b.call(req)),
        };
        step = c.answer(side, answer);
    }
}

#[test]
fn a_session_moves_with_its_conversation_and_its_uncommitted_work() {
    let m = Moves::new("whole", true);
    let mut a = m.a.client();
    let mut b = m.b.client();
    a.subscribe(0);
    b.subscribe(0);
    b.call(Request::AddProject {
        path: m.on_b.display().to_string(),
        create: false,
    })
    .unwrap();
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "remember the word apricot",
    );
    a.wait_turns(&s, 1);
    // Work in progress: a new file and an edit, never committed.
    std::fs::write(m.on_a.join("notes.md"), "draft\n").unwrap();
    std::fs::write(m.on_a.join("README.md"), "app, edited\n").unwrap();
    let to = carry(&mut a, &mut b, &s, "mv1", 300, None)
        .unwrap()
        .unwrap();
    // On the machine it left: a record that points there, fenced.
    a.pump_for(Duration::from_millis(300));
    assert!(a.events(&s).iter().any(|e| matches!(
        e,
        Body::Move { state: dent_protocol::MoveState::Departed, to_session: Some(t), to_machine, .. }
            if *t == to && to_machine == "b-key"
    )));
    let refused = a.call(Request::Send {
        session: s.clone(),
        id: "after".into(),
        text: "still here?".into(),
        steer: false,
        attachments: Vec::new(),
        files: Vec::new(),
    });
    assert!(refused.unwrap_err().contains("moved to b"));
    // On the machine it went to: its conversation carried on, told it moved.
    // (Its history came with it: one turn done already.)
    b.pump(Duration::from_millis(200));
    let before = b.turns_done(&to);
    b.say(&to, "r1", "recall", false);
    b.wait_turns(&to, before + 1);
    let said = b.text(&to);
    assert!(
        said.contains("You first said: remember the word apricot"),
        "{said}"
    );
    assert!(said.contains("Told it moved: true"), "{said}");
    // Its folder: a worktree of the same project, with the work as it was.
    let row_cwd = b
        .events(&to)
        .iter()
        .find_map(|e| match e {
            Body::Created { cwd, .. } => Some(cwd.clone()),
            _ => None,
        })
        .unwrap();
    let cwd = Path::new(&row_cwd);
    assert_eq!(
        std::fs::read_to_string(cwd.join("notes.md")).unwrap(),
        "draft\n"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("README.md")).unwrap(),
        "app, edited\n"
    );
    assert_eq!(git_in(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]), "work");
    // Told again (a carrier that lost the answer): the same session.
    assert_eq!(
        carry(&mut a, &mut b, &s, "mv1", 300, None)
            .unwrap()
            .unwrap(),
        to
    );
}

#[test]
fn a_move_cut_off_anywhere_picks_up_with_the_same_id() {
    let mut m = Moves::new("cut", true);
    let mut a = m.a.client();
    let mut b = m.b.client();
    b.call(Request::AddProject {
        path: m.on_b.display().to_string(),
        create: false,
    })
    .unwrap();
    a.subscribe(0);
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "remember the word fig",
    );
    a.wait_turns(&s, 1);
    // The carrier stops mid-transfer.
    assert_eq!(
        carry(&mut a, &mut b, &s, "mv2", 200, Some(3)).unwrap_err(),
        "stopped"
    );
    // Both machines restart meanwhile.
    m.b.kill();
    m.b.start();
    m.a.kill();
    m.a.start();
    let mut a = m.a.client();
    let mut b = m.b.client();
    // Cancelling now is refused: the other machine may hold a copy.
    let cancel = reply(a.call(Request::MoveStop {
        session: s.clone(),
        id: "mv2".into(),
        continue_here: false,
    }));
    assert!(cancel.unwrap_err().contains("Continue here"));
    // Picked up: the same package, what's there kept, and it arrives.
    let to = carry(&mut a, &mut b, &s, "mv2", 200, None)
        .unwrap()
        .unwrap();
    b.subscribe(0);
    b.pump(Duration::from_millis(200));
    let before = b.turns_done(&to);
    b.say(&to, "r1", "recall", false);
    b.wait_turns(&to, before + 1);
    assert!(
        b.text(&to).contains("remember the word fig"),
        "{}",
        b.text(&to)
    );
}

#[test]
fn a_move_the_other_machine_cant_take_leaves_the_session_working_here() {
    let m = Moves::new("refused", false);
    let mut a = m.a.client();
    let mut b = m.b.client();
    a.subscribe(0);
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "hello there",
    );
    a.wait_turns(&s, 1);
    // Found out before anything is packed: nothing fenced, nothing to undo.
    assert_eq!(
        carry(&mut a, &mut b, &s, "mv3", 4096, None).unwrap_err(),
        "NoProject"
    );
    assert!(!a.events(&s).iter().any(|e| matches!(e, Body::Move { .. })));
    a.say(&s, "again", "and again", false);
    a.wait_turns(&s, 2);
}

#[test]
fn a_project_gone_there_before_it_arrives_leaves_the_session_working_here() {
    let m = Moves::new("gone", true);
    let mut a = m.a.client();
    let mut b = m.b.client();
    a.subscribe(0);
    b.call(Request::AddProject {
        path: m.on_b.display().to_string(),
        create: false,
    })
    .unwrap();
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "hello there",
    );
    a.wait_turns(&s, 1);
    // Everything sent; then the project goes there before it's accepted.
    assert_eq!(
        carry(&mut a, &mut b, &s, "mv3", 4096, Some(usize::MAX)).unwrap_err(),
        "all sent"
    );
    let kept = m.on_b.with_extension("kept");
    std::process::Command::new("cp")
        .arg("-R")
        .arg(&m.on_b)
        .arg(&kept)
        .status()
        .unwrap();
    std::fs::remove_dir_all(&m.on_b).unwrap();
    // Picked up again, the carrier finds it gone there. Everything was
    // sent, so it isn't called off behind your back: you're told to keep
    // it here or retry, and Continue here brings it back.
    let why = carry(&mut a, &mut b, &s, "mv3", 4096, None).unwrap_err();
    assert!(why.contains("Continue here"), "{why}");
    a.call(Request::MoveStop {
        session: s.clone(),
        id: "mv3".into(),
        continue_here: true,
    })
    .unwrap();
    a.say(&s, "again", "and again", false);
    a.wait_turns(&s, 2);
    // Gone in the moment between everything arriving and taking it: the
    // machine it goes to refuses it itself.
    std::fs::rename(&kept, &m.on_b).unwrap();
    let on_b = m.on_b.clone();
    let why = carry_then(&mut a, &mut b, &s, "mv4", 4096, None, || {
        let _ = std::fs::remove_dir_all(&on_b);
    })
    .unwrap()
    .unwrap_err();
    assert!(why.contains("doesn't have the project"), "{why}");
    // Not taken: it works here again, as it was.
    a.say(&s, "again2", "and again", false);
    a.wait_turns(&s, 3);
    assert!(a.events(&s).iter().any(|e| matches!(
        e,
        Body::Move {
            state: dent_protocol::MoveState::Failed { .. },
            ..
        }
    )));
}

/// The agent's conversation can't be found on the machine it leaves (taken
/// away, or kept where that machine doesn't look): it moves without it,
/// and there starts a new one, given what was said, as text.
#[test]
fn a_session_whose_conversation_is_gone_moves_and_is_given_it_as_text() {
    let m = Moves::new("lost", true);
    let mut a = m.a.client();
    let mut b = m.b.client();
    a.subscribe(0);
    b.subscribe(0);
    b.call(Request::AddProject {
        path: m.on_b.display().to_string(),
        create: false,
    })
    .unwrap();
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "hello there",
    );
    a.wait_turns(&s, 1);
    std::fs::remove_dir_all(m.root.join("home-a/.claude/projects")).unwrap();
    let there = carry(&mut a, &mut b, &s, "mv9", 4096, None)
        .unwrap()
        .unwrap();
    b.say(&there, "r", "recall what I said", false);
    // The turn it brought, then this one.
    b.wait_turns(&there, 2);
    let said = b.text(&there);
    assert!(said.contains("Given back: 1 messages"), "{said}");
}

#[test]
fn kept_here_after_it_may_have_gone_the_session_works_again() {
    let m = Moves::new("kept", true);
    let mut a = m.a.client();
    let mut b = m.b.client();
    b.call(Request::AddProject {
        path: m.on_b.display().to_string(),
        create: false,
    })
    .unwrap();
    a.subscribe(0);
    let s = a.create(&m.on_a.display().to_string(), Permission::Full, "hello");
    a.wait_turns(&s, 1);
    assert!(carry(&mut a, &mut b, &s, "mv4", 200, Some(1)).is_err());
    // While it's on its way, nothing starts here.
    let fenced = a.call(Request::Send {
        session: s.clone(),
        id: "x".into(),
        text: "wait".into(),
        steer: false,
        attachments: Vec::new(),
        files: Vec::new(),
    });
    assert!(fenced.unwrap_err().contains("moving to b"));
    reply(a.call(Request::MoveStop {
        session: s.clone(),
        id: "mv4".into(),
        continue_here: true,
    }))
    .unwrap();
    a.say(&s, "y", "carry on", false);
    a.wait_turns(&s, 2);
}

#[test]
fn a_move_waits_for_the_turn_and_a_message_meanwhile_calls_it_off() {
    let m = Moves::new("pending", true);
    let mut a = m.a.client();
    a.subscribe(0);
    let pack = |s: &str, id: &str| Request::MovePack {
        session: s.into(),
        id: id.into(),
        to: "b".into(),
        to_machine: String::new(),
        have: Some(Vec::new()),
    };
    // Asked while it works: it waits, then packs once the turn is over.
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "a long answer, slowly",
    );
    a.wait(&s, "it works", |b| {
        matches!(
            b,
            Body::Status {
                status: Status::Working
            }
        )
    });
    assert!(matches!(
        reply(a.call(pack(&s, "mv5"))).unwrap(),
        dent_protocol::Reply::MoveWaiting
    ));
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        match reply(a.call(pack(&s, "mv5"))).unwrap() {
            dent_protocol::Reply::MovePacked { .. } => break,
            dent_protocol::Reply::MoveWaiting => {}
            o => panic!("{o:?}"),
        }
        assert!(Instant::now() < until, "never packed");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(a.turns_done(&s), 1, "it packed after the turn, not during");
    // Another, asked while it works, then you write to it: the move gives
    // way and the message runs here.
    let t = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "another long answer, slowly",
    );
    a.wait(&t, "it works", |b| {
        matches!(
            b,
            Body::Status {
                status: Status::Working
            }
        )
    });
    assert!(matches!(
        reply(a.call(pack(&t, "mv6"))).unwrap(),
        dent_protocol::Reply::MoveWaiting
    ));
    a.say(&t, "more", "and one more thing", false);
    a.wait(&t, "the move is called off", |b| {
        matches!(
            b,
            Body::Move {
                state: dent_protocol::MoveState::Cancelled,
                ..
            }
        )
    });
    a.wait_turns(&t, 2);
}

#[test]
fn a_target_that_dies_mid_accept_finishes_it_as_the_same_session() {
    let mut m = Moves::new("dies", true);
    let mut a = m.a.client();
    let on_b = m.on_b.display().to_string();
    // The machine it goes to dies between laying the work down and
    // making the session.
    m.b.kill();
    m.b.env.push(("DENT_TEST_DIE_IN_ACCEPT".into(), "1".into()));
    m.b.start();
    let mut b = m.b.client();
    b.call(Request::AddProject {
        path: on_b.clone(),
        create: false,
    })
    .unwrap();
    a.subscribe(0);
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "remember the word plum",
    );
    a.wait_turns(&s, 1);
    std::fs::write(m.on_a.join("wip.txt"), "half done\n").unwrap();
    assert_eq!(
        carry(&mut a, &mut b, &s, "mv7", 4096, Some(usize::MAX)).unwrap_err(),
        "all sent"
    );
    b.send_req(Request::MoveAccept { id: "mv7".into() });
    let until = Instant::now() + Duration::from_secs(10);
    while m
        .b
        .daemon
        .as_mut()
        .is_some_and(|d| d.try_wait().unwrap().is_none())
    {
        assert!(Instant::now() < until, "it didn't die where asked");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Back up, it finishes: one session, its work in place.
    m.b.env.retain(|(k, _)| k != "DENT_TEST_DIE_IN_ACCEPT");
    m.b.kill();
    m.b.start();
    let mut b = m.b.client();
    let to = carry(&mut a, &mut b, &s, "mv7", 4096, None)
        .unwrap()
        .unwrap();
    assert_eq!(
        carry(&mut a, &mut b, &s, "mv7", 4096, None)
            .unwrap()
            .unwrap(),
        to
    );
    b.subscribe(0);
    b.pump(Duration::from_millis(300));
    let cwd = b
        .events(&to)
        .iter()
        .find_map(|e| match e {
            Body::Created { cwd, .. } => Some(cwd.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(Path::new(&cwd).join("wip.txt")).unwrap(),
        "half done\n"
    );
    let before = b.turns_done(&to);
    b.say(&to, "r", "recall", false);
    b.wait_turns(&to, before + 1);
    assert!(
        b.text(&to).contains("remember the word plum"),
        "{}",
        b.text(&to)
    );
}

#[test]
fn a_source_that_dies_while_packing_packs_again_and_the_move_finishes() {
    let mut m = Moves::new("packdies", true);
    let mut b = m.b.client();
    b.call(Request::AddProject {
        path: m.on_b.display().to_string(),
        create: false,
    })
    .unwrap();
    let mut a = m.a.client();
    a.subscribe(0);
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "remember the word kiwi",
    );
    a.wait_turns(&s, 1);
    m.a.kill();
    m.a.env.push(("DENT_TEST_DIE_IN_PACK".into(), "1".into()));
    m.a.start();
    let mut a = m.a.client();
    a.send_req(Request::MovePack {
        session: s.clone(),
        id: "mv8".into(),
        to: "b".into(),
        to_machine: String::new(),
        have: Some(Vec::new()),
    });
    let until = Instant::now() + Duration::from_secs(10);
    while m
        .a
        .daemon
        .as_mut()
        .is_some_and(|d| d.try_wait().unwrap().is_none())
    {
        assert!(Instant::now() < until, "it didn't die where asked");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Fenced across the restart; packed afresh, and it goes.
    m.a.env.retain(|(k, _)| k != "DENT_TEST_DIE_IN_PACK");
    m.a.start();
    let mut a = m.a.client();
    let to = carry(&mut a, &mut b, &s, "mv8", 4096, None)
        .unwrap()
        .unwrap();
    b.subscribe(0);
    b.pump(Duration::from_millis(300));
    let before = b.turns_done(&to);
    b.say(&to, "r", "recall", false);
    b.wait_turns(&to, before + 1);
    assert!(
        b.text(&to).contains("remember the word kiwi"),
        "{}",
        b.text(&to)
    );
}

#[test]
fn a_move_called_off_after_packing_sends_nothing_and_its_id_is_spent() {
    let m = Moves::new("calledoff", true);
    let mut a = m.a.client();
    let mut b = m.b.client();
    b.call(Request::AddProject {
        path: m.on_b.display().to_string(),
        create: false,
    })
    .unwrap();
    a.subscribe(0);
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "remember the word pear",
    );
    a.wait_turns(&s, 1);
    // Packed, then called off before anything was read.
    let packed = reply(a.call(Request::MovePack {
        session: s.clone(),
        id: "mv9".into(),
        to: "b".into(),
        to_machine: String::new(),
        have: Some(Vec::new()),
    }))
    .unwrap();
    assert!(matches!(packed, dent_protocol::Reply::MovePacked { .. }));
    reply(a.call(Request::MoveStop {
        session: s.clone(),
        id: "mv9".into(),
        continue_here: false,
    }))
    .unwrap();
    // A carrier that carries on gets nothing.
    let read = reply(a.call(Request::MoveRead {
        id: "mv9".into(),
        part: "events.jsonl".into(),
        offset: 0,
        len: 100,
    }));
    assert!(read.is_err(), "{read:?}");
    // And the same id doesn't start again: the session has moved on since.
    let again = carry(&mut a, &mut b, &s, "mv9", 4096, None);
    assert!(again.unwrap_err().contains("that move ended"));
    // It works here.
    a.say(&s, "x", "still here", false);
    a.wait_turns(&s, 2);
}

#[test]
fn going_back_to_before_a_turn_works_after_the_move() {
    let m = Moves::new("rewind", true);
    let mut a = m.a.client();
    let mut b = m.b.client();
    b.call(Request::AddProject {
        path: m.on_b.display().to_string(),
        create: false,
    })
    .unwrap();
    a.subscribe(0);
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "remember the word grape",
    );
    a.wait_turns(&s, 1);
    // Made after the first message's checkpoint.
    std::fs::write(m.on_a.join("later.txt"), "later\n").unwrap();
    let to = carry(&mut a, &mut b, &s, "mv10", 4096, None)
        .unwrap()
        .unwrap();
    b.subscribe(0);
    b.pump(Duration::from_millis(300));
    let cwd = b
        .events(&to)
        .iter()
        .find_map(|e| match e {
            Body::Created { cwd, .. } => Some(cwd.clone()),
            _ => None,
        })
        .unwrap();
    assert!(Path::new(&cwd).join("later.txt").exists());
    // The checkpoint came with it: files go back to before that turn.
    b.call(Request::Rewind {
        session: to.clone(),
        to: "m-remember the word grape".into(),
        files: true,
    })
    .unwrap();
    assert!(!Path::new(&cwd).join("later.txt").exists());
}

/// Resident memory of a process, in KiB.
fn rss_kib(pid: u32) -> u64 {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let kib = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    // Unread, the bounds would pass without measuring anything.
    assert!(kib > 0, "couldn't read the memory of {pid}");
    kib
}

/// Many agents at once, each streaming a lot: every turn finishes, one
/// client watching them all sees every event once and in order, and the
/// daemon's memory stays bounded: it grows with sessions running at once,
/// never with turns. `DENT_LOAD_SHAPE=40x4` (sessions x turns) runs it
/// heavier; 10x14 showed it flat from the fifth turn on (2026-10-23).
#[test]
fn many_agents_at_once_all_finish_seen_once_and_memory_stays_bounded() {
    let (sessions, turns): (usize, usize) = std::env::var("DENT_LOAD_SHAPE")
        .ok()
        .and_then(|v| {
            let (a, b) = v.split_once('x')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((12, 2));
    let w = World::new(&[("FAKE_CLAUDE_SPEED", "0.02")]);
    let pid = w.daemon.as_ref().unwrap().id();
    let mut c = w.client();
    c.subscribe(0);
    let before = rss_kib(pid);
    let ids: Vec<String> = (0..sessions)
        .map(|n| {
            c.create(
                &w.cwd(),
                Permission::Full,
                &format!("ten thousand lines, number {n}"),
            )
        })
        .collect();
    let mut peak = 0;
    for t in 1..=turns {
        let until = Instant::now() + Duration::from_secs(120);
        while ids.iter().any(|s| c.turns_done(s) < t) {
            assert!(Instant::now() < until, "turn {t} didn't finish everywhere");
            c.pump(Duration::from_millis(100));
            peak = peak.max(rss_kib(pid));
        }
        if t < turns {
            for (n, s) in ids.iter().enumerate() {
                c.say(
                    s,
                    &format!("again-{t}-{n}"),
                    "ten thousand lines again",
                    false,
                );
            }
        }
    }
    c.pump_for(Duration::from_secs(2));
    // Every event once, in order, with no gaps.
    let seqs: Vec<u64> = c.events.lock().unwrap().iter().map(|e| e.seq).collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "gaps or repeats");
    for s in &ids {
        assert_eq!(c.turns_done(s), turns, "{s}");
        assert_eq!(interruptions(&c, s), 0, "{s}");
    }
    let after = rss_kib(pid);
    let holder = w.holder_pid().map(|p| rss_kib(p as u32)).unwrap_or(0);
    eprintln!(
        "{sessions} sessions x {turns} turns of 10,000 lines: daemon {before} KiB before, {peak} peak, {after} after; holder {holder} KiB"
    );
    // Generous bounds that still catch a reply kept per session in memory
    // (each is about 600 KB) or a leak per turn.
    assert!(peak < 400 * 1024, "daemon peaked at {peak} KiB");
    assert!(holder < 200 * 1024, "holder at {holder} KiB");
}

/// The worst worktree failures: new sessions landing in the main checkout,
/// and stops breaking it. Many sessions asked for at once, each in a new
/// worktree, some stopped while setting up: every one works in a folder of
/// its own, and the main checkout (its branch, HEAD and uncommitted
/// changes) is exactly as it was.
#[test]
fn worktrees_made_at_once_never_land_in_or_disturb_the_main_checkout() {
    let home = std::env::temp_dir().join(format!("dent-wt{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let mut w = World::new(&[("HOME", home.to_str().unwrap())]);
    let project = w.project.clone();
    let git = |args: &[&str]| -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(&project)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    std::fs::create_dir_all(w.project.join(".dent")).unwrap();
    std::fs::write(w.project.join(".dent/setup"), "sleep 1\n").unwrap();
    git(&["add", ".dent/setup"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "setup",
    ]);
    // Work in progress in the main checkout.
    std::fs::write(w.project.join("README.md"), "hello\nmine, unsaved\n").unwrap();
    std::fs::write(w.project.join("scratch.txt"), "untracked\n").unwrap();
    let (branch, head, status) = (
        git(&["branch", "--show-current"]),
        git(&["rev-parse", "HEAD"]),
        git(&["status", "--porcelain"]),
    );
    let n = 12;
    let made: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let w = &w;
                scope.spawn(move || {
                    let mut c = w.client();
                    match c.call(Request::Create {
                        agent: None,
                        cwd: w.cwd(),
                        worktree: Some(dent_protocol::NewWorktree::default()),
                        model: None,
                        permission: Permission::Full,
                        effort: None,
                        id: format!("wt{i}"),
                        text: format!("hello {i}"),
                        attachments: Vec::new(),
                        files: Vec::new(),
                        bring: None,
                    }) {
                        Ok(Out::Reply {
                            reply: dent_protocol::Reply::Created { session },
                        }) => {
                            // Every third is stopped at once, mid-setup.
                            if i % 3 == 0 {
                                c.call(Request::Interrupt {
                                    session: session.clone(),
                                    by: None,
                                })
                                .unwrap();
                            }
                            session
                        }
                        other => panic!("create {i}: {other:?}"),
                    }
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut c = w.client();
    c.subscribe(0);
    let until = Instant::now() + Duration::from_secs(60);
    let quiet = |c: &Client, s: &str| {
        matches!(
            c.status(s),
            Some(Status::Done | Status::Interrupted | Status::Error)
        )
    };
    while made.iter().any(|s| !quiet(&c, s)) {
        assert!(
            Instant::now() < until,
            "sessions never settled: {:#?}",
            made.iter()
                .enumerate()
                .filter(|(_, s)| !quiet(&c, s))
                .map(|(i, s)| (i, c.events(s)))
                .collect::<Vec<_>>()
        );
        c.pump(Duration::from_millis(100));
    }
    // Settled for good: a restarted daemon reads them the same.
    drop(c);
    w.kill();
    w.start();
    let mut c = w.client();
    c.subscribe(0);
    c.pump_for(Duration::from_secs(1));
    for s in &made {
        assert!(quiet(&c, s), "{s} reads {:?} after a restart", c.status(s));
    }
    let mut dirs = std::collections::HashSet::new();
    for s in &made {
        let cwd = c
            .events(s)
            .iter()
            .find_map(|b| match b {
                Body::Created { cwd, .. } => Some(cwd.clone()),
                _ => None,
            })
            .unwrap();
        assert_ne!(cwd, w.cwd(), "{s} landed in the main checkout");
        assert!(
            std::path::Path::new(&cwd).starts_with(&home),
            "{s} works in {cwd}"
        );
        assert!(dirs.insert(cwd.clone()), "two sessions share {cwd}");
    }
    assert_eq!(git(&["branch", "--show-current"]), branch);
    assert_eq!(git(&["rev-parse", "HEAD"]), head);
    assert_eq!(git(&["status", "--porcelain"]), status);
    assert_eq!(
        git(&["worktree", "list", "--porcelain"])
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        n + 1
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// An agent that runs away with memory (nothing stops it on macOS): its
/// session says so past the warning limit, and past the stopping limit
/// dent stops it, the turn ending with why. Nothing carries on by itself,
/// and the next message starts it again.
#[test]
fn an_agent_using_too_much_memory_is_warned_about_then_stopped() {
    let w = World::new(&[
        ("FAKE_CLAUDE_SPEED", "1"),
        ("FAKE_CLAUDE_HOG_MB", "300"),
        ("DENT_MEMORY_EVERY_MS", "200"),
        ("DENT_MEMORY_WARN", &(100u64 << 20).to_string()),
        ("DENT_MEMORY_STOP", &(250u64 << 20).to_string()),
    ]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Full, "eat memory please");
    let ended = c.wait(&s, "the agent stopped", |b| {
        matches!(
            b,
            Body::TurnEnded {
                outcome: Outcome::Interrupted { .. }
            }
        )
    });
    let Body::TurnEnded {
        outcome: Outcome::Interrupted {
            reason: Some(reason),
        },
    } = ended
    else {
        panic!("{ended:?}")
    };
    assert!(
        reason.contains("of memory, past this machine's limit"),
        "{reason}"
    );
    c.pump_for(Duration::from_secs(6));
    assert_eq!(carry_ons(&c, &s), 0);
    c.say(&s, "after", "short", false);
    c.wait_turns(&s, 1);
    // Past the warning alone: said once, and the turn goes on to finish.
    let w = World::new(&[
        ("FAKE_CLAUDE_SPEED", "0.1"),
        ("FAKE_CLAUDE_HOG_MB", "300"),
        ("DENT_MEMORY_EVERY_MS", "200"),
        ("DENT_MEMORY_WARN", &(100u64 << 20).to_string()),
    ]);
    let mut c = w.client();
    c.subscribe(0);
    let s = c.create(&w.cwd(), Permission::Full, "eat memory please");
    c.wait_turns(&s, 1);
    assert_eq!(
        c.count(
            &s,
            |b| matches!(b, Body::Notice { text } if text.contains("of memory. dent stops it at"))
        ),
        1
    );
}

/// A login shell asked for its environment that outlived its daemon (it
/// died mid-ask; such shells can hang for good) is stopped by the next
/// daemon on the same state. One whose pid now belongs to something else
/// is left alone.
#[test]
fn a_login_shell_a_dead_daemon_left_asking_is_stopped_by_the_next() {
    use std::os::unix::process::CommandExt;
    let mut w = World::new(&[]);
    w.kill();
    let mut stuck = Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let mut other = Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let dir = w.state.join("env-shells");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(stuck.id().to_string()), now.to_string()).unwrap();
    // Written down as started an hour ago: not this process.
    std::fs::write(
        dir.join(other.id().to_string()),
        (now - 3_600_000).to_string(),
    )
    .unwrap();
    w.start();
    let until = Instant::now() + Duration::from_secs(5);
    while stuck.try_wait().unwrap().is_none() {
        assert!(Instant::now() < until, "the left shell is still running");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(other.try_wait().unwrap().is_none(), "stopped a stranger");
    let _ = other.kill();
    let _ = other.wait();
}

/// Many sessions moved to one machine at once, each by its own carrier,
/// with ids begun the same millisecond: each arrives whole, as its own
/// session in its own folder, and knows only its own conversation.
#[test]
fn sessions_moved_at_once_each_arrive_whole_and_apart() {
    const N: usize = 4;
    let m = Moves::new("many", true);
    let mut a = m.a.client();
    let mut b = m.b.client();
    a.subscribe(0);
    b.subscribe(0);
    b.call(Request::AddProject {
        path: m.on_b.display().to_string(),
        create: false,
    })
    .unwrap();
    let words = ["apricot", "basil", "cumin", "dill"];
    let sessions: Vec<String> = words
        .iter()
        .map(|w| {
            let s = a.create(
                &m.on_a.display().to_string(),
                Permission::Full,
                &format!("remember the word {w}"),
            );
            a.wait_turns(&s, 1);
            s
        })
        .collect();
    let now = 1_761_200_000_000;
    let ids: Vec<String> = (0..N)
        .map(|i| dent_client_core::carry::new_id(now, 0x1000_0000_0000 + i as u64 * 0x1111))
        .collect();
    let handles: Vec<_> = sessions
        .iter()
        .zip(&ids)
        .map(|(s, id)| {
            let (mut a, mut b) = (m.a.client(), m.b.client());
            let (s, id) = (s.clone(), id.clone());
            std::thread::spawn(move || carry(&mut a, &mut b, &s, &id, 4096, None))
        })
        .collect();
    let arrived: Vec<String> = handles
        .into_iter()
        .map(|h| h.join().unwrap().unwrap().unwrap())
        .collect();
    let distinct: std::collections::HashSet<_> = arrived.iter().collect();
    assert_eq!(distinct.len(), N, "{arrived:?}");
    b.pump_for(Duration::from_millis(300));
    let mut folders = std::collections::HashSet::new();
    for (to, w) in arrived.iter().zip(words) {
        let before = b.turns_done(to);
        b.say(to, "r", "recall", false);
        b.wait_turns(to, before + 1);
        let text = b.text(to);
        assert!(text.contains(&format!("remember the word {w}")), "{text}");
        for other in words.iter().filter(|o| **o != w) {
            assert!(!text.contains(&format!("the word {other}")), "{w}: {text}");
        }
        let cwd = b
            .events(to)
            .into_iter()
            .find_map(|e| match e {
                Body::Created { cwd, .. } => Some(cwd),
                _ => None,
            })
            .unwrap();
        assert!(folders.insert(cwd.clone()), "two landed in {cwd}");
    }
}

/// Moved to a machine without the project, cloning it there first: the
/// clone lands in that machine's projects with the ignored files ticked,
/// and the session lands in a worktree of it with its uncommitted work and
/// those files, carrying on its conversation.
#[test]
fn a_session_moves_to_a_machine_that_clones_its_project_first() {
    use dent_client_core::carry::{Carrier, CloneThere};
    let m = Moves::new("clone", false);
    std::fs::write(
        m.on_a.join(".gitignore"),
        ".env\nnode_modules/\nconfig/master.key\n",
    )
    .unwrap();
    git_in(&m.on_a, &["add", ".gitignore"]);
    git_in(&m.on_a, &["commit", "-qm", "ignore"]);
    git_in(&m.on_a, &["push", "-q", "origin", "work"]);
    // Its branch is the one a clone checks out, and it has a commit not
    // pushed: the clone mustn't hold the branch the session lands on.
    git_in(
        &m.root,
        &[
            "--git-dir",
            "origin.git",
            "symbolic-ref",
            "HEAD",
            "refs/heads/work",
        ],
    );
    std::fs::write(m.on_a.join("local.md"), "unpushed\n").unwrap();
    git_in(&m.on_a, &["add", "local.md"]);
    git_in(&m.on_a, &["commit", "-qm", "not pushed yet"]);
    std::fs::write(m.on_a.join(".env"), "KEY=from-a\n").unwrap();
    std::fs::create_dir_all(m.on_a.join("config")).unwrap();
    std::fs::write(m.on_a.join("config/master.key"), "k3y\n").unwrap();
    let mut a = m.a.client();
    let mut b = m.b.client();
    a.subscribe(0);
    b.subscribe(0);
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "remember the word fig",
    );
    a.wait_turns(&s, 1);
    std::fs::write(m.on_a.join("notes.md"), "draft\n").unwrap();
    let url = git_in(&m.on_a, &["remote", "get-url", "origin"]);
    let c = Carrier::new(&s, "mvc1", "b")
        .with_chunk(4096)
        .clone_there(CloneThere {
            dir: m.on_a.display().to_string(),
            paths: vec![".env".into(), "config/master.key".into()],
            bring: dent_protocol::Bring {
                name: "app".into(),
                remotes: vec![dent_protocol::GitRemote {
                    name: "origin".into(),
                    url: url.trim().to_string(),
                }],
                branch: Some("work".into()),
                files: Vec::new(),
            },
        });
    let to = drive(&mut a, &mut b, c, None, || {}).unwrap().unwrap();
    b.pump_for(Duration::from_millis(300));
    let cwd = b
        .events(&to)
        .into_iter()
        .find_map(|e| match e {
            Body::Created { cwd, .. } => Some(cwd),
            _ => None,
        })
        .unwrap();
    let cwd = Path::new(&cwd);
    // Said at once where it works in git: its project's checkout.
    let root = b.events(&to).into_iter().find_map(|e| match e {
        Body::Workspace { root, .. } => root,
        _ => None,
    });
    assert!(
        root.as_deref()
            .is_some_and(|r| r.ends_with("dent/projects/app")),
        "{root:?}"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("notes.md")).unwrap(),
        "draft\n"
    );
    assert_eq!(
        git_in(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "work"
    );
    assert!(
        git_in(cwd, &["log", "--format=%s"]).contains("not pushed yet"),
        "log: {} | local.md: {:?} | cwd {}",
        git_in(cwd, &["log", "--oneline", "--all", "-n", "8"]),
        std::fs::read_to_string(cwd.join("local.md")),
        cwd.display()
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join(".env")).unwrap(),
        "KEY=from-a\n",
        "the worktree has the ticked .env"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("config/master.key")).unwrap(),
        "k3y\n",
        "and the other file ticked"
    );
    // The clone itself, kept as one of that machine's projects.
    let cloned = m.root.join("home-b/dent/projects/app");
    assert_eq!(
        std::fs::read_to_string(cloned.join(".env")).unwrap(),
        "KEY=from-a\n"
    );
    let before = b.turns_done(&to);
    b.say(&to, "r", "recall", false);
    b.wait_turns(&to, before + 1);
    assert!(
        b.text(&to).contains("remember the word fig"),
        "{}",
        b.text(&to)
    );
}

/// Two sessions of one repository moved at once to a machine without it:
/// it's cloned there once, and both land in worktrees of that clone.
#[test]
fn two_sessions_moving_in_at_once_clone_their_project_once() {
    use dent_client_core::carry::{Carrier, CloneThere};
    let mut m = Moves::new("clone2", false);
    m.b.kill();
    m.b.env.push(("DENT_TEST_SLOW_CLONE".into(), "1".into()));
    m.b.start();
    let mut a = m.a.client();
    a.subscribe(0);
    let url = git_in(&m.on_a, &["remote", "get-url", "origin"]);
    let sessions: Vec<String> = ["one", "two"]
        .iter()
        .map(|w| {
            let s = a.create(
                &m.on_a.display().to_string(),
                Permission::Full,
                &format!("remember {w}"),
            );
            a.wait_turns(&s, 1);
            s
        })
        .collect();
    let handles: Vec<_> = sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (mut a, mut b) = (m.a.client(), m.b.client());
            let c = Carrier::new(s, &format!("mvd{i}"), "b")
                .with_chunk(4096)
                .clone_there(CloneThere {
                    dir: m.on_a.display().to_string(),
                    paths: Vec::new(),
                    bring: dent_protocol::Bring {
                        name: "app".into(),
                        remotes: vec![dent_protocol::GitRemote {
                            name: "origin".into(),
                            url: url.trim().to_string(),
                        }],
                        branch: None,
                        files: Vec::new(),
                    },
                });
            std::thread::spawn(move || drive(&mut a, &mut b, c, None, || {}))
        })
        .collect();
    for h in handles {
        h.join().unwrap().unwrap().unwrap();
    }
    let projects: Vec<String> = std::fs::read_dir(m.root.join("home-b/dent/projects"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    assert_eq!(projects, vec!["app".to_string()], "cloned once");
}

/// Moved to a machine whose own checkout has its branch out, with a commit
/// that machine hasn't: it lands on a branch of its own, says so, and the
/// checkout there is as it was.
#[test]
fn a_move_onto_a_branch_checked_out_there_lands_beside_it() {
    let m = Moves::new("beside", true);
    git_in(&m.on_b, &["checkout", "-q", "work"]);
    let before = git_in(&m.on_b, &["rev-parse", "HEAD"]);
    std::fs::write(m.on_a.join("local.md"), "unpushed\n").unwrap();
    git_in(&m.on_a, &["add", "local.md"]);
    git_in(&m.on_a, &["commit", "-qm", "not pushed yet"]);
    let mut a = m.a.client();
    let mut b = m.b.client();
    a.subscribe(0);
    b.subscribe(0);
    b.call(Request::AddProject {
        path: m.on_b.display().to_string(),
        create: false,
    })
    .unwrap();
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "remember the word plum",
    );
    a.wait_turns(&s, 1);
    let to = carry(&mut a, &mut b, &s, "mvb1", 4096, None)
        .unwrap()
        .unwrap();
    b.pump_for(Duration::from_millis(300));
    let said = b.events(&to).into_iter().find_map(|e| match e {
        Body::Notice { text } if text.starts_with("Moved here") => Some(text),
        _ => None,
    });
    assert!(
        said.as_deref()
            .is_some_and(|t| t.contains("onto a new branch, work-moved: work is checked out in")),
        "{said:?}"
    );
    let cwd = b
        .events(&to)
        .into_iter()
        .find_map(|e| match e {
            Body::Created { cwd, .. } => Some(cwd),
            _ => None,
        })
        .unwrap();
    let cwd = Path::new(&cwd);
    assert_eq!(
        git_in(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "work-moved"
    );
    assert!(git_in(cwd, &["log", "--format=%s"]).contains("not pushed yet"));
    assert_eq!(git_in(&m.on_b, &["rev-parse", "HEAD"]), before);
    assert_eq!(
        git_in(&m.on_b, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "work"
    );
    let n = b.turns_done(&to);
    b.say(&to, "r", "recall", false);
    b.wait_turns(&to, n + 1);
    assert!(b.text(&to).contains("remember the word plum"));
}

/// What a clone of `m`'s project there takes, ticking `paths`.
fn clone_of(m: &Moves, paths: &[&str]) -> dent_client_core::carry::CloneThere {
    let url = git_in(&m.on_a, &["remote", "get-url", "origin"]);
    dent_client_core::carry::CloneThere {
        dir: m.on_a.display().to_string(),
        paths: paths.iter().map(|p| p.to_string()).collect(),
        bring: dent_protocol::Bring {
            name: "app".into(),
            remotes: vec![dent_protocol::GitRemote {
                name: "origin".into(),
                url: url.trim().to_string(),
            }],
            branch: Some("work".into()),
            files: Vec::new(),
        },
    }
}

/// The projects folder there, as names.
fn projects_on(home: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(home.join("dent/projects"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// A clone that can't be had (no such repository): the move stops saying
/// why, nothing is left there, the session works where it was, and trying
/// again fails the same way rather than waiting on a clone forever.
#[test]
fn a_clone_that_fails_stops_the_move_and_leaves_nothing() {
    use dent_client_core::carry::Carrier;
    let m = Moves::new("clonefail", false);
    std::fs::write(m.on_a.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(m.on_a.join(".env"), "K=1\n").unwrap();
    let mut a = m.a.client();
    let mut b = m.b.client();
    a.subscribe(0);
    b.subscribe(0);
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "remember fig",
    );
    a.wait_turns(&s, 1);
    for id in ["mvf1", "mvf2"] {
        let mut c = clone_of(&m, &[".env"]);
        c.bring.remotes[0].url = format!("file://{}/nowhere.git", m.root.display());
        let r = drive(
            &mut a,
            &mut b,
            Carrier::new(&s, id, "b").clone_there(c),
            None,
            || {},
        );
        let why = r.unwrap_err();
        assert!(why.contains("couldn't clone"), "{why}");
    }
    assert!(
        projects_on(&m.root.join("home-b")).is_empty(),
        "{:?}",
        projects_on(&m.root.join("home-b"))
    );
    assert!(
        std::fs::read_dir(m.b.state.join("bring"))
            .map(|d| d.count())
            .unwrap_or(0)
            == 0,
        "staged files left"
    );
    // Not fenced: it carries on here.
    let n = a.turns_done(&s);
    a.say(&s, "r", "recall", false);
    a.wait_turns(&s, n + 1);
}

/// The machine it goes to restarts mid-clone: asked again with the same
/// move, it clones again in the same folder (no `app-2`), and the move
/// then goes as usual.
#[test]
fn a_restart_mid_clone_clones_again_in_the_same_place() {
    use dent_client_core::carry::Carrier;
    let mut m = Moves::new("clonerestart", false);
    m.b.kill();
    m.b.env.push(("DENT_TEST_SLOW_CLONE".into(), "1".into()));
    m.b.start();
    let mut a = m.a.client();
    a.subscribe(0);
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "remember kiwi",
    );
    a.wait_turns(&s, 1);
    let mut b = m.b.client();
    let bring = clone_of(&m, &[]).bring;
    let ask = |b: &mut Client, with: bool| {
        reply(b.call(Request::BringProject {
            id: "mvr1".into(),
            bring: with.then(|| bring.clone()),
        }))
    };
    assert_eq!(ask(&mut b, true), Ok(dent_protocol::Reply::Cloning));
    // Gone while it held the clone open.
    m.b.kill();
    m.b.env.retain(|(k, _)| k != "DENT_TEST_SLOW_CLONE");
    m.b.start();
    let mut b = m.b.client();
    b.subscribe(0);
    let forgot = ask(&mut b, false).unwrap_err();
    assert!(forgot.contains("no such clone"), "{forgot}");
    let c = Carrier::new(&s, "mvr1", "b").clone_there(clone_of(&m, &[]));
    let to = drive(&mut a, &mut b, c, None, || {}).unwrap().unwrap();
    assert_eq!(projects_on(&m.root.join("home-b")), ["app"]);
    b.pump_for(Duration::from_millis(300));
    let n = b.turns_done(&to);
    b.say(&to, "r", "recall", false);
    b.wait_turns(&to, n + 1);
    assert!(b.text(&to).contains("remember kiwi"));
}

/// A ticked file gone by the time it's read: the move stops before any
/// clone, saying which.
#[test]
fn a_ticked_file_gone_stops_the_move_before_any_clone() {
    use dent_client_core::carry::Carrier;
    let m = Moves::new("filegone", false);
    std::fs::write(m.on_a.join(".gitignore"), ".env*\n").unwrap();
    std::fs::write(m.on_a.join(".env"), "K=1\n").unwrap();
    let mut a = m.a.client();
    let mut b = m.b.client();
    a.subscribe(0);
    let s = a.create(&m.on_a.display().to_string(), Permission::Full, "hello");
    a.wait_turns(&s, 1);
    let c = Carrier::new(&s, "mvg1", "b").clone_there(clone_of(&m, &[".env", ".env.gone"]));
    let why = drive(&mut a, &mut b, c, None, || {}).unwrap_err();
    assert!(why.contains(".env.gone"), "{why}");
    assert!(projects_on(&m.root.join("home-b")).is_empty());
}

/// Back and forth three times, its branch checked out on both machines:
/// every hop's work arrives, nothing of either checkout moves, the branch
/// it lands on doesn't grow a name per hop, and it remembers.
#[test]
fn a_session_moved_back_and_forth_keeps_everything() {
    let m = Moves::new("pingpong", true);
    git_in(&m.on_b, &["checkout", "-q", "work"]);
    let (b_head, a_head) = (
        git_in(&m.on_b, &["rev-parse", "HEAD"]),
        git_in(&m.on_a, &["rev-parse", "HEAD"]),
    );
    let mut a = m.a.client();
    let mut b = m.b.client();
    a.subscribe(0);
    b.subscribe(0);
    b.call(Request::AddProject {
        path: m.on_b.display().to_string(),
        create: false,
    })
    .unwrap();
    std::fs::write(m.on_a.join("hop0.txt"), "0\n").unwrap();
    git_in(&m.on_a, &["add", "hop0.txt"]);
    git_in(&m.on_a, &["commit", "-qm", "hop 0"]);
    let a_head = {
        let _ = a_head;
        git_in(&m.on_a, &["rev-parse", "HEAD"])
    };
    let s = a.create(
        &m.on_a.display().to_string(),
        Permission::Full,
        "remember the word quince",
    );
    a.wait_turns(&s, 1);
    let mut here = s.clone();
    let mut branches = Vec::new();
    for hop in 1..=3 {
        let (from, to): (&mut Client, &mut Client) = if hop % 2 == 1 {
            (&mut a, &mut b)
        } else {
            (&mut b, &mut a)
        };
        let next = carry(from, to, &here, &format!("pp{hop}"), 4096, None)
            .unwrap()
            .unwrap();
        to.pump_for(Duration::from_millis(300));
        let cwd = cwd_of(to, &next);
        branches.push(git_in(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"]));
        // Work there, not committed, for the next hop to carry.
        std::fs::write(cwd.join(format!("hop{hop}.txt")), format!("{hop}\n")).unwrap();
        here = next;
    }
    // It ended on b, with every hop's work.
    let cwd = cwd_of(&b, &here);
    for hop in 0..=3 {
        assert_eq!(
            std::fs::read_to_string(cwd.join(format!("hop{hop}.txt"))).unwrap(),
            format!("{hop}\n"),
            "hop {hop} in {}",
            cwd.display()
        );
    }
    assert!(
        branches.iter().all(|b| b.starts_with("work-moved")),
        "{branches:?}"
    );
    assert!(
        !branches.iter().any(|b| b.contains("moved-moved")),
        "{branches:?}"
    );
    assert_eq!(git_in(&m.on_b, &["rev-parse", "HEAD"]), b_head);
    assert_eq!(git_in(&m.on_a, &["rev-parse", "HEAD"]), a_head);
    let n = b.turns_done(&here);
    b.say(&here, "r", "recall", false);
    b.wait_turns(&here, n + 1);
    assert!(b.text(&here).contains("remember the word quince"));
}

/// A branch with a slash, checked out there; and a session working in a
/// subfolder, moved with a clone: each lands where it should.
#[test]
fn slashed_branches_and_subfolders_land_where_they_should() {
    use dent_client_core::carry::Carrier;
    let m = Moves::new("slash", false);
    git_in(&m.on_a, &["checkout", "-q", "-b", "feat/login-fix"]);
    std::fs::create_dir_all(m.on_a.join("web/src")).unwrap();
    std::fs::write(m.on_a.join("web/src/a.ts"), "x\n").unwrap();
    git_in(&m.on_a, &["add", "."]);
    git_in(&m.on_a, &["commit", "-qm", "web"]);
    git_in(&m.on_a, &["push", "-q", "origin", "feat/login-fix"]);
    git_in(
        &m.root,
        &[
            "--git-dir",
            "origin.git",
            "symbolic-ref",
            "HEAD",
            "refs/heads/feat/login-fix",
        ],
    );
    std::fs::write(m.on_a.join("web/src/b.ts"), "unpushed\n").unwrap();
    git_in(&m.on_a, &["add", "."]);
    git_in(&m.on_a, &["commit", "-qm", "not pushed"]);
    let mut a = m.a.client();
    let mut b = m.b.client();
    a.subscribe(0);
    b.subscribe(0);
    let sub = m.on_a.join("web");
    let s = a.create(&sub.display().to_string(), Permission::Full, "hello");
    a.wait_turns(&s, 1);
    let mut c = clone_of(&m, &[]);
    c.bring.branch = Some("feat/login-fix".into());
    let to = drive(
        &mut a,
        &mut b,
        Carrier::new(&s, "mvs1", "b").clone_there(c),
        None,
        || {},
    )
    .unwrap()
    .unwrap();
    b.pump_for(Duration::from_millis(300));
    let cwd = cwd_of(&b, &to);
    assert!(cwd.ends_with("web"), "{}", cwd.display());
    assert_eq!(
        std::fs::read_to_string(cwd.join("src/b.ts")).unwrap(),
        "unpushed\n"
    );
    assert_eq!(
        git_in(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "feat/login-fix"
    );
}

/// Six sessions of one repository moved at once to a machine without it,
/// cloning it on the way, that machine killed partway and started again:
/// each carrier tries again with its move, as Retry does, and every
/// session arrives whole, the project cloned once.
#[test]
fn six_moves_through_a_crash_mid_clone_all_arrive_and_clone_once() {
    use dent_client_core::carry::Carrier;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    const N: usize = 6;
    let mut m = Moves::new("crash6", false);
    m.b.kill();
    m.b.env.push(("DENT_TEST_SLOW_CLONE".into(), "1".into()));
    m.b.start();
    let mut a = m.a.client();
    a.subscribe(0);
    let sessions: Vec<String> = (0..N)
        .map(|i| {
            let s = a.create(
                &m.on_a.display().to_string(),
                Permission::Full,
                &format!("remember the number {i}"),
            );
            a.wait_turns(&s, 1);
            s
        })
        .collect();
    let (sock_a, sock_b) = (
        dent_protocol::daemon_socket(&m.a.state),
        dent_protocol::daemon_socket(&m.b.state),
    );
    let clone = clone_of(&m, &[]);
    static TRIES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let handles: Vec<_> = sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (s, sa, sb, clone) = (s.clone(), sock_a.clone(), sock_b.clone(), clone.clone());
            std::thread::spawn(move || {
                let until = Instant::now() + Duration::from_secs(90);
                loop {
                    assert!(Instant::now() < until, "{s} never arrived");
                    if std::os::unix::net::UnixStream::connect(&sb).is_err() {
                        std::thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                    TRIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let got = catch_unwind(AssertUnwindSafe(|| {
                        let (mut a, mut b) = (Client::connect(&sa), Client::connect(&sb));
                        let c =
                            Carrier::new(&s, &format!("mvc{i}"), "b").clone_there(clone.clone());
                        drive(&mut a, &mut b, c, None, || {})
                    }));
                    match got {
                        Ok(Ok(Ok(to))) => return to,
                        // Stopped, or the link went with the machine: again.
                        _ => std::thread::sleep(Duration::from_millis(300)),
                    }
                }
            })
        })
        .collect();
    // Killed while it holds the clone open, and back at once.
    std::thread::sleep(Duration::from_millis(400));
    m.b.kill();
    m.b.env.retain(|(k, _)| k != "DENT_TEST_SLOW_CLONE");
    m.b.start();
    let arrived: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let distinct: std::collections::HashSet<_> = arrived.iter().collect();
    assert_eq!(distinct.len(), N, "{arrived:?}");
    // The crash did cut some short: they tried again.
    assert!(TRIES.load(std::sync::atomic::Ordering::Relaxed) > N);
    assert_eq!(projects_on(&m.root.join("home-b")), ["app"]);
    let mut b = m.b.client();
    b.subscribe(0);
    b.pump_for(Duration::from_millis(500));
    for (i, to) in arrived.iter().enumerate() {
        let n = b.turns_done(to);
        b.say(to, "r", "recall", false);
        b.wait_turns(to, n + 1);
        let text = b.text(to);
        assert!(text.contains(&format!("remember the number {i}")), "{text}");
    }
    // Every one left a, as a record pointing there.
    a.pump_for(Duration::from_millis(300));
    for s in &sessions {
        assert!(a.events(s).iter().any(|e| matches!(
            e,
            Body::Move {
                state: dent_protocol::MoveState::Departed,
                ..
            }
        )));
    }
}
