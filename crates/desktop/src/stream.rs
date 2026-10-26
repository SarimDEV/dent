//! Another machine's video, played as the player asks: its bytes come in
//! whole blocks, kept in memory only (nothing of it goes to disk), and a
//! player's many small asks (a container's headers, 8 bytes at a time) are
//! answered from them rather than one trip to the machine each. While it
//! plays, the blocks after the one asked for come ahead, a few at once, so
//! a link's round trip never paces the picture.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use dent_protocol::Stamp;

/// A block: as much as one read brings.
pub const BLOCK: u64 = dent_protocol::READ_MAX as u64;
/// How far past what's asked comes ahead.
pub const AHEAD: u64 = 8;
/// The most one answer to the player carries.
pub const SERVE_MAX: u64 = 4 * BLOCK;
/// All the blocks kept, of every video, at most.
pub const KEEP_MAX: u64 = 192 << 20;

/// One version of one file on one machine.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileKey {
    pub session: String,
    pub path: String,
    pub stamp: Stamp,
}

#[derive(Default)]
pub struct Blocks {
    have: HashMap<(FileKey, u64), Arc<[u8]>>,
    /// Least used first, for letting go.
    order: VecDeque<(FileKey, u64)>,
    asked: HashSet<(FileKey, u64)>,
    kept: u64,
}

impl Blocks {
    /// The blocks to ask for so `start..=end` can be answered, and `ahead`
    /// more after it: those neither here nor asked for already, now counted
    /// as asked.
    pub fn plan(
        &mut self,
        file: &FileKey,
        total: u64,
        start: u64,
        end: u64,
        ahead: u64,
    ) -> Vec<u64> {
        if total == 0 {
            return Vec::new();
        }
        let last = (total - 1) / BLOCK;
        let first = start / BLOCK;
        let need = end.min(start + SERVE_MAX - 1) / BLOCK;
        let upto = (need + ahead).min(last);
        let mut out = Vec::new();
        for i in first..=upto {
            let k = (file.clone(), i);
            if !self.have.contains_key(&k) && self.asked.insert(k) {
                out.push(i);
            }
        }
        out
    }

    /// Block `i` came.
    pub fn put(&mut self, file: &FileKey, i: u64, bytes: Arc<[u8]>) {
        let k = (file.clone(), i);
        self.asked.remove(&k);
        self.kept += bytes.len() as u64;
        if let Some(old) = self.have.insert(k.clone(), bytes) {
            self.kept -= old.len() as u64;
        }
        self.order.retain(|o| o != &k);
        self.order.push_back(k);
        while self.kept > KEEP_MAX {
            let Some(o) = self.order.pop_front() else {
                break;
            };
            if let Some(b) = self.have.remove(&o) {
                self.kept -= b.len() as u64;
            }
        }
    }

    /// Block `i` won't come (its link, or the file changed): asked anew
    /// when next needed.
    pub fn failed(&mut self, file: &FileKey, i: u64) {
        self.asked.remove(&(file.clone(), i));
    }

    /// Whether block `i` is here or on its way.
    pub fn asked_for(&self, file: &FileKey, i: u64) -> bool {
        let k = (file.clone(), i);
        self.have.contains_key(&k) || self.asked.contains(&k)
    }

    /// Everything of a version that's gone (the file was rewritten).
    pub fn forget(&mut self, file: &FileKey) {
        self.have.retain(|(f, _), b| {
            if f == file {
                self.kept -= b.len() as u64;
                false
            } else {
                true
            }
        });
        self.order.retain(|(f, _)| f != file);
        self.asked.retain(|(f, _)| f != file);
    }

