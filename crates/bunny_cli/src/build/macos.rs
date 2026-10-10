//! `bunny build macos`: the app as a bundle ready to hand out —
//! `<Name>.app` and a disk image, under `build/macos/`.
//!
//! The bundle is the binary, the project's `macos/Info.plist` filled in,
//! and an icon when `macos/` has one (`AppIcon.icns`, or `AppIcon.png` at
//! 1024 × 1024, which `sips` and `iconutil` turn into one).
//!
//! A Developer ID Application identity from the keychain signs it, with
//! the hardened runtime and a secure timestamp — what notarization asks
//! for. Without one, the app is signed ad hoc: it runs on this Mac, and
//! Gatekeeper refuses it on others. With a notarytool keychain profile
//! (`xcrun notarytool store-credentials`) or an App Store Connect API key,
//! the disk image goes to Apple's notary service, and its ticket is
//! stapled to the disk image and to the app.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::time::Duration;

use super::{Info, Log, Options, Signing};
use crate::cargo::{self, Target};
use crate::error::{self, Error, Result};
use crate::ids;
use crate::json;
use crate::platform;
use crate::project::Project;
use crate::term;
use crate::toolchains;

const QUICK: Duration = Duration::from_secs(120);
const LONG: Duration = Duration::from_secs(600);
/// The notary service answers in minutes, sometimes in an hour.
const NOTARY: Duration = Duration::from_secs(3600);

/// Who signs, decided.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Signer {
    Unsigned,
    AdHoc,
    DeveloperId(String),
}

/// How the notary service knows the developer.
enum Notary {
    Profile(String),
    Key { key: String, id: String, issuer: String },
}

impl Notary {
    fn args(&self) -> Vec<String> {
        match self {
            Notary::Profile(profile) => vec![String::from("--keychain-profile"), profile.clone()],
            Notary::Key { key, id, issuer } => vec![
                String::from("--key"),
                key.clone(),
                String::from("--key-id"),
                id.clone(),
                String::from("--issuer"),
                issuer.clone(),
            ],
        }
    }
}

/// What the build needs before it starts — checked before the last
/// package is cleared away.
pub fn check(project: &Project, options: &Options) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Err(Error::new("a macOS app is built on a Mac").hint("run `bunny build macos` on macOS"));
    }
    project.require_id()?;
    project.require_bin()?;
    project.platform_dir("macos")?;
    if let Some(rust) = toolchains::rust::detect()
        && let Some(missing) = targets(options).into_iter().find(|target| !rust.has_target(target))
    {
        return Err(Error::new(format!("the Rust target {missing} is not installed"))
            .hint(format!("rustup target add {missing}")));
    }
    Ok(())
}

/// The CPUs the binary is built for.
fn targets(options: &Options) -> Vec<&'static str> {
    if options.universal { vec!["aarch64-apple-darwin", "x86_64-apple-darwin"] } else { vec![host_target()] }
}

