//! The pure half of the Linux install: whether the install is ours to
//! replace, and what in the archive replaces what.

use std::path::{Path, PathBuf};

use bunny_ui_release::{Format, UpdateError};

/// Who owns the files of an install, when the app does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Manager {
    /// A distribution's package manager: the FHS prefixes.
    SystemPackage,
    /// A sandboxed store: Flatpak, Snap.
    Store,
    /// A single-file image: AppImage.
    Image,
}

impl Manager {
    /// What to tell the user.
    #[must_use]
    pub const fn advice(self) -> &'static str {
        match self {
            Manager::SystemPackage => "update through your package manager",
            Manager::Store => "update through the store this was installed from",
            Manager::Image => "download the new AppImage and replace this one",
        }
    }
}

/// How this copy got here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// The app's to replace.
    SelfManaged,
    /// Someone else's.
    Managed(Manager),
}

/// Which formats Linux installs: a gzipped tarball only.
///
/// # Errors
///
/// [`UpdateError::Apply`] that names the format.
pub fn accepts(format: Format) -> Result<(), UpdateError> {
    match format {
        Format::TarGz => Ok(()),
        other => Err(UpdateError::Apply(format!("linux cannot install a {other:?} artifact"))),
    }
}

/// Whether this install is the app's to replace, in this order:
///
/// 1. `APPIMAGE` set: an AppImage updates by replacing one file.
/// 2. Flatpak or Snap: the store owns the files.
/// 3. Under `/usr/`, `/bin/` or `/sbin/`: a package put it there, and a
///    replaced file leaves the package database wrong.
///
/// The caller checks that it can write, by creating a file: mode bits do
/// not tell the truth under ACLs, NFS or a read-only mount.
#[must_use]
pub fn classify(exe: &Path, appimage: Option<&str>, flatpak: Option<&str>) -> InstallKind {
    if appimage.is_some_and(|value| !value.is_empty()) {
        return InstallKind::Managed(Manager::Image);
    }
    if flatpak.is_some_and(|value| !value.is_empty()) {
        return InstallKind::Managed(Manager::Store);
    }
    let path = exe.to_string_lossy();
    if path.starts_with("/snap/") || path.starts_with("/var/lib/snapd/") {
        return InstallKind::Managed(Manager::Store);
    }
    if path.starts_with("/usr/") || path.starts_with("/bin/") || path.starts_with("/sbin/") {
        return InstallKind::Managed(Manager::SystemPackage);
    }
    InstallKind::SelfManaged
}

/// What the archive holds, after its one top folder is taken off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    /// The binary at the top, beside its support files: the whole folder
    /// is the install, and each top entry replaces its twin.
    Flat,
    /// The binary in `bin/`, as `bunny build linux` packs it, with
    /// `share/` beside it for the launcher. The binary alone replaces the
    /// running one: `install.sh` copied it out of the archive, and the
    /// launcher entry does not change between versions.
    Prefix,
}

/// The layout of an archive with these entries, or `None` when the
/// binary is not where either layout has it.
#[must_use]
pub fn layout(entries: &[String], binary: &str) -> Option<Layout> {
    if entries.iter().any(|entry| entry == binary) {
        Some(Layout::Flat)
    } else if entries.iter().any(|entry| entry == &format!("bin/{binary}")) {
        Some(Layout::Prefix)
    } else {
        None
    }
}

/// The one folder that every entry is in, when there is one: `app/` of
/// `app/bin/app` and `app/share/…`.
#[must_use]
pub fn common_top(entries: &[String]) -> Option<String> {
    let first = entries.first()?.split('/').next()?.to_owned();
    let below = format!("{first}/");
    let inside = |entry: &String| *entry == first || entry.starts_with(&below);
    let deeper = |entry: &String| entry.len() > below.len() && entry.starts_with(&below);
    (entries.iter().all(inside) && entries.iter().any(deeper)).then_some(first)
}

/// Whether a part of a path is a macOS AppleDouble file (`._name`): what a
/// Mac's tar adds for extended attributes. It is not part of the app.
#[must_use]
pub fn is_apple_double(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|name| name.starts_with("._"))
}

/// Where a flat tree is unpacked before the swap: inside the install
/// folder, because `rename(2)` fails across file systems and `/tmp` is
/// often another one.
#[must_use]
pub fn staging_dir(install_root: &Path, tag: &str) -> PathBuf {
    install_root.join(format!(".update-{tag}"))
}

/// The order of the renames: support files first, the binary last. The
/// binary's rename is the commit point: before it, the old binary runs
/// with new support files; at it, nothing changes when it fails.
#[must_use]
pub fn swap_order(binary: &str, mut entries: Vec<String>) -> Vec<String> {
    entries.retain(|entry| entry != binary);
    entries.sort();
    entries.dedup();
    entries.push(binary.to_owned());
    entries
}
