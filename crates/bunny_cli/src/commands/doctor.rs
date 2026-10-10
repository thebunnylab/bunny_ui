//! `bunny doctor`: what this machine has for each platform, and the
//! command that fixes what it lacks.
//!
//! Each platform is checked on a thread of its own, every tool with a
//! deadline. A section passes, warns (works, with something to know) or
//! fails (that platform will not build); the exit code is 1 when one
//! fails, so a CI step can gate on it.

use std::process::Command;
use std::thread;

use crate::args::{HELP, Matches, Opt};
use crate::devices::{self, Platform, State};
use crate::error::{Error, Result};
use crate::json;
use crate::term;
use crate::toolchains::{android, apple, linux, rust, windows};

pub const SUMMARY: &str = "Check what this machine needs to build for each platform";

pub const USAGE: &str = "bunny doctor [OPTIONS]";

pub const ABOUT: &str = "\
Checks Rust and each platform this machine can build for — Xcode and its
simulators, the Android SDK, NDK and JDK, the system libraries — and prints the
exact command that fixes each problem. Exits with 1 when a platform cannot
build.";

pub const OPTIONS: &[Opt] = &[
    Opt::value("platform", "NAME", "Check only this platform (repeatable): macos, ios, windows, linux, android, web")
        .short('p'),
    Opt::flag("fix", "Install the missing Rust targets, then check again"),
    Opt::flag("verbose", "Show every check, not only the problems").short('v'),
    Opt::flag("json", "Print the checks as JSON"),
    HELP,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
        }
    }

    fn mark(self) -> String {
        match self {
            Status::Ok => term::green("✓"),
            Status::Warn => term::yellow("!"),
            Status::Fail => term::red("✗"),
        }
    }
}

/// One thing checked.
#[derive(Clone, Debug)]
pub struct Check {
    pub status: Status,
    pub title: String,
    /// The commands (or steps) that fix it.
    pub fixes: Vec<String>,
}

impl Check {
    fn ok(title: impl Into<String>) -> Check {
        Check { status: Status::Ok, title: title.into(), fixes: Vec::new() }
    }

    fn warn(title: impl Into<String>, fixes: &[&str]) -> Check {
        Check { status: Status::Warn, title: title.into(), fixes: fixes.iter().map(|fix| fix.to_string()).collect() }
    }

    fn fail(title: impl Into<String>, fixes: &[&str]) -> Check {
        Check { status: Status::Fail, title: title.into(), fixes: fixes.iter().map(|fix| fix.to_string()).collect() }
    }

    fn fixed(mut self, fix: impl Into<String>) -> Check {
        self.fixes.push(fix.into());
        self
    }
}

/// A platform's checks, or the toolchain's.
#[derive(Clone, Debug)]
pub struct Section {
    pub key: &'static str,
    pub title: String,
    pub checks: Vec<Check>,
    /// Rust targets this section needs and lacks — what `--fix` installs.
    pub missing_targets: Vec<&'static str>,
}

impl Section {
    pub fn status(&self) -> Status {
        self.checks.iter().map(|check| check.status).max().unwrap_or(Status::Ok)
    }
}

pub fn run(matches: &Matches) -> Result<()> {
    let platforms = chosen_platforms(matches)?;
    let mut sections = check(&platforms);
    if matches.flag("fix") {
        let targets: Vec<&str> = sections.iter().flat_map(|section| section.missing_targets.iter().copied()).collect();
        if !targets.is_empty() {
            install_targets(&targets)?;
            sections = check(&platforms);
        }
    }
    if matches.flag("json") {
        println!("{}", to_json(&sections));
    } else {
        print(&sections, matches.flag("verbose"));
    }
    if sections.iter().any(|section| section.status() == Status::Fail) {
        // the report is the message; the exit code is the verdict
        std::process::exit(1);
    }
    Ok(())
}

fn chosen_platforms(matches: &Matches) -> Result<Vec<Platform>> {
    let buildable = Platform::buildable();
    let asked = matches.values("platform");
    if asked.is_empty() {
        return Ok(buildable);
    }
    let mut chosen = Vec::new();
    for name in asked.iter().flat_map(|value| value.split(',')).map(str::trim).filter(|name| !name.is_empty()) {
        let Some(platform) = Platform::from_key(name) else {
            return Err(Error::usage(format!("unknown platform `{name}`"))
                .hint("the platforms are: macos, ios, windows, linux, android, web"));
        };
        if !buildable.contains(&platform) {
            return Err(Error::usage(format!("{} apps do not build on this machine", platform.title())).hint(
                match platform {
                    Platform::Ios | Platform::Macos => "iOS and macOS apps build on a Mac",
                    _ => "a desktop app builds on its own desktop: Windows on Windows, Linux on Linux",
                },
            ));
        }
        if !chosen.contains(&platform) {
            chosen.push(platform);
        }
    }
    Ok(chosen)
}

