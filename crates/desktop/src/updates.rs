//! Updates, from the app ([decision 0077]).
//!
//! Every machine makes main's newer builds ready on its own: its daemon
//! downloads, checks and stages one, and says so ([`Out::Available`]). The
//! app does the same for itself. One press of Update all runs every ready
//! build: daemons first (the holder keeps their agents running), then the
//! app, which quits and is put back where it was by a small script that
//! swaps the new dent.app in, opens it, and puts the old one back if the
//! new one doesn't start.
//!
//! A dent CI didn't build (number 0), or one not run from a dent.app, never
//! looks.
//!
//! [decision 0077]: ../../../docs/decisions/0077-updates-from-main.md

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use dent_protocol::{Available, Request};
use gpui_kit::{Context, FollowMode, ListOffset, Window, px};
use serde::{Deserialize, Serialize};

use crate::app::{Dent, ToastAction};

/// How long a press of Update all waits for its machines before the app
/// goes anyway: one that doesn't come back is shown on its row.
const MACHINES_WAIT: Duration = Duration::from_secs(150);

/// How often to look for a newer build.
const EVERY: Duration = Duration::from_secs(120);

/// How long the new app has to start before the old one is put back,
/// in tenths of a second.
const START_WAIT: u32 = 600;

/// A newer app, downloaded, checked and unpacked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ready {
    pub build: u64,
    pub app: PathBuf,
}

static READY: Mutex<Option<Ready>> = Mutex::new(None);

/// What's ready changed off the window's thread: the next tick draws it.
static CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether what's ready changed since last asked.
pub fn take_changed() -> bool {
    CHANGED.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// Where the app keeps its updates, beside its preferences.
fn dir() -> PathBuf {
    crate::live::default_state().join("app-updates")
}

/// The dent.app this runs from, if it runs from one.
pub fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension()? == "app").then(|| app.to_path_buf())
}

