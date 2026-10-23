//! `dent-launcher run --state DIR`: what the service manager runs
//! ([decision 0065]). It owns which daemon version runs: it starts the
//! active one, starts it again when it ends, and runs an update when the
//! daemon asks for one, trialling the new version and putting the old one
//! back if the trial fails. Agents never notice: they run in the holder.
//!
//! Small and standard-library only (and libc), like the holder, and
//! changed rarely, with its own protocol version ([`update::PROTOCOL`]).
//!
//! [decision 0065]: ../../../docs/decisions/0065-launcher-and-updates.md

mod child;
mod disk;
mod record;
mod update;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use child::{Daemon, Ended};
use record::Record;

const USAGE: &str = "usage: dent-launcher run --state DIR
       dent-launcher protocol";

/// Asked to stop (SIGTERM, SIGINT).
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn stopping() -> bool {
    STOP.load(Ordering::Relaxed)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let state = args
        .iter()
        .position(|a| a == "--state")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    match (args.first().map(String::as_str), state) {
        (Some("run"), Some(state)) => {
            if let Err(e) = run(&state) {
                eprintln!("dent-launcher: {e}");
                std::process::exit(1);
            }
        }
        // What `dent-daemon install` checks before relying on it.
        (Some("protocol"), _) => println!("{}", update::PROTOCOL),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}

fn run(state: &Path) -> std::io::Result<()> {
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let mut rec = Record::load(state).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!(
                "can't read {} ({e}); install with `dent-daemon install`",
                record::FILE
            ),
        )
    })?;
    service_env(state);
    update::recover(state, &mut rec)?;
    let mut tries = Backoff::default();
    let mut daemon = Some(start(state, &rec)?);
    loop {
        if stopping() {
            if let Some(d) = daemon.take() {
                d.stop(state, None);
            }
            return Ok(());
        }
        let Some(d) = daemon.as_mut() else {
            // Waiting out a crash, or another daemon holding the folder.
            if tries.due() {
                daemon = Some(start(state, &rec)?);
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        for line in d.lines() {
            if let Some(to) = line.strip_prefix("update ") {
                let to = to.trim();
                // The record on disk, in case an install changed it.
                if let Ok(r) = Record::load(state) {
                    rec = r;
                }
                if let Err(why) = update::preflight(state, &rec, to) {
                    eprintln!("dent-launcher: refused {to}: {why}");
                    d.say(&format!("refused {why}"));
                    continue;
                }
                d.say("accepted");
                let running = daemon.take().expect("running");
                daemon = update::apply(state, &mut rec, running, to, &stopping)?;
                tries = Backoff::default();
                break;
            }
        }
        let Some(d) = daemon.as_mut() else { continue };
        if let Some(e) = d.ended() {
            let lived = d.started.elapsed();
            daemon = None;
            match e {
                Ended::Locked => {
                    eprintln!("dent-launcher: another daemon runs for this folder; waiting");
                    tries.wait(Duration::from_secs(30));
                }
                // Asked to restart: at once, unless it keeps ending.
                Ended::Clean if lived > Duration::from_secs(5) => tries.now(),
                other => {
                    eprintln!("dent-launcher: the daemon {}", update::ended(&other));
                    tries.after_crash(lived);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `service.env`: what you want the service to run with (PATH first),
/// given to every daemon it starts. `KEY=VALUE` lines; `#` starts a comment.
fn service_env(state: &Path) {
    let Ok(text) = std::fs::read_to_string(state.join("service.env")) else {
        return;
    };
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=')
            && !k.trim().is_empty()
            && !k.contains(char::is_whitespace)
        {
            // SAFETY: at startup, before any thread is started.
            unsafe { std::env::set_var(k.trim(), v.trim()) };
        }
    }
}

fn start(state: &Path, rec: &Record) -> std::io::Result<Daemon> {
    let outcome = rec.outcome.as_ref().map(|o| o.line());
    Daemon::start(state, &rec.active, false, outcome.as_deref())
}

/// When to start the daemon again: at once after a clean restart, then
/// 1, 2, 4… up to 30 s while it keeps crashing, starting over once it has
/// stayed up a minute.
#[derive(Default)]
struct Backoff {
    next: Option<Instant>,
    step: u32,
}

impl Backoff {
    fn due(&self) -> bool {
        self.next.is_none_or(|t| Instant::now() >= t)
    }

    fn now(&mut self) {
        self.next = None;
    }

    fn wait(&mut self, d: Duration) {
        self.next = Some(Instant::now() + d);
    }

    fn after_crash(&mut self, lived: Duration) {
        if lived > Duration::from_secs(60) {
            self.step = 0;
        }
        let d = Duration::from_secs(1 << self.step.min(5)).min(Duration::from_secs(30));
        self.step += 1;
        self.wait(d);
    }
}