/// Every section for `platforms`, plus Rust and the devices — checked
/// at once.
pub fn check(platforms: &[Platform]) -> Vec<Section> {
    let toolchain = rust::detect();
    let env = android::Env::current();
    thread::scope(|scope| {
        let toolchain = &toolchain;
        let env = &env;
        let platform_handles: Vec<_> = platforms
            .iter()
            .map(|platform| {
                let platform = *platform;
                scope.spawn(move || match platform {
                    Platform::Macos => macos(),
                    Platform::Ios => ios(toolchain),
                    Platform::Windows => windows_section(),
                    Platform::Linux => linux_section(),
                    Platform::Android => android_section(toolchain, env),
                    Platform::Web => web(toolchain),
                })
            })
            .collect();
        let devices = scope.spawn(devices::discover);
        let mut sections = vec![rust_section(toolchain)];
        sections.extend(platform_handles.into_iter().filter_map(|handle| handle.join().ok()));
        if let Ok(devices) = devices.join() {
            sections.push(devices_section(&devices, platforms));
        }
        sections
    })
}

fn rust_section(toolchain: &Option<rust::Rust>) -> Section {
    let mut section = Section { key: "rust", title: String::from("Rust"), checks: Vec::new(), missing_targets: Vec::new() };
    let Some(toolchain) = toolchain else {
        section.checks.push(Check::fail("No Rust compiler on the PATH", &["install Rust: https://rustup.rs"]));
        return section;
    };
    section.title = format!("Rust {} ({})", toolchain.version, toolchain.host);
    if toolchain.recent_enough() {
        section.checks.push(Check::ok(format!("rustc {} reads edition 2024", toolchain.version)));
    } else {
        section.checks.push(Check::fail(
            format!("rustc {} is older than {}.{}, the first to read edition 2024", toolchain.version, rust::MINIMUM.0, rust::MINIMUM.1),
            &["rustup update stable"],
        ));
    }
    if toolchain.rustup {
        section.checks.push(Check::ok("rustup installs the cross targets"));
    } else {
        section.checks.push(Check::warn(
            "No rustup: the cross targets (iOS, Android, the web) must come with this toolchain",
            &["install rustup: https://rustup.rs"],
        ));
    }
    section
}

/// A check for a Rust target, recording it as missing when it is.
fn target(section: &mut Section, toolchain: &Option<rust::Rust>, target: &'static str, why: &str, required: bool) {
    let Some(toolchain) = toolchain else { return };
    if toolchain.has_target(target) {
        section.checks.push(Check::ok(format!("Rust target {target}")));
        return;
    }
    section.missing_targets.push(target);
    let title = format!("The Rust target {target} is missing ({why})");
    let fix = format!("rustup target add {target}");
    section.checks.push(if required { Check::fail(title, &[&fix]) } else { Check::warn(title, &[&fix]) });
}

fn macos() -> Section {
    let mut section = Section { key: "macos", title: String::from("macOS"), checks: Vec::new(), missing_targets: Vec::new() };
    if apple::has_clang() {
        let dir = apple::developer_dir().unwrap_or_default();
        section.checks.push(Check::ok(format!("Developer tools at {dir}")));
    } else {
        section.checks.push(Check::fail("No Xcode Command Line Tools: nothing links a Mac app", &["xcode-select --install"]));
    }
    section
}

