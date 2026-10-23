//! Writing so a crash or power cut leaves either the old file or the new
//! one, never half of one; copying by clone where the filesystem can.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

/// Flush a file or folder to disk.
pub fn sync(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

/// Flush the folder holding `path`, so a rename or removal in it lasts.
pub fn sync_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => sync(p),
        _ => sync(Path::new(".")),
    }
}

/// Write `data` to `path` whole: a temporary file, flushed, renamed over
/// it, and the folder flushed.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = Path::new(&tmp);
    {
        let mut f = File::create(tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(tmp, path)?;
    sync_parent(path)
}

/// Remove `path` if it's there, and make the removal last.
pub fn remove(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Copy `src` to `dst`, flushed: a clone on APFS and reflink filesystems
/// (instant, no extra room until either changes), a plain copy elsewhere.
pub fn copy(src: &Path, dst: &Path) -> io::Result<()> {
    let _ = std::fs::remove_file(dst);
    let cloned = if cfg!(target_os = "macos") {
        // macOS `cp -c` clones (clonefile); it has no --reflink.
        std::process::Command::new("cp")
            .arg("-c")
            .arg(src)
            .arg(dst)
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    } else {
        std::process::Command::new("cp")
            .arg("--reflink=auto")
            .arg(src)
            .arg(dst)
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if !cloned {
        std::fs::copy(src, dst)?;
    }
    sync(dst)
}

/// Bytes free to this user on the filesystem holding `path`.
pub fn free(path: &Path) -> io::Result<u64> {
    // Tests stand in for a nearly full disk.
    if let Some(n) = std::env::var("DENT_LAUNCHER_FREE")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        return Ok(n);
    }
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: `c` is a valid C string and `st` is plain data the call fills.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return Err(io::Error::last_os_error());
        }
        #[allow(clippy::useless_conversion)]
        Ok(u64::from(st.f_bavail) * u64::from(st.f_frsize))
    }
}
