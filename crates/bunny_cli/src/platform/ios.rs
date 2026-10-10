//! The iOS Simulator: build for it, assemble the `.app` around the
//! binary, install it on the booted simulator and start it with its
//! console in this terminal.
//!
//! No Xcode project: the binary keeps cargo's name (the linker's ad hoc
//! signature is named after it), and the project's `ios/Info.plist`,
//! filled in, is the bundle's.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::{ChildSession, Options, Session};
use crate::cargo::{self, Target};
use crate::devices::Device;
use crate::error::{Error, Result};
use crate::ids;
use crate::process;
use crate::project::Project;
use crate::toolchains::QUICK;

/// The framework's floor, and the oldest iOS `bunny` builds for.
pub const MINIMUM_IOS: (u32, u32) = (17, 0);

/// The Simulator's target on this Mac.
pub fn simulator_target() -> &'static str {
    if cfg!(target_arch = "aarch64") { "aarch64-apple-ios-sim" } else { "x86_64-apple-ios" }
}

/// A built `.app` and the id it installs under.
pub struct App {
    pub path: PathBuf,
    pub bundle_id: String,
}

/// Builds the app for the Simulator and assembles its bundle.
pub fn build_simulator(project: &Project, options: &Options) -> Result<App> {
    let id = project.require_id()?;
    let bin = project.require_bin()?;
    let plist_path = project.platform_dir("ios")?.join("Info.plist");
    let template = std::fs::read_to_string(&plist_path).map_err(crate::error::at(&plist_path))?;
    let minimum = super::plist_string(&template, "MinimumOSVersion").unwrap_or_else(|| String::from("17.0"));
    check_minimum(&minimum)?;
    let mut env = project.build_env();
    // the binary's own floor, the plist's
    env.push((String::from("IPHONEOS_DEPLOYMENT_TARGET"), minimum.clone().into()));
    let built = cargo::build(&cargo::Build {
        manifest: project.manifest.clone(),
        package: project.package.clone(),
        what: Target::Bin(bin.to_string()),
        release: options.release,
        target: Some(simulator_target().to_string()),
        features: options.features.clone(),
        env,
        rustc_args: Vec::new(),
    })?;
    let exe_name = built.artifact.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let app = project.out_dir("ios", "iphonesimulator", options.release).join(format!("{exe_name}.app"));
    super::fresh_dir(&app)?;
    super::copy_binary(&built.artifact, &app.join(&exe_name))?;
    let bundle_id = ids::apple_id(id);
    let version = project.apple_version();
    let build = project.build.to_string();
    let plist = super::render_plist(
        &plist_path,
        &[
            ("APP_NAME", project.name.as_str()),
            ("EXECUTABLE", exe_name.as_str()),
            ("BUNDLE_ID", bundle_id.as_str()),
            ("VERSION", version.as_str()),
            ("BUILD", build.as_str()),
            ("SDK_PLATFORM", "iPhoneSimulator"),
            ("SDK_NAME", "iphonesimulator"),
        ],
    )?;
    std::fs::write(app.join("Info.plist"), plist).map_err(crate::error::at(&app))?;
    Ok(App { path: app, bundle_id })
}

fn check_minimum(minimum: &str) -> Result<()> {
    let mut parts = minimum.split('.').map(|part| part.parse::<u32>().unwrap_or(0));
    let version = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    if version < MINIMUM_IOS {
        return Err(Error::new(format!(
            "ios/Info.plist asks for iOS {minimum}; the framework needs {}.{} or newer",
            MINIMUM_IOS.0, MINIMUM_IOS.1
        ))
        .hint("set MinimumOSVersion to 17.0 or newer in ios/Info.plist"));
    }
    Ok(())
}

/// Installs the app on the booted simulator and starts it, its console
/// in this terminal. The app's environment is this one's `BUNNY_*` and
/// `RUST_BACKTRACE`, the way the desktop app gets it.
pub fn launch_simulator(device: &Device, app: &App, options: &Options) -> Result<Option<Box<dyn Session>>> {
    // installing over a running app relaunches the OLD binary: end it first
    let _ = process::run("xcrun", &["simctl", "terminate", &device.id, &app.bundle_id], QUICK);
    let installed = process::run("xcrun", &["simctl", "install", &device.id, &app.path.to_string_lossy()], QUICK)
        .map_err(|error| Error::new(format!("simctl: {error}")))?;
    if !installed.ok() {
        return Err(Error::new(format!("the Simulator refused the app: {}", installed.stderr.trim())));
    }
    let mut command = Command::new("xcrun");
    command.args(["simctl", "launch", "--terminate-running-process"]);
    if !options.detach {
        // the console blocks until the app ends: that is the session
        let pty = std::io::IsTerminal::is_terminal(&std::io::stdout());
        command.arg(if pty { "--console-pty" } else { "--console" });
    }
    command.args([&device.id, &app.bundle_id]);
    command.args(&options.args).stdin(Stdio::null()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    for (key, value) in child_env(std::env::vars_os()) {
        command.env(key, value);
    }
    if options.detach {
        let out = command.output().map_err(|error| Error::new(format!("simctl: {error}")))?;
        if !out.status.success() {
            return Err(Error::new(format!("the app did not start: {}", String::from_utf8_lossy(&out.stderr).trim())));
        }
        return Ok(None);
    }
    let child = command.spawn().map_err(|error| Error::new(format!("simctl: {error}")))?;
    let udid = device.id.clone();
    let bundle_id = app.bundle_id.clone();
    let on_stop = move || {
        let _ = process::run("xcrun", &["simctl", "terminate", &udid, &bundle_id], QUICK);
    };
    Ok(Some(Box::new(ChildSession { child, on_stop: Some(Box::new(on_stop)) })))
}

/// The variables the app inside the simulator receives: simctl passes
/// on `SIMCTL_CHILD_X` as `X`.
fn child_env(vars: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>) -> Vec<(String, std::ffi::OsString)> {
    let mut env: Vec<(String, std::ffi::OsString)> = vars
        .filter_map(|(key, value)| {
            let key = key.to_str()?.to_string();
            (key.starts_with("BUNNY_") || key == "RUST_BACKTRACE" || key == "RUST_LOG").then(|| (format!("SIMCTL_CHILD_{key}"), value))
        })
        .collect();
    if !env.iter().any(|(key, _)| key == "SIMCTL_CHILD_RUST_BACKTRACE") {
        env.push((String::from("SIMCTL_CHILD_RUST_BACKTRACE"), OsStr::new("1").to_os_string()));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn the_floor_is_ios_17() {
        assert!(check_minimum("17.0").is_ok());
        assert!(check_minimum("18.2").is_ok());
        assert!(check_minimum("16.4").is_err());
    }

    #[test]
    fn the_apps_switches_cross_into_the_simulator() {
        let vars = vec![
            (OsString::from("BUNNY_IOS_TRACE"), OsString::from("1")),
            (OsString::from("HOME"), OsString::from("/Users/x")),
            (OsString::from("RUST_LOG"), OsString::from("debug")),
        ];
        let env = child_env(vars.into_iter());
        let keys: Vec<&str> = env.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, vec!["SIMCTL_CHILD_BUNNY_IOS_TRACE", "SIMCTL_CHILD_RUST_LOG", "SIMCTL_CHILD_RUST_BACKTRACE"]);
    }
}
