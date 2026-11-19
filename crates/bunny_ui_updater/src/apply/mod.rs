//! Putting a verified build in place, one strategy for each system, and
//! starting it.
//!
//! | System  | Formats            | How                                                    | Then              |
//! |---------|--------------------|--------------------------------------------------------|-------------------|
//! | macOS   | `dmg`              | mount, `rsync` the `.app` over the running bundle      | `ReadyOnRestart`  |
//! | Linux   | `tar.gz`           | unpack inside the install folder, rename, binary last  | `RelaunchAt`      |
//! | Windows | `zip`, NSIS, WiX   | a copy of this program waits for the app to quit       | `PendingHelper`   |

pub mod plan;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
pub mod unpack;
#[cfg_attr(not(windows), allow(dead_code))]
mod windows;

use bunny_ui_release::{Installed, UpdateError};

use crate::client::Verified;
use crate::config::UpdaterConfig;

/// How a verified artifact is put in place.
pub trait ApplyStrategy {
    /// Whether an install can happen at all, asked before the download.
    ///
    /// # Errors
    ///
    /// [`UpdateError::Apply`] that names what stands in the way.
    fn precheck(&self) -> Result<(), UpdateError> {
        Ok(())
    }

    /// Puts the build in place. The process that calls this is still the
    /// old build: [`Installed`] says how to start the new one.
    ///
    /// # Errors
    ///
    /// [`UpdateError::Apply`] when the install fails or the format is not
    /// one this system installs.
    fn apply(&self, artifact: &Verified) -> Result<Installed, UpdateError>;
}

/// The strategy of the system this build is for.
#[derive(Debug, Clone)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct PlatformStrategy {
    product: String,
    sidecars: Vec<String>,
}

impl PlatformStrategy {
    #[must_use]
    pub fn for_config(config: &UpdaterConfig) -> PlatformStrategy {
        PlatformStrategy { product: config.product.as_str().to_owned(), sidecars: config.sidecars.clone() }
    }
}

impl ApplyStrategy for PlatformStrategy {
    fn precheck(&self) -> Result<(), UpdateError> {
        #[cfg(target_os = "linux")]
        return linux::precheck();
        #[cfg(not(target_os = "linux"))]
        Ok(())
    }

    fn apply(&self, artifact: &Verified) -> Result<Installed, UpdateError> {
        #[cfg(target_os = "macos")]
        {
            plan::macos::accepts(artifact.format())?;
            macos::install_dmg(&macos::running_bundle()?, artifact.path())
        }
        #[cfg(target_os = "linux")]
        {
            plan::linux::accepts(artifact.format())?;
            let exe = std::env::current_exe().map_err(|_| UpdateError::NotBundled)?;
            unpack::install_tarball(&exe, artifact.path(), &std::process::id().to_string())
        }
        #[cfg(windows)]
        {
            windows::start_helper(artifact.path(), artifact.format(), &self.product, &self.sidecars)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            let _ = artifact;
            Err(UpdateError::Apply("this system does not install updates".into()))
        }
    }
}

/// Starts the build that an install put in place. It does not end this
/// process: the caller quits after it, once it is sure nothing is lost.
///
/// Nothing is started for [`Installed::PendingHelper`]: the helper starts
/// the app, and two processes in one install folder is the failure that
/// variant prevents. A start that fails changes nothing: the build is in
/// place, and the next start runs it.
pub fn relaunch(installed: &Installed) {
    match installed {
        Installed::PendingHelper => {}
        Installed::ReadyOnRestart => {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).spawn();
            }
        }
        Installed::RelaunchAt(path) => {
            let _ = std::process::Command::new(path).spawn();
        }
    }
}

/// Runs the Windows update helper when this launch is one, and says so.
///
/// It is `false` on every ordinary launch, and always `false` outside
/// Windows. When it is `true`, the install is done or has given up, and
/// the caller must exit at once and open no window. The `app!` entry of
/// bunny-ui calls this before anything else; an app with its own `main`
/// calls it first.
#[must_use]
pub fn run_helper_if_asked() -> bool {
    #[cfg(windows)]
    {
        match plan::windows::parse_helper_args(std::env::args_os()) {
            Some(plan) => {
                windows::run_helper(&plan);
                true
            }
            None => false,
        }
    }
    #[cfg(not(windows))]
    false
}

/// Removes old helper copies of `product` from `temp_dir`, on any system:
/// a helper cannot delete its own image, so the next download does. Best
/// effort: a copy that does not go now goes next time.
pub(crate) fn sweep_helpers(temp_dir: &std::path::Path, product: &str) {
    let Ok(entries) = std::fs::read_dir(temp_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let age = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .map_or(0, |age| age.as_secs());
        if plan::windows::is_sweepable_helper(name, product, age) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
