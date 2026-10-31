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
        /// What runs (the launcher's name for it, else the daemon's
        /// version), and how its last update went.
        version: String,
        update: Option<dent_protocol::UpdateOutcome>,
        /// Its number among main's builds, and a newer one it has ready.
        main_build: u64,
        available: Option<dent_protocol::Available>,
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
        // The daemon installed beside the app.
        let bin = std::env::current_exe()
            .map(|p| p.with_file_name("dent-daemon"))
            .unwrap_or_default();
        std::thread::Builder::new()
            .name("dent-live".into())
            .spawn(move || run(state, bin, rx, ui, s))
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
    bin: PathBuf,
    rx: Receiver<(u64, Request)>,
    ui: UnboundedSender<Incoming>,
    seq: Arc<AtomicU64>,
) {
    let sock = dent_protocol::daemon_socket(&state);
    // Requests not yet answered, and whether each went to a daemon: after a
    // reconnect, what can be repeated goes again, and what went and can't
    // is answered as lost (decision 0078).
    let mut pending: BTreeMap<u64, (Request, bool)> = BTreeMap::new();
    let mut started_at: Option<Instant> = None;
    // When an older daemon that refused this app was last replaced.
    let mut replaced_at: Option<Instant> = None;
    let mut wait = Duration::from_millis(50);
    loop {
        let stream = match UnixStream::connect(&sock) {
            Ok(s) => s,
            Err(_) => {
                if started_at.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
                    started_at = Some(Instant::now());
                    if let Err(e) = start_daemon(&state, &bin, ui.clone()) {
                        let _ = ui.unbounded_send(Incoming::Down(e));
                    }
                }
                // Take requests while waiting, so none is lost.
                match rx.recv_timeout(wait) {
                    Ok((n, r)) => {
                        pending.insert(n, (r, false));
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
        let lost: Vec<u64> = pending
            .iter()
            .filter(|(_, (r, gone))| *gone && r.resend() == dent_protocol::Resend::Once)
            .map(|(n, _)| *n)
            .collect();
        for n in lost {
            pending.remove(&n);
            let _ = ui.unbounded_send(Incoming::Out {
                req: n,
                out: Out::Error {
                    message: "dent's background service restarted before it answered, so that may or may not have happened; look before trying again".into(),
                },
            });
        }
        for (n, (r, gone)) in pending.iter_mut() {
            let _ = w.write_all(&encode(*n, r));
            *gone = true;
        }
        // Lines from the daemon come through a reader thread; replies clear
        // pending requests here.
        let (done_tx, done_rx) = mpsc::channel::<u64>();
        let (restart_tx, restart_rx) = mpsc::channel::<()>();
        let (refused_tx, refused_rx) = mpsc::channel::<()>();
        let ui2 = ui.clone();
        let installed = bin.clone();
        let reading = std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                let Some(dent_protocol::Envelope { req, body: out }) =
                    dent_protocol::read_out(line.as_bytes())
                else {
                    continue;
                };
                // A refused hello ends the connection here: what follows is
                // "hello first" for each request sent with it, which stay
                // pending for the daemon that replaces it.
                if req == 1 && matches!(out, Out::Error { .. }) {
                    let _ = refused_tx.send(());
                    break;
                }
                if req != 0 {
                    let _ = done_tx.send(req);
                }
                let msg = match out {
                    Out::Reply {
                        reply:
                            Reply::Welcome {
                                proto,
                                machine,
                                build,
                                agents,
                                id,
                                managed,
                                version,
                                release,
                                update,
                                main_build,
                                available,
                                ..
                            },
                    } if req == 1 => {
                        // A daemon older than the one installed beside the app
                        // makes way for it; agents keep running in the holder.
                        // One the service runs is updated through its
                        // launcher instead: restarted, it would come back as
                        // the same version, again and again. One speaking a
                        // newer protocol than this app stays.
                        let installed = dent_protocol::build_stamp(&installed);
                        // One too old to say its build is older still.
                        if !managed
                            && proto <= PROTOCOL
                            && !installed.is_empty()
                            && installed != build
                        {
                            let _ = restart_tx.send(());
                        }
                        Incoming::Up {
                            machine,
                            id,
                            agents,
                            version: if release.is_empty() { version } else { release },
                            update: update.map(|u| *u),
                            main_build,
                            available,
                        }
                    }
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
                    // A write that fails may still have reached it.
                    pending.insert(n, (r, true));
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
        if refused_rx.try_recv().is_err() {
            let _ = ui.unbounded_send(Incoming::Down("dent's background service restarted".into()));
            continue;
        }
        // It wouldn't talk to this app. One older than the app is replaced
        // by the one beside it, as a welcome with an older build is;
        // otherwise the window says which side is older and what to do, and
        // it's asked again a little later.
        let mut outcome = make_way(&sock, service_runs(&state), &bin);
        // Back again at once: something else starts the old one.
        if outcome == Refused::Replaced {
            if replaced_at.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
                outcome = Refused::Failed("something started the old one again".into());
            }
            replaced_at = Some(Instant::now());
        }
        let (why, again) = refused_words(&outcome, &bin);
        if outcome == Refused::Replaced {
            // Started at once, not after the wait for one started lately.
            started_at = None;
        }
        let _ = ui.unbounded_send(Incoming::Down(why));
        let until = Instant::now() + again;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok((n, r)) => {
                    pending.insert(n, (r, false));
                }
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
    }
}

/// What became of a daemon that refused this app's hello.
#[derive(Debug, PartialEq)]
enum Refused {
    /// Older than the app: it stopped, and the next connection starts the
    /// one beside the app.
    Replaced,
    /// Older, and run by the service: updated through its launcher.
    Service,
    /// Newer than the app: it stays.
    Newer,
    /// Older, but this app has no daemon of its own to replace it with.
    NoDaemon,
    /// Older, but it couldn't be replaced: why.
    Failed(String),
}

/// The protocol a daemon from before protocol ranges spoke, and the only
/// one it took: Hello and Restart mean the same in it as in every later one.
const FIRST: u32 = 1;

/// Ask a daemon that refused this app's hello to make way. Daemons from
/// before protocol ranges took only their own protocol, and nothing before
/// a hello, so it's asked in that one. Taken, the daemon is older: told to
/// restart, it exits and leaves agents running in its holder, which the
/// one beside the app picks up (the holder's wire hasn't changed). Refused
/// even then, it's newer than the app. Nothing is signalled: the state
/// folder's lock names no process, and one found by name could belong to
/// another state folder.
fn make_way(sock: &Path, service: bool, bundled: &Path) -> Refused {
    let failed = |e: std::io::Error| Refused::Failed(e.to_string());
    let mut s = match UnixStream::connect(sock) {
        Ok(s) => s,
        // Gone meanwhile: the next connection starts the new one.
        Err(_) => return Refused::Replaced,
    };
    if let Err(e) = s.set_read_timeout(Some(Duration::from_secs(5))) {
        return failed(e);
    }
    let hello = Request::Hello {
        proto: FIRST,
        client: "desktop".into(),
    };
    if let Err(e) = s.write_all(&encode(1, &hello)) {
        return failed(e);
    }
    let Ok(read) = s.try_clone() else {
        return Refused::Failed("couldn't read from it".into());
    };
    let mut lines = BufReader::new(read).lines();
    let reply = loop {
        match lines.next() {
            Some(Ok(line)) => match dent_protocol::read_out(line.as_bytes()) {
                Some(dent_protocol::Envelope { req: 1, body }) => break body,
                _ => continue,
            },
            Some(Err(e)) => return failed(e),
            None => return Refused::Replaced,
        }
    };
    let managed = match reply {
        Out::Reply {
            reply: Reply::Welcome { proto, managed, .. },
        } if proto <= PROTOCOL => managed,
        _ => return Refused::Newer,
    };
    if managed || service {
        return Refused::Service;
    }
    if !bundled.is_file() {
        return Refused::NoDaemon;
    }
    if let Err(e) = s.write_all(&encode(2, &Request::Restart)) {
        return failed(e);
    }
    // It answers, then exits, which ends the connection.
    for line in lines {
        if line.is_err() {
            return Refused::Failed("it didn't stop when asked".into());
        }
    }
    Refused::Replaced
}

/// What the window says about a refusing daemon, and when to ask it again.
fn refused_words(r: &Refused, bundled: &Path) -> (String, Duration) {
    let dir = bundled
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    match r {
        Refused::Replaced => (
            "Updating dent's background service to this app's version…".into(),
            Duration::ZERO,
        ),
        Refused::Service => (
            format!(
                "dent's service on this Mac is older than this app. Update it in Terminal: \
                 \"{dir}/dent-daemon\" update \"{dir}\". Sessions keep running."
            ),
            Duration::from_secs(30),
        ),
        Refused::Newer => (
            "This dent app is older than dent's background service on this Mac. \
             Quit it and open the newer dent, or install the latest with \
             scripts/bundle.sh --install."
                .into(),
            Duration::from_secs(30),
        ),
        Refused::NoDaemon => (
            format!(
                "dent's background service on this Mac is older than this app, and this app \
                 has no dent-daemon of its own in {dir} to replace it with. Reinstall dent: \
                 scripts/bundle.sh --install."
            ),
            Duration::from_secs(30),
        ),
        Refused::Failed(why) => (
            format!(
                "dent's background service on this Mac is older than this app and couldn't \
                 be replaced: {why}. dent tries again every 10 seconds; if this stays, \
                 restart the Mac."
            ),
            Duration::from_secs(10),
        ),
    }
}

/// Whether `dent-daemon install` set this state folder up as a service:
/// its launcher is there and the service manager has its definition. (An
/// uninstall removes both; a folder left from one isn't a service.)
fn service_runs(state: &Path) -> bool {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let definition = if cfg!(target_os = "macos") {
        home.join("Library/LaunchAgents/sh.dent.launcher.plist")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("systemd/user/dent.service")
    };
    state.join("bin/dent-launcher").is_file() && definition.is_file()
}

/// Start the daemon from beside this app, in its own process group so it
/// outlives the window.
fn start_daemon(state: &Path, bin: &Path, ui: UnboundedSender<Incoming>) -> Result<(), String> {
    // Installed as a service (`dent-daemon install`), the launcher runs the
    // daemon and starts it again itself; one started here would take the
    // state folder from it, mid-update even.
    if service_runs(state) {
        return Ok(());
    }
    if !bin.exists() {
        return Err(format!(
            "{} isn't there: reinstall dent with scripts/bundle.sh --install",
            bin.display()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dent-live-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A daemon from before protocol ranges, answering as it did: its own
    /// protocol (1) only, nothing before a hello, and Restart ends it.
    /// `newer` refuses every hello instead. Says what it was asked.
    fn old_daemon(sock: &Path, newer: bool) -> Receiver<String> {
        let l = UnixListener::bind(sock).unwrap();
        let (tx, rx) = mpsc::channel();
        let sock = sock.to_path_buf();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { return };
                let mut w = s.try_clone().unwrap();
                let mut hello = false;
                for line in BufReader::new(s).lines() {
                    let Ok(line) = line else { break };
                    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
                    let req = v["req"].as_u64().unwrap_or(0);
                    let op = v["op"].as_str().unwrap_or("");
                    let _ = tx.send(format!("{op} {}", v["proto"]));
                    let reply = match op {
                        "hello" if v["proto"] == 1 && !newer => {
                            hello = true;
                            r#""op":"reply","reply":"welcome","proto":1,"version":"0.0.0","machine":"m","home":"/","build":"1-1","agents":[]"#
                        }
                        "hello" => {
                            r#""op":"error","message":"this daemon speaks protocol 1, the app 4; update the older one""#
                        }
                        "restart" if hello => {
                            let _ = writeln!(w, r#"{{"req":{req},"op":"reply","reply":"ok"}}"#);
                            let _ = std::fs::remove_file(&sock);
                            return;
                        }
                        _ => r#""op":"error","message":"hello first""#,
                    };
                    let _ = writeln!(w, "{{\"req\":{req},{reply}}}");
                }
            }
        });
        rx
    }

    #[test]
    fn an_older_daemon_that_refuses_the_app_makes_way_and_requests_wait_for_the_new_one() {
        let state = scratch("refused");
        let sock = dent_protocol::daemon_socket(&state);
        let asked = old_daemon(&sock, false);
        // The daemon beside the app: says it was started, and with what.
        let bin = state.join("dent-daemon");
        std::fs::write(
            &bin,
            "#!/bin/sh\necho \"$@\" > \"$(dirname \"$0\")/started\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (ui, mut heard) = futures::channel::mpsc::unbounded();
        let (tx, rx) = mpsc::channel();
        tx.send((7, Request::Devices)).unwrap();
        let (s, b) = (state.clone(), bin.clone());
        std::thread::spawn(move || run(s, b, rx, ui, Arc::new(AtomicU64::new(0))));

        let started = state.join("started");
        let until = Instant::now() + Duration::from_secs(10);
        while !started.exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let said = std::fs::read_to_string(&started).expect("the app's own daemon was started");
        assert_eq!(said.trim(), format!("--state {}", state.display()));
        let asked: Vec<String> = asked.try_iter().collect();
        assert_eq!(asked[0], "hello 4");
        assert!(asked.contains(&"hello 1".to_string()), "{asked:?}");
        assert_eq!(asked.last().map(String::as_str), Some("restart null"));

        // The new one gets the request the old one never took.
        let l = UnixListener::bind(&sock).unwrap();
        let (s, _) = l.accept().unwrap();
        let mut ops = Vec::new();
        for line in BufReader::new(s).lines().take(3) {
            let v: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
            ops.push(v["op"].as_str().unwrap_or("").to_string());
        }
        assert_eq!(ops, ["hello", "subscribe", "devices"]);

        // The window heard it being replaced, and never "hello first".
        let mut downs = Vec::new();
        while let Ok(m) = heard.try_recv() {
            match m {
                Incoming::Down(why) => downs.push(why),
                Incoming::Out { out, .. } => panic!("a reply from the refusing daemon: {out:?}"),
                _ => {}
            }
        }
        assert!(downs.iter().any(|d| d.starts_with("Updating")), "{downs:?}");
        drop(tx);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_refusing_daemon_stays_when_newer_or_the_services() {
        let dir = scratch("stays");
        let bin = dir.join("dent-daemon");
        std::fs::write(&bin, "").unwrap();
        let cases = [
            ("newer", true, false, Refused::Newer),
            ("service", false, true, Refused::Service),
        ];
        for (name, newer, service, want) in cases {
            let sock = dir.join(name);
            let asked = old_daemon(&sock, newer);
            assert_eq!(make_way(&sock, service, &bin), want, "{name}");
            assert!(
                !asked.try_iter().any(|a| a.starts_with("restart")),
                "{name}"
            );
        }
        let sock = dir.join("bare");
        let _asked = old_daemon(&sock, false);
        assert_eq!(
            make_way(&sock, false, &dir.join("missing")),
            Refused::NoDaemon
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_a_refusal_says_names_the_older_side_and_what_to_do() {
        let bin = Path::new("/Applications/dent.app/Contents/MacOS/dent-daemon");
        for r in [
            Refused::Service,
            Refused::Newer,
            Refused::NoDaemon,
            Refused::Failed("it didn't stop when asked".into()),
        ] {
            let (said, again) = refused_words(&r, bin);
            assert!(said.contains("older than"), "{said}");
            assert!(
                [
                    "scripts/bundle.sh --install",
                    "dent-daemon\" update",
                    "restart the Mac"
                ]
                .iter()
                .any(|how| said.contains(how)),
                "{said}"
            );
            assert!(again > Duration::ZERO);
        }
    }
}
