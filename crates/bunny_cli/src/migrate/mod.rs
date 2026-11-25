//! `bunny migrate`: a crate written for one bunny-ui carried to the next
//! by deterministic rules — the codemods of [`rules::STEPS`], one step
//! per version — with the diff shown before anything is written.
//!
//! A rule rewrites a call site only where the rewrite is sure. Where it
//! is not, the place is a [`Gap`]: its file, line and column, the word
//! found and what to do, for a person or an agent to finish. The block a
//! raw-mode shell runs bundles the engine it was built with, so nothing
//! shipped breaks; this is how its source moves forward (D723).

pub mod diff;
mod lex;
mod manifest;
pub mod rules;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

pub use manifest::{Requirement, manifest, manifest_version};
use rules::{STEPS, Step};

/// A bunny-ui version as migrations count them: below 1.0 a minor is a
/// release that may change the API, so `0.5.3` is `0.5`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32) -> Version {
        Version { major, minor }
    }

    /// `0.5`, `0.5.2` or `v0.5`.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim().trim_start_matches('v');
        let mut parts = text.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        if parts.next().is_some_and(|patch| patch.parse::<u32>().is_err()) {
            return None;
        }
        Some(Version::new(major, minor))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The newest version this `bunny` migrates to.
pub fn newest() -> Version {
    STEPS.last().map_or(Version::new(0, 0), |step| step.to)
}

/// The oldest version this `bunny` migrates from.
pub fn oldest() -> Version {
    STEPS.first().map_or(Version::new(0, 0), |step| step.from)
}

/// The steps from `from` to `to`, in order.
fn chain(from: Version, to: Version) -> Result<Vec<&'static Step>> {
    if to < from {
        return Err(Error::usage(format!("bunny-ui {from} is newer than {to}: migrations go forward")));
    }
    if to > newest() {
        return Err(Error::usage(format!("this bunny migrates up to bunny-ui {}, not {to}", newest()))
            .hint("a newer bunny knows the newer steps: `cargo install bunny-cli`"));
    }
    let mut steps = Vec::new();
    let mut at = from;
    while at < to {
        let Some(step) = STEPS.iter().find(|step| step.from == at) else {
            return Err(Error::usage(format!("this bunny has no migration from bunny-ui {at}")).hint(format!(
                "it migrates from {} up to {}",
                oldest(),
                newest()
            )));
        };
        steps.push(step);
        at = step.to;
    }
    Ok(steps)
}

/// A place no rule carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    /// 1-based, in the migrated text.
    pub line: usize,
    /// 1-based, in characters.
    pub column: usize,
    /// The word found there.
    pub found: String,
    /// What to do.
    pub note: &'static str,
}

/// A source file migrated.
#[derive(Clone, Debug)]
pub struct Migrated {
    pub text: String,
    pub gaps: Vec<Gap>,
}

/// `text`, written for bunny-ui `from`, as `to` reads it, with the
/// places no rule could carry.
///
/// # Errors
///
/// No chain of steps leads from `from` to `to`.
pub fn source(text: &str, from: Version, to: Version) -> Result<Migrated> {
    let steps = chain(from, to)?;
    let mut text = text.to_string();
    for step in &steps {
        for rule in step.rules {
            let tokens = rules::tokens(&text);
            let edits = rule.edits(&text, &tokens);
            if !edits.is_empty() {
                text = rules::apply(&text, edits);
            }
        }
    }
    // the gaps are found in the text as it ends, where their lines are
    let tokens = rules::tokens(&text);
    let mut found: Vec<rules::Found> = steps.iter().flat_map(|step| step.rules).flat_map(|rule| rule.gaps(&text, &tokens)).collect();
    found.sort_by_key(|found| found.at);
    found.dedup_by(|a, b| a.at == b.at && a.note == b.note);
    let gaps = found
        .into_iter()
        .map(|found| {
            let (line, column) = position(&text, found.at);
            Gap { line, column, found: found.word, note: found.note }
        })
        .collect();
    Ok(Migrated { text, gaps })
}

/// The 1-based line and column of byte `at`.
fn position(text: &str, at: usize) -> (usize, usize) {
    let before = &text[..at];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |line| line.chars().count()) + 1;
    (line, column)
}

/// One file a migration changes.
#[derive(Clone, Debug)]
pub struct Change {
    /// Relative to the crate's folder, with `/`.
    pub path: String,
    pub file: PathBuf,
    pub before: String,
    pub after: String,
}

impl Change {
    /// The change as a unified diff.
    pub fn diff(&self) -> String {
        diff::unified(&self.before, &self.after, &format!("a/{}", self.path), &format!("b/{}", self.path))
    }
}

/// What migrating a crate does, worked out and not yet written.
#[derive(Clone, Debug)]
pub struct Plan {
    pub from: Version,
    pub to: Version,
    /// The crate's folder.
    pub dir: PathBuf,
    /// Every file that changes, the manifests first, then by path.
    pub changes: Vec<Change>,
    /// Every place no rule carries, by file and line.
    pub gaps: Vec<(String, Gap)>,
    /// What a person should know: where the version was read, what was
    /// not rewritten.
    pub notes: Vec<String>,
}

