//! What `dev` watches for a rebuild: each member's `src`, manifest and build
//! script, and the workspace's own manifest, lockfile and Cargo
//! configuration. The page is its dev server's to watch, and `target` is
//! never read.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The endings of the files editors write beside a source as it is edited:
/// Vim's swap files, backups, and the safe-write files of `JetBrains` IDEs.
const TEMPORARY: [&str; 7] = [
    "~",
    ".swp",
    ".swo",
    ".swx",
    ".tmp",
    "___jb_tmp___",
    "___jb_old___",
];

/// The watched files and when each was last written: it changes when one is
/// written, added, removed or swapped for another, whatever its time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    files: BTreeMap<PathBuf, SystemTime>,
}

/// The files of a workspace that a rebuild reads.
#[derive(Clone, Debug)]
pub(crate) struct Watch {
    root: PathBuf,
    members: Vec<PathBuf>,
}

/// The workspace's own files a build reads, by their paths below its root:
/// a virtual workspace's manifest is no member's. Not its lockfile, which a
/// build writes: the manifest that changes it is watched.
const WORKSPACE: [&str; 3] = ["Cargo.toml", ".cargo/config.toml", ".cargo/config"];

impl Watch {
    /// Watches the workspace at `root`, and its members whose folders are
    /// `members`.
    pub(crate) fn new<'a>(root: &Path, members: impl IntoIterator<Item = &'a Path>) -> Watch {
        Watch {
            root: root.to_path_buf(),
            members: members.into_iter().map(Path::to_path_buf).collect(),
        }
    }

    /// Returns the watched files' fingerprint now.
    pub(crate) fn fingerprint(&self) -> Fingerprint {
        let mut print = Fingerprint {
            files: BTreeMap::new(),
        };
        for file in WORKSPACE {
            print.add(self.root.join(file));
        }
        for member in &self.members {
            for file in ["Cargo.toml", "build.rs"] {
                print.add(member.join(file));
            }
            walk(&member.join("src"), &mut print);
        }
        print
    }
}

impl Fingerprint {
    fn add(&mut self, file: PathBuf) {
        if let Ok(modified) = fs::metadata(&file).and_then(|m| m.modified()) {
            self.files.insert(file, modified);
        }
    }
}

/// Adds the files below `dir` but an editor's own: a dotfile or folder, a
/// name with a [`TEMPORARY`] ending, or one between `#`s, as Emacs saves.
/// Any other file may be one the code reads, as `include_str!` does, so it
/// is watched whatever its kind.
fn walk(dir: &Path, print: &mut Fingerprint) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let autosave = name.len() > 1 && name.starts_with('#') && name.ends_with('#');
        if name.starts_with('.') || autosave || TEMPORARY.iter().any(|end| name.ends_with(end)) {
            continue;
        }
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk(&path, print),
            Ok(_) => print.add(path),
            Err(_) => {}
        }
    }
}