/// Builds the bundle and the disk image into `out`.
pub fn build(project: &Project, options: &Options, out: &Path, log: &mut Log, info: &mut Info) -> Result<()> {
    check(project, options)?;
    let id = project.require_id()?;
    let bin = project.require_bin()?;
    let macos = project.platform_dir("macos")?;
    let plist_path = macos.join("Info.plist");
    let template = fs::read_to_string(&plist_path).map_err(error::at(&plist_path))?;
    let minimum = platform::plist_string(&template, "LSMinimumSystemVersion").unwrap_or_else(|| String::from("14.0"));

    // the binary, for each CPU asked for
    let targets = targets(options);
    let mut env = project.build_env();
    // the binary's floor, the plist's
    env.push((String::from("MACOSX_DEPLOYMENT_TARGET"), minimum.clone().into()));
    let mut binaries = Vec::new();
    for target in &targets {
        let built = cargo::build(&cargo::Build {
            manifest: project.manifest.clone(),
            package: project.package.clone(),
            what: Target::Bin(bin.to_string()),
            release: options.release,
            profile: None,
            target: Some((*target).to_string()),
            features: options.features.clone(),
            env: env.clone(),
            rustc_args: Vec::new(),
            quiet: false,
        })?;
        log.note(&format!("cargo built {} for {target}", built.artifact.display()));
        binaries.push(built.artifact);
    }

    // the bundle
    let name = project.name.replace('/', "-");
    let exe_name = binaries[0].file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let app = out.join(format!("{name}.app"));
    let contents = app.join("Contents");
    let exe = contents.join("MacOS").join(&exe_name);
    fs::create_dir_all(contents.join("MacOS")).map_err(error::at(&contents))?;
    fs::create_dir_all(contents.join("Resources")).map_err(error::at(&contents))?;
    if binaries.len() > 1 {
        let mut args: Vec<&OsStr> = vec![OsStr::new("-create"), OsStr::new("-output"), exe.as_os_str()];
        args.extend(binaries.iter().map(|binary| binary.as_os_str()));
        let lipo = log.run("lipo", &args, QUICK)?;
        if !lipo.ok() {
            return Err(Error::new(format!("lipo could not join the two binaries: {}", lipo.stderr.trim())));
        }
    } else {
        platform::copy_binary(&binaries[0], &exe)?;
    }
    let icon = icon(&macos, &contents.join("Resources"), log)?;
    let version = project.apple_version();
    let build = project.build.to_string();
    let bundle_id = ids::apple_id(id);
    let mut plist = platform::render_plist(
        &plist_path,
        &[
            ("APP_NAME", project.name.as_str()),
            ("EXECUTABLE", exe_name.as_str()),
            ("BUNDLE_ID", bundle_id.as_str()),
            ("VERSION", version.as_str()),
            ("BUILD", build.as_str()),
        ],
    )?;
    if icon && !plist.contains("<key>CFBundleIconFile</key>") {
        plist = add_key(&plist, "CFBundleIconFile", "AppIcon");
    }
    fs::write(contents.join("Info.plist"), plist).map_err(error::at(&contents))?;
    fs::write(contents.join("PkgInfo"), "APPL????").map_err(error::at(&contents))?;
    info.file(&format!("{name}.app"));

    if options.release {
        symbols(&exe, &out.join(format!("{name}.app.dSYM")), log, info);
    }

    let signer = signer(&options.signing, log)?;
    sign(&app, &signer, &macos, log)?;

    let dmg = if options.dmg {
        let dmg = out.join(format!("{name}-{}.dmg", project.version));
        disk_image(&app, &name, &dmg, &signer, log)?;
        info.file(&dmg.file_name().unwrap_or_default().to_string_lossy());
        Some(dmg)
    } else {
        None
    };

    let notarized = match (&signer, notary(options)) {
        (Signer::DeveloperId(_), Some(notary)) => {
            notarize(&app, dmg.as_deref(), &notary, out, log)?;
            true
        }
        _ if options.notarize => {
            return Err(match signer {
                Signer::DeveloperId(_) => Error::new("nothing to notarize with: no notarytool profile and no API key")
                    .hint("xcrun notarytool store-credentials bunny, then --notary-profile bunny (or BUNNY_MACOS_NOTARY_PROFILE)"),
                _ => Error::new("only an app signed with a Developer ID can be notarized")
                    .hint("install a Developer ID Application certificate in the keychain, or name it with --sign"),
            });
        }
        _ => false,
    };

    info.field(
        "signed_by",
        match &signer {
            Signer::Unsigned => "nobody",
            Signer::AdHoc => "ad hoc",
            Signer::DeveloperId(identity) => identity,
        },
    );
    info.field("notarized", if notarized { "yes" } else { "no" });
    info.field("architectures", &targets.join(" "));
    let accepted = gatekeeper(&app, log);
    info.field("gatekeeper", if accepted { "accepted" } else { "refused" });
    match (&signer, accepted) {
        (_, true) => println!("{}", term::ok("Gatekeeper accepts the app")),
        (Signer::DeveloperId(_), false) => println!(
            "{}",
            term::warn("Gatekeeper refuses the app until it is notarized: --notary-profile, or BUNNY_MACOS_NOTARY_PROFILE")
        ),
        _ => println!(
            "{}",
            term::warn("signed ad hoc: the app runs on this Mac, and Gatekeeper refuses it on others (a Developer ID signs it for them)")
        ),
    }
    Ok(())
}