fn ios(toolchain: &Option<rust::Rust>) -> Section {
    let mut section = Section { key: "ios", title: String::from("iOS"), checks: Vec::new(), missing_targets: Vec::new() };
    let Some(sdk) = apple::simulator_sdk() else {
        let dir = apple::developer_dir().unwrap_or_default();
        let check = if std::path::Path::new("/Applications/Xcode.app").is_dir() {
            Check::fail(
                format!("Xcode is installed, but the selected tools are {dir}"),
                &["sudo xcode-select -s /Applications/Xcode.app/Contents/Developer"],
            )
        } else {
            Check::fail("No Xcode: iOS apps build with it", &["install Xcode from the App Store, then open it once"])
        };
        section.checks.push(check);
        return section;
    };
    let xcode = apple::xcode_version().unwrap_or_else(|| String::from("Xcode"));
    section.title = format!("iOS ({xcode}, iOS SDK {sdk})");
    section.checks.push(Check::ok(format!("{xcode} with the iOS {sdk} Simulator SDK")));
    if !apple::license_accepted() {
        section.checks.push(Check::fail("The Xcode license is not accepted", &["sudo xcodebuild -license accept"]));
    }
    let runtimes = apple::ios_runtimes();
    match runtimes.last() {
        Some(runtime) => section.checks.push(Check::ok(format!("iOS {} Simulator runtime", runtime.version))),
        None => section.checks.push(Check::fail("No iOS Simulator runtime", &["xcodebuild -downloadPlatform iOS"])),
    }
    let simulator = if cfg!(target_arch = "aarch64") { "aarch64-apple-ios-sim" } else { "x86_64-apple-ios" };
    target(&mut section, toolchain, simulator, "the Simulator", true);
    target(&mut section, toolchain, "aarch64-apple-ios", "iPhones and iPads", false);
    match apple::signing_identities() {
        0 => section.checks.push(Check::warn(
            "No signing identity: the Simulator needs none, an iPhone does",
            &["sign in to Xcode › Settings › Accounts with your Apple ID"],
        )),
        count => section.checks.push(Check::ok(format!("{count} signing identit{} for iPhones", if count == 1 { "y" } else { "ies" }))),
    }
    section
}

fn android_section(toolchain: &Option<rust::Rust>, env: &android::Env) -> Section {
    let mut section =
        Section { key: "android", title: String::from("Android"), checks: Vec::new(), missing_targets: Vec::new() };
    let Some(sdk) = android::sdk_root(env) else {
        section.checks.push(Check::fail(
            "No Android SDK (looked at ANDROID_HOME, ANDROID_SDK_ROOT and Android Studio's place)",
            &[
                "install Android Studio: https://developer.android.com/studio — its first run installs the SDK",
                "or point ANDROID_HOME at an SDK you already have",
            ],
        ));
        jdk_check(&mut section, env);
        target(&mut section, toolchain, "aarch64-linux-android", "phones and Arm emulators", true);
        return section;
    };
    section.title = format!("Android (SDK at {})", sdk.display());
    let sdkmanager = android::sdkmanager(&sdk);
    let manager = if sdkmanager.is_file() {
        format!("\"{}\"", sdkmanager.display())
    } else {
        String::from("sdkmanager")
    };
    if !sdkmanager.is_file() {
        section.checks.push(Check::warn(
            "No SDK command-line tools: `doctor` can only name the packages, not install them",
            &["Android Studio › Settings › Languages & Frameworks › Android SDK › SDK Tools › Android SDK Command-line Tools"],
        ));
    }
    for (path, title, package) in [
        (android::adb(&sdk), "platform-tools (adb)", "platform-tools"),
        (android::emulator(&sdk), "the emulator", "emulator"),
    ] {
        if path.is_file() {
            section.checks.push(Check::ok(title));
        } else {
            section.checks.push(Check::fail(format!("No {title}"), &[&format!("{manager} \"{package}\"")]));
        }
    }
    if android::has_platform(&sdk) {
        section.checks.push(Check::ok(format!("Android {} platform (API {})", android::COMPILE_SDK, android::COMPILE_SDK)));
    } else {
        section.checks.push(Check::fail(
            format!("No Android API {} platform", android::COMPILE_SDK),
            &[&format!("{manager} \"platforms;android-{}\"", android::COMPILE_SDK)],
        ));
    }
    if !android::licenses_accepted(&sdk) {
        section.checks.push(Check::fail("The SDK licenses are not accepted", &[&format!("{manager} --licenses")]));
    }
    match android::ndk(env, Some(&sdk)) {
        Some(ndk) => {
            let clang = ndk.clang("aarch64-linux-android", android::MIN_SDK);
            if clang.is_file() {
                section.checks.push(Check::ok(format!("NDK {}", ndk.version)));
            } else {
                section.checks.push(Check::fail(
                    format!("NDK {} has no {}", ndk.version, clang.file_name().unwrap_or_default().to_string_lossy()),
                    &["install a current NDK (Side by side) from the SDK Manager"],
                ));
            }
        }
        None => section.checks.push(Check::fail(
            "No NDK: Rust links Android apps with its compilers",
            &[
                &format!("{manager} --list | grep \"ndk;\"   — then install the newest: {manager} \"ndk;<version>\""),
                "or Android Studio › SDK Manager › SDK Tools › NDK (Side by side)",
            ],
        )),
    }
    jdk_check(&mut section, env);
    target(&mut section, toolchain, "aarch64-linux-android", "phones and Arm emulators", true);
    if cfg!(target_arch = "x86_64") {
        target(&mut section, toolchain, "x86_64-linux-android", "x86_64 emulators", false);
    }
    let emulator = android::emulator(&sdk);
    if emulator.is_file() {
        let avds = android::avds(&emulator);
        if avds.is_empty() {
            section.checks.push(Check::warn(
                "No emulator created (a phone with USB debugging works too)",
                &[
                    &format!("{manager} \"{}\"", android::suggested_image()),
                    &format!("avdmanager create avd -n bunny -k \"{}\"", android::suggested_image()),
                ],
            ));
        } else {
            section.checks.push(Check::ok(format!("Emulators: {}", avds.join(", "))));
        }
    }
    section
}

