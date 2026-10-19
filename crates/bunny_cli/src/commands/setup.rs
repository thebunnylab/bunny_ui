//! `bunny setup android`: the Android toolchain without Android Studio.
//!
//! Google ships the Android CLI — one native binary, `android`, the
//! successor of `sdkmanager` and `avdmanager` — which installs SDK
//! packages and makes emulators. `bunny` fetches it from Google, has it
//! install the platform tools, the emulator, the platform, the NDK and a
//! system image, makes an emulator, and adds a JDK for Gradle (Temurin,
//! checked against Adoptium's checksum).
//!
//! The SDK's terms are Google's and the person's to accept: the Android
//! CLI installs without asking, so `bunny` asks first, and installs
//! nothing until the answer is yes. Google's usage metrics stay off.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::args::{HELP, Matches, Opt};
use crate::error::{self, Error, Result};
use crate::formats::sha::{self, Sha1, Sha256};
use crate::net;
use crate::process;
use crate::term;
use crate::toolchains::android::{self, Env, Jdk};
use crate::toolchains::android_packages::{self as packages, Archive};
use crate::toolchains::{QUICK, rust};

pub const SUMMARY: &str = "Install a platform's toolchain — Android without Android Studio";

pub const USAGE: &str = "bunny setup android [OPTIONS]";

pub const ABOUT: &str = "\
Installs what building and running Android apps needs, without Android Studio:
Google's Android CLI under ~/.bunny/bin, then — through it — the platform tools,
the emulator, the Android platform, the NDK and a system image, an emulator to
run on, and a JDK for Gradle (Temurin) under ~/.bunny/jdk.

The Android SDK's terms are shown for you to accept before anything is
installed; Google's usage metrics are turned off.";

pub const OPTIONS: &[Opt] = &[
    Opt::value("sdk", "DIR", "Where the SDK goes (default: ANDROID_HOME, or the usual place)"),
    Opt::flag("yes", "Download without asking first").short('y'),
    Opt::flag("accept-android-terms", "Accept the Android SDK's terms (https://developer.android.com/studio/terms) — for CI"),
    HELP,
];

/// The terms every Android SDK package is under.
const TERMS: &str = "https://developer.android.com/studio/terms";

/// What `bunny setup android` makes, and where.
struct Plan {
    sdk: PathBuf,
    home: PathBuf,
    /// Google's Android CLI, when one is already here.
    cli: Option<PathBuf>,
    jdk: Option<Jdk>,
    avd: bool,
    targets: Vec<&'static str>,
}

