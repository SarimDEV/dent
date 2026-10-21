//! Every file a session names, for the window: pictures, files linked in
//! replies, what a viewer opens ([decision 0068]). One place asks, the same
//! way for every session:
//!
//! - **This Mac's sessions** are read from disk with `dent-files`, the same
//!   resolution and safe open the daemon uses.
//! - **Another machine's** are never looked for on this Mac, even when the
//!   same path is here (T3 Code fell back to its own disk and showed the
//!   wrong file; dent never does). They're asked of that machine (`Stat`,
//!   then `Read` in small ranges, a few at a time) and kept in a cache
//!   keyed by (machine, the path it resolved to, the version's stamp), so a
//!   rewritten file never shows its old bytes and nothing is fetched twice.
//!
//! Drawing asks every frame through [`picture`], [`stat`] and [`copy`],
//! which answer at once from what's known and note what's wanted; the
//! window sends what's wanted ([`Dent::pump_files`]) and hands back the
//! replies, then draws again.
//!
//! The cache can only make things faster, never wrong: a copy is shown
//! only for the stamp the machine last gave, a machine that's away shows
//! what's kept marked as such, and a failure is asked again on the next
//! change rather than kept for good.
//!
//! [decision 0068]: ../../../docs/decisions/0068-files-across-machines.md

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dent_protocol::{FileRef, FileStat, READ_MAX, Request, Stamp};

/// A file as a session named it. `cwd` is the session's folder, which a
/// relative path on this Mac is read from (another machine's daemon knows
/// its own).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Ask {
    /// The session, qualified with its machine.
    pub session: String,
    pub path: String,
    /// The folder of the document it was written in, when not the
    /// session's.
    pub base: Option<String>,
    pub cwd: String,
}

impl Ask {
    pub fn new(session: &str, path: &str, cwd: &str) -> Self {
        Ask {
            session: session.to_string(),
            path: path.to_string(),
            base: None,
            cwd: cwd.to_string(),
        }
    }

    pub fn based(mut self, base: Option<String>) -> Self {
        self.base = base;
        self
    }

    fn machine(&self) -> &str {
        dent_client_core::fleet::split(&self.session).0
    }

    fn file_ref(&self) -> FileRef {
        FileRef {
            session: self.session.clone(),
            path: self.path.clone(),
            base: self.base.clone(),
        }
    }
}

/// How a file stands for the window.
#[derive(Clone, Debug, PartialEq)]
pub enum Look<T> {
    /// Being asked of its machine.
    Loading,
    Here(T),
    /// Kept from before; its machine is away, so it may have changed.
    Stale(T),
    Gone(Gone),
}

impl<T> Look<T> {
    /// What can be drawn now, kept or current.
    pub fn shown(self) -> Option<T> {
        match self {
            Look::Here(t) | Look::Stale(t) => Some(t),
            _ => None,
        }
    }
}

/// Why a file can't be shown, saying where it was looked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Gone {
    /// The machine's id ("" for this Mac).
    pub machine: String,
    pub checked: String,
    pub why: Why,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Why {
    Missing,
    Folder,
    Refused,
    Unreadable(String),
    TooBig(u64),
    /// The machine is away and nothing of it is kept here.
    Away,
    /// The machine's dent is too old to share files.
    Old,
}

impl Gone {
    /// In words, naming the machine (as `name` gives it) and the path.
    pub fn said(&self, name: &str) -> String {
        let at = &self.checked;
        match &self.why {
            Why::Missing => format!("Not on {name} · {at}"),
            Why::Folder => format!("A folder on {name} · {at}"),
            Why::Refused => format!("{name} doesn't share this · {at}"),
            Why::Unreadable(e) => format!("Can't be read on {name} ({e}) · {at}"),
            Why::TooBig(n) => format!("Too big to show here ({}) · {at}", size(*n)),
            Why::Away => format!("{name} is away; it shows when it's back · {at}"),
            Why::Old => format!("Update dent on {name} to see its files"),
        }
    }
}

/// A size in a word.
pub fn size(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{} KB", n.div_ceil(1024)),
        1_048_576..1_073_741_824 => format!("{:.1} MB", n as f64 / 1_048_576.),
        _ => format!("{:.1} GB", n as f64 / 1_073_741_824.),
    }
}

