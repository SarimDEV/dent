//! The connection to this machine's daemon. A thread owns the socket: it
//! starts the daemon when nothing answers, reconnects when it goes away, and
//! resends requests that never got a reply (the daemon recognises a resent
//! message by its id, so nothing runs twice). The window hears about it
//! through a channel.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use dent_protocol::{Out, PROTOCOL, Reply, Request, encode};
use futures::channel::mpsc::UnboundedSender;

/// What the window hears.
pub enum Incoming {
    /// Connected (again): the daemon's welcome.
    Up {
        machine: String,
    },
    Down(String),
    Out {
        req: u64,
        out: Out,
    },
}

/// The window's handle for sending requests.
#[derive(Clone)]
pub struct Live {
    tx: Sender<(u64, Request)>,
    next: Arc<AtomicU64>,
    /// The last event the window has applied; reconnects subscribe from here.
    pub seq: Arc<AtomicU64>,
}

impl Live {
    pub fn start(state: PathBuf, ui: UnboundedSender<Incoming>) -> Self {
        let (tx, rx) = mpsc::channel();
        let seq = Arc::new(AtomicU64::new(0));
        let s = seq.clone();
        std::thread::Builder::new()
            .name("dent-live".into())
            .spawn(move || run(state, rx, ui, s))
            .ok();
        Self {
            tx,
            next: Arc::new(AtomicU64::new(100)),
            seq,
        }
    }

    /// Send a request; its reply comes back as [`Incoming::Out`] with the
    /// returned number.
    pub fn send(&self, r: Request) -> u64 {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send((n, r));
        n
    }
}

pub fn default_state() -> PathBuf {
    if let Some(s) = std::env::var_os("DENT_STATE") {
        return PathBuf::from(s);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/dent")
}

fn run(
    state: PathBuf,
    rx: Receiver<(u64, Request)>,
    ui: UnboundedSender<Incoming>,
    seq: Arc<AtomicU64>,
) {
    let sock = state.join("daemon.sock");
    // Requests not yet answered, resent after a reconnect.
    let mut pending: BTreeMap<u64, Request> = BTreeMap::new();
    let mut started_at: Option<Instant> = None;
    let mut wait = Duration::from_millis(50);
    loop {
        let stream = match UnixStream::connect(&sock) {
            Ok(s) => s,
            Err(_) => {
                if started_at.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
                    started_at = Some(Instant::now());
                    if let Err(e) = start_daemon(&state) {
                        let _ = ui.unbounded_send(Incoming::Down(e));
                    }
                }
                // Take requests while waiting, so none is lost.
                match rx.recv_timeout(wait) {
                    Ok((n, r)) => {
                        pending.insert(n, r);
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                wait = (wait * 2).min(Duration::from_secs(2));
                continue;
            }
        };
        wait = Duration::from_millis(50);
        let Ok(reader) = stream.try_clone() else {
            continue;
        };
        let mut w = stream;
        let hello = Request::Hello {
            proto: PROTOCOL,
            client: "desktop".into(),
        };
        if w.write_all(&encode(1, &hello)).is_err() {
            continue;
        }
        let after = seq.load(Ordering::Relaxed);
        if w.write_all(&encode(2, &Request::Subscribe { after }))
            .is_err()
        {
            continue;
        }
        for (n, r) in &pending {
            let _ = w.write_all(&encode(*n, r));
        }
        // Lines from the daemon come through a reader thread; replies clear
        // pending requests here.
        let (done_tx, done_rx) = mpsc::channel::<u64>();
        let ui2 = ui.clone();
        let reading = std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                let req = v["req"].as_u64().unwrap_or(0);
                let Ok(out) = serde_json::from_value::<Out>(v) else {
                    continue;
                };
                if req != 0 {
                    let _ = done_tx.send(req);
                }
                let msg = match out {
                    Out::Reply {
                        reply: Reply::Welcome { machine, .. },
                    } if req == 1 => Incoming::Up { machine },
                    Out::Error { message } if req == 1 => Incoming::Down(message),
                    out => Incoming::Out { req, out },
                };
                if ui2.unbounded_send(msg).is_err() {
                    break;
                }
            }
        });
        loop {
            while let Ok(n) = done_rx.try_recv() {
                pending.remove(&n);
            }
            if reading.is_finished() {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok((n, r)) => {
                    let line = encode(n, &r);
                    pending.insert(n, r);
                    if w.write_all(&line).is_err() {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let _ = w.shutdown(std::net::Shutdown::Both);
        let _ = reading.join();
        while let Ok(n) = done_rx.try_recv() {
            pending.remove(&n);
        }
        let _ = ui.unbounded_send(Incoming::Down("dent's background service restarted".into()));
    }
}

/// Start the daemon from beside this app, in its own process group so it
/// outlives the window.
fn start_daemon(state: &Path) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let bin = me.with_file_name("dent-daemon");
    if !bin.exists() {
        return Err(format!(
            "dent-daemon isn't installed next to {}",
            me.display()
        ));
    }
    std::fs::create_dir_all(state).map_err(|e| e.to_string())?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join("daemon.log"))
        .map_err(|e| e.to_string())?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("--state")
        .arg(state)
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(err);
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("couldn't start dent-daemon: {e}"))?;
    // Reap it if it exits while we run (say, another daemon already had the lock).
    std::thread::spawn(move || child.wait());
    Ok(())
}
