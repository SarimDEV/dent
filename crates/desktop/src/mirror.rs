//! Pictures in another machine's sessions: the paths its events name are
//! paths there, which this Mac can't open. Each one is asked of its
//! daemon once (`ReadFile`), kept in a cache folder here, and the
//! session's items are drawn with the copy. Until it's here, the picture
//! is left as it was named (drawn as nothing), and the session is drawn
//! again when it comes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use dent_protocol::Request;

use crate::app::{Dent, Waiting};
use crate::data::{Item, Session};

/// Where one picture stands.
pub enum Copy {
    Asked,
    Here(PathBuf),
    /// It couldn't be read there: not asked again this run.
    Failed,
}

/// The pictures copied from other machines, by (machine, path).
pub type Mirror = HashMap<(String, String), Copy>;

/// The cache folder for machine `machine`'s pictures.
fn folder(machine: &str) -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    home.join("Library/Caches/dent/machines").join(machine)
}

/// Where the copy of `path` from `machine` is kept: in a folder named by a
/// hash of the path, under its own name, so it reads as itself.
fn place(machine: &str, path: &str) -> PathBuf {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
    }
    let name = std::path::Path::new(path)
        .file_name()
        .map_or_else(|| "picture".into(), |n| n.to_string_lossy().into_owned());
    folder(machine).join(format!("{h:016x}")).join(name)
}

fn is_picture(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|e| crate::compose::agent_reads(&e.to_string_lossy()))
}

impl Dent {
    /// The local copy of picture `path` in session `id` of `machine`, or
    /// `None` while it's on its way (asked now if it wasn't).
    fn copy_of(&mut self, machine: &str, id: &str, path: &str) -> Option<PathBuf> {
        let key = (machine.to_string(), path.to_string());
        match self.mirror.get(&key) {
            Some(Copy::Here(p)) => return Some(p.clone()),
            Some(_) => return None,
            None => {}
        }
        // Kept from an earlier run.
        let at = place(machine, path);
        if at.is_file() {
            self.mirror.insert(key, Copy::Here(at.clone()));
            return Some(at);
        }
        self.mirror.insert(key, Copy::Asked);
        let n = self.live.send(Request::ReadFile {
            session: id.to_string(),
            path: path.to_string(),
        });
        self.waiting_insert(n, Waiting::Mirror(id.to_string(), path.to_string()));
        None
    }

    /// Draw another machine's session `id` with this Mac's copies of the
    /// pictures it names, asking for those not here yet.
    pub fn mirror_pictures(&mut self, id: &str, s: &mut Session) {
        let machine = dent_client_core::fleet::split(id).0.to_string();
        if machine.is_empty() {
            return;
        }
        let mut swap: Vec<(usize, Vec<String>)> = Vec::new();
        for (i, item) in s.items.iter().enumerate() {
            let paths = match item {
                Item::Tools { pictures, .. } => pictures,
                Item::User { files, .. } => files,
                _ => continue,
            };
            if !paths.iter().any(|p| is_picture(p)) {
                continue;
            }
            let now: Vec<String> = paths
                .iter()
                .map(|p| {
                    if !is_picture(p) {
                        return p.clone();
                    }
                    self.copy_of(&machine, id, p)
                        .map_or_else(|| p.clone(), |c: PathBuf| c.display().to_string())
                })
                .collect();
            if now != *paths {
                swap.push((i, now));
            }
        }
        if swap.is_empty() {
            return;
        }
        let items = Rc::make_mut(&mut s.items);
        for (i, now) in swap {
            match &mut items[i] {
                Item::Tools { pictures, .. } => *pictures = now,
                Item::User { files, .. } => *files = now,
                _ => {}
            }
        }
    }

    /// A picture from another machine came, or couldn't be read.
    pub fn mirror_came(&mut self, id: String, path: String, bytes: Option<Vec<u8>>) {
        let machine = dent_client_core::fleet::split(&id).0.to_string();
        let key = (machine.clone(), path.clone());
        let at = place(&machine, &path);
        let kept = bytes.is_some_and(|b| {
            at.parent()
                .is_some_and(|p| std::fs::create_dir_all(p).is_ok())
                && std::fs::write(&at, b).is_ok()
        });
        self.mirror
            .insert(key, if kept { Copy::Here(at) } else { Copy::Failed });
        if kept {
            self.rebuild(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_are_named_by_path_and_keep_their_kind() {
        let a = place("m1", "/home/ada/shot.png");
        assert_eq!(a.file_name().unwrap(), "shot.png");
        assert_ne!(a, place("m1", "/home/ada/other/shot.png"));
        assert_ne!(a, place("m2", "/home/ada/shot.png"));
        assert!(is_picture("/x/y.JPG"));
        assert!(!is_picture("/x/y.pdf"));
    }
}
