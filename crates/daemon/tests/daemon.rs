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
            file("a/escape.txt", b"two"),
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
    assert_eq!(paths.len(), 2, "{paths:?}");
    // Kept to their message's folder, whatever the names say, and two
    // files of one name both kept.
    for p in &paths {
        assert!(p.contains("/attachments/carried/m-carry/"), "{p}");
    }
    assert_ne!(paths[0], paths[1]);
    assert_eq!(std::fs::read(&paths[0]).unwrap(), b"one");
    assert_eq!(std::fs::read(&paths[1]).unwrap(), b"two");
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
    c.pump(Duration::from_millis(100));
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
    c.pump(Duration::from_millis(100));
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
        c.pump(Duration::from_millis(50));
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
    c.pump(Duration::from_millis(300));
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
    c.pump(Duration::from_millis(100));
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
