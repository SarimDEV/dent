//! Running dent as a service on a machine: `dent-daemon install`,
//! `uninstall`, `status` and `update DIR` ([decision 0065]).
//!
//! The service manager runs `dent-launcher`, which runs a version of the
//! daemon from `versions/<v>/` in the state folder and owns updates. Linux
//! uses a systemd user unit, macOS a LaunchAgent. What's yours (PATH, and
//! anything else agents need) is in `service.env`, outside the unit, and
//! survives reinstalling.
//!
//! [decision 0065]: ../../../docs/decisions/0065-launcher-and-updates.md

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use dent_protocol::{Reply, Request};

use crate::paths::Paths;

/// The binaries a version is made of, found beside this one.
const PARTS: [&str; 2] = ["dent-daemon", "dent-session-holder"];

/// What `dent-daemon manifest` prints, and a staged version keeps: read by
/// the launcher with nothing but the standard library.
pub fn manifest() -> String {
    format!(
        "version={}\nlauncher_protocol={}\n",
        env!("CARGO_PKG_VERSION"),
        crate::launcher::PROTOCOL
    )
}

fn manifest_field(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        .map(|v| v.trim().to_string())
}

/// Copy the version beside `from` (its daemon and holder) into
/// `versions/<v>/`, named by its version and a hash of its files, so two
/// builds of one version never share a folder. Returns the name.
pub fn stage(state: &Path, from: &Path) -> Result<String> {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in PARTS {
        let bytes = std::fs::read(from.join(part))
            .with_context(|| format!("{part} isn't in {}", from.display()))?;
        for b in bytes {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    // The version says what it is, and what launcher it needs.
    let out = Command::new(from.join("dent-daemon"))
        .arg("manifest")
        .output()
        .context("ask the new daemon for its manifest")?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let Some(version) = manifest_field(&text, "version").filter(|_| out.status.success()) else {
        bail!(
            "{} is too old to install as a service (it has no manifest)",
            from.join("dent-daemon").display()
        );
    };
    let name = format!("{version}-{:08x}", hash as u32);
    let versions = state.join("versions");
    let dir = versions.join(&name);
    if dir.join("manifest").is_file() {
        return Ok(name);
    }
    let staging = versions.join(format!("{name}.staging"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    for part in PARTS {
        std::fs::copy(from.join(part), staging.join(part))?;
    }
    std::fs::write(staging.join("manifest"), &text)?;
    std::fs::File::open(&staging)?.sync_all()?;
    std::fs::rename(&staging, &dir)?;
    Ok(name)
}

/// The service manager this machine has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Manager {
    Systemd,
    Launchd,
}

const UNIT: &str = "dent.service";
const LABEL: &str = "sh.dent.launcher";

impl Manager {
    fn here() -> Result<Self> {
        if cfg!(target_os = "macos") {
            return Ok(Manager::Launchd);
        }
        let user_bus = Command::new("systemctl")
            .args(["--user", "show-environment"])
            .output()
            .is_ok_and(|o| o.status.success());
        if !user_bus {
            bail!(
                "systemd's user manager isn't reachable from here. Log in as this user over SSH (not `su` or `sudo -u`) and try again"
            );
        }
        Ok(Manager::Systemd)
    }

    fn file(self) -> PathBuf {
        let home = crate::paths::home();
        match self {
            Manager::Systemd => std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"))
                .join("systemd/user")
                .join(UNIT),
            Manager::Launchd => home
                .join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist")),
        }
    }

    fn installed(self) -> bool {
        self.file().is_file()
    }

    /// The service's definition, running the launcher from the state folder.
    fn definition(self, state: &Path) -> String {
        let launcher = launcher_path(state);
        match self {
            Manager::Systemd => systemd_unit(&launcher, state),
            Manager::Launchd => launchd_plist(&launcher, state),
        }
    }

    fn running(self) -> bool {
        match self {
            Manager::Systemd => run("systemctl", &["--user", "is-active", "--quiet", UNIT]),
            Manager::Launchd => run("launchctl", &["print", &launchd_target()]),
        }
    }

    /// Start it, or restart it to run a launcher that changed.
    fn start(self, restart: bool) -> Result<()> {
        let ok = match self {
            Manager::Systemd => {
                run("systemctl", &["--user", "daemon-reload"])
                    && run("systemctl", &["--user", "enable", UNIT])
                    && run(
                        "systemctl",
                        &["--user", if restart { "restart" } else { "start" }, UNIT],
                    )
            }
            Manager::Launchd => {
                if restart {
                    let _ = run("launchctl", &["bootout", &launchd_target()]);
                    self.wait_gone();
                }
                run("launchctl", &["print", &launchd_target()])
                    || run(
                        "launchctl",
                        &[
                            "bootstrap",
                            &format!("gui/{}", uid()),
                            &self.file().display().to_string(),
                        ],
                    )
            }
        };
        if !ok {
            bail!(
                "the service manager couldn't start {}",
                self.file().display()
            );
        }
        Ok(())
    }

    fn remove(self) -> Result<()> {
        match self {
            Manager::Systemd => {
                let _ = run("systemctl", &["--user", "disable", "--now", UNIT]);
                let _ = std::fs::remove_file(self.file());
                let _ = run("systemctl", &["--user", "daemon-reload"]);
            }
            Manager::Launchd => {
                let _ = run("launchctl", &["bootout", &launchd_target()]);
                self.wait_gone();
                let _ = std::fs::remove_file(self.file());
            }
        }
        Ok(())
    }

    /// Wait for the service to finish stopping: `launchctl bootout` returns
    /// while the launcher is still stopping its daemon, and a bootstrap made
    /// meanwhile fails.
    fn wait_gone(self) {
        let until = Instant::now() + Duration::from_secs(15);
        while self.running() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn uid() -> u32 {
    // SAFETY: getuid can't fail.
    unsafe { libc::getuid() }
}

fn launchd_target() -> String {
    format!("gui/{}/{LABEL}", uid())
}

fn run(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn launcher_path(state: &Path) -> PathBuf {
    state.join("bin/dent-launcher")
}

/// The systemd user unit. Only the launcher is sent SIGTERM (`mixed`), and
/// it stops the daemon itself, saying why. The holder runs in a scope of
/// its own, outside this unit, so stopping or restarting the service never
/// reaches the agents.
pub fn systemd_unit(launcher: &Path, state: &Path) -> String {
    let q = |p: &Path| systemd_quote(&p.display().to_string());
    format!(
        "[Unit]
Description=dent: runs your agents on this machine
After=network-online.target
Wants=network-online.target

[Service]
ExecStart={launcher} run --state {state}
Restart=always
RestartSec=2
KillMode=mixed
TimeoutStopSec=30

[Install]
WantedBy=default.target
",
        launcher = q(launcher),
        state = q(state),
    )
}

/// A path as one systemd word: quoted, with `\\`, `"` and `%` escaped.
fn systemd_quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

/// The LaunchAgent. It runs while you're logged in, where Claude Code's
/// login in your keychain can be reached.
pub fn launchd_plist(launcher: &Path, state: &Path) -> String {
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
  <array><string>{launcher}</string><string>run</string><string>--state</string><string>{state}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>{log}</string>
  <key>ProcessType</key><string>Background</string>
</dict>
</plist>
"#,
        launcher = esc(launcher),
        state = esc(state),
        log = esc(&state.join("daemon.log")),
    )
}

/// The parts of a semantic version, for telling an older one.
fn semver(v: &str) -> Vec<u64> {
    v.split(['-', '+'])
        .next()
        .unwrap_or_default()
        .split('.')
        .map(|n| n.parse().unwrap_or(0))
        .collect()
}

fn version_of(state: &Path, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(state.join("versions").join(name).join("manifest")).ok()?;
    manifest_field(&text, "version")
}

fn active(state: &Path) -> Option<String> {
    let text = std::fs::read_to_string(state.join("launcher.state")).ok()?;
    manifest_field(&text, "active")
}

/// Wait until a daemon answers on the socket, and say who it is.
fn wait_up(paths: &Paths, limit: Duration) -> Option<Reply> {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if let Ok(w) = crate::admin::welcome(paths) {
            return Some(w);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    None
}

/// `dent-daemon install`: run this build as the machine's service.
pub fn install(paths: &Paths, allow_downgrade: bool) -> Result<()> {
    let manager = Manager::here()?;
    if manager == Manager::Launchd && crate::login::is_on() {
        bail!(
            "dent already starts at login from the app; turn that off first (dent-daemon login-item off)"
        );
    }
    let me = std::env::current_exe()?;
    let from = me.parent().context("no folder for this binary")?;
    let new_launcher = from.join("dent-launcher");
    let speaks = Command::new(&new_launcher)
        .arg("protocol")
        .output()
        .with_context(|| format!("dent-launcher isn't beside {}", me.display()))?;
    let speaks: u32 = String::from_utf8_lossy(&speaks.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    if speaks < crate::launcher::PROTOCOL {
        bail!("the dent-launcher beside this build is too old for it");
    }
    let state = &paths.state;
    std::fs::create_dir_all(state)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(state, std::fs::Permissions::from_mode(0o700))?;
    }
    // Left until the service runs this version, so an install cut short
    // says so next time.
    let intent = state.join("install-intent");
    std::fs::write(&intent, "")?;
    let version = stage(state, from)?;
    // An older build doesn't replace a newer one unless asked to.
    if let Some(was) = active(state).and_then(|a| version_of(state, &a))
        && let Some(now) = version_of(state, &version)
        && semver(&now) < semver(&was)
        && !allow_downgrade
    {
        bail!(
            "this build is {now}, older than the {was} installed; add --allow-downgrade to go back"
        );
    }
    // The launcher: replaced whole, so the service never runs half of one.
    let launcher = launcher_path(state);
    std::fs::create_dir_all(launcher.parent().context("no bin folder")?)?;
    let launcher_changed = std::fs::read(&launcher).ok() != std::fs::read(&new_launcher).ok();
    if launcher_changed {
        let tmp = launcher.with_extension("new");
        std::fs::copy(&new_launcher, &tmp)?;
        std::fs::rename(&tmp, &launcher)?;
    }
    write_service_env(state)?;
    let fresh = active(state).is_none();
    if fresh {
        std::fs::write(state.join("launcher.state"), format!("active={version}\n"))?;
    }
    // A daemon run another way (by hand, or an older install) makes way.
    if let Ok(Reply::Welcome { managed: false, .. }) = crate::admin::welcome(paths) {
        println!("Stopping the daemon that runs outside the service…");
        let _ = crate::admin::ask(paths, Request::Restart);
        std::thread::sleep(Duration::from_millis(500));
    }
    let def = manager.definition(state);
    let def_changed = std::fs::read_to_string(manager.file()).ok().as_deref() != Some(def.as_str());
    if def_changed {
        std::fs::create_dir_all(manager.file().parent().context("no service folder")?)?;
        std::fs::write(manager.file(), &def)?;
    }
    let restart = manager.running() && (launcher_changed || def_changed);
    if restart {
        // Devices hear "restarting", not "quit".
        std::fs::write(state.join("stop-reason"), "restart")?;
    }
    manager.start(restart)?;
    let Some(Reply::Welcome {
        managed, release, ..
    }) = wait_up(paths, Duration::from_secs(30))
    else {
        bail!(
            "the service started but the daemon didn't answer in 30 s; see {}",
            log_hint(manager, state)
        );
    };
    if !managed {
        bail!(
            "another daemon, not the service, answers for {}",
            state.display()
        );
    }
    // Installed over an older version: updated through the launcher, which
    // trials it and puts the old one back if it fails.
    if release != version {
        println!("Updating {release} to {version}…");
        update_to(paths, &version)?;
    }
    let _ = std::fs::remove_file(&intent);
    println!("dent runs as a service on this machine ({version}).");
    if manager == Manager::Systemd {
        linger();
    }
    Ok(())
}

fn log_hint(manager: Manager, state: &Path) -> String {
    match manager {
        Manager::Systemd => "journalctl --user -u dent".into(),
        Manager::Launchd => state.join("daemon.log").display().to_string(),
    }
}

/// `service.env`: written once, from the login shell, and kept after that
/// (it's yours to add to). The launcher gives it to every daemon it runs.
fn write_service_env(state: &Path) -> Result<()> {
    let file = state.join("service.env");
    if file.exists() {
        return Ok(());
    }
    let env = crate::env::AgentEnv::detect();
    let text = format!(
        "# What dent's service runs with, read by dent-launcher at start.\n# PATH came from {}; add anything else agents need as KEY=VALUE.\nPATH={}\n",
        env.source, env.path
    );
    std::fs::write(&file, text)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Without linger, systemd stops a user's services when they log out:
/// a headless box would lose dent with its last SSH session.
fn linger() {
    let user = std::env::var("USER").unwrap_or_default();
    let on = Command::new("loginctl")
        .args(["show-user", &user, "--property=Linger", "--value"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "yes");
    if on || run("loginctl", &["enable-linger", &user]) {
        return;
    }
    println!(
        "To keep dent running after you log out, run once:\n  sudo loginctl enable-linger {user}"
    );
}

/// Ask the running service to move to `version` (staged already), and wait
/// to hear how it went.
fn update_to(paths: &Paths, version: &str) -> Result<()> {
    crate::admin::ask(
        paths,
        Request::Update {
            version: version.to_string(),
        },
    )?;
    // Stopped, trialled, committed or put back: the daemon that answers
    // next says which.
    std::thread::sleep(Duration::from_secs(1));
    let until = Instant::now() + Duration::from_secs(120);
    while Instant::now() < until {
        if let Ok(Reply::Welcome {
            release, update, ..
        }) = crate::admin::welcome(paths)
        {
            if release == version {
                return Ok(());
            }
            if let Some(u) = update.filter(|u| u.to == version && u.rolled_back) {
                bail!(
                    "{} didn't start, so {} runs again; see the service's log",
                    u.to,
                    u.from
                );
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!("the update didn't finish in 2 minutes; `dent-daemon status` says where it is")
}

/// `dent-daemon update DIR`: stage the build in DIR and update to it.
pub fn update(paths: &Paths, from: &Path) -> Result<()> {
    let version = stage(&paths.state, from)?;
    update_to(paths, &version)?;
    println!("Updated to {version}.");
    Ok(())
}

/// The service's file, and whether it runs, if it's installed.
pub fn installed() -> Option<(PathBuf, bool)> {
    let m = Manager::here().ok()?;
    m.installed().then(|| (m.file(), m.running()))
}

/// How to start the service by hand.
pub fn start_hint() -> String {
    if cfg!(target_os = "macos") {
        format!("Run: launchctl kickstart gui/{}/{LABEL}", uid())
    } else {
        format!("Run: systemctl --user start {UNIT}")
    }
}

/// Where the service's log is, here.
pub fn log_hint_here(state: &Path) -> String {
    let m = if cfg!(target_os = "macos") {
        Manager::Launchd
    } else {
        Manager::Systemd
    };
    format!("See its log: {}", log_hint(m, state))
}

/// `dent-daemon uninstall`: stop the service and remove it. Sessions and
/// settings stay in the state folder.
pub fn uninstall(paths: &Paths) -> Result<()> {
    let manager = Manager::here()?;
    if !manager.installed() {
        println!("dent isn't installed as a service here.");
        return Ok(());
    }
    manager.remove()?;
    // Without its launcher the folder no longer reads as run by a service,
    // so the app starts a daemon for it again. Versions and the record
    // stay, for an install later.
    let _ = std::fs::remove_file(launcher_path(&paths.state));
    println!(
        "dent's service is removed. Your sessions stay in {}; agents still running finish their turn and stop.",
        paths.state.display()
    );
    Ok(())
}

/// `dent-daemon status`: how the service stands.
pub fn status(paths: &Paths) -> Result<()> {
    let manager = Manager::here()?;
    if !manager.installed() {
        println!("Not installed as a service (dent-daemon install).");
        return Ok(());
    }
    let state = std::fs::read_to_string(paths.state.join("launcher.state")).unwrap_or_default();
    println!(
        "Service: {} ({})",
        if manager.running() {
            "running"
        } else {
            "stopped"
        },
        manager.file().display()
    );
    if let Some(a) = manifest_field(&state, "active") {
        println!("Version: {a}");
    }
    if let Some(p) = manifest_field(&state, "pending") {
        println!("On trial: {p}");
    }
    if let Some(o) = manifest_field(&state, "outcome") {
        let w: Vec<&str> = o.split(' ').collect();
        if let [_, from, to, how] = w[..] {
            let how = if how == "committed" {
                "updated"
            } else {
                "rolled back"
            };
            println!("Last update: {from} → {to}, {how}");
        }
    }
    if paths.state.join("install-intent").exists() {
        println!("The last install didn't finish; run dent-daemon install again.");
    }
    match crate::admin::welcome(paths) {
        Ok(_) => println!("Daemon: answering"),
        Err(e) => println!("Daemon: {e}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn the_unit_quotes_paths_and_stops_only_the_launcher() {
        let u = super::systemd_unit(
            Path::new("/home/a b/.local/state/dent/bin/dent-launcher"),
            Path::new("/home/a b/.local/state/dent 100%"),
        );
        assert!(u.contains(
            "ExecStart=\"/home/a b/.local/state/dent/bin/dent-launcher\" run --state \"/home/a b/.local/state/dent 100%%\""
        ), "{u}");
        assert!(u.contains("KillMode=mixed"));
        assert!(u.contains("Restart=always"));
    }

    #[test]
    fn the_agent_runs_the_launcher_with_its_state() {
        let p = super::launchd_plist(
            Path::new("/Users/a/Library/Application Support/dent/bin/dent-launcher"),
            Path::new("/Users/a/Library/Application Support/dent"),
        );
        assert!(p.contains("<string>run</string><string>--state</string>"));
        assert!(p.contains("<key>KeepAlive</key><true/>"));
    }

    #[test]
    fn an_older_build_is_older() {
        use super::semver;
        assert!(semver("0.1.0") < semver("0.2.0"));
        assert!(semver("0.9.0") < semver("0.10.0"));
        assert!(semver("0.1.0-3f2a") == semver("0.1.0"));
        assert!(semver("1.0.0+dev") > semver("0.99.0"));
    }
}
