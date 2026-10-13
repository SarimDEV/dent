//! The per-machine dent daemon: it owns every agent session on this
//! machine, keeps their events in a log, and serves clients.
//!
//! `dent-daemon [--state DIR]` runs it in the foreground. Apps start it
//! when it isn't running; it keeps going after they quit.

mod claude;
mod daemon;
mod env;
mod git;
mod holder;
mod paths;
mod projects;
mod server;
mod session;
mod store;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let state = args
        .iter()
        .position(|a| a == "--state")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let paths = paths::Paths::new(state);
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

    let store = store::Store::open(&paths.db())?;
    let env = env::AgentEnv::detect();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let d = daemon::Daemon::new(paths, env, store);
        eprintln!(
            "dent-daemon {} on {} ({})",
            d.version,
            d.machine,
            d.paths.state.display()
        );
        d.restore().await?;
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
        server::serve(d).await
    })?;
    drop(lock);
    Ok(())
}
