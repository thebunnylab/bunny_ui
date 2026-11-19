//! The file work of an install, given its paths: unpack a tarball over a
//! Linux install, unpack a zip over a Windows one. The strategies give
//! the paths of the running app. The tests give folders of their own.

use std::path::{Path, PathBuf};

use bunny_ui_release::UpdateError;

use super::plan;

fn apply_error(what: &str, path: &Path, error: &std::io::Error) -> UpdateError {
    UpdateError::Apply(format!("{what} {}: {error}", path.display()))
}

/// Unpacks the zip at `archive` over `install_dir`. Each file goes to a
/// `.bunny-new` twin first and is renamed over the old one. Files that
/// the new build does not have stay.
///
/// # Errors
///
/// [`UpdateError::Apply`] when the zip does not read, does not hold
/// `exe`, has an entry outside the folder, or a file cannot be written.
pub fn install_zip(install_dir: &Path, archive: &Path, exe: &str) -> Result<(), UpdateError> {
    let bytes = std::fs::read(archive).map_err(|error| apply_error("reading", archive, &error))?;
    let items = bunny_ui_formats::zip::read(&bytes).map_err(UpdateError::Apply)?;
    let files = plan::windows::zip_files(items, exe)?;
    // the app's own executable last, as on Linux: the commit point
    let (mut rest, last): (Vec<_>, Vec<_>) =
        files.into_iter().partition(|(path, _)| !path.as_os_str().eq_ignore_ascii_case(exe));
    rest.extend(last);
    for (relative, bytes) in rest {
        let destination = install_dir.join(&relative);
        if let Some(folder) = destination.parent() {
            std::fs::create_dir_all(folder).map_err(|error| apply_error("creating", folder, &error))?;
        }
        let mut fresh = destination.clone().into_os_string();
        fresh.push(".bunny-new");
        let fresh = PathBuf::from(fresh);
        std::fs::write(&fresh, &bytes).map_err(|error| apply_error("writing", &fresh, &error))?;
        std::fs::rename(&fresh, &destination).map_err(|error| {
            let _ = std::fs::remove_file(&fresh);
            apply_error("replacing", &destination, &error)
        })?;
    }
    Ok(())
}