    /// What can be answered now of `start..=end`: from `start` on, as far
    /// as the blocks here run unbroken (at most [`SERVE_MAX`]). `None`
    /// while the block holding `start` hasn't come.
    pub fn serve(&mut self, file: &FileKey, total: u64, start: u64, end: u64) -> Option<Vec<u8>> {
        if start >= total {
            return None;
        }
        let end = end.min(total - 1).min(start + SERVE_MAX - 1);
        let mut out = Vec::with_capacity((end - start + 1) as usize);
        let mut at = start;
        while at <= end {
            let i = at / BLOCK;
            let k = (file.clone(), i);
            let Some(b) = self.have.get(&k) else { break };
            let from = (at - i * BLOCK) as usize;
            if from >= b.len() {
                break;
            }
            let upto = ((end - i * BLOCK) as usize + 1).min(b.len());
            out.extend_from_slice(&b[from..upto]);
            at = i * BLOCK + upto as u64;
            // Used: kept longer.
            self.order.retain(|o| o != &k);
            self.order.push_back(k);
        }
        (!out.is_empty()).then_some(out)
    }

    #[cfg(test)]
    fn kept(&self) -> u64 {
        self.kept
    }
}

/// How long what's known of a playing file stands without the window
/// looking again (it re-checks the file's version as it does).
const PLAYING_FOR: Duration = Duration::from_secs(3);

/// A file a player is playing: its version and size, and the folder its
/// path is from.
struct Playing {
    key: FileKey,
    total: u64,
    base: Option<String>,
    at: Instant,
}

/// More of a playing file wanted soon: the window asks for it.
pub struct Hint {
    pub key: FileKey,
    pub total: u64,
    pub start: u64,
    pub base: Option<String>,
    /// How many blocks past it to fetch.
    pub ahead: u64,
}

/// Where a file is on its machine, as a page names it: its session, the
/// folder it's named from there, and the name.
pub type Place = (String, String, String);

/// A file's version, size and the folder its path is from, once known.
pub type Known = (FileKey, u64, Option<String>);

/// A file the local server plays for a page, by its token.
struct Served {
    place: Place,
    cwd: String,
    ty: &'static str,
    /// Its version and size, once the window has looked; `Err` once it
    /// can't be played (gone, or rewritten).
    file: Option<Result<Known, String>>,
}

/// What the player's thread and the window share: the blocks, what's
/// playing, and what the window should fetch next.
#[derive(Default)]
struct Shared {
    blocks: Blocks,
    playing: HashMap<(String, String, String), Playing>,
    hints: Vec<Hint>,
    served: HashMap<String, Served>,
    tokens: HashMap<Place, String>,
    /// Versions rewritten on their machine since.
    gone: HashSet<FileKey>,
    /// The file last shown: one shown before it stops streaming.
    current: Option<String>,
    /// Bytes fetched from its machine, by path (for walks and logs).
    fetched: HashMap<String, u64>,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(Default::default);
/// Rung whenever a block comes or a file's version is known.
static CAME: Condvar = Condvar::new();

fn shared() -> std::sync::MutexGuard<'static, Shared> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner())
}

fn wake_window() {
    if let Some(w) = crate::web::FAR_WAKE.get() {
        let _ = w.unbounded_send(());
    }
}

/// The token the local server plays `place` by (made once per place): a
/// page's video points there, so the player fetches it without the
/// window's thread between it and the bytes.
pub fn token_for(place: Place, cwd: &str, ty: &'static str) -> String {
    let mut s = shared();
    if let Some(t) = s.tokens.get(&place).cloned() {
        // One that can't play any more (rewritten since) is made anew.
        if !matches!(s.served.get(&t).and_then(|v| v.file.as_ref()), Some(Err(_))) {
            s.current = Some(t.clone());
            return t;
        }
        s.served.remove(&t);
    }
    let mut b = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut b);
    }
    let t: String = b.iter().map(|x| format!("{x:02x}")).collect();
    s.tokens.insert(place.clone(), t.clone());
    s.current = Some(t.clone());
    s.served.insert(
        t.clone(),
        Served {
            place,
            cwd: cwd.to_string(),
            ty,
            file: None,
        },
    );
    t
}

/// A block of `path` came over the link (`bytes` of it).
pub fn count_fetched(path: &str, bytes: u64) {
    *shared().fetched.entry(path.to_string()).or_default() += bytes;
}

