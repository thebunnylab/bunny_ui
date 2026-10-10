//! The desktop this program runs on: build the binary and start it, its
//! output in this terminal. On macOS, an app with a `macos/` folder runs
//! inside a bundle — notifications and the app's own name in the menu
//! bar need one.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{ChildSession, Options, Session};
use crate::cargo::{self, Target};
use crate::error::{Error, Result};
use crate::ids;
use crate::process;
use crate::project::Project;
use crate::term;
use crate::toolchains::QUICK;

/// Builds the app and answers the executable to start.
pub fn build(project: &Project, options: &Options) -> Result<PathBuf> {
    let bin = project.require_bin()?;
    let built = cargo::build(&cargo::Build {
        manifest: project.manifest.clone(),
        package: project.package.clone(),
        what: Target::Bin(bin.to_string()),
        release: options.release,
        profile: None,
        target: None,
        features: options.features.clone(),
        env: project.build_env(),
        rustc_args: Vec::new(),
    })?;
    if cfg!(target_os = "macos") && project.dir.join("macos").is_dir() {
        return bundle(project, &built.artifact, options.release);
    }
    Ok(built.artifact)
}

/// The app inside `<Name>.app`: the binary under its cargo name, the
/// project's `macos/Info.plist` filled in, the whole signed ad hoc so
/// the system binds the plist to it. The executable inside is what runs
/// — its output stays in this terminal, and the system still sees the
/// bundle around it.
fn bundle(project: &Project, executable: &Path, release: bool) -> Result<PathBuf> {
    let Some(id) = project.id.as_deref() else {
        println!("{}", term::warn("no `id` in [package.metadata.bunny]: running the bare binary, without a bundle"));
        return Ok(executable.to_path_buf());
    };
    let exe_name = executable.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let app = project.out_dir("macos", "app", release).join(format!("{}.app", project.name.replace('/', "-")));
    let contents = app.join("Contents");
    super::fresh_dir(&contents.join("MacOS"))?;
    let inner = contents.join("MacOS").join(&exe_name);
    super::copy_binary(executable, &inner)?;
    let version = project.apple_version();
    let build = project.build.to_string();
    let bundle_id = ids::apple_id(id);
    let plist = super::render_plist(
        &project.dir.join("macos/Info.plist"),
        &[
            ("APP_NAME", project.name.as_str()),
            ("EXECUTABLE", exe_name.as_str()),
            ("BUNDLE_ID", bundle_id.as_str()),
            ("VERSION", version.as_str()),
            ("BUILD", build.as_str()),
        ],
    )?;
    std::fs::write(contents.join("Info.plist"), plist).map_err(crate::error::at(&contents))?;
    let signed = process::run("codesign", &["--force", "--sign", "-", &app.to_string_lossy()], QUICK);
    if !signed.as_ref().is_ok_and(process::Output::ok) {
        println!("{}", term::warn("codesign could not sign the bundle ad hoc; it runs unsigned"));
    }
    Ok(inner)
}

/// Starts the app, its output in this terminal — or, detached, on its
/// own (`None`: nothing to watch).
pub fn launch(executable: &Path, options: &Options) -> Result<Option<Box<dyn Session>>> {
    if options.detach {
        crate::devices::spawn_detached(executable, &options.args)
            .map_err(|error| Error::new(format!("{} did not start: {error}", executable.display())))?;
        return Ok(None);
    }
    let mut command = Command::new(executable);
    command.args(&options.args).stdin(Stdio::null()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        command.env("RUST_BACKTRACE", "1");
    }
    let child = command
        .spawn()
        .map_err(|error| Error::new(format!("{} did not start: {error}", executable.display())))?;
    Ok(Some(Box::new(ChildSession { child, on_stop: None })))
}
