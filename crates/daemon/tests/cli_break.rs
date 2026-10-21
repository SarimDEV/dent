//! `dent` (the CLI) against a real daemon and against machines that
//! misbehave: what a script calling it would see. The CLI keeps its key and
//! saved machines in `DENT_CLIENT_STATE`, a fresh folder per test.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use dent_protocol::{Envelope, Out, PROTOCOL, Reply, Request, encode};

fn rand_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ (N.fetch_add(1, Ordering::Relaxed) << 48)
}

fn bins() -> (PathBuf, PathBuf) {
    let dir = Path::new(env!("CARGO_BIN_EXE_dent-daemon"))
        .parent()
        .unwrap();
    let built = Command::new(env!("CARGO"))
        .args([
            "build",
            "-q",
            "-p",
            "dent-cli",
            "-p",
            "dent-fake-claude",
            "-p",
            "dent-session-holder",
        ])
        .status()
        .unwrap();
    assert!(built.success());
    (dir.join("dent"), dir.join("dent-fake-claude"))
}

struct Daemon {
    root: PathBuf,
    state: PathBuf,
    port: u16,
    child: Child,
}

impl Daemon {
    fn start(fake: &Path) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dent-cli-break-{}-{:x}",
            std::process::id(),
            rand_u64()
        ));
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let child = Command::new(env!("CARGO_BIN_EXE_dent-daemon"))
            .arg("--state")
            .arg(&state)
            .env("DENT_REMOTE_PORT", port.to_string())
            .env("DENT_TAILSCALE_LOCALAPI", "127.0.0.1:1")
            .env("DENT_CLAUDE", fake)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err()
            || UnixStream::connect(dent_protocol::daemon_socket(&state)).is_err()
        {
            assert!(Instant::now() < until, "daemon didn't start");
            std::thread::sleep(Duration::from_millis(20));
        }
        Self {
            root,
            state,
            port,
            child,
        }
    }

    fn ask(&self, req: Request) -> Reply {
        let mut w = UnixStream::connect(dent_protocol::daemon_socket(&self.state)).unwrap();
        let mut r = BufReader::new(w.try_clone().unwrap());
        w.write_all(&encode(
            1,
            &Request::Hello {
                proto: PROTOCOL,
                client: "test".into(),
            },
        ))
        .unwrap();
        w.write_all(&encode(2, &req)).unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            assert!(r.read_line(&mut line).unwrap() > 0, "socket closed");
            let Ok(env) = serde_json::from_str::<Envelope<Out>>(&line) else {
                continue;
            };
            if env.req == 2 {
                match env.body {
                    Out::Reply { reply } => return reply,
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
    }

    /// A session working in `work`.
    fn session(&self, work: &Path) -> String {
        match self.ask(Request::Create {
            agent: None,
            cwd: work.display().to_string(),
            worktree: None,
            model: None,
            permission: Default::default(),
            effort: None,
            id: format!("t{:x}", rand_u64()),
            text: "hello".into(),
            attachments: Vec::new(),
            files: Vec::new(),
            bring: None,
        }) {
            Reply::Created { session } => session,
            o => panic!("{o:?}"),
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Runs `dent args…` with this client state, giving up after `limit`.
fn dent(bin: &Path, client: &Path, cwd: &Path, args: &[&str], limit: Duration) -> Output {
    let mut c = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .env("DENT_CLIENT_STATE", client)
        .env_remove("DENT_STATE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let until = Instant::now() + limit;
    loop {
        if c.try_wait().unwrap().is_some() {
            return c.wait_with_output().unwrap();
        }
        if Instant::now() > until {
            let _ = c.kill();
            let out = c.wait_with_output().unwrap();
            panic!(
                "`dent {}` hung for {limit:?}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A client folder with `d` added under its id, which is returned.
fn added(bin: &Path, d: &Daemon, client: &Path) -> String {
    std::fs::create_dir_all(client).unwrap();
    let code = match d.ask(Request::OpenPairing) {
        Reply::Pairing { code, .. } => code,
        o => panic!("{o:?}"),
    };
    let addr = format!("127.0.0.1:{}", d.port);
    let out = dent(
        bin,
        client,
        client,
        &["machine", "add", &addr, "--code", &code],
        Duration::from_secs(40),
    );
    assert!(
        out.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    // "added NAME (ID)"
    text.rsplit('(')
        .next()
        .and_then(|s| s.split(')').next())
        .expect("no id printed")
        .to_string()
}

#[test]
fn get_copies_odd_files_exactly_and_fails_clearly_otherwise() {
    let (bin, fake) = bins();
    let d = Daemon::start(&fake);
    let client = d.root.join("client");
    let id = added(&bin, &d, &client);
    let work = d.root.join("work");
    let odd = work.join("my folder/naïve 文件 #1.bin");
    std::fs::create_dir_all(odd.parent().unwrap()).unwrap();
    // Not a multiple of a read, so the last range is short.
    let body: Vec<u8> = (0..(3 * dent_protocol::READ_MAX as usize + 12_345))
        .map(|i| (i * 31 % 251) as u8)
        .collect();
    std::fs::write(&odd, &body).unwrap();
    std::fs::write(work.join("empty"), b"").unwrap();
    let session = d.session(&work);
    let outdir = d.root.join("out");
    std::fs::create_dir_all(&outdir).unwrap();
    let limit = Duration::from_secs(90);

    // Spaces, unicode and '#' in the path; a destination with them too.
    let dest = outdir.join("got it ✓.bin");
    let o = dent(
        &bin,
        &client,
        &outdir,
        &[
            "get",
            &id,
            &session,
            "my folder/naïve 文件 #1.bin",
            dest.to_str().unwrap(),
        ],
        limit,
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(std::fs::read(&dest).unwrap() == body, "the copy differs");

    // A 0-byte file is a 0-byte copy, not an error or a hang.
    let o = dent(
        &bin,
        &client,
        &outdir,
        &["get", &id, &session, "empty"],
        limit,
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(std::fs::read(outdir.join("empty")).unwrap(), b"");

    // A session that doesn't exist, and a file that doesn't: a clear
    // failure at once, nothing written.
    let start = Instant::now();
    let o = dent(
        &bin,
        &client,
        &outdir,
        &["get", &id, "no-such-session", "x.txt"],
        limit,
    );
    assert!(!o.status.success());
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "took {:?}",
        start.elapsed()
    );
    let o = dent(
        &bin,
        &client,
        &outdir,
        &["get", &id, &session, "nothing.txt"],
        limit,
    );
    assert!(!o.status.success());
    assert!(!outdir.join("nothing.txt").exists());
}

/// A file rewritten over and over while it's fetched: `dent get` may fail
/// saying it changed, or succeed with one whole version, but must never
/// save a file that is neither, nor sit silent for a minute.
#[test]
fn get_of_a_file_rewritten_mid_transfer_is_never_a_mix() {
    let (bin, fake) = bins();
    let d = Daemon::start(&fake);
    let client = d.root.join("client");
    let id = added(&bin, &d, &client);
    let work = d.root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let size = 64 << 20;
    let a = vec![b'a'; size];
    let b = vec![b'b'; size];
    let path = work.join("live.log");
    std::fs::write(&path, &a).unwrap();
    let session = d.session(&work);
    let outdir = d.root.join("out");
    std::fs::create_dir_all(&outdir).unwrap();

    // The writer starts at a different moment each round, so some rounds
    // catch the transfer midway.
    for round in 0..8u64 {
        std::fs::write(&path, &a).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = {
            let (stop, path, a, b) = (stop.clone(), path.clone(), a.clone(), b.clone());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150 + 60 * round));
                let mut flip = false;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    // As an editor or a log rotation might: truncate, write.
                    let _ = std::fs::write(&path, if flip { &a } else { &b });
                    flip = !flip;
                }
            })
        };
        let start = Instant::now();
        let o = dent(
            &bin,
            &client,
            &outdir,
            &["get", &id, &session, "live.log"],
            Duration::from_secs(120),
        );
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        writer.join().unwrap();
        let took = start.elapsed();
        let err = String::from_utf8_lossy(&o.stderr).to_string();
        eprintln!("round {round}: {:?} in {took:?}: {}", o.status, err.trim());
        assert!(
            took < Duration::from_secs(30),
            "round {round}: {took:?} before it ended: {err}"
        );
        if o.status.success() {
            let got = std::fs::read(outdir.join("live.log")).unwrap();
            assert!(
                got == a || got == b,
                "round {round}: saved a file that's neither version ({} bytes, {} a's, {} b's)",
                got.len(),
                got.iter().filter(|c| **c == b'a').count(),
                got.iter().filter(|c| **c == b'b').count(),
            );
        } else {
            assert!(
                err.contains("changed") || err.contains("gone"),
                "round {round}: {err}"
            );
        }
    }
}

/// A saved machine whose address accepts connections and then says
/// nothing (a wedged daemon, a port forward to nowhere): `dent call` ends
/// with an error a script can see, rather than hang.
#[test]
fn call_to_a_machine_that_never_answers_fails_rather_than_hangs() {
    let (bin, _) = bins();
    let root = std::env::temp_dir().join(format!("dent-cli-silent-{:x}", rand_u64()));
    let client = root.join("client");
    std::fs::create_dir_all(&client).unwrap();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let held = std::thread::spawn(move || {
        let mut socks = Vec::new();
        for s in l.incoming().take(200).flatten() {
            socks.push(s);
        }
    });
    let key = dent_link::Keys::generate().unwrap();
    let saved = serde_json::json!([{
        "name": "wedged",
        "id": key.id(),
        "key": hex::encode(&key.public),
        "routes": [format!("127.0.0.1:{port}")],
    }]);
    std::fs::write(client.join("machines.json"), saved.to_string()).unwrap();
    let start = Instant::now();
    let o = dent(
        &bin,
        &client,
        &client,
        &["call", "wedged", r#"{"op":"projects"}"#],
        Duration::from_secs(90),
    );
    assert!(!o.status.success(), "a call nobody answered succeeded");
    assert!(
        !String::from_utf8_lossy(&o.stderr).trim().is_empty(),
        "failed without saying why"
    );
    eprintln!("gave up after {:?}", start.elapsed());
    drop(held);
    let _ = std::fs::remove_dir_all(&root);
}

/// Two saved machines with the same name (two Macs both called "studio" is
/// common: it's the default hostname scheme). Removing one by name must
/// not silently forget both. Debatable whether it should refuse and ask
/// for the id, or remove one; forgetting both is the wrong answer.
#[test]
fn removing_by_a_shared_name_never_forgets_both() {
    let (bin, _) = bins();
    let root = std::env::temp_dir().join(format!("dent-cli-names-{:x}", rand_u64()));
    std::fs::create_dir_all(&root).unwrap();
    let saved = serde_json::json!([
        {"name": "studio", "id": "aaaaaaaaaaaa", "key": "", "routes": ["10.0.0.2:7480"]},
        {"name": "studio", "id": "bbbbbbbbbbbb", "key": "", "routes": ["10.0.0.3:7480"]},
    ]);
    std::fs::write(root.join("machines.json"), saved.to_string()).unwrap();
    let _ = dent(
        &bin,
        &root,
        &root,
        &["machine", "remove", "studio"],
        Duration::from_secs(10),
    );
    let left: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(root.join("machines.json")).unwrap())
            .unwrap();
    assert!(
        !left.is_empty(),
        "both machines named studio were forgotten"
    );
    let _ = std::fs::remove_dir_all(&root);
}