/// Bytes fetched so far of files whose path ends with `name`.
pub fn fetched_of(name: &str) -> u64 {
    shared()
        .fetched
        .iter()
        .filter(|(k, _)| k.ends_with(name))
        .map(|(_, v)| v)
        .sum()
}

/// Bytes fetched of files whose path ends with `name`, and forget them.
pub fn take_fetched(name: &str) -> u64 {
    let mut s = shared();
    let keys: Vec<String> = s
        .fetched
        .keys()
        .filter(|k| k.ends_with(name))
        .cloned()
        .collect();
    keys.iter().filter_map(|k| s.fetched.remove(k)).sum()
}

/// Whether `token` is the file shown now: one shown before stops
/// streaming, so it never eats the link the new one needs.
pub fn is_current(token: &str) -> bool {
    shared().current.as_deref() == Some(token)
}

/// What the window should look up: places whose version isn't known yet
/// (token, place, the session's folder).
pub fn unknown() -> Vec<(String, Place, String)> {
    shared()
        .served
        .iter()
        .filter(|(_, s)| s.file.is_none())
        .map(|(t, s)| (t.clone(), s.place.clone(), s.cwd.clone()))
        .collect()
}

/// The window looked: `token` is this version now, or can't be played.
pub fn known(token: &str, file: Result<Known, String>) {
    if let Some(s) = shared().served.get_mut(token) {
        s.file = Some(file);
    }
    CAME.notify_all();
}

/// A file the server plays, once its version is known (asking the window
/// to look, and waiting up to `patience`).
pub fn wait_file(
    token: &str,
    patience: Duration,
) -> Result<(FileKey, u64, Option<String>, &'static str), String> {
    let until = Instant::now() + patience;
    let mut s = shared();
    let mut asked = false;
    loop {
        let Some(served) = s.served.get(token) else {
            return Err("not a file this app plays".into());
        };
        match &served.file {
            Some(Ok((k, t, b))) => return Ok((k.clone(), *t, b.clone(), served.ty)),
            Some(Err(why)) => return Err(why.clone()),
            None => {}
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("its machine didn't say".into());
        }
        if !asked {
            asked = true;
            drop(s);
            wake_window();
            s = shared();
            continue;
        }
        s = CAME
            .wait_timeout(s, left.min(Duration::from_secs(1)))
            .map_or_else(|e| e.into_inner().0, |r| r.0);
    }
}

/// Up to [`SERVE_MAX`] of `start..=end` of `file`, waiting (up to
/// `patience`) for its blocks, which the window is asked for.
pub fn wait_bytes(
    file: &FileKey,
    total: u64,
    start: u64,
    end: u64,
    base: &Option<String>,
    ahead: u64,
    patience: Duration,
) -> Result<Vec<u8>, String> {
    let until = Instant::now() + patience;
    let mut s = shared();
    let mut hinted = false;
    loop {
        if let Some(b) = s.blocks.serve(file, total, start, end) {
            // Running low ahead: more asked for, without waiting on it.
            let next = start + b.len() as u64;
            if next < total
                && s.blocks
                    .serve(file, total, next + AHEAD / 2 * BLOCK, total - 1)
                    .is_none()
            {
                s.hints.push(Hint {
                    key: file.clone(),
                    total,
                    start: next,
                    base: base.clone(),
                    ahead,
                });
                drop(s);
                wake_window();
            }
            return Ok(b);
        }
        if s.gone.contains(file) {
            return Err("it changed on its machine".into());
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("its machine didn't send it in time".into());
        }
        // Not here, nor asked for: the window is asked.
        if !hinted || !s.blocks.asked_for(file, start / BLOCK) {
            hinted = true;
            s.hints.push(Hint {
                key: file.clone(),
                total,
                start,
                base: base.clone(),
                ahead,
            });
            drop(s);
            wake_window();
            s = shared();
            continue;
        }
        s = CAME
            .wait_timeout(s, left.min(Duration::from_secs(1)))
            .map_or_else(|e| e.into_inner().0, |r| r.0);
    }
}