fn host_target() -> &'static str {
    if cfg!(target_arch = "aarch64") { "aarch64-apple-darwin" } else { "x86_64-apple-darwin" }
}

/// The plist with one more string key, before its last `</dict>`.
fn add_key(plist: &str, key: &str, value: &str) -> String {
    match plist.rfind("</dict>") {
        Some(end) => format!("{}    <key>{key}</key>\n    <string>{value}</string>\n{}", &plist[..end], &plist[end..]),
        None => plist.to_string(),
    }
}

/// The app's icon into `resources`: `macos/AppIcon.icns` as it is, or
/// `macos/AppIcon.png` made into one. `false`: the app has no icon.
fn icon(macos: &Path, resources: &Path, log: &mut Log) -> Result<bool> {
    let icns = macos.join("AppIcon.icns");
    let target = resources.join("AppIcon.icns");
    if icns.is_file() {
        fs::copy(&icns, &target).map_err(error::at(&icns))?;
        return Ok(true);
    }
    let png = macos.join("AppIcon.png");
    if !png.is_file() {
        log.note("no macos/AppIcon.png or AppIcon.icns: the app has the system's icon");
        return Ok(false);
    }
    let iconset = resources.join("AppIcon.iconset");
    fs::create_dir_all(&iconset).map_err(error::at(&iconset))?;
    for (points, scale) in [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)] {
        let pixels = (points * scale).to_string();
        let file = if scale == 1 { format!("icon_{points}x{points}.png") } else { format!("icon_{points}x{points}@2x.png") };
        let out = iconset.join(file);
        let args = [OsStr::new("-z"), OsStr::new(&pixels), OsStr::new(&pixels), png.as_os_str(), OsStr::new("--out"), out.as_os_str()];
        if !log.run("sips", &args, QUICK)?.ok() {
            return Err(Error::new("sips could not size macos/AppIcon.png").hint("build.log has its answer"));
        }
    }
    let made = log.run("iconutil", &[OsStr::new("-c"), OsStr::new("icns"), iconset.as_os_str(), OsStr::new("-o"), target.as_os_str()], QUICK)?;
    let _ = fs::remove_dir_all(&iconset);
    if !made.ok() {
        return Err(Error::new(format!("iconutil could not make the icon: {}", made.stderr.trim())));
    }
    Ok(true)
}

/// The debug symbols beside the app, for a crash report to be read
/// against — when the build kept any.
fn symbols(exe: &Path, dsym: &Path, log: &mut Log, info: &mut Info) {
    let Ok(out) = log.run("dsymutil", &[exe.as_os_str(), OsStr::new("-o"), dsym.as_os_str()], LONG) else { return };
    if out.ok() && !out.stderr.contains("no debug symbols") && dsym.is_dir() {
        info.file(&dsym.file_name().unwrap_or_default().to_string_lossy());
    } else {
        let _ = fs::remove_dir_all(dsym);
    }
}

/// Who signs: the identity named, the one Developer ID in the keychain,
/// or — with none — an ad hoc signature.
fn signer(signing: &Signing, log: &mut Log) -> Result<Signer> {
    match signing {
        Signing::Unsigned => Ok(Signer::Unsigned),
        Signing::Identity(identity) => Ok(Signer::DeveloperId(identity.clone())),
        Signing::Auto => {
            let out = log.run("security", &["find-identity", "-v", "-p", "codesigning"], QUICK)?;
            let found = developer_ids(&out.stdout);
            match found.as_slice() {
                [] => Ok(Signer::AdHoc),
                [one] => Ok(Signer::DeveloperId(one.clone())),
                several => Err(Error::new(format!(
                    "the keychain holds {} Developer ID Application identities:\n  {}",
                    several.len(),
                    several.join("\n  ")
                ))
                .hint("name one: bunny build macos --sign \"Developer ID Application: …\"")),
            }
        }
    }
}

