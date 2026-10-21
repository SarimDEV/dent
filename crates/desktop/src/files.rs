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

/// How long a stat of this Mac's file is trusted.
const LOCAL_FRESH: Duration = Duration::from_secs(3);

/// A question to a machine with no answer in this long is asked again: the
/// link it went on may be gone for good (a machine removed, replaced).
const PATIENCE: Duration = Duration::from_secs(20);

/// A file asked about, on another machine.
struct Far {
    /// The machine's last answer about it.
    stat: Option<FileStat>,
    /// It needs asking (new, or something says it changed).
    stale: bool,
    /// Asked, and when.
    asked_at: Option<Instant>,
    /// The copy here of the version the machine last gave (or, before it
    /// answers, the one kept from before).
    copy: Option<PathBuf>,
    /// How much of it is wanted whole, if any.
    want: u64,
    /// Something failed: asked again after this.
    retry_at: Option<Instant>,
    /// When it was last drawn: what's on screen is fetched first.
    seen_at: Instant,
}

impl Far {
    fn new(copy: Option<PathBuf>) -> Self {
        Far {
            stat: None,
            stale: true,
            asked_at: None,
            copy,
            want: 0,
            retry_at: None,
            seen_at: Instant::now(),
        }
    }

    /// Where the copy of the version last answered is kept.
    fn dest(&self, machine: &str) -> Option<PathBuf> {
        match &self.stat {
            Some(FileStat::File { path, stamp, .. }) => Some(place(machine, path, stamp)),
            _ => None,
        }
    }
}

/// A copy being made: one per version of a file, whoever asked for it.
struct Fetch {
    machine: String,
    /// Who asked first: its read names the file.
    ask: Ask,
    stamp: Stamp,
    /// A name of its own, so no two copies ever write one file.
    tmp: PathBuf,
    got: u64,
    total: u64,
    /// A read is out for it, since then.
    sent_at: Option<Instant>,
    /// Asked again no sooner than this, after a failed read.
    wait_until: Option<Instant>,
    seen_at: Instant,
}

/// A request the window should send for the files, and what to do with
/// the reply.
pub enum Send {
    Stat {
        asks: Vec<Ask>,
        req: Request,
    },
    /// A range of the copy kept at `key`.
    Read {
        key: PathBuf,
        offset: u64,
        req: Request,
    },
}