/// Whether this dent.app can update itself where it is: not run from a
/// disk image or a download macOS moved aside, and in a folder it can
/// write to (the swap moves it).
fn can_swap(app: &Path) -> bool {
    let s = app.to_string_lossy();
    if s.contains("/AppTranslocation/") || s.starts_with("/Volumes/") {
        return false;
    }
    let Some(parent) = app.parent() else {
        return false;
    };
    let probe = parent.join(format!(".dent-update-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Whether this run of the app updates itself: one CI built, from a
/// dent.app it can swap, in the foreground.
fn updates_itself() -> bool {
    cfg!(target_os = "macos")
        && dent_update::enabled()
        && !crate::BACKGROUND.load(std::sync::atomic::Ordering::Relaxed)
        && bundle().is_some_and(|b| can_swap(&b))
}

/// Look for newer builds now and then, for as long as the app runs.
pub fn start() {
    if !updates_itself() {
        return;
    }
    // `DENT_UPDATE_EVERY` (seconds) for trials.
    let every = std::env::var("DENT_UPDATE_EVERY")
        .ok()
        .and_then(|s| s.parse().ok())
        .map_or(EVERY, Duration::from_secs);
    std::thread::spawn(move || {
        loop {
            if let Err(e) = look(&dent_update::feed_url(), &dir()) {
                eprintln!("dent: looking for an update: {e}");
            }
            std::thread::sleep(every);
        }
    });
}

/// The newer app that's ready, by build.
pub fn ready() -> Option<u64> {
    READY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|r| r.build)
}

/// Make the newest build ready in `dir`, if it's newer than this one.
fn look(feed: &str, dir: &Path) -> Result<(), String> {
    let feed = dent_update::fetch(feed)?;
    if !feed.is_newer() || skipped(dir) == Some(feed.build) || ready() == Some(feed.build) {
        return Ok(());
    }
    let part = feed.part("app")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let archive = dir.join(format!("{}.tar.gz", feed.build));
    dent_update::download(part, &archive)?;
    let into = dir.join(feed.build.to_string());
    dent_update::unpack(&archive, &into)?;
    let _ = std::fs::remove_file(&archive);
    // What an older look left (a download that didn't finish) goes too.
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".tar.gz") || name.ends_with(".part") {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let app = into.join("dent.app");
    if !app.join("Contents/MacOS/dent-desktop").is_file() {
        return Err(format!("build {} has no dent.app", feed.build));
    }
    // Signed by whoever signed this one, or it doesn't go in.
    if let Some(running) = bundle()
        && let Err(e) = dent_update::same_signer(&running, &app)
    {
        let _ = std::fs::remove_dir_all(&into);
        return Err(e);
    }
    // Builds made ready before this one are done with.
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.parse::<u64>().is_ok_and(|b| b != feed.build) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    *READY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Ready {
        build: feed.build,
        app,
    });
    CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

/// A walk's stand-in for a newer app being ready.
pub fn pretend_ready(build: Option<u64>) {
    *READY.lock().unwrap_or_else(|e| e.into_inner()) = build.map(|build| Ready {
        build,
        app: PathBuf::from("/nowhere/dent.app"),
    });
}

/// A build that didn't start here: never made ready again.
fn skipped(dir: &Path) -> Option<u64> {
    std::fs::read_to_string(dir.join("skipped"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Running: the script that opened this app knows it started. Every start
/// says so; only a swap waits for it.
pub fn started() {
    if updates_itself() {
        let d = dir();
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join("started"), dent_update::number().to_string());
    }
}

/// The build that didn't start, if the last update went back: said once.
pub fn went_back() -> Option<u64> {
    let note = dir().join("went-back");
    let build = std::fs::read_to_string(&note).ok()?.trim().parse().ok();
    let _ = std::fs::remove_file(&note);
    build
}

/// Where the app was, kept across its update.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Resume {
    pub session: Option<String>,
    /// x, y, width, height.
    pub window: Option<(f32, f32, f32, f32)>,
    /// The chat's top row and how far into it, unless it followed the end.
    pub scroll: Option<(usize, f32)>,
}

/// Where the app was before its update, if it just updated: read once.
/// Anything older than a few minutes is from an update that didn't happen.
pub fn take_resume() -> Option<Resume> {
    let path = dir().join("resume.json");
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(300));
    let r = fresh
        .then(|| std::fs::read(&path).ok())
        .flatten()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let _ = std::fs::remove_file(&path);
    r
}

/// Swaps a new dent.app in once the running one has quit: opens it, and if
/// it hasn't started (`$ok` written) in time, puts the old one back, says
/// which build didn't start in `$note`, and opens the old one.
const SWAP: &str = r#"#!/bin/sh
pid=$1 new=$2 app=$3 prev=$4 ok=$5 note=$6 build=$7 wait=$8
open=${DENT_OPEN:-open}
# The app's binary, as a pattern matching it and nothing else.
exe=$(printf '%s' "$app/Contents/MacOS/dent-desktop" | sed 's/[][\.*^$()+?{}|]/\\&/g')
i=0
while kill -0 "$pid" 2>/dev/null; do
  i=$((i+1))
  # It never quit: leave it be.
  [ "$i" -gt 600 ] && exit 1
  sleep 0.1
done
rm -rf "$prev" "$ok"
mkdir -p "$(dirname "$prev")"
# Whatever goes wrong, dent opens again, as it was.
if ! mv "$app" "$prev"; then
  $open "$app"; exit 1
fi
if ! mv "$new" "$app"; then
  rm -rf "$app"; mv "$prev" "$app"; $open "$app"; exit 1
fi
rmdir "$(dirname "$new")" 2>/dev/null
$open "$app"
i=0
while [ "$(cat "$ok" 2>/dev/null)" != "$build" ]; do
  i=$((i+1))
  if [ "$i" -gt "$wait" ]; then
    pkill -f "^$exe"
    j=0
    while pgrep -qf "^$exe" && [ "$j" -lt 50 ]; do j=$((j+1)); sleep 0.1; done
    pkill -9 -f "^$exe"
    rm -rf "$app"
    mv "$prev" "$app"
    echo "$build" > "$note"
    echo "$build" > "$(dirname "$note")/skipped"
    $open "$app"
    exit 1
  fi
  sleep 0.1
done
exit 0
"#;

/// Start the swap for `ready` over the app at `app`, waiting for process
/// `pid` to quit: the caller quits next.
fn swap(ready: &Ready, app: &Path, pid: u32, dir: &Path, wait: u32) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let script = dir.join("swap.sh");
    std::fs::write(&script, SWAP).map_err(|e| e.to_string())?;
    std::process::Command::new("/bin/sh")
        .arg(&script)
        .arg(pid.to_string())
        .arg(&ready.app)
        .arg(app)
        .arg(dir.join("previous/dent.app"))
        .arg(dir.join("started"))
        .arg(dir.join("went-back"))
        .arg(ready.build.to_string())
        .arg(wait.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // Its own group: quitting the app doesn't take it along.
        .process_group(0)
        .spawn()
        .map_err(|e| format!("couldn't start the update: {e}"))?;
    Ok(())
}

/// One press of Update all, under way.
#[derive(Debug)]
pub struct Run {
    pub since: Instant,
    /// Machines asked to update ("" is this Mac's daemon): the build each
    /// was asked to run, and the id of its last update before (a rollback
    /// counts only if it's a new one).
    pub machines: BTreeMap<String, (u64, Option<String>)>,
    /// The app goes once the machines are back.
    pub app: bool,
}

impl Dent {
    /// Machines with a build ready ("" is this Mac's daemon), online now.
    pub fn machines_ready(&self) -> Vec<(String, Available)> {
        let mut out: Vec<(String, Available)> = self
            .machines
            .iter()
            .filter(|m| matches!(m.state, dent_client_core::machine::State::Online))
            .filter_map(|m| Some((m.id.clone(), m.available.clone()?)))
            .collect();
        if self.connection.is_ok()
            && let Some(a) = &self.machine_available
        {
            out.insert(0, (String::new(), a.clone()));
        }
        out
    }

    /// How many things one press of Update all updates: machines, and
    /// this app.
    pub fn updates_ready(&self) -> usize {
        self.machines_ready().len() + usize::from(ready().is_some())
    }

    /// What one press of Update all updates, in words: "Updates this app
    /// and dent-linux".
    pub fn update_words(&self) -> String {
        let mut what: Vec<String> = Vec::new();
        if ready().is_some() {
            what.push("this app".into());
        }
        for (m, _) in self.machines_ready() {
            what.push(if m.is_empty() {
                "this Mac's dent service".into()
            } else {
                self.machine_name(&m)
            });
        }
        match what.as_slice() {
            [] => "Nothing to update".into(),
            [one] => format!("Updates {one}"),
            [rest @ .., last] => format!("Updates {} and {last}", rest.join(", ")),
        }
    }

    /// The newer build ready for `machine` ("" is this Mac's daemon).
    pub fn available_there(
        &mut self,
        machine: &str,
        available: Option<Available>,
        cx: &mut Context<Self>,
    ) {
        if machine.is_empty() {
            self.machine_available = available;
        } else if let Some(m) = self.machines.iter_mut().find(|m| m.id == machine) {
            m.available = available;
        }
        cx.notify();
    }

    /// Update every machine with a build ready, then this app.
    pub fn update_all(&mut self, cx: &mut Context<Self>) {
        if self.updating.is_some() {
            return;
        }
        let machines: BTreeMap<String, (u64, Option<String>)> = self
            .machines_ready()
            .into_iter()
            .map(|(m, a)| {
                self.ask_update(&m, &a);
                let before = self.last_update_id(&m);
                (m, (a.main_build, before))
            })
            .collect();
        let app = ready().is_some();
        if machines.is_empty() && !app {
            return;
        }
        self.updating = Some(Run {
            since: Instant::now(),
            machines,
            app,
        });
        cx.notify();
    }

    /// Update one machine ("" is this Mac: its daemon, then the app).
    pub fn update_one(&mut self, machine: &str, cx: &mut Context<Self>) {
        if self.updating.is_some() {
            return;
        }
        let mut machines = BTreeMap::new();
        if let Some((_, a)) = self
            .machines_ready()
            .into_iter()
            .find(|(m, _)| m == machine)
        {
            self.ask_update(machine, &a);
            let before = self.last_update_id(machine);
            machines.insert(machine.to_string(), (a.main_build, before));
        }
        let app = machine.is_empty() && ready().is_some();
        if machines.is_empty() && !app {
            return;
        }
        self.updating = Some(Run {
            since: Instant::now(),
            machines,
            app,
        });
        cx.notify();
    }

    fn ask_update(&mut self, machine: &str, a: &Available) {
        let n = self.live.send_to(
            machine,
            Request::Update {
                version: a.version.clone(),
                once: format!(
                    "{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_nanos())
                ),
            },
        );
        self.waiting_insert(n, crate::app::Waiting::Update(machine.to_string()));
    }

    /// A machine said no to updating: it stays as it is, and says why.
    pub fn update_refused(&mut self, machine: &str, why: String, cx: &mut Context<Self>) {
        if let Some(run) = &mut self.updating {
            run.machines.remove(machine);
        }
        let name = if machine.is_empty() {
            "This Mac".to_string()
        } else {
            self.machines
                .iter()
                .find(|m| m.id == machine)
                .map_or_else(|| machine.to_string(), |m| m.name().to_string())
        };
        self.toast(ToastAction::Info, format!("{name} didn't update: {why}"));
        cx.notify();
    }

    /// The id of `machine`'s last update, as it last said.
    fn last_update_id(&self, machine: &str) -> Option<String> {
        if machine.is_empty() {
            self.machine_update.as_ref().map(|u| u.id.clone())
        } else {
            self.machines
                .iter()
                .find(|m| m.id == machine)?
                .update
                .as_ref()
                .map(|u| u.id.clone())
        }
    }

    /// Whether `machine` runs the build it was asked to, or has gone back
    /// since (`before` is its last update's id when asked).
    fn machine_done(&self, machine: &str, build: u64, before: Option<&str>) -> bool {
        let (runs, update) = if machine.is_empty() {
            (self.machine_main_build, self.machine_update.as_ref())
        } else {
            match self.machines.iter().find(|m| m.id == machine) {
                Some(m) => (m.main_build, m.update.as_ref()),
                None => return true,
            }
        };
        runs >= build || update.is_some_and(|u| u.rolled_back && Some(u.id.as_str()) != before)
    }

    /// Moves a press of Update all along: once its machines are back (or
    /// it's waited long enough), the app goes.
    pub fn update_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A debug build told to (`DENT_UPDATE_NOW`, for trials) presses
        // Update all itself, once something is ready.
        #[cfg(debug_assertions)]
        {
            static PRESSED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if std::env::var_os("DENT_UPDATE_NOW").is_some()
                && self.updates_ready() > 0
                && !PRESSED.swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                self.update_all(cx);
            }
        }
        let Some(run) = &self.updating else {
            return;
        };
        let waiting = run
            .machines
            .iter()
            .any(|(m, (b, before))| !self.machine_done(m, *b, before.as_deref()));
        if waiting && run.since.elapsed() < MACHINES_WAIT {
            return;
        }
        let app = run.app;
        self.updating = None;
        cx.notify();
        if app {
            self.relaunch(window, cx);
        }
    }

    /// Put back where the app was before it updated: its session once it's
    /// there, then the chat's place once that many rows are.
    pub fn resume_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((r, at)) = &self.resuming else {
            return;
        };
        if at.elapsed() > Duration::from_secs(20) {
            self.resuming = None;
            return;
        }
        let r = r.clone();
        if let Some(s) = &r.session {
            if self.session(s).is_none() {
                return;
            }
            if self.selected.as_deref() != Some(s.as_str()) {
                self.select(Some(s.clone()), window, cx);
            }
        }
        if let Some((ix, off)) = r.scroll {
            if self.transcript.item_count() <= ix {
                return;
            }
            self.transcript.set_follow_mode(FollowMode::Normal);
            self.transcript.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(off),
            });
        }
        self.resuming = None;
        cx.notify();
    }

    /// Quit into the new app, which opens where this one was.
    fn relaunch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ready) = READY.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
            return;
        };
        let Some(app) = bundle() else {
            return;
        };
        let b = window.bounds();
        let top = self.transcript.logical_scroll_top();
        let resume = Resume {
            session: self.selected.clone(),
            window: Some((
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            )),
            scroll: (self.selected.is_some() && !self.transcript.is_following_tail())
                .then(|| (top.item_ix, f32::from(top.offset_in_item))),
        };
        let d = dir();
        let wrote = serde_json::to_vec(&resume)
            .map_err(|e| e.to_string())
            .and_then(|b| std::fs::write(d.join("resume.json"), b).map_err(|e| e.to_string()));
        self.save_prefs(cx);
        match wrote.and_then(|()| swap(&ready, &app, std::process::id(), &d, START_WAIT)) {
            Ok(()) => cx.quit(),
            Err(e) => {
                let _ = std::fs::remove_file(d.join("resume.json"));
                self.toast(ToastAction::Info, format!("dent couldn't update: {e}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dent-swap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A stand-in dent.app saying which it is.
    fn app(at: &Path, which: &str) {
        std::fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
        std::fs::write(at.join("Contents/MacOS/dent-desktop"), which).unwrap();
    }

    fn which(at: &Path) -> String {
        std::fs::read_to_string(at.join("Contents/MacOS/dent-desktop")).unwrap_or_default()
    }

    /// Run the swap with `open` standing in for macOS's, against a process
    /// that has already quit, and wait for it.
    fn run(d: &Path, open: &str, wait: u32) -> bool {
        let installed = d.join("Applications/dent.app");
        let new = d.join("updates/7/dent.app");
        app(&installed, "old");
        app(&new, "new");
        let gone = std::process::Command::new("true").spawn().unwrap();
        let pid = gone.id();
        let _ = { gone }.wait();
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(SWAP)
            .arg("swap")
            .arg(pid.to_string())
            .arg(&new)
            .arg(&installed)
            .arg(d.join("updates/previous/dent.app"))
            .arg(d.join("updates/started"))
            .arg(d.join("updates/went-back"))
            .arg("7")
            .arg(wait.to_string())
            .env("DENT_OPEN", open)
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn a_new_app_that_starts_takes_the_old_ones_place() {
        let d = scratch("ok");
        // "Opening" it starts it: it says so.
        let open = d.join("open.sh");
        std::fs::write(
            &open,
            format!(
                "#!/bin/sh\necho 7 > {}\n",
                d.join("updates/started").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&open, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        assert!(run(&d, open.to_str().unwrap(), 50));
        assert_eq!(which(&d.join("Applications/dent.app")), "new");
        assert_eq!(which(&d.join("updates/previous/dent.app")), "old");
        assert!(!d.join("updates/went-back").exists());
    }

    #[test]
    fn a_new_app_that_doesnt_start_is_put_back_and_never_offered_again() {
        let d = scratch("back");
        assert!(!run(&d, "true", 5));
        assert_eq!(which(&d.join("Applications/dent.app")), "old");
        assert_eq!(
            std::fs::read_to_string(d.join("updates/went-back"))
                .unwrap()
                .trim(),
            "7"
        );
        assert_eq!(skipped(&d.join("updates")), Some(7));
    }

    #[test]
    fn a_feed_makes_its_app_ready_once_and_older_ones_go() {
        let d = scratch("look");
        let src = d.join("src");
        app(&src.join("dent.app"), "new");
        let tgz = d.join("app.tar.gz");
        assert!(
            std::process::Command::new("tar")
                .arg("-czf")
                .arg(&tgz)
                .arg("-C")
                .arg(&src)
                .arg("dent.app")
                .status()
                .unwrap()
                .success()
        );
        let feed = dent_update::Feed {
            build: 9,
            commit: String::new(),
            parts: [(
                dent_update::part_name("app"),
                dent_update::Part {
                    url: format!("file://{}", tgz.display()),
                    sha256: dent_update::sha256(&tgz).unwrap(),
                    size: std::fs::metadata(&tgz).unwrap().len(),
                },
            )]
            .into(),
        };
        let json = d.join("latest.json");
        std::fs::write(&json, serde_json::to_vec(&feed).unwrap()).unwrap();
        let updates = d.join("updates");
        std::fs::create_dir_all(updates.join("3")).unwrap();
        look(&format!("file://{}", json.display()), &updates).unwrap();
        assert_eq!(ready(), Some(9));
        assert_eq!(which(&updates.join("9/dent.app")), "new");
        assert!(!updates.join("3").exists() && !updates.join("9.tar.gz").exists());
        *READY.lock().unwrap() = None;
        // A build that didn't start here isn't made ready again.
        std::fs::write(updates.join("skipped"), "9").unwrap();
        look(&format!("file://{}", json.display()), &updates).unwrap();
        assert_eq!(ready(), None);
    }
}
