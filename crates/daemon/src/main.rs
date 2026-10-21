//! The per-machine dent daemon: it owns every agent session on this
//! machine, keeps their events in a log, and serves clients.
//!
//! `dent-daemon [--state DIR]` runs it in the foreground. Apps start it
//! when it isn't running; it keeps going after they quit.
//! `dent-daemon login-item on|off` starts it at login, or stops doing so.
//! `dent-daemon check-transcripts [DIR] [--into STATE]` checks the import of
//! Claude Code transcripts (see [`claude::check`]).
//! `dent-daemon plan-tool` serves the agent's plan tool to a session (see
//! [`plan_tool`]).
//! `dent-daemon pair` shows a code another device can pair with (see
//! [`remote`]); `dent-daemon devices [remove ID]` lists or removes paired
//! devices; `dent-daemon reidentify` gives this machine a new identity.

mod agent;
mod bring;
mod claude;
mod daemon;
mod env;
mod escapes;
mod git;
mod holder;
mod login;
mod machine;
mod paths;
mod plan_tool;
mod power;
mod projects;
mod remote;
mod secrets;
mod server;
mod session;
mod ship;
mod store;
mod workspace;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

const USAGE: &str = "usage: dent-daemon [--state DIR]   run the daemon
       dent-daemon pair                 show a pairing code for a new device
       dent-daemon devices [remove ID]  list paired devices, or remove one
       dent-daemon reidentify           take a new identity (devices pair again)
       dent-daemon login-item on|off    start at login, or stop doing so";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let state = args
        .iter()
        .position(|a| a == "--state")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let paths = paths::Paths::new(state);
    // What's asked, with `--state DIR` taken out wherever it was put, so
    // `dent-daemon --state DIR pair` works as well as `pair --state DIR`.
    let args: Vec<String> = {
        let mut words = Vec::new();
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            if a == "--state" {
                // Without its folder, never the default one by mistake.
                if it.next().is_none() {
                    eprintln!("{USAGE}");
                    anyhow::bail!("--state needs a folder");
                }
            } else {
                words.push(a);
            }
        }
        words
    };
    // `dent-daemon login-item on|off`: start at login, or stop doing so.
    if args.first().map(String::as_str) == Some("login-item") {
        let on = match args.get(1).map(String::as_str) {
            Some("on") => true,
            Some("off") => false,
            _ => anyhow::bail!("usage: dent-daemon login-item on|off"),
        };
        return login::set(on, &paths.state);
    }
    match args.first().map(String::as_str) {
        Some("pair") => return admin::pair(&paths),
        Some("devices") => return admin::devices(&paths, &args[1..]),
        Some("reidentify") => {
            let i = remote::identity::reidentify(&paths.state)?;
            println!(
                "This machine is now {}. Devices paired before need to pair again; restart the daemon to use it.",
                i.keys.id()
            );
            return Ok(());
        }
        _ => {}
    }
    if args.first().map(String::as_str) == Some("check-transcripts") {
        return claude::check::run(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("plan-tool") {
        return plan_tool::run();
    }
    // Anything else said to it (`--help`, a typo) is answered with how to
    // use it, never by quietly starting a second daemon.
    if let Some(a) = args.first() {
        let help = matches!(a.as_str(), "--help" | "-h" | "help");
        eprintln!("{USAGE}");
        if help {
            return Ok(());
        }
        anyhow::bail!("not something dent-daemon knows: {a}");
    }
    std::fs::create_dir_all(&paths.state)
        .with_context(|| format!("create {}", paths.state.display()))?;
    std::fs::set_permissions(&paths.state, std::fs::Permissions::from_mode(0o700))?;

    // One daemon per state directory. A second one exits quietly.
    let lock = std::fs::File::create(paths.lock())?;
    // SAFETY: a valid open descriptor; the lock lives as long as `lock`.
    let locked = unsafe {
        libc::flock(
            std::os::fd::AsRawFd::as_raw_fd(&lock),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    } == 0;
    if !locked {
        eprintln!("dent-daemon: already running for {}", paths.state.display());
        return Ok(());
    }

    // A reason left by a stop that never happened isn't this run's.
    let _ = std::fs::remove_file(paths.state.join("stop-reason"));

    let store = store::Store::open(&paths.db())?;
    let env = env::AgentEnv::detect();
    let rt = tokio::runtime::Runtime::new()?;
    let run = rt.block_on(async move {
        let d = daemon::Daemon::new(paths, env, store);
        eprintln!(
            "dent-daemon {} on {} ({})",
            d.version,
            d.machine,
            d.paths.state.display()
        );
        d.restore().await?;
        d.watch_prs();
        {
            let d = d.clone();
            tokio::spawn(async move {
                let mut every = tokio::time::interval(Duration::from_secs(300));
                loop {
                    every.tick().await;
                    let _ = d.store().checkpoint();
                }
            });
        }
        // Remote devices, beside the local socket. Asked to stop, the
        // daemon says goodbye to them first, so they show it as quitting
        // (or restarting, when the launcher says so) rather than gone
        // silent.
        {
            let d = d.clone();
            tokio::spawn(async move {
                if let Err(e) = remote::serve(d).await {
                    eprintln!("dent-daemon: remote access off: {e:#}");
                }
            });
        }
        // The machine going to sleep: every device is told, so it reads
        // as asleep at once instead of gone quiet. The links close; they
        // dial again once it wakes.
        {
            let d = d.clone();
            power::watch(move |sleeping| {
                // Set first, so a device that dials in after the goodbyes
                // (sleep can take a while) is told too.
                d.net
                    .asleep
                    .store(sleeping, std::sync::atomic::Ordering::Relaxed);
                if sleeping {
                    let _ = d.net.bye.send(dent_link::Goodbye::Sleep);
                    // Time for the goodbyes to leave before the network
                    // sleeps.
                    std::thread::sleep(Duration::from_millis(300));
                }
            });
        }
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            r = server::serve(d.clone()) => r?,
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
        let _ = d.net.bye.send(stop_reason(&d.paths.state));
        tokio::time::sleep(Duration::from_millis(200)).await;
        anyhow::Ok(())
    });
    // Dropping a runtime waits for every blocking task (a git command, an
    // agent's pipe), however long; asked to stop, the daemon stops promptly.
    rt.shutdown_timeout(Duration::from_millis(500));
    run?;
    drop(lock);
    Ok(())
}

/// Why the daemon is stopping, from `DENT_STOP_REASON` or the
/// `stop-reason` file the launcher writes before stopping it ([decision
/// 0065](../../docs/decisions/0065-launcher-and-updates.md)): `restart`,
/// `update <version>`, else quitting.
fn stop_reason(state: &std::path::Path) -> dent_link::Goodbye {
    let why = std::env::var("DENT_STOP_REASON")
        .ok()
        .or_else(|| std::fs::read_to_string(state.join("stop-reason")).ok())
        .unwrap_or_default();
    let _ = std::fs::remove_file(state.join("stop-reason"));
    let mut words = why.split_whitespace();
    match words.next() {
        Some("restart") => dent_link::Goodbye::Restart {
            expected_down_ms: 3000,
        },
        Some("update") => dent_link::Goodbye::Update {
            to: words.next().unwrap_or_default().to_string(),
            expected_down_ms: 10_000,
        },
        _ => dent_link::Goodbye::Quit,
    }
}

/// Commands that talk to the running daemon over its socket.
mod admin {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    use anyhow::{Context, Result, bail};
    use dent_protocol::{Envelope, Out, PROTOCOL, Reply, Request, encode};

    use crate::paths::Paths;

    fn ask(paths: &Paths, req: Request) -> Result<Reply> {
        let sock = paths.socket();
        let mut w = UnixStream::connect(&sock)
            .with_context(|| format!("the daemon isn't running ({})", sock.display()))?;
        let mut r = BufReader::new(w.try_clone()?);
        w.write_all(&encode(
            1,
            &Request::Hello {
                proto: PROTOCOL,
                client: "dent-daemon".into(),
            },
        ))?;
        w.write_all(&encode(2, &req))?;
        let mut line = String::new();
        loop {
            line.clear();
            if r.read_line(&mut line)? == 0 {
                bail!("the daemon closed the connection");
            }
            let Ok(env) = serde_json::from_str::<Envelope<Out>>(&line) else {
                continue;
            };
            if env.req != 2 {
                continue;
            }
            return match env.body {
                Out::Reply { reply } => Ok(reply),
                Out::Error { message } => bail!("{message}"),
                _ => continue,
            };
        }
    }

    pub fn pair(paths: &Paths) -> Result<()> {
        match ask(paths, Request::OpenPairing)? {
            Reply::Pairing { code, expires } => {
                let mins = expires.saturating_sub(crate::remote::trust::now_ms()) / 60_000;
                let (a, b) = code.split_at_checked(3).unwrap_or((&code, ""));
                println!("Pairing code: {a} {b}");
                println!("Type it on the other device. It works once, for {mins} more minutes.");
                Ok(())
            }
            other => bail!("unexpected answer: {other:?}"),
        }
    }

    pub fn devices(paths: &Paths, args: &[String]) -> Result<()> {
        if args.first().map(String::as_str) == Some("remove") {
            let id = args
                .get(1)
                .context("usage: dent-daemon devices remove ID")?;
            ask(paths, Request::RemoveDevice { id: id.clone() })?;
            println!("Removed {id}.");
            return Ok(());
        }
        match ask(paths, Request::Devices)? {
            Reply::Devices { devices } if devices.is_empty() => {
                println!(
                    "No devices yet. Devices on your own Tailscale account connect without a code; others pair with `dent-daemon pair`."
                );
                Ok(())
            }
            Reply::Devices { devices } => {
                for d in devices {
                    println!("{}  {}  ({}, {})", d.id, d.name, d.how, d.access);
                }
                Ok(())
            }
            other => bail!("unexpected answer: {other:?}"),
        }
    }
}