/// Pictures and text are fetched whole up to this; bigger files are read
/// in ranges by what plays them.
pub const WHOLE_MAX: u64 = 32 << 20;

/// What `copy` fetches whole, for another app to open or to save.
pub const COPY_MAX: u64 = 2 << 30;

/// Copies kept from other machines, at most.
pub const CACHE_MAX: u64 = 2 << 30;

/// Reads in flight to one machine.
const WINDOW: usize = 4;

/// How long a stat of this Mac's file, or an answer for another's while
/// nothing says it changed, is trusted.
const LOCAL_FRESH: Duration = Duration::from_secs(3);

/// A file asked about, on another machine.
#[derive(Default)]
struct Far {
    /// The machine's last answer about it.
    stat: Option<FileStat>,
    /// It needs asking (new, or something says it changed).
    stale: bool,
    asked: bool,
    /// The copy here of the version the machine last gave.
    copy: Option<PathBuf>,
    /// How much of it is wanted whole, if any.
    want: u64,
    fetch: Option<Fetch>,
    /// A read failed: asked again after this.
    retry_at: Option<Instant>,
}

struct Fetch {
    stamp: Stamp,
    real: String,
    tmp: PathBuf,
    got: u64,
    total: u64,
    inflight: bool,
}

/// A request the window should send for the files, and what to do with
/// the reply.
pub enum Send {
    Stat { asks: Vec<Ask>, req: Request },
    Read { ask: Ask, offset: u64, req: Request },
}

#[derive(Default)]
struct Files {
    far: HashMap<Ask, Far>,
    /// This Mac's files, as last seen.
    near: HashMap<Ask, (FileStat, Instant)>,
    online: HashSet<String>,
    /// Machines too old to share files.
    old: HashSet<String>,
    reading: HashMap<String, usize>,
    /// What the window should send next.
    dirty: bool,
    /// Each machine's files as last answered, by what a session wrote, to
    /// the copy kept: drawn in a launch's first frame, then asked again.
    index: HashMap<String, HashMap<String, PathBuf>>,
    index_dirty: HashSet<String>,
    /// Counts copies that came, for what's built from them to know.
    came: u64,
}

/// An index key: what a session wrote, where.
fn index_key(a: &Ask) -> String {
    format!(
        "{}\u{1}{}\u{1}{}",
        dent_client_core::fleet::split(&a.session).1,
        a.path,
        a.base.as_deref().unwrap_or("")
    )
}

fn index_path(machine: &str) -> PathBuf {
    cache_root().join(machine).join("index.json")
}

thread_local! {
    static FILES: RefCell<Files> = RefCell::new(Files::default());
}

/// The folder copies from other machines are kept in, readable by this
/// user only.
pub fn cache_root() -> PathBuf {
    // Tests and walkthroughs keep theirs apart.
    if let Some(d) = std::env::var_os("DENT_FILES_CACHE") {
        return PathBuf::from(d);
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    home.join("Library/Caches/dent/files")
}

fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
    }
    h
}

/// Where the copy of version `stamp` of `real` from `machine` is kept,
/// under its own name so it reads as itself.
fn place(machine: &str, real: &str, stamp: &Stamp) -> PathBuf {
    let name = Path::new(real)
        .file_name()
        .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
    let version = format!(
        "{:x}-{:x}-{:x}-{:x}",
        stamp.dev, stamp.ino, stamp.size, stamp.mtime_ns
    );
    cache_root()
        .join(machine)
        .join(format!("{:016x}", hash(real)))
        .join(version)
        .join(name)
}

