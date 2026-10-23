//! The launcher against a stand-in daemon (a shell script per version), for
//! every way an update can go: committed, a failed migration, a trial that
//! never gets ready, a launcher cut off mid-restore or mid-trial, a version
//! needing a newer launcher, too little room; and a daemon that crashes or
//! finds another daemon running. As spike 13 ran them against the
//! prototype ([R-E3, R-E4](../../../docs/remote-tests.md)).

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// What a version's stand-in does on trial before saying it's prepared.
#[derive(Clone, Copy)]
enum Trial {
    /// Migrates: adds a line to the database.
    Migrates,
    /// Starts migrating, then fails.
    Fails,
    /// Never gets ready.
    Hangs,
    /// Finds another daemon holding the folder (which writes to the
    /// database meanwhile).
    Locked,
}

const DAEMON: &str = r#"#!/bin/sh
S="$2"
V="$DENT_VERSION"
log() { echo "$V $*" >> "$S/log"; }
stop() {
  log "stopped reason=$(cat "$S/stop-reason" 2>/dev/null)"
  rm -f "$S/stop-reason"
  kill "$R" 2>/dev/null
  exit 0
}
trap stop TERM
log "start trial=$DENT_TRIAL outcome=$DENT_UPDATE_OUTCOME"
if [ "$DENT_TRIAL" = 1 ]; then
  @@TRIAL@@
  echo prepared >&3
  read line <&4
  log "heard $line"
fi
if [ -f "$S/exit-with" ]; then
  code=$(cat "$S/exit-with"); rm -f "$S/exit-with"; log "exiting $code"; exit "$code"
fi
( while read l <&4; do log "heard $l"; done ) &
R=$!
while :; do
  if [ -f "$S/ask" ]; then echo "update $(cat "$S/ask")" >&3; rm -f "$S/ask"; fi
  if [ -f "$S/ask-auto" ]; then echo "auto-update $(cat "$S/ask-auto")" >&3; rm -f "$S/ask-auto"; fi
  sleep 0.05 & wait $!
done
"#;

struct Rig {
    state: PathBuf,
    launcher: Option<Child>,
}

impl Rig {
    fn new(name: &str) -> Rig {
        let state =
            std::env::temp_dir().join(format!("dent-launcher-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("dent.db"), "event 1\nevent 2\n").unwrap();
        Rig {
            state,
            launcher: None,
        }
    }

    fn version(&self, v: &str, trial: Trial, protocol: u32) {
        let dir = self.state.join("versions").join(v);
        std::fs::create_dir_all(&dir).unwrap();
        let trial = match trial {
            Trial::Migrates => r#"echo "column from $V" >> "$S/dent.db""#,
            Trial::Fails => r#"echo "half a table" >> "$S/dent.db"; log failing; exit 3"#,
            Trial::Hangs => "sleep 100 & wait $!",
            Trial::Locked => {
                r#"echo "the other daemon's write" >> "$S/dent.db"; log locked; exit 75"#
            }
        };
        let exe = dir.join("dent-daemon");
        std::fs::write(&exe, DAEMON.replace("@@TRIAL@@", trial)).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            dir.join("manifest"),
            format!("version={v}\nlauncher_protocol={protocol}\n"),
        )
        .unwrap();
    }

    fn active(&self, v: &str) {
        std::fs::write(self.state.join("launcher.state"), format!("active={v}\n")).unwrap();
    }

    fn start(&mut self, env: &[(&str, &str)]) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dent-launcher"));
        cmd.arg("run").arg("--state").arg(&self.state);
        for (k, v) in env {
            cmd.env(k, v);
        }
        self.launcher = Some(cmd.spawn().unwrap());
    }

    /// Stop the launcher as the service manager would.
    fn stop(&mut self) {
        if let Some(mut l) = self.launcher.take() {
            unsafe { libc_kill(l.id() as i32, 15) };
            let _ = l.wait();
        }
    }

