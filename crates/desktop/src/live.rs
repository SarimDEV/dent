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
        /// Its id: its key's fingerprint, empty from a daemon older than
        /// remote access.
        id: String,
        /// The agents it drives, the default first; empty from a daemon
        /// older than agent ids.
        agents: Vec<dent_protocol::AgentInfo>,
    },
    Down(String),
    Out {
        req: u64,
        out: Out,
    },
    /// Anything from a remote machine, by the id it was started under
    /// ([`crate::net`]).
    Remote {
        id: String,
        event: dent_remote::Event,
    },
    /// A machine being added was named by its welcome: it goes by `to` now.
    Renamed {
        from: String,
        to: String,
    },
}

/// The window's handle for sending requests.
#[derive(Clone)]
pub struct Live {
    tx: Sender<(u64, Request)>,
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
        Self { tx, seq }
    }

    /// Send request number `n`; its reply comes back as [`Incoming::Out`]
    /// with the same number.
    pub fn send_as(&self, n: u64, r: Request) {
        let _ = self.tx.send((n, r));
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
    let sock = dent_protocol::daemon_socket(&state);
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
                    if let Err(e) = start_daemon(&state, ui.clone()) {
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
        if w.write_all(&encode(2, &Request::Subscribe { after, epoch: None }))
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
        let (restart_tx, restart_rx) = mpsc::channel::<()>();
        let ui2 = ui.clone();
        let reading = std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                let Some(dent_protocol::Envelope { req, body: out }) =
                    dent_protocol::read_out(line.as_bytes())
                else {
                    continue;
                };
                if req != 0 {
                    let _ = done_tx.send(req);
                }
                let msg = match out {
                    Out::Reply {
                        reply:
                            Reply::Welcome {
                                machine,
                                build,
                                agents,
                                id,
                                managed,
                                ..
                            },
                    } if req == 1 => {
                        // A daemon older than the one installed beside the app
                        // makes way for it; agents keep running in the holder.
                        // One the service runs is updated through its
                        // launcher instead: restarted, it would come back as
                        // the same version, again and again.
                        let installed = std::env::current_exe()
                            .ok()
                            .map(|p| dent_protocol::build_stamp(&p.with_file_name("dent-daemon")))
                            .unwrap_or_default();
                        // One too old to say its build is older still.
                        if !managed && !installed.is_empty() && installed != build {
                            let _ = restart_tx.send(());
                        }
                        Incoming::Up {
                            machine,
                            id,
                            agents,
                        }
                    }
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
            if restart_rx.try_recv().is_ok() {
                let _ = w.write_all(&encode(3, &Request::Restart));
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
fn start_daemon(state: &Path, ui: UnboundedSender<Incoming>) -> Result<(), String> {
    // Installed as a service (`dent-daemon install`), the launcher runs the
    // daemon and starts it again itself; one started here would take the
    // state folder from it, mid-update even.
    if state.join("launcher.state").is_file() && state.join("bin/dent-launcher").is_file() {
        return Ok(());
    }
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let bin = me.with_file_name("dent-daemon");
    if !bin.exists() {
        return Err(format!(
            "dent-daemon isn't installed next to {}",
            me.display()
        ));
    }
    std::fs::create_dir_all(state).map_err(|e| e.to_string())?;
    // Logs rotate by size, one old file kept.
    let path = state.join("daemon.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 5 * 1024 * 1024) {
        let _ = std::fs::rename(&path, state.join("daemon.log.1"));
    }
    let from = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
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
    // Reap it if it exits while we run. Another daemon holding the lock is a
    // quiet exit; a failure says why, from what it wrote to its log.
    std::thread::spawn(move || {
        let Ok(status) = child.wait() else { return };
        if status.success() {
            return;
        }
        let said = std::fs::read(&path)
            .map(|b| String::from_utf8_lossy(&b[(from as usize).min(b.len())..]).into_owned())
            .unwrap_or_default();
        let why = said
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with("dent-daemon "))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");
        let why = if why.is_empty() {
            format!("dent's background service stopped ({status})")
        } else {
            format!("dent's background service couldn't start: {why}")
        };
        let _ = ui.unbounded_send(Incoming::Down(why));
    });
    Ok(())
}
