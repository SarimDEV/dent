//! Updates from main's builds ([decision 0077]).
//!
//! CI builds every merge to main and publishes it in one rolling release:
//! its parts, named by build number, and `latest.json`, which says which
//! build is newest and how to check each part. A dent CI built knows its
//! number and updates from there; one built anywhere else (number 0) never
//! does. Downloads and unpacking go through `curl` and `tar`, which every
//! Mac and Linux machine has.
//!
//! [decision 0077]: ../../../docs/decisions/0077-updates-from-main.md

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Where main's newest build is said.
pub const FEED: &str = "https://github.com/SarimDEV/dent/releases/download/edge/latest.json";

/// The feed to read: `DENT_FEED` (a URL, `file://` included), else [`FEED`].
pub fn feed_url() -> String {
    std::env::var("DENT_FEED")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| FEED.to_string())
}

/// This build's number among main's builds; 0 when it wasn't built by CI.
/// A debug build takes `DENT_MAIN_BUILD` at run time too, for tests.
pub fn number() -> u64 {
    #[cfg(debug_assertions)]
    if let Some(n) = std::env::var("DENT_MAIN_BUILD")
        .ok()
        .and_then(|n| n.parse().ok())
    {
        return n;
    }
    option_env!("DENT_MAIN_BUILD")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// The commit this build was made from, when CI made it.
pub fn commit() -> &'static str {
    option_env!("DENT_COMMIT").unwrap_or("")
}

/// Whether this build updates itself: CI built it, and it isn't told not to
/// (`DENT_UPDATES_OFF`).
pub fn enabled() -> bool {
    number() > 0 && std::env::var_os("DENT_UPDATES_OFF").is_none()
}

/// What `latest.json` says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    /// The build's number: CI's run number, counting up.
    pub build: u64,
    #[serde(default)]
    pub commit: String,
    /// Its parts by name ([`part_name`]).
    pub parts: BTreeMap<String, Part>,
}

/// One downloadable part: a `.tar.gz`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

impl Feed {
    /// Newer than what runs here.
    pub fn is_newer(&self) -> bool {
        self.build > number()
    }

    /// This machine's `kind` of part (`daemon`, `app`).
    pub fn part(&self, kind: &str) -> Result<&Part, String> {
        let name = part_name(kind);
        self.parts
            .get(&name)
            .ok_or_else(|| format!("build {} has no {name}", self.build))
    }
}

/// A part's name for this machine: `daemon-macos-aarch64`,
/// `app-macos-aarch64`, `daemon-linux-x86_64`.
pub fn part_name(kind: &str) -> String {
    format!("{kind}-{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Read the feed at `url`.
pub fn fetch(url: &str) -> Result<Feed, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "30", "--retry", "2", url])
        .output()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "couldn't read {url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("{url} isn't a feed: {e}"))
}

/// Download `part` to `to`, checked against its size and sha256; what's
/// already there and checks out isn't fetched again. A stalled download
/// fails (under 1 KB/s for 30 s); a slow one doesn't.
pub fn download(part: &Part, to: &Path) -> Result<(), String> {
    if check(part, to).is_ok() {
        return Ok(());
    }
    let mut partial = to.as_os_str().to_owned();
    partial.push(".part");
    let partial = std::path::PathBuf::from(partial);
    let _ = std::fs::remove_file(&partial);
    let status = Command::new("curl")
        .args([
            "-fsSL",
            "--speed-limit",
            "1024",
            "--speed-time",
            "30",
            "--retry",
            "2",
            "-o",
        ])
        .arg(&partial)
        .arg(&part.url)
        .status()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("couldn't download {}", part.url));
    }
    if let Err(e) = check(part, &partial) {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    std::fs::rename(&partial, to).map_err(|e| format!("couldn't keep the download: {e}"))
}

/// Whether the file at `path` is `part`.
pub fn check(part: &Part, path: &Path) -> Result<(), String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    if size != part.size {
        return Err(format!("{} is {size} bytes, not {}", part.url, part.size));
    }
    let sum = sha256(path)?;
    if !sum.eq_ignore_ascii_case(&part.sha256) {
        return Err(format!("{} doesn't match its checksum", part.url));
    }
    Ok(())
}

/// A file's sha256, in hex.
pub fn sha256(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// The team that signed the code at `path` (an app or a binary), on macOS;
/// `None` when it's signed ad hoc or not at all, or off macOS.
pub fn team(path: &Path) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = Command::new("codesign")
        .arg("-dv")
        .arg(path)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .find_map(|l| l.strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "not set")
        .map(str::to_string)
}

/// Whether `new` may take the place of `running` (both apps, or both
/// binaries): once what runs is signed by a team, an update must be signed
/// by that team too, intact. Off macOS, and from a build that isn't signed
/// by a team, anything checked against the feed may.
pub fn same_signer(running: &Path, new: &Path) -> Result<(), String> {
    let Some(ours) = team(running) else {
        return Ok(());
    };
    let intact = Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(new)
        .status()
        .is_ok_and(|s| s.success());
    match team(new) {
        Some(t) if t == ours && intact => Ok(()),
        Some(t) if t == ours => Err(format!("{} isn't intact", new.display())),
        Some(t) => Err(format!("{} is signed by {t}, not {ours}", new.display())),
        None => Err(format!("{} isn't signed by {ours}", new.display())),
    }
}