impl Plan {
    /// Writes every change.
    ///
    /// # Errors
    ///
    /// A file that cannot be written; the files before it are written.
    pub fn write(&self) -> Result<()> {
        for change in &self.changes {
            fs::write(&change.file, &change.after).map_err(crate::error::at(&change.file))?;
        }
        Ok(())
    }
}

/// The crate at or above `dir`: the folder of the nearest `Cargo.toml`
/// with a `[package]`.
///
/// # Errors
///
/// No package manifest is found.
pub fn package_dir(dir: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    for folder in absolute.ancestors() {
        let manifest = folder.join("Cargo.toml");
        if let Ok(text) = fs::read_to_string(&manifest)
            && text.lines().any(|line| line.trim() == "[package]")
        {
            return Ok(folder.to_path_buf());
        }
    }
    Err(Error::usage(format!("no crate here: no Cargo.toml with a [package] at or above {}", absolute.display())))
}

/// The workspace manifest above the crate in `dir`, if there is one.
fn workspace_manifest(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().skip(1).map(|folder| folder.join("Cargo.toml")).find(|manifest| {
        fs::read_to_string(manifest).is_ok_and(|text| text.lines().any(|line| line.trim() == "[workspace]"))
    })
}

/// Works out the migration of the crate in `dir` from `from` (else the
/// version its manifest asks for) to `to` (else [`newest`]).
///
/// # Errors
///
/// No crate, no version to start from, no chain of steps, or a file that
/// cannot be read.
pub fn plan(dir: &Path, from: Option<Version>, to: Option<Version>) -> Result<Plan> {
    let dir = package_dir(dir)?;
    let manifest_path = dir.join("Cargo.toml");
    let manifest_text = fs::read_to_string(&manifest_path).map_err(crate::error::at(&manifest_path))?;
    let mut notes = Vec::new();
    // the manifest that says the version: the crate's, or its workspace's
    let mut says: Option<(PathBuf, String, Version)> = None;
    match manifest_version(&manifest_text) {
        Requirement::Version(version) => says = Some((manifest_path.clone(), manifest_text.clone(), version)),
        Requirement::Workspace => {
            if let Some(root) = workspace_manifest(&dir) {
                let text = fs::read_to_string(&root).map_err(crate::error::at(&root))?;
                if let Requirement::Version(version) = manifest_version(&text) {
                    notes.push(format!(
                        "the version is the workspace's ({}): every crate of the workspace builds with the one it names",
                        root.display()
                    ));
                    says = Some((root, text, version));
                }
            }
        }
        Requirement::None => {}
    }
    let from = match (from, &says) {
        (Some(from), _) => from,
        (None, Some((_, _, version))) => *version,
        (None, None) => {
            return Err(Error::usage("Cargo.toml names no bunny-ui version to migrate from")
                .hint("say which with `--from`, as in `bunny migrate --from 0.5`"));
        }
    };
    let to = to.unwrap_or_else(newest);
    chain(from, to)?;
    let mut changes = Vec::new();
    match &says {
        Some((path, text, _)) => {
            if let Some(after) = manifest(text, to)
                && after != *text
            {
                changes.push(Change { path: relative(&dir, path), file: path.clone(), before: text.clone(), after });
            }
        }
        None => notes.push(format!("Cargo.toml names no version of bunny-ui: it is left as it is, ask for {to} where it is set")),
    }
    let mut gaps = Vec::new();
    for file in sources(&dir)? {
        let before = fs::read_to_string(&file).map_err(crate::error::at(&file))?;
        let migrated = source(&before, from, to)?;
        let path = relative(&dir, &file);
        gaps.extend(migrated.gaps.into_iter().map(|gap| (path.clone(), gap)));
        if migrated.text != before {
            changes.push(Change { path, file, before, after: migrated.text });
        }
    }
    Ok(Plan { from, to, dir, changes, gaps, notes })
}

/// `file` relative to `dir`, with `/`; a file outside it (a workspace's
/// manifest) as `../…`.
fn relative(dir: &Path, file: &Path) -> String {
    if let Ok(inside) = file.strip_prefix(dir) {
        return inside.to_string_lossy().replace('\\', "/");
    }
    let mut up = PathBuf::new();
    for ancestor in dir.ancestors().skip(1) {
        up.push("..");
        if let Ok(inside) = file.strip_prefix(ancestor) {
            return up.join(inside).to_string_lossy().replace('\\', "/");
        }
    }
    file.to_string_lossy().into_owned()
}

/// The crate's Rust sources, by path: every `.rs` under its folder but
/// `target/`, hidden folders and the folders of other crates.
fn sources(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let entries = fs::read_dir(&folder).map_err(crate::error::at(&folder))?;
        for entry in entries {
            let path = entry?.path();
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            if path.is_dir() {
                let skipped = name.starts_with('.') || name == "target" || path.join("Cargo.toml").is_file();
                if !skipped {
                    pending.push(path);
                }
            } else if name.ends_with(".rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}