/// The window knows `rel` (in `session`, from `dir`) is `key` now.
pub fn note_playing(
    (session, dir, rel): (&str, &str, &str),
    key: &FileKey,
    total: u64,
    base: Option<String>,
) {
    shared().playing.insert(
        (session.into(), dir.into(), rel.into()),
        Playing {
            key: key.clone(),
            total,
            base,
            at: Instant::now(),
        },
    );
}

/// Answer a player's ask right where it's made, from the blocks here: no
/// trip through the window. `None` when it isn't all known yet (the window
/// takes it then). What follows is hinted for the window to fetch.
pub fn answer_here(
    (session, dir, rel): (&str, &str, &str),
    range: &str,
) -> Option<(u64, u64, Vec<u8>)> {
    let mut s = shared();
    let p = s
        .playing
        .get(&(session.into(), dir.into(), rel.into()))
        .filter(|p| p.at.elapsed() < PLAYING_FOR)?;
    let (key, total, base) = (p.key.clone(), p.total, p.base.clone());
    let (start, end) = crate::web::byte_range(range, total)?;
    let bytes = s.blocks.serve(&key, total, start, end)?;
    let next = start + bytes.len() as u64;
    // Running low ahead of where it plays: more asked for.
    if next < total
        && s.blocks
            .serve(&key, total, next + AHEAD / 2 * BLOCK, total - 1)
            .is_none()
    {
        s.hints.push(Hint {
            key,
            total,
            start: next,
            base,
            ahead: AHEAD,
        });
    }
    Some((start, total, bytes))
}

/// Whether the player's thread wants the window to fetch something.
pub fn hinted() -> bool {
    !shared().hints.is_empty()
}

/// What the player's thread asked the window to fetch.
pub fn take_hints() -> Vec<Hint> {
    std::mem::take(&mut shared().hints)
}

/// The blocks, for the window; whoever waits on them hears after.
pub fn with_blocks<T>(f: impl FnOnce(&mut Blocks) -> T) -> T {
    let out = f(&mut shared().blocks);
    CAME.notify_all();
    out
}

