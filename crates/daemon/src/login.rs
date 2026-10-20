//! Starting the daemon at login, as a launchd agent, so sessions resume
//! after a restart without opening the app. Off unless asked for:
//! `dent-daemon login-item on|off`.
//!
//! The agent runs this binary where it's installed, never from Documents,
//! Desktop or Downloads, which macOS guards from background programs.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const LABEL: &str = "sh.dent.daemon";

fn plist_path() -> PathBuf {
    crate::paths::home()
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// The launchd agent for `bin`, keeping state in `state`.
pub fn plist(bin: &Path, state: &Path) -> String {
    let esc = |p: &Path| {
        p.display()
            .to_string()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{bin}</string><string>--state</string><string>{state}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>StandardErrorPath</key><string>{log}</string>
  <key>ProcessType</key><string>Background</string>
</dict>
</plist>
"#,
        bin = esc(bin),
        state = esc(state),
        log = esc(&state.join("daemon.log")),
    )
}

/// The account's own home folder, from the user database: what macOS
/// guards, whatever `HOME` says.
fn real_home() -> Option<PathBuf> {
    // SAFETY: getpwuid returns a pointer into static storage or null; it's
    // read at once, on this thread, before anything else could call it.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        let dir = std::ffi::CStr::from_ptr((*pw).pw_dir);
        Some(PathBuf::from(dir.to_string_lossy().into_owned()))
    }
}

fn guarded(p: &Path) -> bool {
    let home = real_home().unwrap_or_else(crate::paths::home);
    ["Documents", "Desktop", "Downloads"]
        .iter()
        .any(|d| p.starts_with(home.join(d)))
}

/// The daemon starts at login.
pub fn is_on() -> bool {
    plist_path().is_file()
}

/// Why this daemon can't be made to start at login, when it can't.
pub fn blocked() -> Option<String> {
    // A scratch home (walkthroughs, tests) mustn't reach the real launchd.
    if real_home().is_some_and(|h| h != crate::paths::home()) {
        return Some("dent is running with a home folder of its own here".into());
    }
    let bin = std::env::current_exe().ok()?;
    guarded(&bin).then(|| {
        "This build runs from a folder macOS keeps background programs out of; install dent.app in Applications first".to_string()
    })
}

pub fn set(on: bool, state: &Path) -> Result<()> {
    if on && let Some(why) = blocked() {
        bail!("{why}");
    }
    let uid = unsafe { libc::getuid() };
    let target = format!("gui/{uid}");
    let path = plist_path();
    // Stop any earlier agent first; it's fine if there was none.
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &format!("{target}/{LABEL}")])
        .status();
    if !on {
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        println!("dent won't start at login");
        return Ok(());
    }
    let bin = std::env::current_exe()?;
    if guarded(&bin) {
        bail!(
            "{} is in a folder macOS guards from background programs; install dent.app in ~/Applications first",
            bin.display()
        );
    }
    std::fs::create_dir_all(path.parent().context("no LaunchAgents folder")?)?;
    std::fs::write(&path, plist(&bin, state))?;
    let ok = std::process::Command::new("launchctl")
        .args(["bootstrap", &target])
        .arg(&path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        bail!("launchctl couldn't load {}", path.display());
    }
    println!("dent starts at login ({})", bin.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_agent_runs_this_daemon_with_its_state() {
        let p = super::plist(
            std::path::Path::new("/Users/a/Applications/dent.app/Contents/MacOS/dent-daemon"),
            std::path::Path::new("/Users/a/Library/Application Support/dent"),
        );
        assert!(p.contains(
            "<string>/Users/a/Applications/dent.app/Contents/MacOS/dent-daemon</string>"
        ));
        assert!(p.contains(
            "<string>--state</string><string>/Users/a/Library/Application Support/dent</string>"
        ));
        assert!(p.contains("<key>RunAtLoad</key><true/>"));
    }
}
