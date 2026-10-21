//! Files a session names, resolved and read the same way on the daemon and
//! in a client ([decision 0068]). One function says where a written path
//! points (`~` is the machine's own home, a relative path is the session's
//! or a document's folder); one opens it safely and answers how it stands,
//! with a stamp per version; one reads a range of one version only.
//!
//! The daemon uses it for every device; a client uses the same code for
//! this machine's own sessions, so the two never disagree.
//!
//! [decision 0068]: ../../docs/decisions/0068-files-across-machines.md

use std::fs::File;
use std::io::Read;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use dent_protocol::{FileStat, READ_MAX, Stamp};

/// Which paths a device may not read, whatever it asks: the daemon's own
/// state (its key, trust list, secrets, database), except the files it
/// wrote for sessions to show.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub deny: Vec<PathBuf>,
    /// Inside a denied folder, but shared all the same.
    pub allow: Vec<PathBuf>,
}

impl Policy {
    /// Whether `checked` (a full, canonical path) is off limits.
    pub fn refuses(&self, checked: &Path) -> bool {
        let under = |roots: &[PathBuf]| {
            roots.iter().any(|r| {
                // Compared canonical to canonical: a root through a link
                // (`/tmp` on macOS) still matches.
                let r = r.canonicalize().unwrap_or_else(|_| r.clone());
                checked.starts_with(r)
            })
        };
        under(&self.deny) && !under(&self.allow)
    }
}