pub fn run(matches: &Matches) -> Result<()> {
    match matches.positionals.as_slice() {
        [platform] if platform == "android" => {}
        [platform] => {
            return Err(Error::usage(format!("`bunny setup {platform}` is not a thing yet"))
                .hint("`bunny setup android` installs the Android toolchain; `bunny doctor` says what the others need"));
        }
        _ => return Err(Error::usage("which platform?").hint("bunny setup android")),
    }
    let env = Env::current();
    let sdk = matches
        .value("sdk")
        .map(PathBuf::from)
        .or_else(|| android::sdk_root(&env))
        .or_else(|| android::default_sdk(&env))
        .ok_or_else(|| Error::new("no home folder to install into").hint("pass --sdk <DIR>"))?;
    let home = android::bunny_home(&env).ok_or_else(|| Error::new("no home folder for ~/.bunny"))?;
    let cli_url = packages::cli_url()
        .ok_or_else(|| Error::new("Google ships no Android CLI for this machine (Linux and Windows on Arm)"))?;
    let plan = Plan {
        cli: find_cli(&home),
        jdk: android::jdk(&env).filter(|jdk| jdk.version >= android::MIN_JDK),
        avd: !android::avds(&android::emulator(&sdk)).is_empty(),
        targets: missing_targets(),
        sdk,
        home,
    };
    show(&plan);

    // the terms first: nothing is fetched for someone who does not take them
    let interactive = std::io::stdin().is_terminal();
    if !matches.flag("accept-android-terms") {
        if !interactive {
            return Err(Error::new("nothing installed: the Android SDK's terms need an answer")
                .hint(format!("read {TERMS}, then pass --accept-android-terms")));
        }
        println!();
        println!("The Android SDK is Google's, under its terms: {}", term::bold(TERMS));
        if !term::confirm("Do you accept them? [y/N] ", false) {
            println!("{}", term::dim("nothing installed"));
            return Ok(());
        }
    }
    if !matches.flag("yes") {
        if !interactive {
            return Err(Error::new("nothing installed: confirm with --yes when no one is at the terminal"));
        }
        if !term::confirm("Download and install? [Y/n] ", true) {
            println!("{}", term::dim("nothing installed"));
            return Ok(());
        }
    }

    let cli = match plan.cli.clone() {
        Some(cli) => cli,
        None => install_cli(&cli_url, &plan.home)?,
    };
    let jdk = match plan.jdk.clone() {
        Some(jdk) => jdk,
        None => install_jdk(&plan.home)?,
    };
    let android = Android { cli, sdk: plan.sdk.clone() };

    step("Finding the current packages");
    let listed = android.capture(&["sdk", "list", "--all"])?;
    let available = packages::listed_packages(&listed);
    let ndk = packages::newest_listed_ndk(&available).ok_or_else(|| {
        Error::new("the Android CLI lists no NDK").hint("`android sdk list --all` shows what it offers")
    })?;
    let separator = if ndk.contains(';') { ";" } else { "/" };
    let platform = format!("platforms{separator}android-{}", android::COMPILE_SDK);
    // the system image is `emulator create`'s to choose: it installs the one
    // its device profile runs on
    let install = vec!["sdk", "install", "platform-tools", "emulator", platform.as_str(), ndk.as_str()];
    step(&format!("Installing {}", install[2..].join(", ")));
    android.run(&install)?;
    if !plan.avd {
        step("An emulator (the medium_phone profile)");
        android.run(&["emulator", "create"])?;
    }
    if !plan.targets.is_empty() {
        step(&format!("rustup target add {}", plan.targets.join(" ")));
        let status = Command::new("rustup").arg("target").arg("add").args(&plan.targets).status();
        if !status.is_ok_and(|status| status.success()) {
            println!("{}", term::warn("rustup could not add the targets; `bunny doctor --fix` tries again"));
        }
    }
    let _ = jdk;
    println!();
    println!("{}", term::ok(&format!("Android is ready, without Android Studio (SDK at {})", plan.sdk.display())));
    if android::sdk_root(&Env::current()).as_deref() != Some(plan.sdk.as_path()) {
        println!("{}", term::warn(&format!("set ANDROID_HOME={} so every tool finds this SDK", plan.sdk.display())));
    }
    println!("    bunny run -d android   {}", term::dim("boots the emulator and runs the app"));
    Ok(())
}

/// Google's Android CLI on one SDK, metrics off.
struct Android {
    cli: PathBuf,
    sdk: PathBuf,
}

