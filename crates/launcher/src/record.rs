//! What the launcher remembers about versions, and each version's manifest.
//!
//! Both are a line per `key=value`, which a reader with only the standard
//! library parses without guessing. The record is written whole (see
//! [`crate::disk::write_atomic`]); one that can't be read stops startup
//! rather than running something by guesswork.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// The launcher's record (`launcher.state` in the state folder).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// The version that runs.
    pub active: String,
    /// A version on trial, set before anything is stopped and cleared once
    /// it commits or rolls back. Found set at startup, the trial was cut
    /// short: the snapshot goes back and the active version runs.
    pub pending: Option<String>,
    /// The update under way: its id and the version it came from.
    pub update: Option<(String, String)>,
    /// Versions that rolled back, never tried again on their own.
    pub never_retry: Vec<String>,
    /// How the last update went.
    pub outcome: Option<Outcome>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub id: String,
    pub from: String,
    pub to: String,
    /// `committed` or `rolled_back`.
    pub how: String,
}

impl Outcome {
    pub fn line(&self) -> String {
        format!("{} {} {} {}", self.id, self.from, self.to, self.how)
    }

    fn parse(s: &str) -> Option<Self> {
        let mut w = s.split(' ');
        let o = Outcome {
            id: w.next()?.into(),
            from: w.next()?.into(),
            to: w.next()?.into(),
            how: w.next()?.into(),
        };
        w.next().is_none().then_some(o)
    }
}

pub const FILE: &str = "launcher.state";

/// `key=value` lines into a map; any other line is a broken file.
fn pairs(text: &str) -> io::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let (k, v) = line.split_once('=').ok_or_else(|| bad(line))?;
        out.insert(k.trim().to_string(), v.trim().to_string());
    }
    Ok(out)
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("unreadable: {what:?}"))
}

/// A version's name names a folder: nothing that could leave `versions/`.
pub fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v != "."
        && v != ".."
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+'))
}

impl Record {
    pub fn load(state: &Path) -> io::Result<Self> {
        let text = std::fs::read_to_string(state.join(FILE))?;
        let m = pairs(&text)?;
        let active = m
            .get("active")
            .cloned()
            .ok_or_else(|| bad("no active version"))?;
        if !valid_version(&active) {
            return Err(bad(&active));
        }
        let some = |k: &str| m.get(k).filter(|v| !v.is_empty()).cloned();
        let update = match (some("update_id"), some("update_from")) {
            (Some(id), Some(from)) => Some((id, from)),
            (None, None) => None,
            _ => return Err(bad("half an update")),
        };
        // A trial is always of an update: without its id there's no
        // snapshot to go back to.
        if some("pending").is_some() && update.is_none() {
            return Err(bad("a trial with no update"));
        }
        let outcome = match some("outcome") {
            Some(o) => Some(Outcome::parse(&o).ok_or_else(|| bad(&o))?),
            None => None,
        };
        Ok(Record {
            active,
            pending: some("pending"),
            update,
            never_retry: some("never_retry")
                .map(|v| v.split(',').map(str::to_string).collect())
                .unwrap_or_default(),
            outcome,
        })
    }

    pub fn save(&self, state: &Path) -> io::Result<()> {
        let mut s = format!("active={}\n", self.active);
        if let Some(p) = &self.pending {
            s += &format!("pending={p}\n");
        }
        if let Some((id, from)) = &self.update {
            s += &format!("update_id={id}\nupdate_from={from}\n");
        }
        if !self.never_retry.is_empty() {
            s += &format!("never_retry={}\n", self.never_retry.join(","));
        }
        if let Some(o) = &self.outcome {
            s += &format!("outcome={}\n", o.line());
        }
        crate::disk::write_atomic(&state.join(FILE), s.as_bytes())
    }

    /// The pending version rolled back: recorded so it's never retried on
    /// its own, however the rollback came about.
    pub fn rolled_back(&mut self) {
        let to = self.pending.take().unwrap_or_default();
        let (id, from) = self.update.take().unwrap_or_default();
        if !to.is_empty() && !self.never_retry.contains(&to) {
            self.never_retry.push(to.clone());
        }
        self.outcome = Some(Outcome {
            id,
            from,
            to,
            how: "rolled_back".into(),
        });
    }

    /// The pending version passed its trial and runs from now on.
    pub fn committed(&mut self) {
        let to = self.pending.take().unwrap_or_default();
        let (id, from) = self.update.take().unwrap_or_default();
        self.active = to.clone();
        self.outcome = Some(Outcome {
            id,
            from,
            to,
            how: "committed".into(),
        });
    }
}

/// What a version says about itself (`versions/<v>/manifest`).
#[derive(Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    /// The launcher protocol it needs.
    pub launcher: u32,
}

impl Manifest {
    pub const FILE: &str = "manifest";

    pub fn load(dir: &Path) -> io::Result<Self> {
        let m = pairs(&std::fs::read_to_string(dir.join(Self::FILE))?)?;
        Ok(Manifest {
            version: m.get("version").cloned().ok_or_else(|| bad("no version"))?,
            launcher: m
                .get("launcher_protocol")
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| bad("no launcher_protocol"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_reads_back_as_written_and_a_broken_one_is_refused() {
        let dir = std::env::temp_dir().join(format!("dent-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Record {
            active: "0.1.0-a1".into(),
            pending: Some("0.2.0-b2".into()),
            update: Some(("u1".into(), "0.1.0-a1".into())),
            never_retry: vec!["0.1.5-c3".into(), "0.1.6-d4".into()],
            outcome: Some(Outcome {
                id: "u0".into(),
                from: "0.0.9-z".into(),
                to: "0.1.0-a1".into(),
                how: "committed".into(),
            }),
        };
        r.save(&dir).unwrap();
        assert_eq!(Record::load(&dir).unwrap(), r);
        // Half an update, a stray line or a name that leaves the folder:
        // startup stops rather than guessing.
        for broken in [
            "active=a\nupdate_id=u1\n",
            "active=a\njunk\n",
            "active=../x\n",
            "pending=a\n",
            "active=a\npending=b\n",
            "active=a\noutcome=u1 a b\n",
        ] {
            std::fs::write(dir.join(FILE), broken).unwrap();
            assert!(Record::load(&dir).is_err(), "{broken:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rollback_is_never_retried_and_a_commit_moves_on() {
        let mut r = Record {
            active: "a".into(),
            pending: Some("b".into()),
            update: Some(("u1".into(), "a".into())),
            ..Default::default()
        };
        let mut back = r.clone();
        back.rolled_back();
        assert_eq!(back.active, "a");
        assert_eq!(back.pending, None);
        assert_eq!(back.never_retry, ["b"]);
        assert_eq!(back.outcome.unwrap().line(), "u1 a b rolled_back");
        r.committed();
        assert_eq!(r.active, "b");
        assert_eq!(r.outcome.unwrap().line(), "u1 a b committed");
    }

    #[test]
    fn version_names_stay_inside_versions() {
        for ok in ["0.1.0", "0.1.0-3f2a9c", "0.1.0+dev.1"] {
            assert!(valid_version(ok), "{ok}");
        }
        for no in ["", ".", "..", "a/b", "../a", "a b", "a\nb"] {
            assert!(!valid_version(no), "{no:?}");
        }
    }
}
