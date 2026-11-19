//! Windows: start a copy of this executable as the helper, and let it
//! finish after this process quits.
//!
//! The helper is this same binary, never a second program: a second
//! artifact needs its own signature and can go missing from a package,
//! and a script runs unsigned in the places AppLocker blocks.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use bunny_ui_release::{Installed, UpdateError};

use super::plan::windows::{self as plan, Arg, HelperPlan, ImageState, Pacer};

/// Copies this executable to the temp folder and starts it as the helper.
pub fn start_helper(artifact: &Path, format: bunny_ui_release::Format, product: &str, sidecars: &[String]) -> Result<Installed, UpdateError> {
    let running = std::env::current_exe().map_err(|_| UpdateError::NotBundled)?;
    let plan = HelperPlan::derive(&running, artifact, format, sidecars)?;
    // the copy is the whole trick: a running image cannot be replaced
    let helper = plan::helper_path(&std::env::temp_dir(), product, std::process::id(), env!("CARGO_PKG_VERSION"));
    std::fs::copy(&running, &helper).map_err(|error| UpdateError::Apply(format!("copying the update helper: {error}")))?;
    spawn_detached(&helper, &plan::helper_argv(&plan))?;
    // the caller must not start the app: the helper does
    Ok(Installed::PendingHelper)
}

#[cfg(windows)]
fn spawn_detached(helper: &Path, argv: &[std::ffi::OsString]) -> Result<(), UpdateError> {
    use std::os::windows::process::CommandExt as _;
    // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP, through std's safe setter
    const DETACHED: u32 = 0x0000_0008 | 0x0000_0200;
    Command::new(helper)
        .args(argv)
        .creation_flags(DETACHED)
        .spawn()
        .map_err(|error| UpdateError::Apply(format!("starting the update helper: {error}")))?;
    Ok(())
}

#[cfg(not(windows))]
fn spawn_detached(_helper: &Path, _argv: &[std::ffi::OsString]) -> Result<(), UpdateError> {
    Err(UpdateError::Apply("the Windows update helper runs on Windows only".into()))
}

/// The helper's work: wait for every image, install, start the app. It
/// writes what went wrong to stderr, and returns either way.
pub fn run_helper(plan: &HelperPlan) {
    let unlocked = match plan::await_images(
        plan,
        Duration::from_secs(plan::HELPER_DEADLINE_SECS),
        probe_image,
        &mut WallPacer(Instant::now()),
    ) {
        Ok(unlocked) => unlocked,
        Err(still_busy) => {
            // never install over a held file: an installer half-succeeds there
            eprintln!("bunny-ui update: {still_busy}");
            return;
        }
    };
    match plan::installer_command(&unlocked) {
        Some((program, args)) => {
            let mut command = Command::new(program);
            for arg in args {
                match arg {
                    Arg::Quoted(value) => {
                        command.arg(value);
                    }
                    Arg::Raw(value) => raw_arg(&mut command, &value),
                }
            }
            match command.status() {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    eprintln!("bunny-ui update: the installer exited with {status}");
                    return;
                }
                Err(error) => {
                    eprintln!("bunny-ui update: the installer did not start: {error}");
                    return;
                }
            }
        }
        None => {
            let exe = plan.target.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            if let Err(error) = super::unpack::install_zip(&plan.install_dir, &plan.artifact, exe) {
                eprintln!("bunny-ui update: {error}");
                return;
            }
        }
    }
    let _ = std::fs::remove_file(&plan.artifact);
    let _ = Command::new(&plan.target).spawn();
}

#[cfg(windows)]
fn raw_arg(command: &mut Command, value: &str) {
    use std::os::windows::process::CommandExt as _;
    command.raw_arg(value);
}

#[cfg(not(windows))]
fn raw_arg(command: &mut Command, value: &str) {
    command.arg(value);
}

/// An open for writing with no sharing: it fails while any process maps
/// the image or holds a handle to it. Without truncation, it changes
/// nothing.
#[cfg(windows)]
fn probe_image(image: &Path) -> ImageState {
    use std::os::windows::fs::OpenOptionsExt as _;
    ImageState::from(std::fs::OpenOptions::new().write(true).share_mode(0).open(image))
}

#[cfg(not(windows))]
fn probe_image(image: &Path) -> ImageState {
    ImageState::from(std::fs::OpenOptions::new().write(true).open(image))
}

struct WallPacer(Instant);

impl Pacer for WallPacer {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    fn pause(&mut self, span: Duration) {
        std::thread::sleep(span);
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::probe_image;
    use crate::apply::plan::windows::ImageState;

    #[test]
    fn a_file_another_handle_holds_is_busy_until_released() {
        let image = std::env::temp_dir().join(format!("bunny-probe-{}.exe", std::process::id()));
        let held = std::fs::File::create(&image).unwrap();
        assert!(matches!(probe_image(&image), ImageState::Busy(_)), "an open handle blocks an exclusive open");
        drop(held);
        assert!(matches!(probe_image(&image), ImageState::Free));
        std::fs::remove_file(&image).unwrap();
        assert!(matches!(probe_image(&image), ImageState::Absent));
    }
}
