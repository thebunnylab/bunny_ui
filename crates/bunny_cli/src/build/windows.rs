//! `bunny build windows`: the app as an executable that runs on any
//! Windows 10 or 11 — the C runtime linked in, nothing to install first —
//! zipped under `build/windows/`.
//!
//! The executable carries its resources, written by `bunny` and handed
//! to the linker as a `.res`: the icon from `windows/AppIcon.ico` (group
//! 1, the one the window class loads), the version the file's properties
//! show, and the manifest — per-monitor DPI, the common controls of
//! version 6, UTF-8 as the code page, long paths. A certificate in
//! `BUNNY_WINDOWS_PFX` (and `_PFX_PASSWORD`) signs it with signtool.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Info, Log, Options};
use crate::cargo::{self, Target};
use crate::error::{self, Error, Result};
use crate::formats::{pe, res, zip};
use crate::process;
use crate::project::Project;
use crate::term;

const QUICK: Duration = Duration::from_secs(120);

/// What the build needs before it starts — checked before the last
/// package is cleared away.
pub fn check(project: &Project) -> Result<()> {
    if !cfg!(windows) {
        return Err(Error::new("a Windows app is built on Windows")
            .hint("run `bunny build windows` on Windows; its linker and libraries are Microsoft's"));
    }
    project.require_bin()?;
    Ok(())
}

/// Builds the executable and its zip into `out`.
pub fn build(project: &Project, options: &Options, out: &Path, log: &mut Log, info: &mut Info) -> Result<()> {
    check(project)?;
    let bin = project.require_bin()?;
    let exe_name = format!("{bin}.exe");

    // the resources, beside cargo's own output
    let icon_path = project.dir.join("windows").join("AppIcon.ico");
    let icon = if icon_path.is_file() { Some(fs::read(&icon_path).map_err(error::at(&icon_path))?) } else { None };
    if icon.is_none() {
        log.note("no windows/AppIcon.ico: the executable has the system's icon");
    }
    let version_text = project.version.clone();
    let resources = res::Resources {
        icon: icon.as_deref(),
        version: res::version_numbers(&project.version, project.build),
        version_text: &version_text,
        product: &project.name,
        file_name: &exe_name,
        id: project.id.as_deref().unwrap_or(&project.package),
    };
    let res_bytes = res::write(&resources).map_err(|why| Error::new(format!("windows/AppIcon.ico: {why}")))?;
    let res_dir = project.out_dir("windows", "resources", options.release);
    fs::create_dir_all(&res_dir).map_err(error::at(&res_dir))?;
    let res_path = res_dir.join("app.res");
    fs::write(&res_path, res_bytes).map_err(error::at(&res_path))?;

    let built = cargo::build(&cargo::Build {
        manifest: project.manifest.clone(),
        package: project.package.clone(),
        what: Target::Bin(bin.to_string()),
        release: options.release,
        profile: None,
        target: None,
        features: options.features.clone(),
        env: project.build_env(),
        // the C runtime linked in, so no Visual C++ Redistributable is
        // needed; the resources; and no manifest of the linker's, the
        // resources carry one
        rustc_args: vec![
            String::from("-C"),
            String::from("target-feature=+crt-static"),
            String::from("-C"),
            format!("link-arg={}", res_path.display()),
            String::from("-C"),
            String::from("link-arg=/MANIFEST:NO"),
        ],
        quiet: false,
    })?;
    log.note(&format!("cargo built {}", built.artifact.display()));
    let exe = out.join(&exe_name);
    fs::copy(&built.artifact, &exe).map_err(error::at(&built.artifact))?;

    // what the executable says about itself
    let image = pe::read(&fs::read(&exe).map_err(error::at(&exe))?).map_err(|why| Error::new(format!("{exe_name}: {why}")))?;
    log.note(&format!("{exe_name}: subsystem {}, imports {}", image.subsystem, image.imports.join(" ")));
    if pe::needs_c_runtime(&image) {
        return Err(Error::new(format!("{exe_name} needs the C runtime DLL: it would not start without the Visual C++ Redistributable"))
            .hint("build.log lists what it imports"));
    }
    if !image.resources {
        return Err(Error::new(format!("{exe_name} carries no resources: the linker left out the icon, the version and the manifest")));
    }
    if options.release && image.subsystem != pe::GUI {
        println!(
            "{}",
            term::warn("the app opens a console window next to its own: src/main.rs needs #![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")]")
        );
    }
    info.field("subsystem", if image.subsystem == pe::GUI { "windows" } else { "console" });
    info.field("imports", &image.imports.join(" "));

    let signed = sign(&exe, log)?;
    info.field("signed_by", signed.as_deref().unwrap_or("nobody"));

    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    let folder = project.name.replace(['/', '\\', ':'], "-");
    let zip_name = format!("{folder}-{}-windows-{arch}.zip", project.version);
    let mut archive = Vec::new();
    let bytes = fs::read(&exe).map_err(error::at(&exe))?;
    zip::write(&mut archive, &[(format!("{folder}/{exe_name}"), bytes)]).map_err(|error| Error::new(format!("{zip_name}: {error}")))?;
    fs::write(out.join(&zip_name), archive).map_err(error::at(out))?;
    info.file(&exe_name);
    info.file(&zip_name);
    if signed.is_none() {
        println!("{}", term::dim("unsigned: SmartScreen warns whoever downloads it (BUNNY_WINDOWS_PFX signs it)"));
    }
    Ok(())
}

