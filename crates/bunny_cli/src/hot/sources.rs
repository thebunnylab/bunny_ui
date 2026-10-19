//! Source provenance is captured around a build, never when its frame arrives.
//!
//! ## Wiring
//! The hot lane captures all package sources, including executable inputs. A
//! stable capture becomes `BuildSources` only after the build finishes. Reloads
//! also compare inputs that only a restart can replace against the initial build.
//! These are local build observations, not a second definition of the app.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Input {
    text: String,
    modified: SystemTime,
    len: u64,
}

/// A complete readable set of inputs observed before compilation.
#[derive(Clone, Debug)]
pub struct BuildInputs {
    roots: Vec<PathBuf>,
    inputs: BTreeMap<PathBuf, Input>,
}

/// Source text that remained stable across a completed build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildSources {
    files: BTreeMap<PathBuf, String>,
}

impl BuildSources {
    /// Absolute source paths and their observed text; manifests are guards only.
    pub fn files(&self) -> &BTreeMap<PathBuf, String> {
        &self.files
    }
}

impl BuildInputs {
    /// Read package manifests, build scripts and all Rust under each `src/`.
    ///
    /// # Errors
    /// Refuses unreadable inputs, missing roots, symlinks and files that move
    /// while being read. An incomplete scan must never look like a valid build.
    pub fn capture(roots: &[PathBuf]) -> io::Result<Self> {
        let mut inputs = BTreeMap::new();
        for root in roots {
            read(&root.join("Cargo.toml"), &mut inputs)?;
            match fs::symlink_metadata(root.join("build.rs")) {
                Ok(_) => read(&root.join("build.rs"), &mut inputs)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            walk(&root.join("src"), &mut inputs)?;
        }
        Ok(Self {
            roots: roots.to_vec(),
            inputs,
        })
    }

    /// Complete a build observation. Any changed input leaves provenance unknown.
    ///
    /// # Errors
    /// Returns the I/O failure when inputs cannot be re-read after compilation.
    pub fn finish(self) -> io::Result<Option<BuildSources>> {
        let after = Self::capture(&self.roots)?;
        Ok((self.inputs == after.inputs).then(|| BuildSources {
            files: self
                .inputs
                .into_iter()
                .filter_map(|(path, input)| {
                    (path.extension().is_some_and(|ext| ext == "rs")
                        && path.file_name().is_none_or(|name| name != "build.rs"))
                    .then_some((path, input.text))
                })
                .collect(),
        }))
    }

    /// Whether every selected input still has its startup text, including
    /// additions/removals. Used for sources a library-only reload cannot replace.
    pub fn unchanged_where(&self, initial: &Self, include: impl Fn(&Path) -> bool) -> bool {
        self.inputs
            .keys()
            .chain(initial.inputs.keys())
            .filter(|path| include(path))
            .all(|path| {
                self.inputs.get(path).map(|input| &input.text)
                    == initial.inputs.get(path).map(|input| &input.text)
            })
    }
}

fn read(path: &Path, inputs: &mut BTreeMap<PathBuf, Input>) -> io::Result<()> {
    let before = fs::symlink_metadata(path)?;
    if !before.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source input is not a regular file: {}", path.display()),
        ));
    }
    let text = fs::read_to_string(path)?;
    let after = fs::symlink_metadata(path)?;
    if before.modified()? != after.modified()? || before.len() != after.len() {
        return Err(io::Error::other(format!(
            "source input changed while being read: {}",
            path.display()
        )));
    }
    inputs.insert(
        path.to_path_buf(),
        Input {
            text,
            modified: after.modified()?,
            len: after.len(),
        },
    );
    Ok(())
}

fn walk(dir: &Path, inputs: &mut BTreeMap<PathBuf, Input>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("source input is a symlink: {}", path.display()),
            ));
        }
        if kind.is_dir() {
            walk(&path, inputs)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            read(&path, inputs)?;
        }
    }
    Ok(())
}
