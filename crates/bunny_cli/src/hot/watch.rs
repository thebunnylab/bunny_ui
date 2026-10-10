//! Which of the app's files changed — by asking the file system, a few
//! times a second, for each one's size and modification time. No
//! notification API: the standard library has none, and the app's
//! sources are a few hundred files at most.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

/// The files of the app that a save can change, and how each one was.
pub struct Watch {
    dir: PathBuf,
    seen: BTreeMap<PathBuf, (SystemTime, u64)>,
}

impl Watch {
    /// Watches the package in `dir`: `Cargo.toml`, `build.rs`, and every
    /// `.rs` file under `src/`.
    pub fn new(dir: &Path) -> Watch {
        let mut watch = Watch { dir: dir.to_path_buf(), seen: BTreeMap::new() };
        watch.seen = watch.scan();
        watch
    }

    /// The files that changed, appeared or went away since the last
    /// call. An editor saves in steps (a temporary file, a rename): the
    /// answer waits until two looks in a row agree.
    pub fn changed(&mut self) -> Vec<PathBuf> {
        let mut now = self.scan();
        if now == self.seen {
            return Vec::new();
        }
        loop {
            thread::sleep(Duration::from_millis(40));
            let again = self.scan();
            if again == now {
                break;
            }
            now = again;
        }
        let mut changed: Vec<PathBuf> = now
            .iter()
            .filter(|(path, stamp)| self.seen.get(*path) != Some(*stamp))
            .map(|(path, _)| path.clone())
            .collect();
        changed.extend(self.seen.keys().filter(|path| !now.contains_key(*path)).cloned());
        self.seen = now;
        changed
    }

    fn scan(&self) -> BTreeMap<PathBuf, (SystemTime, u64)> {
        let mut files = BTreeMap::new();
        for name in ["Cargo.toml", "build.rs"] {
            stamp(&self.dir.join(name), &mut files);
        }
        walk(&self.dir.join("src"), &mut files);
        files
    }
}

fn stamp(path: &Path, files: &mut BTreeMap<PathBuf, (SystemTime, u64)>) {
    if let Ok(meta) = fs::metadata(path)
        && meta.is_file()
    {
        files.insert(path.to_path_buf(), (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len()));
    }
}

fn walk(dir: &Path, files: &mut BTreeMap<PathBuf, (SystemTime, u64)>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            walk(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            stamp(&path, files);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_a_new_file_and_a_removal_are_changes() {
        let dir = std::env::temp_dir().join(format!("bunny-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/ui")).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
        fs::write(dir.join("src/lib.rs"), "mod ui;").unwrap();
        fs::write(dir.join("src/notes.txt"), "not code").unwrap();
        let mut watch = Watch::new(&dir);
        assert!(watch.changed().is_empty());

        fs::write(dir.join("src/lib.rs"), "mod ui; // edited").unwrap();
        fs::write(dir.join("src/ui/row.rs"), "fn row() {}").unwrap();
        fs::write(dir.join("src/notes.txt"), "still not code").unwrap();
        let changed = watch.changed();
        assert_eq!(changed, [dir.join("src/lib.rs"), dir.join("src/ui/row.rs")]);

        fs::remove_file(dir.join("src/ui/row.rs")).unwrap();
        assert_eq!(watch.changed(), [dir.join("src/ui/row.rs")]);
        assert!(watch.changed().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
