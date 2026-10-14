//! The session holder: a tiny process that owns agent processes, so they
//! keep running while the daemon restarts or upgrades.
//!
//! It does four things: start a process, pass stdin to it, copy its output
//! to a file, and stream that file to the daemon from any offset. Output is
//! on disk, so a daemon that was away catches up exactly, and a stuck daemon
//! never blocks an agent. Standard library only, plus libc for signals.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::wire::{self, Msg, PROTO, PROTO_MIN, Req, now_ms, read_frame, write_frame};

pub struct Options {
    /// Private directory for the socket, the lock and process output.
    pub dir: PathBuf,
    /// Exit after this long with no processes and no daemon.
    pub idle: Duration,
    /// With no daemon for this long, close every process's stdin so agents
    /// finish what they're doing and exit, rather than wait forever for a
    /// daemon that isn't coming back.
    pub orphan: Duration,
    /// Overrides for testing upgrades.
    pub proto: u32,
    pub version: String,
}

impl Options {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            idle: Duration::from_secs(60),
            orphan: Duration::from_secs(600),
            proto: PROTO,
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

pub fn socket(dir: &Path) -> PathBuf {
    dir.join("holder.sock")
}

/// Once the agent has exited, how long to wait for anything it started
/// to let go of its output before calling the output finished.
const LINGER: Duration = Duration::from_secs(2);
const CHUNK: usize = 256 * 1024;

struct Holder {
    o: Options,
    procs: Mutex<HashMap<String, Arc<Proc>>>,
    /// The connection allowed to act. A newer daemon's hello takes over.
    current: Mutex<(u64, Option<Writer>)>,
    next: AtomicU64,
    draining: AtomicBool,
    idle_since: Mutex<Instant>,
}

type Writer = Arc<Mutex<UnixStream>>;

struct Proc {
    id: String,
    pid: u32,
    argv: Vec<String>,
    started: u64,
    dir: PathBuf,
    stdin: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    out: u64,
    err: u64,
    out_done: bool,
    err_done: bool,
    exit: Option<(Option<i32>, Option<i32>)>,
}

impl State {
    fn finished(&self) -> bool {
        self.exit.is_some() && self.out_done && self.err_done
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicked thread can't leave these maps half-updated in a way that
    // matters more than stopping would.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run the holder until it is idle. Returns straight away if another
/// holder already owns `dir`.
pub fn serve(o: Options) -> Result<()> {
    std::fs::create_dir_all(o.dir.join("procs"))?;
    let meta = std::fs::metadata(&o.dir)?;
    // SAFETY: getuid has no preconditions.
    if std::os::unix::fs::MetadataExt::uid(&meta) != unsafe { libc::getuid() } {
        bail!("{} belongs to another user", o.dir.display());
    }
    std::fs::set_permissions(&o.dir, std::fs::Permissions::from_mode(0o700))?;
    let lockf = File::create(o.dir.join("holder.lock"))?;
    // SAFETY: a valid open descriptor; the lock lives as long as `lockf`.
    if unsafe {
        libc::flock(
            std::os::fd::AsRawFd::as_raw_fd(&lockf),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    } != 0
    {
        eprintln!("holder: already running");
        return Ok(());
    }
    // The lock file names the holder, for tools and tests.
    let _ = lockf.set_len(0);
    let _ = write!(&lockf, "{}", std::process::id());
    // Leave the daemon's process group and session, so a service manager
    // stopping the daemon, or ^C in its terminal, doesn't reach us.
    // SAFETY: setsid has no preconditions; it fails harmlessly if we
    // already lead a group.
    unsafe { libc::setsid() };
    let sock = socket(&o.dir);
    // Unix socket paths are short (104 bytes on macOS).
    if sock.as_os_str().len() > 100 {
        bail!("socket path too long: {}", sock.display());
    }
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).context("bind holder socket")?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    eprintln!(
        "holder {} (proto {}) pid {}",
        o.version,
        o.proto,
        std::process::id()
    );
    let h = Arc::new(Holder {
        o,
        procs: Mutex::new(HashMap::new()),
        current: Mutex::new((0, None)),
        next: AtomicU64::new(1),
        draining: AtomicBool::new(false),
        idle_since: Mutex::new(Instant::now()),
    });
    {
        let h = h.clone();
        std::thread::spawn(move || h.watch_idle(lockf));
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let h = h.clone();
        std::thread::spawn(move || h.conn(stream));
    }
    Ok(())
}

impl Holder {
    fn watch_idle(&self, _lock: File) {
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let empty = lock(&self.procs).is_empty();
            let alone = lock(&self.current).1.is_none();
            let idle = lock(&self.idle_since).elapsed() >= self.o.idle;
            if alone && !empty && lock(&self.idle_since).elapsed() >= self.o.orphan {
                for p in lock(&self.procs).values() {
                    if lock(&p.stdin).take().is_some() {
                        eprintln!("holder: no daemon for a while; closing stdin of {}", p.id);
                    }
                }
            }
            if empty && (self.draining.load(Ordering::SeqCst) || (alone && idle)) {
                let _ = std::fs::remove_file(socket(&self.o.dir));
                eprintln!("holder: idle, exiting");
                std::process::exit(0);
            }
        }
    }

    fn is_current(&self, conn: u64) -> bool {
        lock(&self.current).0 == conn
    }

    fn conn(self: Arc<Self>, stream: UnixStream) {
        let Ok(w) = stream.try_clone() else { return };
        let w = Arc::new(Mutex::new(w));
        let mut r = BufReader::new(stream);
        let attaches = Arc::new(Mutex::new(HashMap::<String, u64>::new()));
        let mut me = 0;
        while let Ok(Some((env, payload))) = read_frame::<Req>(&mut r) {
            let reply = match env.body {
                Req::Hello { proto } if proto >= PROTO_MIN => {
                    me = self.next.fetch_add(1, Ordering::SeqCst);
                    let mut cur = lock(&self.current);
                    if let Some(old) = cur.1.take() {
                        // Tell the old daemon why, so it stops rather than
                        // taking over back.
                        let mut old = lock(&old);
                        let msg = Msg::Error {
                            msg: "replaced".into(),
                        };
                        let _ = write_frame(&mut *old, 0, msg, &[]);
                        let _ = old.shutdown(std::net::Shutdown::Both);
                    }
                    *cur = (me, Some(w.clone()));
                    Msg::Hello {
                        proto: self.o.proto,
                        min: PROTO_MIN,
                        version: self.o.version.clone(),
                        pid: std::process::id(),
                    }
                }
                Req::Hello { .. } => Msg::Error {
                    msg: format!("version: holder needs proto {PROTO_MIN} or newer"),
                },
                _ if me == 0 => Msg::Error {
                    msg: "hello first".into(),
                },
                _ if !self.is_current(me) => break,
                Req::Attach { id, out, err } => match self.get(&id) {
                    Some(p) => {
                        let token = self.next.fetch_add(1, Ordering::SeqCst);
                        lock(&attaches).insert(id.clone(), token);
                        let (h, w, a) = (self.clone(), w.clone(), attaches.clone());
                        std::thread::spawn(move || {
                            let _ = h.stream(p, (out, err), w, me, (a, token));
                        });
                        Msg::Ok
                    }
                    None => missing(&id),
                },
                body => self.handle(body, payload),
            };
            if write_frame(&mut *lock(&w), env.req, reply, &[]).is_err() {
                break;
            }
        }
        let mut cur = lock(&self.current);
        if cur.0 == me {
            *cur = (0, None);
            *lock(&self.idle_since) = Instant::now();
        }
    }

    fn get(&self, id: &str) -> Option<Arc<Proc>> {
        lock(&self.procs).get(id).cloned()
    }

    fn handle(&self, body: Req, payload: Vec<u8>) -> Msg {
        let res = match body {
            Req::Spawn { id, argv, cwd, env } => self.spawn(id, argv, cwd, env),
            Req::Write { id } => match self.get(&id) {
                Some(p) => match lock(&p.stdin).as_ref() {
                    Some(tx) if tx.send(payload).is_ok() => Ok(Msg::Ok),
                    _ => Err("stdin closed".into()),
                },
                None => return missing(&id),
            },
            Req::CloseStdin { id } => match self.get(&id) {
                Some(p) => {
                    lock(&p.stdin).take();
                    Ok(Msg::Ok)
                }
                None => return missing(&id),
            },
            Req::Stop { id, grace_ms } => match self.get(&id) {
                Some(p) => {
                    stop(p, Duration::from_millis(grace_ms));
                    Ok(Msg::Ok)
                }
                None => return missing(&id),
            },
            Req::Release { id } => {
                let mut procs = lock(&self.procs);
                match procs.get(&id) {
                    Some(p) if !lock(&p.state).finished() => Err("still running".into()),
                    Some(p) => {
                        let _ = std::fs::remove_dir_all(&p.dir);
                        procs.remove(&id);
                        *lock(&self.idle_since) = Instant::now();
                        Ok(Msg::Ok)
                    }
                    None => return missing(&id),
                }
            }
            Req::List => Ok(Msg::Procs {
                procs: lock(&self.procs)
                    .values()
                    .map(|p| {
                        let s = lock(&p.state);
                        wire::Proc {
                            id: p.id.clone(),
                            pid: p.pid,
                            argv: p.argv.clone(),
                            started: p.started,
                            out: s.out,
                            err: s.err,
                            exited: s.finished(),
                        }
                    })
                    .collect(),
            }),
            Req::ExitWhenIdle => {
                self.draining.store(true, Ordering::SeqCst);
                Ok(Msg::Ok)
            }
            Req::Hello { .. } | Req::Attach { .. } => Err("unexpected".into()),
        };
        res.unwrap_or_else(|msg: String| Msg::Error { msg })
    }

    fn spawn(
        &self,
        id: String,
        argv: Vec<String>,
        cwd: Option<String>,
        env: Vec<(String, String)>,
    ) -> Result<Msg, String> {
        if self.draining.load(Ordering::SeqCst) {
            return Err("draining".into());
        }
        let ok_id = !id.is_empty()
            && id.len() <= 128
            && !id.starts_with('.')
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
        if !ok_id {
            return Err("bad id".into());
        }
        let Some((cmd, args)) = argv.split_first() else {
            return Err("empty argv".into());
        };
        // Hold the map for the whole spawn so the same id can't start twice.
        let mut procs = lock(&self.procs);
        if procs.contains_key(&id) {
            return Err("exists".into());
        }
        let dir = self.o.dir.join("procs").join(&id);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut c = Command::new(cmd);
        c.args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own group, so stopping it also stops what it started.
            .process_group(0);
        if let Some(cwd) = cwd {
            c.current_dir(cwd);
        }
        let mut child = match c.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(format!("spawn {cmd}: {e}"));
            }
        };
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let p = Arc::new(Proc {
            id: id.clone(),
            pid: child.id(),
            argv: argv.clone(),
            started: now_ms(),
            dir: dir.clone(),
            stdin: Mutex::new(Some(tx)),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        if let Some(mut stdin) = child.stdin.take() {
            // Writes queue here, so an agent that stops reading never holds
            // up the daemon's other requests.
            std::thread::spawn(move || {
                while let Ok(buf) = rx.recv() {
                    if stdin.write_all(&buf).is_err() {
                        break;
                    }
                }
            });
        }
        for (src, err) in [
            (
                child
                    .stdout
                    .take()
                    .map(|s| Box::new(s) as Box<dyn Read + Send>),
                false,
            ),
            (
                child
                    .stderr
                    .take()
                    .map(|s| Box::new(s) as Box<dyn Read + Send>),
                true,
            ),
        ] {
            let p = p.clone();
            let path = dir.join(if err { "err" } else { "out" });
            std::thread::spawn(move || pump(p, src, path, err));
        }
        {
            let p = p.clone();
            std::thread::spawn(move || {
                let status = child.wait().ok();
                let mut s = lock(&p.state);
                s.exit = Some((
                    status.and_then(|s| s.code()),
                    status.and_then(|s| s.signal()),
                ));
                p.changed.notify_all();
                let until = Instant::now() + LINGER;
                while !(s.out_done && s.err_done) && Instant::now() < until {
                    s = p
                        .changed
                        .wait_timeout(s, Duration::from_millis(50))
                        .map(|r| r.0)
                        .unwrap_or_else(|e| e.into_inner().0);
                }
                // Something it started still holds the output open: stop
                // waiting for it.
                s.out_done = true;
                s.err_done = true;
                p.changed.notify_all();
            });
        }
        let pid = p.pid;
        procs.insert(id, p);
        Ok(Msg::Spawned { pid })
    }

    /// Send a process's output from `from` on, as it arrives, then its exit.
    fn stream(
        &self,
        p: Arc<Proc>,
        from: (u64, u64),
        w: Arc<Mutex<UnixStream>>,
        conn: u64,
        (attaches, token): (Arc<Mutex<HashMap<String, u64>>>, u64),
    ) -> std::io::Result<()> {
        let (mut out, mut err) = from;
        let mut files = (None::<File>, None::<File>);
        let mut buf = vec![0; CHUNK];
        loop {
            let (lens, finished) = {
                let mut s = lock(&p.state);
                while s.out == out && s.err == err && !s.finished() && self.is_current(conn) {
                    s = p
                        .changed
                        .wait_timeout(s, Duration::from_millis(500))
                        .map(|r| r.0)
                        .unwrap_or_else(|e| e.into_inner().0);
                }
                ((s.out, s.err), s.finished())
            };
            if !self.is_current(conn) || lock(&attaches).get(&p.id) != Some(&token) {
                return Ok(());
            }
            for (is_err, at, len, file) in [
                (false, &mut out, lens.0, &mut files.0),
                (true, &mut err, lens.1, &mut files.1),
            ] {
                while *at < len {
                    if file.is_none() {
                        *file = Some(File::open(p.dir.join(if is_err { "err" } else { "out" }))?);
                    }
                    let f = file.as_mut().ok_or(std::io::ErrorKind::NotFound)?;
                    f.seek(SeekFrom::Start(*at))?;
                    let want = ((len - *at) as usize).min(CHUNK);
                    f.read_exact(&mut buf[..want])?;
                    let msg = Msg::Data {
                        id: p.id.clone(),
                        err: is_err,
                        off: *at,
                    };
                    write_frame(&mut *lock(&w), 0, msg, &buf[..want])?;
                    *at += want as u64;
                }
            }
            if finished && (out, err) == lens {
                let (code, signal) = lock(&p.state).exit.unwrap_or_default();
                let msg = Msg::Exit {
                    id: p.id.clone(),
                    code,
                    signal,
                };
                return write_frame(&mut *lock(&w), 0, msg, &[]);
            }
        }
    }
}

fn missing(id: &str) -> Msg {
    Msg::Error {
        msg: format!("no process {id}"),
    }
}

/// Copy one output pipe to its file, counting what's safely written.
fn pump(p: Arc<Proc>, src: Option<Box<dyn Read + Send>>, path: PathBuf, err: bool) {
    let file = OpenOptions::new().create(true).append(true).open(&path);
    if let (Some(mut src), Ok(mut file)) = (src, file) {
        let mut buf = vec![0; 64 * 1024];
        loop {
            let n = match src.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            // A full disk loses output but never blocks the agent.
            if file.write_all(&buf[..n]).is_err() {
                continue;
            }
            let mut s = lock(&p.state);
            if err {
                s.err += n as u64;
            } else {
                s.out += n as u64;
            }
            p.changed.notify_all();
        }
    }
    let mut s = lock(&p.state);
    if err {
        s.err_done = true;
    } else {
        s.out_done = true;
    }
    p.changed.notify_all();
}

fn stop(p: Arc<Proc>, grace: Duration) {
    let group = -(p.pid as i32);
    // SAFETY: kill has no memory preconditions.
    unsafe { libc::kill(group, libc::SIGTERM) };
    std::thread::spawn(move || {
        let until = Instant::now() + grace;
        let mut s = lock(&p.state);
        while s.exit.is_none() && Instant::now() < until {
            s = p
                .changed
                .wait_timeout(s, Duration::from_millis(50))
                .map(|r| r.0)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        drop(s);
        // Also catches anything it started that ignored the first signal.
        // SAFETY: as above.
        unsafe { libc::kill(group, libc::SIGKILL) };
    });
}