#[derive(Default)]
struct Files {
    far: HashMap<Ask, Far>,
    /// This Mac's files, as last seen.
    near: HashMap<Ask, (FileStat, Instant)>,
    online: HashSet<String>,
    /// Machines too old to share files.
    old: HashSet<String>,
    /// Copies being made, by where they'll be kept.
    fetches: HashMap<PathBuf, Fetch>,
    /// What the window should send next.
    dirty: bool,
    /// Each machine's files as last answered, by what a session wrote, to
    /// the copy kept: drawn in a launch's first frame, then asked again.
    index: HashMap<String, HashMap<String, PathBuf>>,
    index_dirty: HashSet<String>,
    /// Counts copies that came, for what's built from them to know.
    came: u64,
    /// Numbers the copies' temporary names.
    made: u64,
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

/// A file that changed while it was being opened there: asked again, not
/// taken as unreadable.
fn passing(s: &FileStat) -> bool {
    matches!(s, FileStat::Unreadable { error, .. } if error.contains("changed while being opened"))
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
        // No picture size here: drawing asks every few seconds, and this
        // Mac's pictures are measured as they're drawn.
        let s = dent_files::stat_plain(
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
            self.far.insert(a.clone(), Far::new(copy));
        }
        let f = self.far.get_mut(a).expect("just made");
        f.seen_at = Instant::now();
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
                // This Mac's file is itself: nothing to copy, no cap.
                FileStat::File { path, .. } => Look::Here(PathBuf::from(path)),
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
            Look::Gone(g) => match (&g.why, &f.copy) {
                // Away, but a copy is kept: shown, marked.
                (Why::Away, Some(c)) => Look::Stale(c.clone()),
                _ => Look::Gone(g),
            },
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
    /// the next ranges of the copies wanted, what was drawn last first, a
    /// few at a time per machine. Questions with no answer in a long while
    /// are asked again.
    fn drain(&mut self) -> Vec<(String, Send)> {
        for m in std::mem::take(&mut self.index_dirty) {
            if let Some(index) = self.index.get(&m)
                && let Ok(json) = serde_json::to_vec(index)
                && private_dirs(&cache_root().join(&m)).is_ok()
            {
                let _ = dent_remote::store::write_private(&index_path(&m), &json);
            }
        }
        let now = Instant::now();
        for f in self.far.values_mut() {
            if f.retry_at.is_some_and(|t| now >= t) {
                f.retry_at = None;
                f.stale = true;
                self.dirty = true;
            }
            if f.asked_at
                .is_some_and(|t| now.duration_since(t) >= PATIENCE)
            {
                f.asked_at = None;
                f.stale = true;
                self.dirty = true;
            }
        }
        for x in self.fetches.values_mut() {
            if x.sent_at.is_some_and(|t| now.duration_since(t) >= PATIENCE) {
                x.sent_at = None;
                self.dirty = true;
            }
            if x.wait_until.is_some_and(|t| now >= t) {
                x.wait_until = None;
                self.dirty = true;
            }
        }
        if !self.dirty {
            return Vec::new();
        }
        self.dirty = false;
        let mut out = Vec::new();
        // Stats, batched per machine.
        let mut stats: HashMap<String, Vec<Ask>> = HashMap::new();
        for (a, f) in self.far.iter_mut() {
            let m = a.machine();
            if !self.online.contains(m) || self.old.contains(m) || f.retry_at.is_some() {
                continue;
            }
            if f.stale && f.asked_at.is_none() {
                f.asked_at = Some(now);
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
        // The copies wanted now, one per version whoever wants it.
        let mut wanted: HashMap<PathBuf, (Ask, Stamp, Instant)> = HashMap::new();
        for (a, f) in &self.far {
            let m = a.machine();
            if f.copy.is_some() && !f.stale {
                continue;
            }
            let Some(FileStat::File { path, stamp, .. }) = &f.stat else {
                continue;
            };
            if f.want == 0 || stamp.size > f.want || f.retry_at.is_some() {
                continue;
            }
            let dest = place(m, path, stamp);
            if f.copy.as_ref() == Some(&dest) {
                continue;
            }
            let e = wanted.entry(dest).or_insert((a.clone(), *stamp, f.seen_at));
            e.2 = e.2.max(f.seen_at);
        }
        // A copy no one wants any more (its file changed) stops.
        self.fetches.retain(|k, x| {
            let keep = wanted.contains_key(k);
            if !keep {
                let _ = std::fs::remove_file(&x.tmp);
            }
            keep
        });
        for (dest, (ask, stamp, seen)) in wanted {
            match self.fetches.get_mut(&dest) {
                Some(x) => x.seen_at = x.seen_at.max(seen),
                None => {
                    self.made += 1;
                    let tmp =
                        dest.with_extension(format!("part{}-{}", std::process::id(), self.made));
                    self.fetches.insert(
                        dest,
                        Fetch {
                            machine: ask.machine().to_string(),
                            ask,
                            stamp,
                            tmp,
                            got: 0,
                            total: stamp.size,
                            sent_at: None,
                            wait_until: None,
                            seen_at: seen,
                        },
                    );
                }
            }
        }
        let mut busy: HashMap<String, usize> = HashMap::new();
        for x in self.fetches.values() {
            if x.sent_at.is_some() {
                *busy.entry(x.machine.clone()).or_default() += 1;
            }
        }
        let mut next: Vec<(&PathBuf, &Fetch)> = self
            .fetches
            .iter()
            .filter(|(_, x)| {
                x.sent_at.is_none() && x.wait_until.is_none() && self.online.contains(&x.machine)
            })
            .collect();
        next.sort_by_key(|x| std::cmp::Reverse(x.1.seen_at));
        let next: Vec<PathBuf> = next.into_iter().map(|(k, _)| k.clone()).collect();
        for key in next {
            let x = self.fetches.get_mut(&key).expect("listed");
            let n = busy.entry(x.machine.clone()).or_default();
            if *n >= WINDOW {
                continue;
            }
            *n += 1;
            x.sent_at = Some(now);
            let req = Request::Read {
                file: x.ask.file_ref(),
                offset: x.got,
                len: READ_MAX,
                expect: Some(x.stamp),
            };
            out.push((
                x.machine.clone(),
                Send::Read {
                    key,
                    offset: x.got,
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
            let Some(f) = self.far.get_mut(&a) else {
                continue;
            };
            f.asked_at = None;
            if passing(&s) {
                // Caught mid-rewrite there: asked again in a moment.
                f.retry_at = Some(Instant::now() + Duration::from_secs(1));
                continue;
            }
            f.stale = false;
            f.stat = Some(s);
            match f.dest(&m) {
                Some(dest) => {
                    f.copy = dest.is_file().then(|| dest.clone());
                    let key = index_key(&a);
                    let index = self.index.entry(m.clone()).or_default();
                    if f.copy.is_some() && index.get(&key) != Some(&dest) {
                        index.insert(key, dest);
                        self.index_dirty.insert(m.clone());
                    }
                }
                None => {
                    f.copy = None;
                    if let Some(index) = self.index.get_mut(&m)
                        && index.remove(&index_key(&a)).is_some()
                    {
                        self.index_dirty.insert(m.clone());
                    }
                }
            }
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
                f.asked_at = None;
                f.retry_at = Some(Instant::now() + Duration::from_secs(5));
            }
        }
    }

    /// The asks whose copy is kept at `key`.
    fn wanting(&self, key: &Path) -> Vec<Ask> {
        self.far
            .iter()
            .filter(|(a, f)| f.dest(a.machine()).as_deref() == Some(key))
            .map(|(a, _)| a.clone())
            .collect()
    }

    fn bytes_came(
        &mut self,
        key: PathBuf,
        offset: u64,
        stamp: Stamp,
        total: u64,
        bytes: Option<Vec<u8>>,
    ) -> Vec<String> {
        self.dirty = true;
        let Some(x) = self.fetches.get_mut(&key) else {
            return Vec::new();
        };
        // A reply sent again after a reconnect, or one for a read since
        // given up on: already had, or asked again.
        if offset != x.got {
            return Vec::new();
        }
        x.sent_at = None;
        let Some(bytes) = bytes.filter(|_| x.stamp == stamp) else {
            // Another version there: what wants it asks what it is now.
            if let Some(x) = self.fetches.remove(&key) {
                let _ = std::fs::remove_file(&x.tmp);
            }
            for a in self.wanting(&key) {
                if let Some(f) = self.far.get_mut(&a) {
                    f.stale = true;
                }
            }
            return Vec::new();
        };
        let wrote = (|| -> std::io::Result<()> {
            use std::os::unix::fs::FileExt;
            if let Some(dir) = x.tmp.parent() {
                private_dirs(dir)?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&x.tmp)?;
            file.write_all_at(&bytes, offset)?;
            Ok(())
        })();
        if wrote.is_err() || (bytes.is_empty() && x.got < total) {
            // The disk said no, or the file got shorter: again in a while.
            let _ = std::fs::remove_file(&x.tmp);
            x.got = 0;
            x.wait_until = Some(Instant::now() + Duration::from_secs(5));
            return Vec::new();
        }
        x.got += bytes.len() as u64;
        if x.got < x.total {
            return Vec::new();
        }
        let Some(x) = self.fetches.remove(&key) else {
            return Vec::new();
        };
        if std::fs::rename(&x.tmp, &key).is_err() {
            let _ = std::fs::remove_file(&x.tmp);
            return Vec::new();
        }
        self.came += 1;
        // Whole: everything that wants it draws it.
        let mut sessions = Vec::new();
        for a in self.wanting(&key) {
            let m = a.machine().to_string();
            if let Some(f) = self.far.get_mut(&a) {
                f.copy = Some(key.clone());
            }
            let index = self.index.entry(m.clone()).or_default();
            index.insert(index_key(&a), key.clone());
            self.index_dirty.insert(m);
            if !sessions.contains(&a.session) {
                sessions.push(a.session.clone());
            }
        }
        sessions
    }

    fn read_gone(&mut self, key: PathBuf, stat: FileStat, changed: bool) {
        self.dirty = true;
        if let Some(x) = self.fetches.remove(&key) {
            let _ = std::fs::remove_file(&x.tmp);
        }
        for a in self.wanting(&key) {
            if let Some(f) = self.far.get_mut(&a) {
                if changed || passing(&stat) {
                    // Rewritten mid-read: ask what it is now.
                    f.stale = true;
                } else {
                    f.copy = None;
                    f.stat = Some(stat.clone());
                }
            }
        }
    }

    fn read_failed(&mut self, key: PathBuf) {
        self.dirty = true;
        if let Some(x) = self.fetches.get_mut(&key) {
            x.sent_at = None;
            x.wait_until = Some(Instant::now() + Duration::from_secs(2));
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

/// Whether `machine` can be asked now.
pub fn online(machine: &str) -> bool {
    machine.is_empty() || FILES.with(|f| f.borrow().online.contains(machine))
}

/// Something says `machine`'s files may have changed: those named by
/// `paths` (as written, or as they resolved there), or all of them.
pub fn changed(machine: &str, paths: Option<&[String]>) {
    FILES.with(|f| {
        let mut f = f.borrow_mut();
        let mut any = false;
        for (a, e) in f.far.iter_mut() {
            if a.machine() != machine {
                continue;
            }
            let named = paths.is_none_or(|p| {
                p.iter().any(|p| {
                    *p == a.path
                        || matches!(&e.stat, Some(FileStat::File { path, .. }) if path == p)
                })
            });
            if named && e.asked_at.is_none() {
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

/// Something happened in session `id`: its files are asked about again
/// (another machine's), or read again (this Mac's).
pub fn session_changed(id: &str) {
    FILES.with(|f| {
        let mut f = f.borrow_mut();
        let mut any = false;
        for (a, e) in f.far.iter_mut() {
            if a.session == id && e.asked_at.is_none() {
                e.stale = true;
                e.retry_at = None;
                any = true;
            }
        }
        f.near.retain(|a, _| a.session != id);
        f.dirty |= any;
    });
}

/// Whether `machine` can be asked now. Coming online asks again about
/// everything of it; what was in flight is sent again by its link, or
/// asked again if no answer comes.
pub fn set_online(machine: &str, online: bool, proto: Option<u32>) {
    FILES.with(|f| {
        let mut f = f.borrow_mut();
        let was = f.online.contains(machine);
        if online {
            f.online.insert(machine.to_string());
        } else {
            f.online.remove(machine);
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
                if a.machine() == machine && e.asked_at.is_none() {
                    e.stale = true;
                    e.retry_at = None;
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

/// A range came; the sessions to draw again when it made a copy whole.
pub fn bytes_came(
    key: PathBuf,
    offset: u64,
    stamp: Stamp,
    total: u64,
    bytes: Option<Vec<u8>>,
) -> Vec<String> {
    FILES.with(|f| f.borrow_mut().bytes_came(key, offset, stamp, total, bytes))
}

/// How many copies have come: what's built from them is built again when
/// it moves.
pub fn came() -> u64 {
    FILES.with(|f| f.borrow().came)
}

pub fn read_gone(key: PathBuf, stat: FileStat, changed: bool) {
    FILES.with(|f| f.borrow_mut().read_gone(key, stat, changed));
}

/// A read that got no answer it understood (an error): asked again
/// shortly.
pub fn read_failed(key: PathBuf) {
    FILES.with(|f| f.borrow_mut().read_failed(key));
}

/// Forget `machine`'s files, and delete the copies kept of them.
pub fn forget(machine: &str) {
    FILES.with(|f| {
        let mut f = f.borrow_mut();
        f.far.retain(|a, _| a.machine() != machine);
        f.fetches.retain(|_, x| x.machine != machine);
        f.index.remove(machine);
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
    FILES.with(|f| f.borrow().dirty)
}

/// Whether something's waiting on a clock (a retry, an unanswered
/// question): the window looks again in a while, without drawing every
/// frame meanwhile.
pub fn waiting() -> bool {
    FILES.with(|f| {
        let f = f.borrow();
        f.far
            .values()
            .any(|e| e.retry_at.is_some() || e.asked_at.is_some())
            || f.fetches
                .values()
                .any(|x| x.sent_at.is_some() || x.wait_until.is_some())
    })
}

#[cfg(test)]
mod tests;
