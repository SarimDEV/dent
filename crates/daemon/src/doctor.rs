//! `dent-daemon doctor`: how dent stands on this machine, a line per check,
//! each saying what to do when it isn't right. Run from a terminal, about
//! the service (or the daemon the app runs): it works out what agents get
//! as the service would ([0074]) and asks the running daemon what it can.
//!
//! [0074]: ../../../docs/decisions/0074-what-agents-get-from-the-login-shell.md

use std::path::Path;
use std::process::Command;

use dent_protocol::{Account, Reply};

use crate::paths::Paths;

/// One check: how it stands, what was found, and what to do about it.
struct Check {
    mark: Mark,
    what: String,
    fix: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Ok,
    /// Worth knowing, not wrong: it depends on how you work.
    Note,
    Bad,
}

fn ok(what: impl Into<String>) -> Check {
    Check {
        mark: Mark::Ok,
        what: what.into(),
        fix: None,
    }
}

fn note(what: impl Into<String>, more: impl Into<String>) -> Check {
    Check {
        mark: Mark::Note,
        what: what.into(),
        fix: Some(more.into()),
    }
}

fn bad(what: impl Into<String>, fix: impl Into<String>) -> Check {
    Check {
        mark: Mark::Bad,
        what: what.into(),
        fix: Some(fix.into()),
    }
}

pub fn run(paths: &Paths) -> anyhow::Result<()> {
    let checks = checks(paths);
    let bad = checks.iter().filter(|c| c.mark == Mark::Bad).count();
    for c in &checks {
        let mark = match c.mark {
            Mark::Ok => "✓",
            Mark::Note => "·",
            Mark::Bad => "✗",
        };
        println!("{mark} {}", c.what);
        if let Some(f) = &c.fix {
            println!("    {f}");
        }
    }
    println!();
    if bad == 0 {
        println!("Everything checked is fine.");
    } else {
        println!("{bad} to put right.");
    }
    Ok(())
}

fn checks(paths: &Paths) -> Vec<Check> {
    let state = &paths.state;
    let mut out = Vec::new();
    let service = crate::service::installed();
    // The service, or the daemon however it runs.
    match &service {
        Some((file, running)) if *running => {
            out.push(ok(format!("Runs as a service ({})", file.display())))
        }
        Some((file, _)) => out.push(bad(
            format!(
                "Installed as a service but not running ({})",
                file.display()
            ),
            crate::service::start_hint(),
        )),
        None => out.push(ok(
            "Not installed as a service (the app, or you, start the daemon)",
        )),
    }
    if state.join("install-intent").exists() {
        out.push(bad(
            "The last install didn't finish",
            "Run dent-daemon install again",
        ));
    }
    if cfg!(target_os = "linux") && service.is_some() {
        out.push(linger());
    }
    let welcome = crate::admin::welcome(paths);
    match &welcome {
        Ok(Reply::Welcome {
            release,
            version,
            addresses,
            managed,
            ..
        }) => {
            let v = if release.is_empty() { version } else { release };
            out.push(ok(format!(
                "The daemon answers ({v}{})",
                if *managed {
                    ", run by its launcher"
                } else {
                    ""
                }
            )));
            out.push(if addresses.is_empty() {
                bad(
                    "Other devices can't reach it: it listens on no address they can use",
                    "Sign in to Tailscale here, or set DENT_REMOTE_LAN=1 in service.env for the local network",
                )
            } else {
                ok(format!("Other devices reach it at {}", addresses.join(", ")))
            });
        }
        Ok(_) => {}
        Err(e) => out.push(bad(
            format!("The daemon doesn't answer ({e})"),
            match &service {
                Some(_) => crate::service::log_hint_here(state),
                None => "Open dent, or run dent-daemon".to_string(),
            },
        )),
    }
    // What agents get, as the service would give it.
    let env = crate::env::AgentEnv::detect_as(service.is_some());
    out.push(ok(format!("Agents' PATH comes from {}", env.source)));
    for d in crate::agent::DRIVERS {
        let (version, account) = d.status(&env);
        // "2.1.301 (Claude Code)": the name's said already.
        let version = version.map(|v| v.split(" (").next().unwrap_or(&v).to_string());
        out.push(match (version, account) {
            (None, _) => bad(
                format!("{} isn't found on that PATH ({})", d.name(), d.command()),
                format!("Install it ({}), or add its folder to PATH in your shell profile", d.install_url()),
            ),
            (Some(v), Account::SignedIn { email, .. }) => ok(format!(
                "{} {v}, signed in{}",
                d.name(),
                email.map(|e| format!(" as {e}")).unwrap_or_default()
            )),
            (Some(v), Account::ApiKey) => ok(format!("{} {v}, with an API key", d.name())),
            (Some(v), Account::SignedOut) => bad(
                format!("{} {v} isn't signed in", d.name()),
                match d.sign_in() {
                    Some(cmd) if service.is_some() => format!(
                        "Run `{cmd}` here, or `claude setup-token` and export its token from your shell profile"
                    ),
                    Some(cmd) => format!("Run `{cmd}`"),
                    None => "Sign it in".into(),
                },
            ),
            (Some(v), Account::Unknown) => ok(format!("{} {v} (couldn't tell how it's signed in)", d.name())),
        });
    }
    let ssh = env.vars.iter().find(|(k, _)| k == "SSH_AUTH_SOCK");
    out.push(match ssh {
        Some((_, sock)) if Path::new(sock).exists() => {
            ok("Agents can use your SSH keys (an SSH agent is running)")
        }
        _ => note(
            "No SSH agent in your login shell, so agents can't use keys with a passphrase",
            "Fine for HTTPS remotes. For git over SSH, start an agent from your shell profile",
        ),
    });
    out.push(room(state));
    out.push(database(&paths.db()));
    out
}