/// Unpacks the tarball at `archive` over the install of `exe`, in the
/// layout the archive has (see [`plan::linux::Layout`]).
///
/// # Errors
///
/// [`UpdateError::Apply`] when the archive does not read, has an entry
/// outside the folder, does not hold the binary, or a file cannot be
/// written. A failure before the binary's rename leaves the old binary.
#[cfg(unix)]
pub fn install_tarball(exe: &Path, archive: &Path, tag: &str) -> Result<bunny_ui_release::Installed, UpdateError> {
    use bunny_ui_formats::tar::Kind;
    use bunny_ui_release::Installed;

    let packed = std::fs::read(archive).map_err(|error| apply_error("reading", archive, &error))?;
    let unpacked =
        bunny_ui_formats::inflate::gunzip(&packed).ok_or_else(|| UpdateError::Apply("the archive is not gzip".into()))?;
    let mut items = bunny_ui_formats::tar::read(&unpacked).map_err(UpdateError::Apply)?;
    items.retain(|item| !plan::linux::is_apple_double(&item.path));
    let names: Vec<String> = items.iter().map(|item| item.path.trim_end_matches('/').to_owned()).collect();
    let top = plan::linux::common_top(&names);
    let strip = |path: &str| -> String {
        let path = path.trim_end_matches('/');
        match &top {
            Some(top) => path.strip_prefix(top.as_str()).unwrap_or(path).trim_start_matches('/').to_owned(),
            None => path.to_owned(),
        }
    };
    let entries: Vec<String> = names.iter().map(|name| strip(name)).filter(|name| !name.is_empty()).collect();
    let root = exe.parent().ok_or(UpdateError::NotBundled)?;
    let binary = exe.file_name().and_then(|name| name.to_str()).ok_or(UpdateError::NotBundled)?.to_owned();
    let layout = plan::linux::layout(&entries, &binary)
        .ok_or_else(|| UpdateError::Apply(format!("the archive has no {binary} in it")))?;

    match layout {
        plan::linux::Layout::Prefix => {
            let wanted = format!("bin/{binary}");
            let item = items
                .into_iter()
                .find(|item| strip(&item.path) == wanted)
                .ok_or_else(|| UpdateError::Apply(format!("the archive has no {wanted}")))?;
            let Kind::File(bytes) = item.kind else {
                return Err(UpdateError::Apply(format!("{wanted} in the archive is not a file")));
            };
            let fresh = root.join(format!(".{binary}.update-{tag}"));
            write_file(&fresh, &bytes, item.mode | 0o700)?;
            std::fs::rename(&fresh, exe).map_err(|error| {
                let _ = std::fs::remove_file(&fresh);
                apply_error("replacing", exe, &error)
            })?;
            Ok(Installed::RelaunchAt(exe.to_path_buf()))
        }
        plan::linux::Layout::Flat => {
            // unpacked inside the install folder: a rename fails across
            // file systems
            let staging = plan::linux::staging_dir(root, tag);
            if staging.exists() {
                let _ = std::fs::remove_dir_all(&staging);
            }
            let unpacked = (|| {
                std::fs::create_dir_all(&staging).map_err(|error| apply_error("creating", &staging, &error))?;
                for item in items {
                    let relative = strip(&item.path);
                    if relative.is_empty() {
                        continue;
                    }
                    let relative = bunny_ui_formats::paths::contained(&relative).ok_or_else(|| {
                        UpdateError::Apply(format!("the archive entry {:?} leaves the install folder", item.path))
                    })?;
                    let destination = staging.join(relative);
                    match item.kind {
                        Kind::Folder => {
                            std::fs::create_dir_all(&destination)
                                .map_err(|error| apply_error("creating", &destination, &error))?;
                        }
                        Kind::File(bytes) => write_file(&destination, &bytes, item.mode)?,
                        Kind::Link(target) => {
                            if bunny_ui_formats::paths::contained(&target).is_none() {
                                return Err(UpdateError::Apply(format!(
                                    "the link {:?} points out of the install folder",
                                    item.path
                                )));
                            }
                            if let Some(folder) = destination.parent() {
                                std::fs::create_dir_all(folder).map_err(|error| apply_error("creating", folder, &error))?;
                            }
                            std::os::unix::fs::symlink(&target, &destination)
                                .map_err(|error| apply_error("linking", &destination, &error))?;
                        }
                    }
                }
                Ok(())
            })();
            if let Err(error) = unpacked {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(error);
            }
            let tops = std::fs::read_dir(&staging)
                .map_err(|error| apply_error("reading", &staging, &error))?
                .flatten()
                .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                .collect();
            for name in plan::linux::swap_order(&binary, tops) {
                let (from, to) = (staging.join(&name), root.join(&name));
                if to.is_dir() && !to.is_symlink() {
                    let _ = std::fs::remove_dir_all(&to);
                }
                std::fs::rename(&from, &to).map_err(|error| apply_error("replacing", &to, &error))?;
            }
            let _ = std::fs::remove_dir_all(&staging);
            // the old binary's inode is gone, and `/proc/self/exe` now ends in
            // " (deleted)": the caller starts this path instead
            Ok(Installed::RelaunchAt(root.join(&binary)))
        }
    }
}

#[cfg(unix)]
fn write_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt as _;
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| apply_error("creating", folder, &error))?;
    }
    std::fs::write(path, bytes).map_err(|error| apply_error("writing", path, &error))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o7777))
        .map_err(|error| apply_error("setting the mode of", path, &error))
}

#[cfg(test)]
#[path = "unpack_tests.rs"]
mod tests;