/// Where a written path points, before anything is checked on disk.
/// `file://` addresses are decoded; `~` is `home`; a relative path is taken
/// from `base`.
pub fn resolve(written: &str, base: &Path, home: &Path) -> PathBuf {
    let w = written.trim();
    let w = match w.strip_prefix("file://") {
        // `file:///x` and `file://localhost/x` both name `/x`.
        Some(rest) => decode(rest.strip_prefix("localhost").unwrap_or(rest)),
        None => w.to_string(),
    };
    if w == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = w.strip_prefix("~/") {
        return home.join(rest);
    }
    let p = Path::new(&w);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// `%20` and the like, as a `file://` address writes them.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (hex(b.get(i + 1)), hex(b.get(i + 2)))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: Option<&u8>) -> Option<u8> {
    match c? {
        c @ b'0'..=b'9' => Some(c - b'0'),
        c @ b'a'..=b'f' => Some(c - b'a' + 10),
        c @ b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The candidates a written path could mean, best first: as written, then
/// without a `#fragment` or `?query` a link may carry (a file named with
/// a `#` in it wins when it's there).
fn candidates(written: &str, base: &Path, home: &Path) -> Vec<PathBuf> {
    let mut v = vec![resolve(written, base, home)];
    if let Some(cut) = written.find(['#', '?']).filter(|&i| i > 0) {
        let short = resolve(&written[..cut], base, home);
        if !v.contains(&short) {
            v.push(short);
        }
    }
    v
}

/// How the file `written` names stands.
pub fn stat(written: &str, base: &Path, home: &Path, policy: &Policy) -> FileStat {
    stat_with(written, base, home, policy, true)
}

/// Like [`stat`], without reading a picture's header for its size: for
/// asking often.
pub fn stat_plain(written: &str, base: &Path, home: &Path, policy: &Policy) -> FileStat {
    stat_with(written, base, home, policy, false)
}

fn stat_with(written: &str, base: &Path, home: &Path, policy: &Policy, size: bool) -> FileStat {
    let all = candidates(written, base, home);
    for p in &all {
        match open(p, policy) {
            Ok(o) if size => return o.stat(),
            Ok(o) => {
                return FileStat::File {
                    path: o.path.display().to_string(),
                    stamp: o.stamp,
                    width: None,
                    height: None,
                };
            }
            // Not as written: the next way of reading it.
            Err(FileStat::Missing { .. }) => continue,
            Err(s) => return s,
        }
    }
    // Missing every way: said as written.
    FileStat::Missing {
        checked: all[0].display().to_string(),
    }
}

/// An open file, checked: a regular file the policy allows, the same one
/// its path names now.
pub struct Opened {
    file: File,
    path: PathBuf,
    stamp: Stamp,
}

impl Opened {
    pub fn stamp(&self) -> Stamp {
        self.stamp
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Its answer to a `Stat`, with a picture's size from its header.
    pub fn stat(&self) -> FileStat {
        let mut head = vec![0u8; 64 * 1024];
        let n = self.file.read_at(&mut head, 0).unwrap_or(0);
        head.truncate(n);
        let (width, height) = picture_size(&head).map_or((None, None), |(w, h)| (Some(w), Some(h)));
        FileStat::File {
            path: self.path.display().to_string(),
            stamp: self.stamp,
            width,
            height,
        }
    }

    /// Up to `len` bytes (no more than [`READ_MAX`]) from `offset`.
    pub fn read(&self, offset: u64, len: u32) -> std::io::Result<Vec<u8>> {
        let want = len.min(READ_MAX) as u64;
        let end = (offset.saturating_add(want)).min(self.stamp.size);
        let mut buf = vec![0u8; end.saturating_sub(offset) as usize];
        let mut got = 0;
        while got < buf.len() {
            match self.file.read_at(&mut buf[got..], offset + got as u64)? {
                0 => break,
                n => got += n,
            }
        }
        buf.truncate(got);
        Ok(buf)
    }

    /// The whole file, if it's no bigger than `cap`.
    pub fn read_all(&self, cap: u64) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        (&self.file).take(cap + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > cap {
            return Err(std::io::Error::other("too big"));
        }
        Ok(bytes)
    }
}

fn stamp_of(m: &std::fs::Metadata) -> Stamp {
    Stamp {
        dev: m.dev(),
        ino: m.ino(),
        size: m.size(),
        mtime_ns: m
            .mtime()
            .saturating_mul(1_000_000_000)
            .saturating_add(m.mtime_nsec()),
    }
}

/// Open the file at `path` for reading, or say why not. Links on the way
/// are followed to the real path, which is what the policy is asked about;
/// the file opened must be a regular file, and the one that path names
/// (nothing swapped in between).
pub fn open(path: &Path, policy: &Policy) -> Result<Opened, FileStat> {
    let shown = path.display().to_string();
    let real = match path.canonicalize() {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(FileStat::Missing { checked: shown });
        }
        Err(e) => {
            return Err(FileStat::Unreadable {
                checked: shown,
                error: e.to_string(),
            });
        }
    };
    let checked = real.display().to_string();
    if policy.refuses(&real) {
        return Err(FileStat::Refused { checked });
    }
    let meta = match std::fs::symlink_metadata(&real) {
        Ok(m) => m,
        Err(_) => return Err(FileStat::Missing { checked }),
    };
    if meta.is_dir() {
        return Err(FileStat::Dir { path: checked });
    }
    if !meta.is_file() {
        return Err(FileStat::Unreadable {
            checked,
            error: "not a regular file".into(),
        });
    }
    // Non-blocking, so a pipe swapped in can't hang the read; never through
    // a link at the last step.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&real)
        .map_err(|e| FileStat::Unreadable {
            checked: checked.clone(),
            error: e.to_string(),
        })?;
    let now = file.metadata().map_err(|e| FileStat::Unreadable {
        checked: checked.clone(),
        error: e.to_string(),
    })?;
    if !now.is_file() || now.dev() != meta.dev() || now.ino() != meta.ino() {
        return Err(FileStat::Unreadable {
            checked,
            error: "it changed while being opened".into(),
        });
    }
    Ok(Opened {
        file,
        path: real,
        stamp: stamp_of(&now),
    })
}

/// Read a range of the file `written` names. With `expect`, only from that
/// version: `Err((stat, true))` when it's been rewritten since.
pub fn read(
    written: &str,
    base: &Path,
    home: &Path,
    policy: &Policy,
    offset: u64,
    len: u32,
    expect: Option<Stamp>,
) -> Result<(Stamp, u64, Vec<u8>), (FileStat, bool)> {
    let all = candidates(written, base, home);
    for p in &all {
        match open(p, policy) {
            Ok(o) => {
                if expect.is_some_and(|e| e != o.stamp()) {
                    return Err((o.stat(), true));
                }
                return o
                    .read(offset, len)
                    .map(|b| (o.stamp(), o.stamp().size, b))
                    .map_err(|e| {
                        (
                            FileStat::Unreadable {
                                checked: o.path().display().to_string(),
                                error: e.to_string(),
                            },
                            false,
                        )
                    });
            }
            Err(FileStat::Missing { .. }) => continue,
            Err(s) => return Err((s, false)),
        }
    }
    Err((
        FileStat::Missing {
            checked: all[0].display().to_string(),
        },
        false,
    ))
}

/// A picture's width and height from the start of its file: PNG, GIF,
/// JPEG, WebP.
pub fn picture_size(b: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32);
    let le16 = |i: usize| Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32);
    let be32 = |i: usize| {
        Some(u32::from_be_bytes([
            *b.get(i)?,
            *b.get(i + 1)?,
            *b.get(i + 2)?,
            *b.get(i + 3)?,
        ]))
    };
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF8") {
        return Some((le16(6)?, le16(8)?));
    }
    if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        return match b.get(12..16)? {
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let v = u32::from_le_bytes([*b.get(21)?, *b.get(22)?, *b.get(23)?, *b.get(24)?]);
                Some(((v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => {
                let w = u32::from_le_bytes([*b.get(24)?, *b.get(25)?, *b.get(26)?, 0]) + 1;
                let h = u32::from_le_bytes([*b.get(27)?, *b.get(28)?, *b.get(29)?, 0]) + 1;
                Some((w, h))
            }
            _ => None,
        };
    }
    if b.starts_with(&[0xff, 0xd8]) {
        // Walk the markers to a start-of-frame.
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xff {
                i += 1;
                continue;
            }
            let m = b[i + 1];
            if matches!(m, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                return Some((be16(i + 7)?, be16(i + 5)?));
            }
            if m == 0xd8 || m == 0x01 || (0xd0..=0xd7).contains(&m) {
                i += 2;
                continue;
            }
            i += 2 + be16(i + 2)? as usize;
        }
    }
    None
}

#[cfg(test)]
mod tests;