fn linger() -> Check {
    let user = std::env::var("USER").unwrap_or_default();
    let on = Command::new("loginctl")
        .args(["show-user", &user, "--property=Linger", "--value"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "yes");
    if on {
        ok("Keeps running after you log out (linger)")
    } else {
        bad(
            "Stops when you log out: linger is off",
            format!("Run once: sudo loginctl enable-linger {user}"),
        )
    }
}

/// Room for updates: a snapshot of the database, and some to spare.
fn room(state: &Path) -> Check {
    let db: u64 = ["dent.db", "dent.db-wal"]
        .iter()
        .filter_map(|f| std::fs::metadata(state.join(f)).ok())
        .map(|m| m.len())
        .sum();
    let free = free_bytes(state).unwrap_or(u64::MAX);
    let need = db + db / 20 + 16 * 1024 * 1024;
    if free >= need * 4 {
        ok(format!(
            "Room for updates ({} free, database {})",
            size(free),
            size(db)
        ))
    } else if free >= need {
        bad(
            format!(
                "Little room left ({} free; an update needs {})",
                size(free),
                size(need)
            ),
            "Free some space soon: updates are refused without room for a snapshot",
        )
    } else {
        bad(
            format!(
                "No room for an update ({} free, {} needed)",
                size(free),
                size(need)
            ),
            "Free some space: updates are refused until there's room for a snapshot of the database",
        )
    }
}

/// A size as people read one: 640 KB, 12 MB, 4.2 GB.
fn size(n: u64) -> String {
    let n = n as f64;
    if n < 1024. * 1024. {
        format!("{:.0} KB", (n / 1024.).max(1.))
    } else if n < 1024. * 1024. * 1024. {
        format!("{:.0} MB", n / 1_048_576.)
    } else {
        format!("{:.1} GB", n / 1_073_741_824.)
    }
}

// The field types differ by platform.
#[allow(clippy::useless_conversion)]
fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: a valid C string, and plain data the call fills.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        (libc::statvfs(c.as_ptr(), &mut st) == 0)
            .then(|| u64::from(st.f_bavail) * u64::from(st.f_frsize))
    }
}

/// SQLite's own quick check, read only: safe beside the running daemon.
fn database(db: &Path) -> Check {
    if !db.exists() {
        return ok("No database yet");
    }
    let opened = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    );
    let said =
        opened.and_then(|c| c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)));
    match said {
        Ok(s) if s == "ok" => ok("The database checks out"),
        Ok(s) => bad(
            format!("The database is damaged ({s})"),
            "Stop dent, copy dent.db somewhere safe, and open an issue with what this said",
        ),
        Err(e) => bad(
            format!("The database couldn't be read ({e})"),
            "Check the state folder's permissions",
        ),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(super::size(0), "1 KB");
        assert_eq!(super::size(640 * 1024), "640 KB");
        assert_eq!(super::size(12 * 1024 * 1024), "12 MB");
        assert_eq!(super::size(4_509_715_660), "4.2 GB");
    }
}
