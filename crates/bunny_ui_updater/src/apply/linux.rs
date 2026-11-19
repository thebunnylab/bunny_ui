//! Linux: refuse an install that is not the app's, then unpack and swap.

use bunny_ui_release::UpdateError;

use super::plan::linux::{self, InstallKind};

/// Refuses before the download, so a user does not wait for a download to
/// learn that `/usr/bin` is not the app's to write.
pub fn precheck() -> Result<(), UpdateError> {
    let exe = std::env::current_exe().map_err(|_| UpdateError::NotBundled)?;
    let appimage = std::env::var("APPIMAGE").ok();
    let flatpak = std::env::var("FLATPAK_ID").ok();
    if let InstallKind::Managed(manager) = linux::classify(&exe, appimage.as_deref(), flatpak.as_deref()) {
        return Err(UpdateError::Apply(format!("another tool manages this copy: {}", manager.advice())));
    }
    // a file made and removed: mode bits do not tell the truth under ACLs,
    // NFS or a read-only mount
    let root = exe.parent().ok_or(UpdateError::NotBundled)?;
    let probe = root.join(format!(".bunny-write-probe-{}", std::process::id()));
    std::fs::write(&probe, b"").map_err(|error| UpdateError::Apply(format!("{} is not writable: {error}", root.display())))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}