/// `file` was rewritten on its machine: its blocks go, and what plays it
/// stops rather than mix two versions.
pub fn gone(file: &FileKey) {
    let mut s = shared();
    s.blocks.forget(file);
    s.gone.insert(file.clone());
    // What plays it stops, never fed the new file's bytes after the
    // old one's start; opened again, it's a new token, the new version.
    for v in s.served.values_mut() {
        if matches!(&v.file, Some(Ok((k, ..))) if k == file) {
            v.file = Some(Err("it changed on its machine".into()));
        }
    }
    drop(s);
    CAME.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(p: &str) -> FileKey {
        FileKey {
            session: "s".into(),
            path: p.into(),
            stamp: Stamp {
                dev: 1,
                ino: 2,
                size: 10 * BLOCK,
                mtime_ns: 3,
            },
        }
    }

    fn block(i: u64) -> Arc<[u8]> {
        (0..BLOCK).map(|b| ((i * BLOCK + b) % 251) as u8).collect()
    }

    #[test]
    fn many_small_asks_are_one_trip() {
        let (mut b, f, total) = (Blocks::default(), key("v.mp4"), 10 * BLOCK);
        // A container's headers: 8 bytes here and there in the first block.
        let first = b.plan(&f, total, 0, 7, AHEAD);
        assert_eq!(first[0], 0);
        assert!(
            b.plan(&f, total, 32, 39, AHEAD).is_empty(),
            "asked once, not again"
        );
        assert_eq!(b.serve(&f, total, 32, 39), None, "not here yet");
        b.put(&f, 0, block(0));
        for (s, e) in [(0, 7), (32, 39), (148, 155), (29_591, 29_598)] {
            let got = b.serve(&f, total, s, e).unwrap();
            let want: Vec<u8> = (s..=e).map(|x| (x % 251) as u8).collect();
            assert_eq!(got, want, "{s}-{e}");
        }
    }

    #[test]
    fn playing_on_comes_ahead_a_few_at_once() {
        let (mut b, f, total) = (Blocks::default(), key("v.mp4"), 100 * BLOCK);
        let asked = b.plan(&f, total, 0, total - 1, AHEAD);
        // What's asked (up to an answer's worth) and the blocks after it.
        assert_eq!(asked, (0..4 + AHEAD).collect::<Vec<_>>());
        for i in 0..4 {
            b.put(&f, i, block(i));
        }
        // An answer runs as far as the blocks here go unbroken.
        let got = b.serve(&f, total, 10, total - 1).unwrap();
        assert_eq!(got.len() as u64, SERVE_MAX - 10);
        // Asked further on, only what isn't on its way already.
        let next = b.plan(&f, total, 4 * BLOCK, total - 1, AHEAD);
        assert_eq!(next, (4 + AHEAD..8 + AHEAD).collect::<Vec<_>>());
    }

    #[test]
    fn the_end_of_a_file_is_never_passed() {
        let (mut b, f) = (Blocks::default(), key("v.mp4"));
        let total = 2 * BLOCK + 100;
        assert_eq!(b.plan(&f, total, 2 * BLOCK, total - 1, AHEAD), vec![2]);
        let last: Arc<[u8]> = vec![7u8; 100].into();
        b.put(&f, 2, last);
        assert_eq!(
            b.serve(&f, total, 2 * BLOCK + 90, total + 50)
                .unwrap()
                .len(),
            10
        );
        assert_eq!(b.serve(&f, total, total, total + 5), None);
    }

    #[test]
    fn a_rewritten_file_is_never_served_its_old_bytes() {
        let (mut b, f) = (Blocks::default(), key("v.mp4"));
        b.plan(&f, 10 * BLOCK, 0, 0, AHEAD);
        b.put(&f, 0, block(0));
        b.forget(&f);
        assert_eq!(b.serve(&f, 10 * BLOCK, 0, 7), None);
        assert_eq!(b.kept(), 0);
        // Asked again, for the new version.
        assert_eq!(b.plan(&f, 10 * BLOCK, 0, 0, AHEAD)[0], 0);
    }

    #[test]
    fn what_failed_is_asked_again_and_memory_stays_bounded() {
        let (mut b, f) = (Blocks::default(), key("v.mp4"));
        let total = 10_000 * BLOCK;
        b.plan(&f, total, 0, 0, AHEAD);
        b.failed(&f, 0);
        assert_eq!(b.plan(&f, total, 0, 0, AHEAD)[0], 0, "asked again");
        for i in 0..2_000 {
            b.put(&f, i, block(i));
        }
        assert!(b.kept() <= KEEP_MAX, "{}", b.kept());
        // The oldest went first; the newest is here.
        assert_eq!(b.serve(&f, total, 0, 7), None);
        assert!(
            b.serve(&f, total, 1_999 * BLOCK, 1_999 * BLOCK + 7)
                .is_some()
        );
    }

    #[test]
    fn a_known_file_is_answered_where_its_asked_and_runs_ahead() {
        let f = FileKey {
            session: "m/s9".into(),
            ..key("here.mp4")
        };
        let total = 40 * BLOCK;
        let at = ("m/s9", "/d", "here.mp4");
        assert!(answer_here(at, "bytes=0-7").is_none(), "nothing known yet");
        note_playing(at, &f, total, None);
        assert!(
            answer_here(at, "bytes=0-7").is_none(),
            "known, but no blocks yet"
        );
        with_blocks(|b| {
            b.plan(&f, total, 0, 7, AHEAD);
            b.put(&f, 0, block(0));
        });
        let (start, all, bytes) = answer_here(at, "bytes=32-39").unwrap();
        assert_eq!((start, all, bytes.len()), (32, total, 8));
        // Little here past it: the window is asked for more.
        assert!(take_hints().iter().any(|h| h.key == f && h.start == 40));
    }
}