/// The Developer ID Application identities in `security find-identity`'s
/// answer, by name.
fn developer_ids(text: &str) -> Vec<String> {
    let mut found: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let start = line.find('"')? + 1;
            let end = start + line[start..].find('"')?;
            Some(line[start..end].to_string())
        })
        .filter(|name| name.starts_with("Developer ID Application:"))
        .collect();
    found.dedup();
    found
}

fn sign(app: &Path, signer: &Signer, macos: &Path, log: &mut Log) -> Result<()> {
    let mut args: Vec<&OsStr> = vec![OsStr::new("--force")];
    let entitlements = macos.join("entitlements.plist");
    match signer {
        Signer::Unsigned => {
            log.note("--no-codesign: the app keeps the linker's signature of its binary");
            return Ok(());
        }
        Signer::AdHoc => args.extend([OsStr::new("--sign"), OsStr::new("-")]),
        Signer::DeveloperId(identity) => {
            // the hardened runtime and a secure timestamp: what the
            // notary service requires of everything it notarizes
            args.extend([OsStr::new("--options"), OsStr::new("runtime"), OsStr::new("--timestamp")]);
            if entitlements.is_file() {
                args.extend([OsStr::new("--entitlements"), entitlements.as_os_str()]);
            }
            args.extend([OsStr::new("--sign"), OsStr::new(identity.as_str())]);
        }
    }
    args.push(app.as_os_str());
    let signed = log.run("codesign", &args, QUICK)?;
    if !signed.ok() {
        return Err(Error::new(format!("codesign failed: {}", signed.stderr.trim())).hint("build.log has its whole answer"));
    }
    let verified = log.run("codesign", &[OsStr::new("--verify"), OsStr::new("--strict"), OsStr::new("--deep"), app.as_os_str()], QUICK)?;
    if !verified.ok() {
        return Err(Error::new(format!("the signature does not verify: {}", verified.stderr.trim())));
    }
    Ok(())
}

