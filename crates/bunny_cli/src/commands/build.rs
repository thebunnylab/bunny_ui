//! `bunny build <platform>`: the app as a package to ship, under the
//! project's `build/<platform>/`.

use crate::args::{HELP, Matches, Opt};
use crate::build::{self, Info, Log, Options, Signing};
use crate::commands::doctor;
use crate::devices::Platform;
use crate::error::{Error, Result};
use crate::project::Project;
use crate::term;

pub const SUMMARY: &str = "Build the app to ship: a site for the web, a signed app for macOS";

pub const USAGE: &str = "bunny build <PLATFORM> [OPTIONS]";

pub const ABOUT: &str = "\
Builds the app for release and packs it the way the platform hands it out,
under build/<PLATFORM>/ next to build-info.json (what each file is) and
build.log (every command that made them).

  web     the page as a folder any static host serves: the wasm optimized
          (wasm-opt, when it is installed), the files bunny adds named
          after their content, and a _headers file that caches them
  macos   <Name>.app and <Name>-<version>.dmg, signed with the keychain's
          Developer ID Application identity (or ad hoc, without one) and
          notarized when a notarytool profile or an API key is at hand

The version and the build number come from Cargo.toml: `version`, and
`build` in [package.metadata.bunny]; --build-name and --build-number
override them for one build. iOS, Android, Windows and Linux come next.";

pub const OPTIONS: &[Opt] = &[
    Opt::flag("debug", "Build without optimizations"),
    Opt::value("build-name", "VERSION", "The version people see (default: Cargo.toml's version)"),
    Opt::value("build-number", "N", "The build number (default: `build` in [package.metadata.bunny])"),
    Opt::value("features", "LIST", "Cargo features to turn on (repeatable)"),
    Opt::value("package", "NAME", "The app to build, in a workspace of several").short('p'),
    Opt::value("sign", "IDENTITY", "macOS: the signing identity (default: the keychain's one Developer ID Application, or $BUNNY_MACOS_SIGN)"),
    Opt::flag("no-codesign", "macOS: leave the app unsigned"),
    Opt::flag("notarize", "macOS: notarize the app, or fail"),
    Opt::value("notary-profile", "NAME", "macOS: the notarytool keychain profile (default: $BUNNY_MACOS_NOTARY_PROFILE)"),
    Opt::flag("universal", "macOS: one binary for Apple silicon and Intel"),
    Opt::flag("no-dmg", "macOS: the app alone, without a disk image"),
    HELP,
];

pub fn run(matches: &Matches) -> Result<()> {
    let platform = match matches.positionals.as_slice() {
        [platform] => platform.as_str(),
        [] => return Err(Error::usage("name the platform: bunny build web, or bunny build macos")),
        [_, extra, ..] => return Err(Error::usage(format!("`{extra}`: build one platform at a time"))),
    };
    let checked = match platform {
        "web" => Platform::Web,
        "macos" => Platform::Macos,
        "ios" | "android" | "windows" | "linux" => {
            return Err(Error::new(format!("`bunny build {platform}` comes later"))
                .hint("bunny build web and bunny build macos are here; `bunny run` runs on every platform"));
        }
        other => {
            return Err(Error::usage(format!("`{other}` is not a platform: web, macos, ios, android, windows or linux")));
        }
    };
    let cwd = std::env::current_dir()?;
    let mut project = Project::discover(&cwd, matches.value("package"))?;
    if let Some(version) = matches.value("build-name") {
        project.version = version.to_string();
    }
    if let Some(number) = matches.value("build-number") {
        project.build = number.parse().map_err(|_| Error::usage(format!("`--build-number {number}` is not a whole number")))?;
    }
    let signing = match (matches.flag("no-codesign"), matches.value("sign")) {
        (true, _) => Signing::Unsigned,
        (false, Some(identity)) => Signing::Identity(identity.to_string()),
        (false, None) => std::env::var("BUNNY_MACOS_SIGN").ok().filter(|identity| !identity.is_empty()).map_or(Signing::Auto, Signing::Identity),
    };
    let options = Options {
        release: !matches.flag("debug"),
        features: matches.values("features").iter().flat_map(|list| list.split(',')).map(str::trim).filter(|f| !f.is_empty()).map(String::from).collect(),
        signing,
        notary_profile: matches.value("notary-profile").map(String::from),
        notarize: matches.flag("notarize"),
        universal: matches.flag("universal"),
        dmg: !matches.flag("no-dmg"),
    };
    doctor::preflight(checked)?;
    if checked == Platform::Macos {
        build::macos::check(&project, &options)?;
    }
    println!("{}", term::bold(&format!("Building {} {} ({}) for {platform}…", project.name, project.version, project.build)));
    let out = build::out_dir(&project, platform)?;
    let mut log = Log::default();
    let mut info = Info::new(&project, platform, &options);
    let built = match checked {
        Platform::Web => build::web::build(&project, &options, &out, &mut log, &mut info),
        _ => build::macos::build(&project, &options, &out, &mut log, &mut info),
    };
    // the log is written whatever happened: it is what explains a failure
    log.write(&out.join("build.log"))?;
    built?;
    info.write(&out.join("build-info.json"))?;
    let shown = out.strip_prefix(&cwd).unwrap_or(&out);
    println!("{} {}", term::ok("Built"), term::bold(&shown.display().to_string()));
    Ok(())
}
