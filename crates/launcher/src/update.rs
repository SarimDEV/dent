//! An update, as [decision 0065] has it: preflight, record, stop, snapshot,
//! trial, then commit or restore. Every step is recorded before the next,
//! so a launcher stopped anywhere finishes or undoes it when it starts again.
//!
//! [decision 0065]: ../../../docs/decisions/0065-launcher-and-updates.md

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::child::{Daemon, Ended};
use crate::disk;
use crate::record::{Manifest, Record, valid_version};

/// The launcher protocol this launcher speaks: a version needing more
/// waits for a newer launcher.
pub const PROTOCOL: u32 = 1;

/// The database and its write-ahead files, which a snapshot keeps together.
const DB: [&str; 3] = ["dent.db", "dent.db-wal", "dent.db-shm"];

/// Left while the database is being put back: a launcher that finds it
/// finishes the restore before anything runs.
const RESTORE: &str = "restore-pending";

fn snapshots(state: &Path) -> PathBuf {
    state.join("snapshots")
}

/// Why an update can't go ahead, said before anything is stopped. `auto`
/// when nobody asked for it: a version that rolled back is tried again only
/// when someone does (it may have failed for something since put right,
/// like a port another program held).
pub fn preflight(state: &Path, rec: &Record, to: &str, auto: bool) -> Result<(), String> {
    if !valid_version(to) {
        return Err(format!("{to:?} isn't a version"));
    }
    if to == rec.active {
        return Err(format!("{to} already runs"));
    }
    if auto && rec.never_retry.iter().any(|v| v == to) {
        return Err(format!("{to} rolled back before"));
    }
    let dir = state.join("versions").join(to);
    let m = Manifest::load(&dir).map_err(|e| format!("{to} has no readable manifest ({e})"))?;
    if m.launcher > PROTOCOL {
        return Err(format!(
            "{to} needs a newer launcher (protocol {}; this one speaks {PROTOCOL}): install it with `dent-daemon install`",
            m.launcher
        ));
    }
    if !dir.join("dent-daemon").is_file() {
        return Err(format!("{to} has no dent-daemon"));
    }
    // Room for the snapshot, and some for the trial's migration.
    let size: u64 = DB
        .iter()
        .filter_map(|f| std::fs::metadata(state.join(f)).ok())
        .map(|m| m.len())
        .sum();
    let need = size + size / 20 + 16 * 1024 * 1024;
    let free = disk::free(state).map_err(|e| format!("can't tell the free space ({e})"))?;
    if free < need {
        return Err(format!(
            "not enough room: {} free, {} needed for a {} database's snapshot",
            mb(free),
            mb(need),
            mb(size)
        ));
    }
    Ok(())
}

fn mb(n: u64) -> String {
    format!("{:.1} MB", n as f64 / 1_048_576.)
}