fn jdk_check(section: &mut Section, env: &android::Env) {
    match android::jdk(env) {
        Some(jdk) if jdk.version >= android::MIN_JDK => {
            section.checks.push(Check::ok(format!("JDK {} at {}", jdk.version, jdk.home.display())));
        }
        Some(jdk) => section.checks.push(Check::fail(
            format!("JDK {} at {} is older than the {} Gradle needs", jdk.version, jdk.home.display(), android::MIN_JDK),
            &["point JAVA_HOME at JDK 17 or newer — Android Studio bundles one"],
        )),
        None => section.checks.push(Check::fail(
            "No JDK: Gradle packages the app with it",
            &["Android Studio bundles one; or install JDK 17+ and set JAVA_HOME"],
        )),
    }
}

fn windows_section() -> Section {
    let mut section =
        Section { key: "windows", title: String::from("Windows"), checks: Vec::new(), missing_targets: Vec::new() };
    match windows::msvc() {
        Some(path) => section.checks.push(Check::ok(format!("Visual Studio C++ tools at {}", path.display()))),
        None => section.checks.push(Check::fail(
            "No Visual Studio C++ build tools: Rust links Windows apps with them",
            &["winget install Microsoft.VisualStudio.2022.BuildTools --override \"--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive\""],
        )),
    }
    section
}

fn linux_section() -> Section {
    let mut section = Section { key: "linux", title: String::from("Linux"), checks: Vec::new(), missing_targets: Vec::new() };
    let family = linux::family();
    let missing: Vec<&(&str, &str, &str, &str)> =
        linux::LINKED.iter().filter(|(name, ..)| !linux::has_library(&format!("lib{name}.so"))).collect();
    if missing.is_empty() {
        section.checks.push(Check::ok(format!("The {} libraries the shell links with", linux::LINKED.len())));
    } else {
        let packages: Vec<&str> = missing
            .iter()
            .map(|(_, debian, fedora, arch)| match family {
                Some("fedora") => *fedora,
                Some("arch") => *arch,
                _ => *debian,
            })
            .collect();
        let names: Vec<&str> = missing.iter().map(|(name, ..)| *name).collect();
        section.checks.push(
            Check::fail(format!("Missing development libraries: {}", names.join(", ")), &[])
                .fixed(linux::install_command(family, &packages)),
        );
    }
    let absent: Vec<&(&str, &str)> = linux::LOADED.iter().filter(|(file, _)| !linux::has_library(file)).collect();
    if absent.is_empty() {
        section.checks.push(Check::ok("EGL, wayland-egl and Vulkan for the GPU paths"));
    } else {
        let files: Vec<&str> = absent.iter().map(|(file, _)| *file).collect();
        let packages: Vec<&str> = absent.iter().map(|(_, package)| *package).collect();
        section.checks.push(
            Check::warn(format!("No {}: windows draw on the CPU", files.join(", ")), &[])
                .fixed(linux::install_command(family, &packages)),
        );
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        section.checks.push(Check::warn("No display (neither WAYLAND_DISPLAY nor DISPLAY): a window has nowhere to open", &[]));
    }
    section
}

fn web(toolchain: &Option<rust::Rust>) -> Section {
    let mut section = Section { key: "web", title: String::from("Web"), checks: Vec::new(), missing_targets: Vec::new() };
    target(&mut section, toolchain, "wasm32-unknown-unknown", "the browser", true);
    section
}

