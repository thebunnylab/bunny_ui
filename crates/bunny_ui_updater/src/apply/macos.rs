//! macOS: mount the disk image, copy its bundle over the running one with
//! `rsync`, unmount. The signature and the notarization are inside the
//! bundle and survive the copy, so no helper and no Gatekeeper prompt.

use std::path::{Path, PathBuf};
use std::process::Command;

use bunny_ui_release::{Installed, UpdateError};

use super::plan::macos as plan;

/// Installs the disk image at `dmg` over `bundle`.
///
/// # Errors
///
/// [`UpdateError::Apply`] when the image does not mount, holds no
/// `.app`, or `rsync` fails.
pub fn install_dmg(bundle: &Path, dmg: &Path) -> Result<Installed, UpdateError> {
    let mount = MountedDmg::attach(dmg)?;
    let entries: Vec<String> = std::fs::read_dir(&mount.mountpoint)
        .map_err(|error| UpdateError::Apply(format!("reading the mounted image: {error}")))?
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect();
    let app = plan::dotapp_in(&entries).ok_or_else(|| UpdateError::Apply("no .app in the disk image".into()))?;
    let status = Command::new("rsync")
        .args(plan::rsync_argv(&mount.mountpoint.join(app), bundle))
        .status()
        .map_err(|error| UpdateError::Apply(format!("starting rsync: {error}")))?;
    drop(mount);
    if status.success() {
        Ok(Installed::ReadyOnRestart)
    } else {
        Err(UpdateError::Apply(format!("rsync exited with {status}")))
    }
}

/// The bundle this process runs from.
pub fn running_bundle() -> Result<PathBuf, UpdateError> {
    let exe = std::env::current_exe().map_err(|error| UpdateError::Apply(format!("finding this program: {error}")))?;
    plan::bundle_root(&exe).ok_or(UpdateError::NotBundled)
}

/// A mounted image, unmounted when it drops.
struct MountedDmg {
    mountpoint: PathBuf,
}

impl MountedDmg {
    fn attach(dmg: &Path) -> Result<MountedDmg, UpdateError> {
        static MOUNTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = MOUNTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mountpoint = std::env::temp_dir().join(format!("bunny-update-mount-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&mountpoint)
            .map_err(|error| UpdateError::Apply(format!("creating the mount point: {error}")))?;
        let status = Command::new("hdiutil")
            .args(plan::mount_argv(dmg, &mountpoint))
            .stdout(std::process::Stdio::null())
            .status()
            .map_err(|error| UpdateError::Apply(format!("starting hdiutil: {error}")))?;
        if status.success() {
            Ok(MountedDmg { mountpoint })
        } else {
            let _ = std::fs::remove_dir(&mountpoint);
            Err(UpdateError::Apply(format!("hdiutil attach exited with {status}")))
        }
    }
}

impl Drop for MountedDmg {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .args(["detach", "-force", "-quiet"])
            .arg(&self.mountpoint)
            .status();
        let _ = std::fs::remove_dir(&self.mountpoint);
    }
}

#[cfg(test)]
#[path = "macos_tests.rs"]
mod tests;
