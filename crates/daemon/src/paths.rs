//! Where the daemon keeps things.
//!
//! State lives outside Documents, Desktop and Downloads, since macOS asks
//! before a background program reads those ([spike 06]). The holder's
//! directory is derived from the state path with a fixed hash, so every
//! daemon version finds the same holder, and kept short because Unix socket
//! paths are limited to about 100 bytes.
//!
//! [spike 06]: ../../docs/spikes/06-session-holder.md

use std::path::{Path, PathBuf};

pub struct Paths {
    pub state: PathBuf,
}

impl Paths {
    /// `--state DIR`, then `DENT_STATE`, then the platform default.
    pub fn new(state: Option<PathBuf>) -> Self {
        let state = state
            .or_else(|| std::env::var_os("DENT_STATE").map(PathBuf::from))
            .unwrap_or_else(default_state);
        Self { state }
    }

    pub fn db(&self) -> PathBuf {
        self.state.join("dent.db")
    }

    pub fn socket(&self) -> PathBuf {
        socket(&self.state)
    }

    pub fn lock(&self) -> PathBuf {
        self.state.join("daemon.lock")
    }

    pub fn holder(&self) -> PathBuf {
        // On Linux the user's runtime folder: set alike in an SSH login and
        // a systemd service, and never swept, where /tmp's files go after
        // ten days unused (a holder waiting on an agent for that long would
        // lose its socket).
        let runtime = cfg!(target_os = "linux")
            .then(|| std::env::var_os("XDG_RUNTIME_DIR"))
            .flatten()
            .map(PathBuf::from)
            .filter(|p| p.is_dir());
        let tmp = runtime
            .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        tmp.join(format!(
            "dent-{:016x}",
            fnv(self.state.as_os_str().as_encoded_bytes())
        ))
    }
}

/// The socket clients connect to, for a given state directory.
pub fn socket(state: &Path) -> PathBuf {
    dent_protocol::daemon_socket(state)
}

pub fn default_state() -> PathBuf {
    let home = home();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/dent")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"))
            .join("dent")
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// FNV-1a: stable across compilers and versions, unlike std's hasher.
fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    #[test]
    fn hash_is_fixed() {
        assert_eq!(super::fnv(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(super::fnv(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