/// Signs `exe` with the certificate in `BUNNY_WINDOWS_PFX`, timestamped
/// by `BUNNY_WINDOWS_TIMESTAMP` (DigiCert's by default) — or leaves it
/// unsigned without one. Answers who signed.
fn sign(exe: &Path, log: &mut Log) -> Result<Option<String>> {
    let Some(pfx) = std::env::var_os("BUNNY_WINDOWS_PFX").filter(|pfx| !pfx.is_empty()) else {
        return Ok(None);
    };
    let signtool = signtool().ok_or_else(|| {
        Error::new("BUNNY_WINDOWS_PFX names a certificate, and there is no signtool to sign with")
            .hint("signtool comes with the Windows SDK (Visual Studio Installer › Individual components)")
    })?;
    let timestamp = std::env::var("BUNNY_WINDOWS_TIMESTAMP").unwrap_or_else(|_| String::from("http://timestamp.digicert.com"));
    let mut args: Vec<&OsStr> = vec![OsStr::new("sign"), OsStr::new("/fd"), OsStr::new("SHA256"), OsStr::new("/f"), pfx.as_os_str()];
    let password = std::env::var_os("BUNNY_WINDOWS_PFX_PASSWORD");
    if let Some(password) = &password {
        args.extend([OsStr::new("/p"), password.as_os_str()]);
    }
    if !timestamp.is_empty() {
        args.extend([OsStr::new("/tr"), OsStr::new(timestamp.as_str()), OsStr::new("/td"), OsStr::new("SHA256")]);
    }
    args.push(exe.as_os_str());
    // the password is an argument signtool needs; the log keeps it out
    let shown: Vec<&OsStr> = args.iter().map(|arg| if Some(*arg) == password.as_deref() { OsStr::new("********") } else { *arg }).collect();
    log.note(&format!("signtool {}", shown.iter().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>().join(" ")));
    let out = process::run(&signtool, &args, QUICK).map_err(|error| Error::new(format!("signtool: {error}")))?;
    log.note(out.stdout.trim());
    if !out.ok() {
        return Err(Error::new(format!("signtool could not sign the app: {}", out.stderr.trim().lines().last().unwrap_or(""))));
    }
    Ok(Some(Path::new(&pfx).file_name().unwrap_or_default().to_string_lossy().into_owned()))
}

/// signtool: on the PATH, or in the newest Windows SDK.
fn signtool() -> Option<PathBuf> {
    if let Some(found) = process::which("signtool") {
        return Some(found);
    }
    let kits = PathBuf::from(std::env::var_os("ProgramFiles(x86)")?).join("Windows Kits").join("10").join("bin");
    let mut versions: Vec<PathBuf> = fs::read_dir(kits).ok()?.flatten().map(|entry| entry.path()).collect();
    versions.sort();
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    versions.iter().rev().map(|dir| dir.join(arch).join("signtool.exe")).find(|path| path.is_file())
}