/// Copy the database aside for update `id`, once: a retry reuses the first
/// snapshot, never one taken of a half-migrated database.
pub fn snapshot(state: &Path, id: &str) -> io::Result<()> {
    let snap = snapshots(state).join(id);
    if snap.is_dir() {
        return Ok(());
    }
    let staging = snapshots(state).join(format!("{id}.staging"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    for f in DB {
        let src = state.join(f);
        if src.exists() {
            disk::copy(&src, &staging.join(f))?;
        }
    }
    disk::sync(&staging)?;
    std::fs::rename(&staging, &snap)?;
    disk::sync_parent(&snap)
}

/// Put update `id`'s snapshot back, behind a marker, so a restore cut
/// short is finished before either version runs.
pub fn restore(state: &Path, id: &str) -> io::Result<()> {
    let snap = snapshots(state).join(id);
    if !snap.is_dir() {
        // Stopped before its snapshot was taken: the old daemon was the
        // last to touch the database, and it's as it left it.
        return disk::remove(&state.join(RESTORE));
    }
    disk::write_atomic(&state.join(RESTORE), id.as_bytes())?;
    if std::env::var_os("DENT_LAUNCHER_CRASH_IN_RESTORE").is_some() {
        std::process::exit(9);
    }
    for f in DB {
        let _ = std::fs::remove_file(state.join(f));
    }
    for f in DB {
        let src = snap.join(f);
        if src.exists() {
            disk::copy(&src, &state.join(f))?;
        }
    }
    // The database must be on disk before the marker goes, or a power cut
    // could leave half of it and nothing saying so.
    disk::sync(state)?;
    disk::remove(&state.join(RESTORE))
}

/// At startup: finish whatever the last launcher was doing when it stopped.
pub fn recover(state: &Path, rec: &mut Record) -> io::Result<()> {
    if let Ok(id) = std::fs::read_to_string(state.join(RESTORE)) {
        eprintln!("dent-launcher: finishing an interrupted restore");
        restore(state, id.trim())?;
        if rec.pending.is_some() {
            rec.rolled_back();
        }
        rec.save(state)?;
    }
    if let Some(to) = rec.pending.clone() {
        let id = rec.update.as_ref().map(|u| u.0.clone()).unwrap_or_default();
        eprintln!("dent-launcher: {to} was on trial when the launcher stopped; going back");
        restore(state, &id)?;
        rec.rolled_back();
        rec.save(state)?;
    }
    Ok(())
}

/// How long a trial may take to migrate and bind: a big database's
/// migration is slow, so this is generous.
fn prepare_limit() -> Duration {
    std::env::var("DENT_PREPARE_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_secs_f64)
        .unwrap_or(Duration::from_secs(30 * 60))
}

/// Run update `to` from `running`: returns the daemon running afterwards,
/// the new one or the old one again. `stopping` says the launcher was asked
/// to stop meanwhile; then it returns `None`, leaving the trial recorded
/// so the next start puts things back.
pub fn apply(
    state: &Path,
    rec: &mut Record,
    running: Daemon,
    to: &str,
    stopping: &dyn Fn() -> bool,
) -> io::Result<Option<Daemon>> {
    let id = format!(
        "u{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    rec.pending = Some(to.to_string());
    rec.update = Some((id.clone(), rec.active.clone()));
    rec.save(state)?; // before anything stops
    eprintln!("dent-launcher: update {id}: {} -> {to}", rec.active);
    running.stop(state, Some(&format!("update {to}")));
    snapshot(state, &id)?;
    let mut trial = Daemon::start(state, to, true, None)?;
    let until = Instant::now() + prepare_limit();
    let why = loop {
        if stopping() {
            trial.stop(state, None);
            return Ok(None);
        }
        if trial.lines().iter().any(|l| l == "prepared") {
            break None;
        }
        if let Some(e) = trial.ended() {
            break Some(format!("it {}", ended(&e)));
        }
        if Instant::now() >= until {
            break Some(format!("it wasn't ready in {:?}", prepare_limit()));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    match why {
        None => {
            let from = rec.active.clone();
            rec.committed();
            // Asked for again and passed this time: no longer held back.
            rec.never_retry.retain(|v| v != to);
            rec.save(state)?; // durable before the daemon is told
            trial.say("committed");
            eprintln!("dent-launcher: {to} committed");
            prune(state, &[&rec.active, &from], &id);
            Ok(Some(trial))
        }
        Some(why) => {
            eprintln!("dent-launcher: {to} rolled back: {why}");
            trial.stop(state, None);
            restore(state, &id)?;
            rec.rolled_back();
            rec.save(state)?;
            let outcome = rec.outcome.as_ref().map(|o| o.line());
            Daemon::start(state, &rec.active, false, outcome.as_deref()).map(Some)
        }
    }
}

pub fn ended(e: &Ended) -> String {
    match e {
        Ended::Clean => "stopped".into(),
        Ended::Locked => "found another daemon running".into(),
        Ended::Crashed(how) => how.clone(),
    }
}

/// Keep the versions in use and this update's snapshot; the rest go. A
/// version's files can go while it runs (Unix keeps them until it ends).
fn prune(state: &Path, keep: &[&str], snapshot: &str) {
    let gone = |dir: PathBuf, keep: &dyn Fn(&str) -> bool| {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !keep(&name) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    };
    gone(state.join("versions"), &|n| {
        keep.contains(&n) || n.ends_with(".staging")
    });
    gone(snapshots(state), &|n| n == snapshot);
}