/// Unpack the `.tar.gz` at `archive` into `into`, made afresh.
pub fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let out = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .output()
        .map_err(|e| format!("couldn't run tar: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(into);
        return Err(format!(
            "couldn't unpack {}: {}",
            archive.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("dent-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A part holding one file, `hello`, and its feed entry.
    fn part(dir: &Path) -> Part {
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("hello"), "hi").unwrap();
        let tgz = dir.join("p.tar.gz");
        assert!(
            Command::new("tar")
                .arg("-czf")
                .arg(&tgz)
                .arg("-C")
                .arg(&src)
                .arg("hello")
                .status()
                .unwrap()
                .success()
        );
        Part {
            url: format!("file://{}", tgz.display()),
            sha256: sha256(&tgz).unwrap(),
            size: std::fs::metadata(&tgz).unwrap().len(),
        }
    }

    #[test]
    fn a_feed_is_read_and_its_part_downloaded_checked_and_unpacked() {
        let d = scratch("ok");
        let p = part(&d);
        let feed = Feed {
            build: 7,
            commit: "abc".into(),
            parts: [(part_name("daemon"), p)].into(),
        };
        let json = d.join("latest.json");
        std::fs::write(&json, serde_json::to_vec(&feed).unwrap()).unwrap();
        let read = fetch(&format!("file://{}", json.display())).unwrap();
        assert_eq!(read, feed);
        assert!(read.is_newer(), "a source build is number 0");
        let got = d.join("got.tar.gz");
        download(read.part("daemon").unwrap(), &got).unwrap();
        // Already there and right: kept, not fetched again.
        download(read.part("daemon").unwrap(), &got).unwrap();
        unpack(&got, &d.join("out")).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("out/hello")).unwrap(), "hi");
        assert!(read.part("app").is_err());
    }

    #[test]
    fn a_part_that_doesnt_match_is_refused_and_not_kept() {
        let d = scratch("bad");
        let mut p = part(&d);
        p.sha256 = "00".repeat(32);
        let got = d.join("got.tar.gz");
        assert!(download(&p, &got).unwrap_err().contains("checksum"));
        assert!(!got.exists());
        let mut p = part(&d);
        p.size += 1;
        assert!(download(&p, &got).unwrap_err().contains("bytes"));
        assert!(!got.exists() && !d.join("got.tar.gz.part").exists());
    }

    #[test]
    fn what_isnt_there_or_isnt_a_feed_says_so() {
        let d = scratch("missing");
        assert!(fetch(&format!("file://{}/nope.json", d.display())).is_err());
        std::fs::write(d.join("x.json"), "not json").unwrap();
        assert!(
            fetch(&format!("file://{}/x.json", d.display()))
                .unwrap_err()
                .contains("isn't a feed")
        );
        let p = Part {
            url: format!("file://{}/nope.tar.gz", d.display()),
            sha256: String::new(),
            size: 0,
        };
        assert!(download(&p, &d.join("n.tar.gz")).is_err());
        std::fs::write(d.join("junk.tar.gz"), "junk").unwrap();
        assert!(unpack(&d.join("junk.tar.gz"), &d.join("o")).is_err());
        assert!(!d.join("o").exists());
    }

    /// Against a real signed dent.app, given as `DENT_SIGNED_APP` (else
    /// skipped): the same team passes; an ad hoc copy and a changed copy
    /// don't; an ad hoc build takes anything.
    #[test]
    fn an_update_must_come_from_the_same_team() {
        let Some(signed) = std::env::var_os("DENT_SIGNED_APP").map(std::path::PathBuf::from) else {
            return;
        };
        let d = scratch("signer");
        let copy = |name: &str| {
            let to = d.join(name).join("dent.app");
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            assert!(
                Command::new("cp")
                    .arg("-R")
                    .arg(&signed)
                    .arg(&to)
                    .status()
                    .unwrap()
                    .success()
            );
            to
        };
        assert!(team(&signed).is_some());
        assert_eq!(same_signer(&signed, &copy("same")), Ok(()));
        let adhoc = copy("adhoc");
        assert!(
            Command::new("codesign")
                .args(["--force", "--deep", "--sign", "-"])
                .arg(&adhoc)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            same_signer(&signed, &adhoc)
                .unwrap_err()
                .contains("isn't signed by")
        );
        let changed = copy("changed");
        std::fs::write(changed.join("Contents/MacOS/dent-daemon"), "#!/bin/sh\n").unwrap();
        assert!(
            same_signer(&signed, &changed)
                .unwrap_err()
                .contains("isn't intact")
        );
        assert_eq!(same_signer(&adhoc, &changed), Ok(()));
    }
}
