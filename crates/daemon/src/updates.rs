//! Main's newer builds made ready here ([decision 0077]): looked for now
//! and then, downloaded, checked and staged beside the running version, then
//! said to every device. Running one is the person's call
//! ([`dent_protocol::Request::Update`]): the launcher trials it, and the
//! holder keeps agents running through it.
//!
//! Only a daemon the launcher runs, and that CI built, looks: one built from
//! source updates when it's built again.
//!
//! [decision 0077]: ../../../docs/decisions/0077-updates-from-main.md

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use dent_protocol::{Available, Out};

use crate::daemon::{Daemon, Live};

/// How often to look: a merge shows minutes after its build; reading the
/// feed (a small file) costs nothing. `DENT_UPDATE_EVERY` (seconds) for tests.
fn every() -> Duration {
    std::env::var("DENT_UPDATE_EVERY")
        .ok()
        .and_then(|s| s.parse().ok())
        .map_or(Duration::from_secs(120), Duration::from_secs)
}

/// Look for newer builds for as long as the daemon runs.
pub async fn watch(d: Arc<Daemon>) {
    if !dent_update::enabled() || crate::launcher::get().is_none() {
        return;
    }
    loop {
        let state = d.paths.state.clone();
        let rolled_back = crate::launcher::outcome()
            .filter(|o| o.rolled_back)
            .map(|o| o.to);
        let have = d.available();
        let found =
            tokio::task::spawn_blocking(move || ready(&state, have, rolled_back.as_deref()))
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
        match found {
            Ok(a) => d.set_available(a),
            Err(e) => eprintln!("dent-daemon: looking for an update: {e}"),
        }
        tokio::time::sleep(every()).await;
    }
}

/// The newest build, downloaded, checked and staged, if it's newer than
/// this one; `None` when this is the newest. What's already ready (`have`)
/// isn't fetched again. A build that already went back here (`rolled_back`,
/// the launcher's name for it) isn't offered again: the next one is.
pub fn ready(
    state: &Path,
    have: Option<Available>,
    rolled_back: Option<&str>,
) -> Result<Option<Available>, String> {
    let feed = dent_update::fetch(&dent_update::feed_url())?;
    if !feed.is_newer() {
        return Ok(None);
    }
    if let Some(a) = have.filter(|a| a.main_build == feed.build) {
        return Ok(Some(a));
    }
    // One that went back is known by its number from then on, so it isn't
    // downloaded again just to find that out.
    let skip = state.join("update-skipped");
    if std::fs::read_to_string(&skip).is_ok_and(|s| s.trim() == feed.build.to_string()) {
        return Ok(None);
    }
    let part = feed.part("daemon")?;
    // One build's download at a time: whatever an older look left goes.
    let dir = state.join("updates");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let archive = dir.join(format!("{}.tar.gz", feed.build));
    let unpacked = dir.join(feed.build.to_string());
    let staged = dent_update::download(part, &archive)
        .and_then(|()| dent_update::unpack(&archive, &unpacked))
        .and_then(|()| crate::service::stage(state, &unpacked).map_err(|e| format!("{e:#}")));
    let _ = std::fs::remove_dir_all(&dir);
    let version = match staged {
        Ok(v) => v,
        // A build that can't be staged here (it won't run on this system,
        // say) isn't tried again every few minutes: the next one is.
        Err(e) if e.contains("manifest") || e.contains("launcher") => {
            let _ = std::fs::write(&skip, feed.build.to_string());
            return Err(e);
        }
        Err(e) => return Err(e),
    };
    prune(state, &version);
    if rolled_back == Some(version.as_str()) {
        let _ = std::fs::write(&skip, feed.build.to_string());
        return Ok(None);
    }
    Ok(Some(Available {
        main_build: feed.build,
        version,
    }))
}

/// Main's builds staged before `kept` and never run go: only what the
/// launcher's record names (what runs, what it came from) stays.
fn prune(state: &Path, kept: &str) {
    let record = std::fs::read_to_string(state.join("launcher.state")).unwrap_or_default();
    let Ok(all) = std::fs::read_dir(state.join("versions")) else {
        return;
    };
    for e in all.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let main_build = name.split('-').nth(1).is_some_and(|p| {
            p.strip_prefix('b')
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        });
        if main_build && name != kept && !name.ends_with(".staging") && !record.contains(&name) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

impl Daemon {
    /// What this daemon has ready, for a welcome.
    pub fn available(&self) -> Option<Available> {
        self.available
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// What's ready changed: every device hears it.
    fn set_available(&self, a: Option<Available>) {
        {
            let mut now = self.available.lock().unwrap_or_else(|e| e.into_inner());
            if *now == a {
                return;
            }
            now.clone_from(&a);
        }
        let _ = self
            .live
            .send(Arc::new(Live::Out(Out::Available { available: a })));
    }
}