fn private_dirs(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let root = cache_root();
    if root.is_dir() {
        let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

impl Files {
    fn near_stat(&mut self, a: &Ask) -> FileStat {
        let now = Instant::now();
        if let Some((s, at)) = self.near.get(a)
            && now.duration_since(*at) < LOCAL_FRESH
        {
            return s.clone();
        }
        let base = a.base.clone().unwrap_or_else(|| a.cwd.clone());
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let s = dent_files::stat(
            &a.path,
            Path::new(&base),
            &home,
            &dent_files::Policy::default(),
        );
        self.near.insert(a.clone(), (s.clone(), now));
        s
    }

    fn gone(a: &Ask, s: &FileStat) -> Option<Gone> {
        let (checked, why) = match s {
            FileStat::File { .. } => return None,
            FileStat::Dir { path } => (path.clone(), Why::Folder),
            FileStat::Missing { checked } => (checked.clone(), Why::Missing),
            FileStat::Refused { checked } => (checked.clone(), Why::Refused),
            FileStat::Unreadable { checked, error } => {
                (checked.clone(), Why::Unreadable(error.clone()))
            }
        };
        Some(Gone {
            machine: a.machine().to_string(),
            checked,
            why,
        })
    }

    /// The far file's entry, made (asked for) on first sight, with the copy
    /// kept from before, if any, to draw while it's asked again.
    fn far(&mut self, a: &Ask) -> &mut Far {
        if !self.far.contains_key(a) {
            self.dirty = true;
            let m = a.machine().to_string();
            let index = self.index.entry(m.clone()).or_insert_with(|| {
                std::fs::read(index_path(&m))
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default()
            });
            let copy = index.get(&index_key(a)).filter(|p| p.is_file()).cloned();
            self.far.insert(
                a.clone(),
                Far {
                    stale: true,
                    copy,
                    ..Far::default()
                },
            );
        }
        let f = self.far.get_mut(a).expect("just made");
        if let Some(t) = f.retry_at
            && Instant::now() >= t
        {
            f.retry_at = None;
            f.stale = true;
            self.dirty = true;
        }
        f
    }

    fn look_stat(&mut self, a: &Ask) -> Look<FileStat> {
        let m = a.machine().to_string();
        if m.is_empty() {
            return Look::Here(self.near_stat(a));
        }
        let online = self.online.contains(&m);
        let old = self.old.contains(&m);
        let f = self.far(a);
        match (&f.stat, online) {
            _ if old => Look::Gone(Gone {
                machine: m,
                checked: a.path.clone(),
                why: Why::Old,
            }),
            (Some(s), true) if !f.stale => Look::Here(s.clone()),
            (Some(s), _) => Look::Stale(s.clone()),
            (None, true) => Look::Loading,
            (None, false) => Look::Gone(Gone {
                machine: m,
                checked: a.path.clone(),
                why: Why::Away,
            }),
        }
    }

    /// A file to open from this Mac: the file itself for this Mac's
    /// sessions, a whole copy for another's (up to `cap`).
    fn look_copy(&mut self, a: &Ask, cap: u64) -> Look<PathBuf> {
        let m = a.machine().to_string();
        if m.is_empty() {
            let s = self.near_stat(a);
            return match &s {
                FileStat::File { path, stamp, .. } if stamp.size <= cap => {
                    Look::Here(PathBuf::from(path))
                }
                FileStat::File { path, stamp, .. } => Look::Gone(Gone {
                    machine: m,
                    checked: path.clone(),
                    why: Why::TooBig(stamp.size),
                }),
                other => Look::Gone(Files::gone(a, other).expect("not a file")),
            };
        }
        let stat = self.look_stat(a);
        let online = self.online.contains(&m);
        let f = self.far(a);
        if f.want < cap {
            f.want = cap;
            self.dirty = true;
        }
        let f = self.far.get_mut(a).expect("made above");
        match stat {
            Look::Gone(g) => {
                // Away, but a copy is kept: shown, marked.
                match (&g.why, &f.copy) {
                    (Why::Away, Some(c)) => Look::Stale(c.clone()),
                    _ => Look::Gone(g),
                }
            }
            // Being asked: the copy kept from before shows meanwhile.
            Look::Loading => match &f.copy {
                Some(c) => Look::Stale(c.clone()),
                None => Look::Loading,
            },
            Look::Here(s) | Look::Stale(s) => {
                if let Some(g) = Files::gone(a, &s) {
                    return Look::Gone(g);
                }
                if let FileStat::File { path, stamp, .. } = &s
                    && stamp.size > cap
                {
                    return Look::Gone(Gone {
                        machine: m,
                        checked: path.clone(),
                        why: Why::TooBig(stamp.size),
                    });
                }
                match &f.copy {
                    Some(c) if online && !f.stale => Look::Here(c.clone()),
                    Some(c) => Look::Stale(c.clone()),
                    None => Look::Loading,
                }
            }
        }
    }

    /// The requests to send now: stats for what's unknown or changed, and
    /// the next ranges of what's wanted whole, a few at a time per machine.
    fn drain(&mut self) -> Vec<(String, Send)> {
        for m in std::mem::take(&mut self.index_dirty) {
            if let Some(index) = self.index.get(&m)
                && let Ok(json) = serde_json::to_vec(index)
                && private_dirs(&cache_root().join(&m)).is_ok()
            {
                let _ = dent_remote::store::write_private(&index_path(&m), &json);
            }
        }
        if !self.dirty {
            return Vec::new();
        }
        self.dirty = false;
        let mut out = Vec::new();
        let mut stats: HashMap<String, Vec<Ask>> = HashMap::new();
        for (a, f) in self.far.iter_mut() {
            let m = a.machine();
            if !self.online.contains(m) || self.old.contains(m) || f.retry_at.is_some() {
                continue;
            }
            if f.stale && !f.asked {
                f.asked = true;
                stats.entry(m.to_string()).or_default().push(a.clone());
            }
        }
        for (m, asks) in stats {
            for chunk in asks.chunks(256) {
                let req = Request::Stat {
                    files: chunk.iter().map(Ask::file_ref).collect(),
                };
                out.push((
                    m.clone(),
                    Send::Stat {
                        asks: chunk.to_vec(),
                        req,
                    },
                ));
            }
        }
        // Next ranges, in no particular order beyond the window.
        let mut asks: Vec<&Ask> = self.far.keys().collect();
        asks.sort_by(|a, b| a.path.cmp(&b.path));
        let asks: Vec<Ask> = asks.into_iter().cloned().collect();
        for a in asks {
            let m = a.machine().to_string();
            if !self.online.contains(&m) {
                continue;
            }
            let busy = self.reading.get(&m).copied().unwrap_or(0);
            if busy >= WINDOW {
                continue;
            }
            let f = self.far.get_mut(&a).expect("listed");
            if f.stale || f.copy.is_some() || f.retry_at.is_some() {
                continue;
            }
            let Some(FileStat::File { path, stamp, .. }) = &f.stat else {
                continue;
            };
            if stamp.size > f.want || f.want == 0 {
                continue;
            }
            let fetch = f.fetch.get_or_insert_with(|| Fetch {
                stamp: *stamp,
                real: path.clone(),
                tmp: place(&m, path, stamp).with_extension("part"),
                got: 0,
                total: stamp.size,
                inflight: false,
            });
            if fetch.inflight {
                continue;
            }
            fetch.inflight = true;
            let offset = fetch.got;
            let req = Request::Read {
                file: a.file_ref(),
                offset,
                len: READ_MAX,
                expect: Some(fetch.stamp),
            };
            *self.reading.entry(m.clone()).or_default() += 1;
            out.push((
                m,
                Send::Read {
                    ask: a,
                    offset,
                    req,
                },
            ));
        }
        out
    }

    fn stats_came(&mut self, asks: Vec<Ask>, stats: Vec<FileStat>) {
        self.dirty = true;
        for (a, s) in asks.into_iter().zip(stats) {
            let m = a.machine().to_string();
            let f = self.far.entry(a.clone()).or_default();
            f.asked = false;
            f.stale = false;
            if let FileStat::File { path, stamp, .. } = &s {
                let kept = place(&m, path, stamp);
                f.copy = kept.is_file().then_some(kept.clone());
                let key = index_key(&a);
                let index = self.index.entry(m.clone()).or_default();
                if index.get(&key) != Some(&kept) {
                    index.insert(key, kept);
                    self.index_dirty.insert(m.clone());
                }
                // A fetch of another version stops.
                if f.fetch.as_ref().is_some_and(|x| x.stamp != *stamp) {
                    if let Some(x) = f.fetch.take() {
                        let _ = std::fs::remove_file(&x.tmp);
                    }
                }
            } else {
                f.copy = None;
                f.fetch = None;
                if let Some(index) = self.index.get_mut(&m)
                    && index.remove(&index_key(&a)).is_some()
                {
                    self.index_dirty.insert(m.clone());
                }
            }
            f.stat = Some(s);
        }
    }

    fn stats_failed(&mut self, asks: Vec<Ask>, message: &str) {
        self.dirty = true;
        for a in asks {
            let m = a.machine().to_string();
            if message.contains("unknown variant") {
                self.old.insert(m);
            }
            if let Some(f) = self.far.get_mut(&a) {
                f.asked = false;
                f.retry_at = Some(Instant::now() + Duration::from_secs(5));
            }
        }
    }

    fn bytes_came(
        &mut self,
        a: Ask,
        offset: u64,
        stamp: Stamp,
        total: u64,
        bytes: Option<Vec<u8>>,
    ) -> Option<String> {
        self.dirty = true;
        let m = a.machine().to_string();
        if let Some(n) = self.reading.get_mut(&m) {
            *n = n.saturating_sub(1);
        }
        let f = self.far.get_mut(&a)?;
        let x = f.fetch.as_mut()?;
        x.inflight = false;
        let (Some(bytes), true) = (bytes, x.stamp == stamp && x.got == offset) else {
            f.retry_at = Some(Instant::now() + Duration::from_secs(2));
            return None;
        };
        let wrote = (|| -> std::io::Result<()> {
            use std::os::unix::fs::FileExt;
            if let Some(dir) = x.tmp.parent() {
                private_dirs(dir)?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(offset == 0)
                .open(&x.tmp)?;
            file.write_all_at(&bytes, offset)?;
            Ok(())
        })();
        if wrote.is_err() || (bytes.is_empty() && x.got < total) {
            f.fetch = None;
            f.retry_at = Some(Instant::now() + Duration::from_secs(5));
            return None;
        }
        x.got += bytes.len() as u64;
        if x.got >= x.total {
            let done = place(&m, &x.real, &x.stamp);
            if std::fs::rename(&x.tmp, &done).is_ok() {
                f.copy = Some(done);
                self.came += 1;
            }
            f.fetch = None;
            // Whole: the session that named it draws it again.
            return Some(a.session.clone());
        }
        None
    }

    fn read_gone(&mut self, a: Ask, stat: FileStat, changed: bool) {
        self.dirty = true;
        let m = a.machine().to_string();
        if let Some(n) = self.reading.get_mut(&m) {
            *n = n.saturating_sub(1);
        }
        if let Some(f) = self.far.get_mut(&a) {
            if let Some(x) = f.fetch.take() {
                let _ = std::fs::remove_file(&x.tmp);
            }
            if changed {
                // Rewritten mid-read: ask what it is now.
                f.stale = true;
            } else {
                f.copy = None;
                f.stat = Some(stat);
            }
        }
    }
}

/// How a session's file stands.
pub fn stat(a: &Ask) -> Look<FileStat> {
    FILES.with(|f| f.borrow_mut().look_stat(a))
}

/// A picture to draw: this Mac's file itself, or another machine's copy
/// here, fetched whole (up to [`WHOLE_MAX`]).
pub fn picture(a: &Ask) -> Look<PathBuf> {
    FILES.with(|f| f.borrow_mut().look_copy(a, WHOLE_MAX))
}

/// A whole file on this Mac, for another app to open or to save: this
/// Mac's own, or a copy of another machine's (up to `cap`).
pub fn copy(a: &Ask, cap: u64) -> Look<PathBuf> {
    FILES.with(|f| f.borrow_mut().look_copy(a, cap))
}

/// Something says `machine`'s files may have changed: those named by
/// `paths` (as written), or all of them.
pub fn changed(machine: &str, paths: Option<&[String]>) {
    FILES.with(|f| {
        let mut f = f.borrow_mut();
        let mut any = false;
        for (a, e) in f.far.iter_mut() {
            if a.machine() == machine && paths.is_none_or(|p| p.iter().any(|p| *p == a.path)) {
                e.stale = true;
                e.retry_at = None;
                any = true;
            }
        }
        if machine.is_empty() {
            match paths {
                Some(p) => f.near.retain(|a, _| !p.contains(&a.path)),
                None => f.near.clear(),
            }
        }
        f.dirty |= any;
    });
}

/// Whether `machine` can be asked now. Coming online asks again about
/// everything of it.
pub fn set_online(machine: &str, online: bool, proto: Option<u32>) {
    FILES.with(|f| {
        let mut f = f.borrow_mut();
        let was = f.online.contains(machine);
        if online {
            f.online.insert(machine.to_string());
        } else {
            f.online.remove(machine);
            f.reading.remove(machine);
        }
        if let Some(p) = proto {
            if p < 4 {
                f.old.insert(machine.to_string());
            } else {
                f.old.remove(machine);
            }
        }
        if online && !was {
            for (a, e) in f.far.iter_mut() {
                if a.machine() == machine {
                    e.stale = true;
                    e.asked = false;
                    e.retry_at = None;
                    if let Some(x) = e.fetch.as_mut() {
                        x.inflight = false;
                    }
                }
            }
            f.dirty = true;
        }
    });
}

/// The requests to send now.
pub fn drain() -> Vec<(String, Send)> {
    FILES.with(|f| f.borrow_mut().drain())
}

pub fn stats_came(asks: Vec<Ask>, stats: Vec<FileStat>) {
    FILES.with(|f| f.borrow_mut().stats_came(asks, stats));
}

pub fn stats_failed(asks: Vec<Ask>, message: &str) {
    FILES.with(|f| f.borrow_mut().stats_failed(asks, message));
}

/// A range came; the session to draw again when it made a copy whole.
pub fn bytes_came(
    a: Ask,
    offset: u64,
    stamp: Stamp,
    total: u64,
    bytes: Option<Vec<u8>>,
) -> Option<String> {
    FILES.with(|f| f.borrow_mut().bytes_came(a, offset, stamp, total, bytes))
}

/// How many copies have come: what's built from them is built again when
/// it moves.
pub fn came() -> u64 {
    FILES.with(|f| f.borrow().came)
}

pub fn read_gone(a: Ask, stat: FileStat, changed: bool) {
    FILES.with(|f| f.borrow_mut().read_gone(a, stat, changed));
}

/// A read that got no answer it understood (the link dropped, an error):
/// asked again shortly.
pub fn read_failed(a: Ask) {
    FILES.with(|f| {
        let mut f = f.borrow_mut();
        let m = a.machine().to_string();
        if let Some(n) = f.reading.get_mut(&m) {
            *n = n.saturating_sub(1);
        }
        if let Some(e) = f.far.get_mut(&a) {
            if let Some(x) = e.fetch.as_mut() {
                x.inflight = false;
            }
            e.retry_at = Some(Instant::now() + Duration::from_secs(2));
        }
        f.dirty = true;
    });
}

/// Forget `machine`'s files, and delete the copies kept of them.
pub fn forget(machine: &str) {
    FILES.with(|f| {
        let mut f = f.borrow_mut();
        f.far.retain(|a, _| a.machine() != machine);
        f.reading.remove(machine);
    });
    if !machine.is_empty() {
        let _ = std::fs::remove_dir_all(cache_root().join(machine));
    }
}

/// Keep the copies under `cap` bytes, the least recently used going first;
/// a launch does it once, off the window's thread.
pub fn trim(cap: u64) {
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    let mut stack = vec![cache_root()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else if e.file_name() != "index.json" {
                let used = m
                    .accessed()
                    .or_else(|_| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                files.push((used, m.len(), e.path()));
            }
        }
    }
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, p) in files {
        if total <= cap {
            break;
        }
        // A copy half-written by a run that ended goes too.
        if std::fs::remove_file(&p).is_ok() {
            total = total.saturating_sub(len);
            if let Some(dir) = p.parent() {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }
}

/// Whether anything's waiting to be sent: the window pumps again.
pub fn busy() -> bool {
    FILES.with(|f| {
        let f = f.borrow();
        f.dirty || f.far.values().any(|e| e.retry_at.is_some())
    })
}

#[cfg(test)]
mod tests;