    /// Wait for the launcher to end by itself, with its exit code.
    fn ends(&mut self, secs: u64) -> Option<i32> {
        let l = self.launcher.as_mut()?;
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until {
            if let Some(s) = l.try_wait().unwrap() {
                self.launcher = None;
                return s.code();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.state.join("log")).unwrap_or_default()
    }

    fn record(&self) -> String {
        std::fs::read_to_string(self.state.join("launcher.state")).unwrap_or_default()
    }

    fn db(&self) -> String {
        std::fs::read_to_string(self.state.join("dent.db")).unwrap_or_default()
    }

    /// Wait until the log has `needle` `n` times.
    fn saw(&self, needle: &str, n: usize, secs: f64) -> bool {
        let until = Instant::now() + Duration::from_secs_f64(secs);
        while Instant::now() < until {
            if self.log().matches(needle).count() >= n {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// The running daemon asks for an update, as `dent-daemon` does.
    fn ask(&self, v: &str) {
        std::fs::write(self.state.join("ask"), v).unwrap();
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.state);
    }
}

unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[test]
fn a_good_update_commits_and_the_old_daemon_hears_why_it_stops() {
    let mut r = Rig::new("commit");
    r.version("a", Trial::Migrates, 1);
    r.version("b", Trial::Migrates, 1);
    r.active("a");
    r.start(&[]);
    assert!(r.saw("a start trial=0", 1, 5.), "{}", r.log());
    r.ask("b");
    assert!(r.saw("b heard committed", 1, 10.), "{}", r.log());
    let log = r.log();
    assert!(log.contains("a heard accepted"), "{log}");
    assert!(log.contains("a stopped reason=update b"), "{log}");
    assert!(log.contains("b start trial=1"), "{log}");
    assert!(r.record().contains("active=b"), "{}", r.record());
    assert!(r.record().contains("committed"), "{}", r.record());
    assert!(r.db().contains("column from b"));
    // Once committed, the old version and older snapshots are pruned; the
    // one it came from stays.
    assert!(r.state.join("versions/a").is_dir());
    // Restarted after that, b runs and is told how the update went.
    r.stop();
    r.start(&[]);
    assert!(r.saw("b start trial=0 outcome=", 1, 5.));
    assert!(r.log().contains(" a b committed"), "{}", r.log());
}

#[test]
fn a_failed_migration_rolls_back_and_is_never_retried() {
    let mut r = Rig::new("fail");
    r.version("a", Trial::Migrates, 1);
    r.version("b", Trial::Fails, 1);
    r.active("a");
    let before = r.db();
    r.start(&[]);
    assert!(r.saw("a start", 1, 5.));
    r.ask("b");
    // Back on a, with the database as it was and the outcome to tell.
    assert!(r.saw("a start trial=0", 2, 10.), "{}", r.log());
    assert_eq!(r.db(), before, "the half-made table is gone");
    assert!(r.log().contains("rolled_back"), "{}", r.log());
    assert!(r.record().contains("active=a"));
    assert!(r.record().contains("never_retry=b"), "{}", r.record());
    // Not tried again on the daemon's own say-so...
    std::fs::write(r.state.join("ask-auto"), "b").unwrap();
    assert!(
        r.saw("a heard refused b rolled back before", 1, 5.),
        "{}",
        r.log()
    );
    assert_eq!(r.log().matches("b start").count(), 1);
    // ...but when someone asks for it, it's tried (and fails again here).
    r.ask("b");
    assert!(r.saw("b start trial=1", 2, 10.), "{}", r.log());
    assert!(r.saw("a start trial=0", 3, 10.), "{}", r.log());
}

#[test]
fn a_version_retried_by_hand_that_passes_is_held_back_no_more() {
    let mut r = Rig::new("retry");
    r.version("a", Trial::Migrates, 1);
    r.version("b", Trial::Migrates, 1);
    std::fs::write(r.state.join("launcher.state"), "active=a\nnever_retry=b\n").unwrap();
    r.start(&[]);
    assert!(r.saw("a start", 1, 5.));
    r.ask("b");
    assert!(r.saw("b heard committed", 1, 10.), "{}", r.log());
    assert!(!r.record().contains("never_retry"), "{}", r.record());
}

#[test]
fn a_trial_that_never_gets_ready_rolls_back() {
    let mut r = Rig::new("hang");
    r.version("a", Trial::Migrates, 1);
    r.version("b", Trial::Hangs, 1);
    r.active("a");
    r.start(&[("DENT_PREPARE_TIMEOUT", "1")]);
    assert!(r.saw("a start", 1, 5.));
    r.ask("b");
    assert!(r.saw("a start trial=0", 2, 10.), "{}", r.log());
    assert!(r.record().contains("never_retry=b"));
}

#[test]
fn what_would_break_an_update_is_refused_before_anything_stops() {
    let mut r = Rig::new("refuse");
    r.version("a", Trial::Migrates, 1);
    r.version("future", Trial::Migrates, 99);
    r.active("a");
    r.start(&[("DENT_LAUNCHER_FREE", "1000")]);
    assert!(r.saw("a start", 1, 5.));
    for (ask, said) in [
        ("future", "needs a newer launcher"),
        ("missing", "no readable manifest"),
        ("../x", "isn't a version"),
        ("a", "already runs"),
    ] {
        r.ask(ask);
        assert!(r.saw(said, 1, 5.), "{ask}: {}", r.log());
    }
    // Room for the database but not its snapshot.
    r.version("b", Trial::Migrates, 1);
    r.ask("b");
    assert!(r.saw("not enough room", 1, 5.), "{}", r.log());
    // Nothing stopped.
    assert_eq!(r.log().matches(" start").count(), 1, "{}", r.log());
}

#[test]
fn a_launcher_cut_off_mid_restore_finishes_it_before_running_anything() {
    let mut r = Rig::new("restore");
    r.version("a", Trial::Migrates, 1);
    r.version("b", Trial::Fails, 1);
    r.active("a");
    let before = r.db();
    r.start(&[("DENT_LAUNCHER_CRASH_IN_RESTORE", "1")]);
    assert!(r.saw("a start", 1, 5.));
    r.ask("b");
    assert_eq!(r.ends(10), Some(9), "the stand-in crash");
    assert!(r.state.join("restore-pending").exists());
    assert_ne!(r.db(), before, "left broken, as a crash would");
    r.start(&[]);
    assert!(r.saw("a start", 2, 10.), "{}", r.log());
    assert_eq!(r.db(), before);
    assert!(!r.state.join("restore-pending").exists());
    assert!(r.record().contains("never_retry=b"), "{}", r.record());
}

#[test]
fn a_launcher_stopped_mid_trial_goes_back_when_it_starts_again() {
    let mut r = Rig::new("midtrial");
    r.version("a", Trial::Migrates, 1);
    r.version("b", Trial::Hangs, 1);
    r.active("a");
    r.start(&[]);
    assert!(r.saw("a start", 1, 5.));
    r.ask("b");
    assert!(r.saw("b start trial=1", 1, 5.));
    r.stop();
    assert!(r.record().contains("pending=b"), "{}", r.record());
    r.start(&[]);
    assert!(r.saw("a start trial=0", 2, 10.), "{}", r.log());
    assert!(r.record().contains("never_retry=b"));
}

#[test]
fn a_crashed_daemon_is_started_again() {
    let mut r = Rig::new("crash");
    r.version("a", Trial::Migrates, 1);
    r.active("a");
    std::fs::write(r.state.join("exit-with"), "1").unwrap();
    r.start(&[]);
    assert!(r.saw("a exiting 1", 1, 5.));
    // A second later, and this time it stays up.
    assert!(r.saw("a start", 2, 5.), "{}", r.log());
}

#[test]
fn a_daemon_finding_another_running_is_left_waiting_not_looping() {
    let mut r = Rig::new("locked");
    r.version("a", Trial::Migrates, 1);
    r.active("a");
    std::fs::write(r.state.join("exit-with"), "75").unwrap();
    r.start(&[]);
    assert!(r.saw("a exiting 75", 1, 5.));
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(r.log().matches("a start").count(), 1, "{}", r.log());
}

#[test]
fn a_record_it_cant_read_stops_startup() {
    let mut r = Rig::new("broken");
    std::fs::write(r.state.join("launcher.state"), "active=a\nhalf").unwrap();
    r.start(&[]);
    assert_eq!(r.ends(5), Some(1));
    assert!(r.log().is_empty(), "nothing ran");
}

#[allow(dead_code)]
fn here() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn a_trial_that_finds_another_daemon_leaves_its_database_alone() {
    let mut r = Rig::new("lockedtrial");
    r.version("a", Trial::Migrates, 1);
    r.version("b", Trial::Locked, 1);
    r.active("a");
    r.start(&[]);
    assert!(r.saw("a start", 1, 5.));
    r.ask("b");
    assert!(r.saw("b locked", 1, 10.), "{}", r.log());
    assert!(r.saw("a start", 2, 10.), "{}", r.log());
    // The database is the other daemon's now: nothing is put back over it.
    assert!(r.db().contains("the other daemon's write"), "{}", r.db());
    // And b wasn't the trouble: it isn't held back.
    let rec = r.record();
    assert!(!rec.contains("never_retry"), "{rec}");
    assert!(rec.contains("not_tried"), "{rec}");
    assert!(
        !r.state
            .join("snapshots")
            .read_dir()
            .is_ok_and(|mut d| d.next().is_some())
    );
}
