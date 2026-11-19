//! The pure half of the macOS install.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use bunny_ui_release::{Format, UpdateError};

/// Which formats macOS installs: a disk image only.
///
/// A bare `.app` is refused: the strategy gives the artifact to `hdiutil
/// attach`, which would fail about the wrong thing.
///
/// # Errors
///
/// [`UpdateError::Apply`] that names the format.
pub fn accepts(format: Format) -> Result<(), UpdateError> {
    match format {
        Format::Dmg => Ok(()),
        other => Err(UpdateError::Apply(format!("macos cannot install a {other:?} artifact"))),
    }
}

/// The `rsync` arguments that replace a bundle in place.
///
/// `--delete` removes here what the new build removed. `Icon?` stays out,
/// because the Finder writes one into a bundle. The slash after the
/// source is what makes it a replace: without it rsync copies the bundle
/// into the destination, `App.app/App.app`.
#[must_use]
pub fn rsync_argv(source_app: &Path, destination: &Path) -> Vec<OsString> {
    let mut source = source_app.as_os_str().to_owned();
    source.push("/");
    let mut destination = destination.as_os_str().to_owned();
    destination.push("/");
    vec!["-a".into(), "--delete".into(), "--exclude".into(), "Icon?".into(), source, destination]
}

/// The `hdiutil` arguments that mount an image quietly and read-only.
#[must_use]
pub fn mount_argv(artifact: &Path, mountpoint: &Path) -> Vec<OsString> {
    vec![
        "attach".into(),
        "-nobrowse".into(),
        "-readonly".into(),
        "-noautoopen".into(),
        "-mountpoint".into(),
        mountpoint.as_os_str().to_owned(),
        artifact.as_os_str().to_owned(),
    ]
}

/// The `.app` among the entries of a mounted image. The case does not
/// matter: APFS and HFS+ ignore it by default.
#[must_use]
pub fn dotapp_in(entries: &[String]) -> Option<PathBuf> {
    entries
        .iter()
        .find(|entry| Path::new(entry).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("app")))
        .map(PathBuf::from)
}

/// The bundle of an executable: `…/App.app/Contents/MacOS/app` gives
/// `…/App.app`, and a bare binary gives `None`.
#[must_use]
pub fn bundle_root(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && app.extension()? == "app")
        .then(|| app.to_path_buf())
}