impl Android {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.cli);
        command.arg(format!("--sdk={}", self.sdk.display())).arg("--no-metrics").args(args);
        command
    }

    /// Runs it with its output on this terminal.
    fn run(&self, args: &[&str]) -> Result<()> {
        let status = self.command(args).status().map_err(|error| Error::new(format!("android: {error}")))?;
        if !status.success() {
            return Err(Error::new(format!("`android {}` failed", args.join(" "))));
        }
        Ok(())
    }

    /// Runs it and answers what it printed.
    fn capture(&self, args: &[&str]) -> Result<String> {
        let out = self.command(args).output().map_err(|error| Error::new(format!("android: {error}")))?;
        if !out.status.success() {
            return Err(Error::new(format!(
                "`android {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// The Android CLI `bunny` installed, or one on the PATH that answers.
pub fn find_cli(home: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "android.exe" } else { "android" };
    let ours = home.join("bin").join(name);
    if process::is_executable(&ours) {
        return Some(ours);
    }
    let on_path = process::which("android")?;
    let answers = process::run(&on_path, &["--version"], QUICK).is_ok_and(|out| out.ok());
    answers.then_some(on_path)
}

fn missing_targets() -> Vec<&'static str> {
    let Some(toolchain) = rust::detect() else { return Vec::new() };
    let mut wanted = vec!["aarch64-linux-android"];
    if cfg!(target_arch = "x86_64") {
        wanted.push("x86_64-linux-android");
    }
    wanted.into_iter().filter(|target| !toolchain.has_target(target)).collect()
}

fn show(plan: &Plan) {
    println!("{}", term::bold(&format!("The Android toolchain, into {}:", plan.sdk.display())));
    let item = |done: bool, text: String| {
        if done {
            println!("    {}", term::ok(&term::dim(&text)));
        } else {
            println!("    • {text}");
        }
    };
    item(
        plan.cli.is_some(),
        match &plan.cli {
            Some(cli) => format!("Google's Android CLI at {}", cli.display()),
            None => format!("Google's Android CLI, into {}", plan.home.join("bin").display()),
        },
    );
    item(
        plan.jdk.is_some(),
        match &plan.jdk {
            Some(jdk) => format!("JDK {} at {}", jdk.version, jdk.home.display()),
            None => format!("JDK {} (Temurin), about 200 MB, into {}", packages::JDK_MAJOR, plan.home.join("jdk").display()),
        },
    );
    item(false, format!("platform-tools, emulator, Android {} platform, the newest NDK — about 2 GB", android::COMPILE_SDK));
    item(plan.avd, String::from("an emulator, with its system image — about 1.5 GB"));
    if !plan.targets.is_empty() {
        item(false, format!("Rust targets: {}", plan.targets.join(", ")));
    }
}

fn step(title: &str) {
    println!();
    println!("{}", term::bold(title));
}

/// Downloads `archive` into the cache and checks it; a file already
/// there with the right checksum is not fetched again.
fn fetch_checked(archive: &Archive, cache: &Path) -> Result<PathBuf> {
    let file = cache.join(&archive.name);
    if file.is_file() && verify(&file, &archive.checksum)? {
        return Ok(file);
    }
    net::download(&archive.url, &file)?;
    if !verify(&file, &archive.checksum)? {
        let _ = std::fs::remove_file(&file);
        return Err(Error::new(format!("{} does not match its publisher's checksum; it was deleted", archive.name))
            .hint("run it again — a download cut short, or a proxy that rewrote it"));
    }
    Ok(file)
}

fn verify(file: &Path, checksum: &str) -> Result<bool> {
    let (kind, expected) = checksum.split_once(':').unwrap_or(("", checksum));
    let actual = match kind {
        "sha1" => sha::file::<Sha1>(file),
        "sha256" => sha::file::<Sha256>(file),
        _ => return Ok(false),
    }
    .map_err(error::at(file))?;
    Ok(actual.eq_ignore_ascii_case(expected))
}

/// Google's Android CLI, into `~/.bunny/bin` — fetched over HTTPS from
/// Google's own host, the way its install script does.
fn install_cli(url: &str, home: &Path) -> Result<PathBuf> {
    step("Google's Android CLI");
    let name = if cfg!(windows) { "android.exe" } else { "android" };
    let path = home.join("bin").join(name);
    net::download(url, &path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).map_err(error::at(&path))?;
    }
    let answers = process::run(&path, &["--no-metrics", "--version"], QUICK).is_ok_and(|out| out.ok());
    if !answers {
        return Err(Error::new(format!("{} was downloaded but does not run", path.display())));
    }
    Ok(path)
}

fn install_jdk(home: &Path) -> Result<Jdk> {
    step(&format!("JDK {} (Temurin), for Gradle", packages::JDK_MAJOR));
    let answer = net::fetch(&packages::temurin_query())?;
    let archive = packages::temurin(&answer).ok_or_else(|| Error::new("Adoptium named no JDK for this machine"))?;
    let file = fetch_checked(&archive, &home.join("downloads"))?;
    let dest = home.join("jdk");
    std::fs::create_dir_all(&dest).map_err(error::at(&dest))?;
    let flag = if archive.name.ends_with(".tar.gz") { "-xzf" } else { "-xf" };
    let status = Command::new("tar").arg(flag).arg(&file).arg("-C").arg(&dest).status();
    if !status.is_ok_and(|status| status.success()) {
        return Err(Error::new(format!("{} could not be unpacked", file.display())));
    }
    android::jdk(&Env::current())
        .filter(|jdk| jdk.version >= android::MIN_JDK)
        .ok_or_else(|| Error::new("the JDK was unpacked but does not answer"))
}