fn devices_section(found: &[devices::Device], platforms: &[Platform]) -> Section {
    let ready: Vec<&devices::Device> = found
        .iter()
        .filter(|device| device.state == State::Ready && platforms.contains(&device.platform))
        .collect();
    let bootable = found.iter().filter(|device| device.state == State::Off && platforms.contains(&device.platform)).count();
    let mut section = Section {
        key: "devices",
        title: format!("Devices: {} ready, {} to boot", ready.len(), bootable),
        checks: Vec::new(),
        missing_targets: Vec::new(),
    };
    for device in ready {
        section.checks.push(Check::ok(format!("{} ({})", device.name, device.platform.title())));
    }
    for device in found
        .iter()
        .filter(|device| matches!(device.state, State::Unavailable(_)) && platforms.contains(&device.platform))
    {
        if let State::Unavailable(why) = &device.state {
            section.checks.push(Check::warn(format!("{}: {why}", device.name), &[]));
        }
    }
    section
}

fn install_targets(targets: &[&str]) -> Result<()> {
    println!("{}", term::bold(&format!("rustup target add {}", targets.join(" "))));
    let status = Command::new("rustup")
        .arg("target")
        .arg("add")
        .args(targets)
        .status()
        .map_err(|error| Error::new(format!("rustup: {error}")).hint("install rustup: https://rustup.rs"))?;
    if !status.success() {
        return Err(Error::new("rustup could not add the targets"));
    }
    println!();
    Ok(())
}

fn print(sections: &[Section], verbose: bool) {
    for section in sections {
        let status = section.status();
        println!("[{}] {}", status.mark(), term::bold(&section.title));
        for check in &section.checks {
            if check.status == Status::Ok && !verbose {
                continue;
            }
            println!("    {} {}", check.status.mark(), check.title);
            for fix in &check.fixes {
                println!("      {} {}", term::dim("→"), term::cyan(fix));
            }
        }
    }
    let failing = sections.iter().filter(|section| section.status() == Status::Fail).count();
    let warning = sections.iter().filter(|section| section.status() == Status::Warn).count();
    println!();
    match (failing, warning) {
        (0, 0) => println!("{}", term::ok("Everything is in place.")),
        (0, _) => println!("{}", term::ok("Every platform builds; the notes above are worth a look.")),
        _ => {
            let missing: usize = sections.iter().map(|section| section.missing_targets.len()).sum();
            let tail = if missing > 0 { " `bunny doctor --fix` installs the missing Rust targets." } else { "" };
            println!("{} {failing} section{} cannot build yet.{tail}", Status::Fail.mark(), if failing == 1 { "" } else { "s" });
        }
    }
    if !verbose {
        println!("{}", term::dim("`bunny doctor -v` shows every check."));
    }
}

fn to_json(sections: &[Section]) -> String {
    let items: Vec<String> = sections
        .iter()
        .map(|section| {
            let checks: Vec<String> = section
                .checks
                .iter()
                .map(|check| {
                    let fixes: Vec<String> = check.fixes.iter().map(|fix| json::quote(fix)).collect();
                    format!(
                        "{{\"status\":\"{}\",\"title\":{},\"fixes\":[{}]}}",
                        check.status.word(),
                        json::quote(&check.title),
                        fixes.join(",")
                    )
                })
                .collect();
            format!(
                "{{\"section\":\"{}\",\"title\":{},\"status\":\"{}\",\"checks\":[{}]}}",
                section.key,
                json::quote(&section.title),
                section.status().word(),
                checks.join(",")
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_is_as_bad_as_its_worst_check() {
        let mut section = Section { key: "x", title: String::new(), checks: vec![Check::ok("a")], missing_targets: Vec::new() };
        assert_eq!(section.status(), Status::Ok);
        section.checks.push(Check::warn("b", &[]));
        assert_eq!(section.status(), Status::Warn);
        section.checks.push(Check::fail("c", &["fix it"]));
        assert_eq!(section.status(), Status::Fail);
    }

    #[test]
    fn the_json_is_valid_and_complete() {
        let section = Section {
            key: "ios",
            title: String::from("iOS (Xcode \"27\")"),
            checks: vec![Check::fail("missing", &["rustup target add aarch64-apple-ios-sim"])],
            missing_targets: vec!["aarch64-apple-ios-sim"],
        };
        let value = json::parse(&to_json(&[section])).unwrap();
        let first = &value.as_array()[0];
        assert_eq!(first.str_at(&["status"]), Some("fail"));
        assert_eq!(first.str_at(&["title"]), Some("iOS (Xcode \"27\")"));
        assert_eq!(first.get("checks").unwrap().as_array()[0].get("fixes").unwrap().as_array().len(), 1);
    }
}