/// A compressed disk image with the app and a link to Applications, the
/// way a Mac app is handed out — signed by the app's Developer ID.
fn disk_image(app: &Path, name: &str, dmg: &Path, signer: &Signer, log: &mut Log) -> Result<()> {
    let staging = dmg.with_extension("staging");
    platform::fresh_dir(&staging)?;
    let inside = staging.join(app.file_name().unwrap_or_default());
    // ditto keeps what a copy may lose: the signature's extended attributes
    let copied = log.run("ditto", &[app.as_os_str(), inside.as_os_str()], LONG)?;
    if !copied.ok() {
        return Err(Error::new(format!("ditto could not copy the app: {}", copied.stderr.trim())));
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("/Applications", staging.join("Applications")).map_err(error::at(&staging))?;
    let args = [
        OsStr::new("create"),
        OsStr::new("-volname"),
        OsStr::new(name),
        OsStr::new("-srcfolder"),
        staging.as_os_str(),
        OsStr::new("-ov"),
        OsStr::new("-format"),
        OsStr::new("UDZO"),
        dmg.as_os_str(),
    ];
    let made = log.run("hdiutil", &args, LONG)?;
    let _ = fs::remove_dir_all(&staging);
    if !made.ok() {
        return Err(Error::new(format!("hdiutil could not make the disk image: {}", made.stderr.trim())));
    }
    if let Signer::DeveloperId(identity) = signer {
        let signed = log.run(
            "codesign",
            &[OsStr::new("--timestamp"), OsStr::new("--sign"), OsStr::new(identity.as_str()), dmg.as_os_str()],
            QUICK,
        )?;
        if !signed.ok() {
            return Err(Error::new(format!("codesign could not sign the disk image: {}", signed.stderr.trim())));
        }
    }
    Ok(())
}

/// How to reach the notary service: `--notary-profile`, then
/// `BUNNY_MACOS_NOTARY_PROFILE`, then an App Store Connect API key in
/// `BUNNY_MACOS_NOTARY_KEY` (the `.p8` file), `_KEY_ID` and `_ISSUER`.
fn notary(options: &Options) -> Option<Notary> {
    if let Some(profile) = options.notary_profile.clone().or_else(|| std::env::var("BUNNY_MACOS_NOTARY_PROFILE").ok()) {
        return Some(Notary::Profile(profile));
    }
    let var = |name: &str| std::env::var(format!("BUNNY_MACOS_NOTARY_{name}")).ok().filter(|value| !value.is_empty());
    Some(Notary::Key { key: var("KEY")?, id: var("KEY_ID")?, issuer: var("ISSUER")? })
}

/// Sends the disk image — or the app, zipped, when there is none — to
/// the notary service, waits for its verdict, and staples the ticket.
fn notarize(app: &Path, dmg: Option<&Path>, notary: &Notary, out: &Path, log: &mut Log) -> Result<()> {
    println!("{}", term::dim("Notarizing: Apple's notary service answers in minutes…"));
    let zip = out.join("notarize.zip");
    let upload = match dmg {
        Some(dmg) => dmg.to_path_buf(),
        None => {
            let zipped = log.run("ditto", &[OsStr::new("-c"), OsStr::new("-k"), OsStr::new("--keepParent"), app.as_os_str(), zip.as_os_str()], LONG)?;
            if !zipped.ok() {
                return Err(Error::new(format!("ditto could not zip the app: {}", zipped.stderr.trim())));
            }
            zip.clone()
        }
    };
    let mut args: Vec<String> = vec![String::from("notarytool"), String::from("submit"), upload.to_string_lossy().into_owned()];
    args.extend(notary.args());
    args.extend([String::from("--wait"), String::from("--output-format"), String::from("json")]);
    let submitted = log.run("xcrun", &args, NOTARY)?;
    let _ = fs::remove_file(&zip);
    let answer = json::parse(submitted.stdout.trim()).ok();
    let status = answer.as_ref().and_then(|answer| answer.str_at(&["status"])).unwrap_or("").to_string();
    if status != "Accepted" {
        if let Some(id) = answer.as_ref().and_then(|answer| answer.str_at(&["id"])) {
            let mut log_args = vec![String::from("notarytool"), String::from("log"), id.to_string()];
            log_args.extend(notary.args());
            let _ = log.run("xcrun", &log_args, QUICK);
        }
        let why = if status.is_empty() { submitted.stderr.trim().to_string() } else { format!("the notary service answered {status}") };
        return Err(Error::new(format!("notarization failed: {why}")).hint("build.log has the notary service's log"));
    }
    for stapled in dmg.into_iter().chain([app]) {
        let out = log.run("xcrun", &[OsStr::new("stapler"), OsStr::new("staple"), stapled.as_os_str()], QUICK)?;
        if !out.ok() {
            return Err(Error::new(format!("stapler could not staple {}: {}", stapled.display(), out.stderr.trim())));
        }
    }
    Ok(())
}

/// Whether Gatekeeper lets the app open on a Mac that downloaded it.
fn gatekeeper(app: &Path, log: &mut Log) -> bool {
    let args = [OsStr::new("--assess"), OsStr::new("--type"), OsStr::new("execute"), OsStr::new("--verbose=2"), app.as_os_str()];
    log.run("spctl", &args, QUICK).is_ok_and(|out| out.ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_developer_ids_are_read_off_the_keychain() {
        let text = r#"  1) 0123456789ABCDEF0123456789ABCDEF01234567 "Apple Development: Ada Lovelace (AAAAAAAAAA)"
  2) 89ABCDEF0123456789ABCDEF0123456789ABCDEF "Developer ID Application: Bunny Lab (AB12CD34EF)"
     2 valid identities found"#;
        assert_eq!(developer_ids(text), ["Developer ID Application: Bunny Lab (AB12CD34EF)"]);
        assert!(developer_ids("     0 valid identities found").is_empty());
    }

    #[test]
    fn a_key_goes_in_before_the_plist_closes() {
        let plist = "<plist>\n<dict>\n    <key>A</key>\n    <string>a</string>\n</dict>\n</plist>\n";
        let added = add_key(plist, "CFBundleIconFile", "AppIcon");
        assert!(added.contains("<string>a</string>\n    <key>CFBundleIconFile</key>\n    <string>AppIcon</string>\n</dict>"));
    }
}
