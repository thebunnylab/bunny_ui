//! Android: the app's library as a shared object linked by the NDK,
//! packed into an APK by the project's own Gradle, installed with adb
//! and started — its log in this terminal.
//!
//! Gradle reads every name and path from `android/local.properties`,
//! which this writes on each build; the project's Gradle files stay the
//! same for every app. The log is the app's process (by its pid) plus
//! the system's crash reports, which come from other processes — a
//! filter by tag alone hides exactly the crash one wants to see. Debug
//! and verbose lines stay out.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::{Options, Session};
use crate::cargo::{self, Target};
use crate::error::{self, Error, Result};
use crate::ids;
use crate::process;
use crate::project::Project;
use crate::term;
use crate::toolchains::QUICK;
use crate::toolchains::android::{self, Env, Jdk, Ndk};

/// What building and installing needs, found once.
pub struct Toolchain {
    pub sdk: PathBuf,
    pub ndk: Ndk,
    pub jdk: Jdk,
    pub adb: PathBuf,
}

impl Toolchain {
    pub fn find() -> Result<Toolchain> {
        let env = Env::current();
        let setup = "bunny setup android installs it, without Android Studio";
        let sdk = android::sdk_root(&env).ok_or_else(|| Error::new("no Android SDK").hint(setup))?;
        let ndk = android::ndk(&env, Some(&sdk)).ok_or_else(|| Error::new("no Android NDK").hint(setup))?;
        let jdk = android::jdk(&env)
            .filter(|jdk| jdk.version >= android::MIN_JDK)
            .ok_or_else(|| Error::new("no JDK 17 or newer for Gradle").hint(setup))?;
        let adb = android::adb(&sdk);
        Ok(Toolchain { sdk, ndk, jdk, adb })
    }

    pub fn adb(&self, serial: &str, args: &[&str]) -> Result<process::Output> {
        let mut full = vec!["-s", serial];
        full.extend_from_slice(args);
        process::run(&self.adb, &full, Duration::from_secs(120)).map_err(|error| Error::new(format!("adb: {error}")))
    }
}

/// A built app, ready to install.
pub struct Apk {
    pub path: PathBuf,
    pub package: String,
}

/// The device's ABI and the Rust target that builds for it.
pub fn abi(toolchain: &Toolchain, serial: &str) -> Result<(String, &'static str)> {
    let out = toolchain.adb(serial, &["shell", "getprop", "ro.product.cpu.abi"])?;
    let abi = out.stdout.trim().to_string();
    let triple = match abi.as_str() {
        "arm64-v8a" => "aarch64-linux-android",
        "x86_64" => "x86_64-linux-android",
        other => {
            return Err(Error::new(format!("the device's CPU ({other}) is not one bunny-ui builds for"))
                .hint("an arm64-v8a or x86_64 device or emulator"));
        }
    };
    Ok((abi, triple))
}

/// What a hot build adds to the APK (`bunny run`, hot reload): the
/// feature that links the framework's shared library, the salt of the
/// first generation, and the folder of the standard library that shared
/// library links.
pub struct HotBuild<'a> {
    pub feature: &'a str,
    pub salt: u32,
    pub std_libs: &'a Path,
}

/// Builds the shared object for the device and packs the APK.
pub fn build(project: &Project, options: &Options, toolchain: &Toolchain, serial: &str) -> Result<Apk> {
    build_with(project, options, toolchain, serial, None)
}

/// [`build`], or a hot build: the app's library is the first generation
/// — built exactly as the next ones are, so their types are its types —
/// and the APK carries the framework's shared library and the standard
/// library next to it.
pub fn build_with(project: &Project, options: &Options, toolchain: &Toolchain, serial: &str, hot: Option<HotBuild>) -> Result<Apk> {
    let id = project.require_id()?;
    let package = ids::android_id(id);
    let gradle_dir = project.platform_dir("android")?;
    let (abi, triple) = abi(toolchain, serial)?;
    let mut features = options.features.clone();
    let mut rustc_args = page_args();
    if let Some(hot) = &hot {
        features.push(hot.feature.to_string());
        rustc_args.extend([String::from("-C"), format!("metadata=bunny-salt-{}", hot.salt)]);
    }
    let built = cargo::build(&cargo::Build {
        manifest: project.manifest.clone(),
        package: project.package.clone(),
        what: Target::CdylibLib,
        release: options.release,
        profile: None,
        target: Some(triple.to_string()),
        features,
        env: cargo_env(project, toolchain, triple),
        rustc_args,
        quiet: false,
    })?;
    let shared = built.artifact;
    check_entry(toolchain, &shared)?;
    let file = shared.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let lib_name = file.trim_start_matches("lib").trim_end_matches(".so").to_string();

    let jni = project.out_dir("android", "jniLibs", options.release);
    super::fresh_dir(&jni.join(&abi))?;
    super::copy_binary(&shared, &jni.join(&abi).join(&file))?;
    if let Some(hot) = &hot {
        for library in hot_libraries(&shared, hot.std_libs)? {
            let name = library.file_name().unwrap_or_default();
            super::copy_binary(&library, &jni.join(&abi).join(name))?;
        }
    }
    write_properties(&gradle_dir, &[
        ("sdk.dir", toolchain.sdk.display().to_string()),
        ("bunny.applicationId", package.clone()),
        ("bunny.versionName", project.version.clone()),
        ("bunny.versionCode", project.build.to_string()),
        ("bunny.abis", abi.clone()),
        ("bunny.libName", lib_name),
        ("bunny.appLabel", project.name.clone()),
        ("bunny.jniLibsDir", jni.display().to_string()),
    ])?;
    gradle(&gradle_dir, toolchain, &["assembleDebug"], &[])?;
    let path = gradle_dir.join("app/build/outputs/apk/debug/app-debug.apk");
    if !path.is_file() {
        return Err(Error::new(format!("Gradle finished without {}", path.display())));
    }
    Ok(Apk { path, package })
}

/// The environment of every cargo build for the device: who the app is,
/// and the NDK's compiler driver to link with and its archiver for any C
/// a dependency builds — both for this target only.
pub fn cargo_env(project: &Project, toolchain: &Toolchain, triple: &str) -> Vec<(String, std::ffi::OsString)> {
    let clang = toolchain.ndk.clang(triple, android::MIN_SDK);
    let upper = triple.to_uppercase().replace('-', "_");
    let lower = triple.replace('-', "_");
    let mut env = project.build_env();
    env.push((format!("CARGO_TARGET_{upper}_LINKER"), clang.clone().into()));
    env.push((format!("CC_{lower}"), clang.into()));
    env.push((format!("AR_{lower}"), toolchain.ndk.llvm("llvm-ar").into()));
    env
}

/// 16 KB pages, which Android 15 devices may use and Play requires;
/// after `--`, so the flag reaches the app's crate alone and survives
/// RUSTFLAGS.
pub fn page_args() -> Vec<String> {
    vec![String::from("-C"), String::from("link-arg=-Wl,-z,max-page-size=16384")]
}

/// The shared libraries a hot app's library links: the framework's, next
/// to it in cargo's `deps`, and the toolchain's standard library.
fn hot_libraries(shared: &Path, std_libs: &Path) -> Result<Vec<PathBuf>> {
    let framework = shared.parent().map(|dir| dir.join("deps").join("libbunny_ui_dylib.so")).unwrap_or_default();
    if !framework.is_file() {
        return Err(Error::new(format!("the hot build made no {}", framework.display())));
    }
    let mut libraries = vec![framework];
    let entries = std::fs::read_dir(std_libs).map_err(error::at(std_libs))?;
    libraries.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
        path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("libstd-") && name.ends_with(".so"))
    }));
    Ok(libraries)
}

/// The activity's entry is in the shared object, or the system finds
/// nothing to start and closes the app without a word.
pub fn check_entry(toolchain: &Toolchain, shared: &Path) -> Result<()> {
    let nm = toolchain.ndk.llvm("llvm-nm");
    let args = [std::ffi::OsStr::new("-D"), std::ffi::OsStr::new("--defined-only"), shared.as_os_str()];
    let out = process::run(&nm, &args, QUICK)
        .map_err(|error| Error::new(format!("llvm-nm: {error}")))?;
    if !out.stdout.lines().any(|line| line.ends_with(" ANativeActivity_onCreate")) {
        return Err(Error::new("the library exports no ANativeActivity_onCreate: Android has nothing to start")
            .hint("end src/lib.rs with `bunny_ui::app!(home)`"));
    }
    Ok(())
}

/// `android/local.properties`, rewritten only when it changes — Gradle
/// and Android Studio both watch it.
pub fn write_properties(dir: &Path, values: &[(&str, String)]) -> Result<()> {
    let mut text = String::from("# Written by `bunny` on every build, from Cargo.toml. Not yours to edit.\n");
    for (key, value) in values {
        text.push_str(&format!("{key}={}\n", escape_property(value)));
    }
    let path = dir.join("local.properties");
    if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    std::fs::write(&path, text).map_err(error::at(&path))
}

/// A Java properties value: `\`, `:`, `=` and leading spaces escaped,
/// anything past ASCII as `\uXXXX`.
pub fn escape_property(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for (index, c) in value.chars().enumerate() {
        match c {
            '\\' => out.push_str("\\\\"),
            ':' => out.push_str("\\:"),
            '=' => out.push_str("\\="),
            '#' | '!' if index == 0 => {
                out.push('\\');
                out.push(c);
            }
            ' ' if index == 0 => out.push_str("\\ "),
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04X}"));
                }
            }
        }
    }
    out
}

/// Runs the project's Gradle wrapper on `tasks`, with more environment —
/// a release's signing, as `ORG_GRADLE_PROJECT_*` properties Gradle
/// reads and no process list shows.
pub fn gradle(dir: &Path, toolchain: &Toolchain, tasks: &[&str], env: &[(String, String)]) -> Result<()> {
    let task = tasks.join(" ");
    println!("{}", term::dim(&format!("Gradle {task} (its first run downloads Gradle itself)…")));
    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", "gradlew.bat"]);
        command
    } else {
        Command::new(dir.join("gradlew"))
    };
    for (key, value) in env {
        command.env(key, value);
    }
    let status = command
        .args(["--console=plain", "-q"])
        .args(tasks)
        .current_dir(dir)
        .env("JAVA_HOME", &toolchain.jdk.home)
        .env("ANDROID_HOME", &toolchain.sdk)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| Error::new(format!("gradlew: {error}")))?;
    if !status.success() {
        return Err(Error::new(format!("Gradle {task} failed")).hint("the output above says why"));
    }
    Ok(())
}

/// Installs the APK, starts its activity, and follows its log.
pub fn launch(toolchain: &Toolchain, serial: &str, apk: &Apk, options: &Options) -> Result<Option<Box<dyn Session>>> {
    let _ = toolchain.adb(serial, &["shell", "am", "force-stop", &apk.package]);
    let path = apk.path.to_string_lossy();
    let installed = toolchain.adb(serial, &["install", "-r", &path])?;
    if !installed.ok() || installed.stdout.contains("Failure") {
        let why = format!("{}{}", installed.stdout.trim(), installed.stderr.trim());
        let error = Error::new(format!("the device refused the app: {why}"));
        return Err(if why.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE") {
            error.hint(format!("an older copy was signed by another key: adb -s {serial} uninstall {}", apk.package))
        } else {
            error
        });
    }
    // the log from now on: the device's clock, not this machine's
    let since = toolchain
        .adb(serial, &["shell", "date", "+%m-%d\\ %H:%M:%S.000"])
        .map(|out| out.stdout.trim().to_string())
        .unwrap_or_default();
    let mut logcat = None;
    if !options.detach {
        let mut command = Command::new(&toolchain.adb);
        command.args(["-s", serial, "logcat", "-v", "threadtime"]);
        if !since.is_empty() {
            command.args(["-T", &since]);
        }
        logcat = Some(command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
            .map_err(|error| Error::new(format!("adb logcat: {error}")))?);
    }
    let component = format!("{}/android.app.NativeActivity", apk.package);
    let started = toolchain.adb(serial, &["shell", "am", "start", "-W", "-n", &component])?;
    if !started.ok() || started.stdout.contains("Error") {
        return Err(Error::new(format!("the app did not start: {}", started.stdout.trim())));
    }
    let Some(mut logcat) = logcat else { return Ok(None) };
    let pid = wait_for_pid(toolchain, serial, &apk.package);
    let died = Arc::new(AtomicBool::new(false));
    if let Some(output) = logcat.stdout.take() {
        let died = Arc::clone(&died);
        let package = apk.package.clone();
        std::thread::spawn(move || follow(output, pid, &package, &died));
    }
    Ok(Some(Box::new(AndroidSession {
        logcat,
        died,
        adb: toolchain.adb.clone(),
        serial: serial.to_string(),
        package: apk.package.clone(),
    })))
}

/// The app's process id, once the system has started it.
fn wait_for_pid(toolchain: &Toolchain, serial: &str, package: &str) -> Option<String> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if let Ok(out) = toolchain.adb(serial, &["shell", "pidof", package]) {
            let pid = out.stdout.split_whitespace().next().map(String::from);
            if pid.is_some() {
                return pid;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

/// Prints the app's lines and the system's crash reports; notes when the
/// app's process dies.
fn follow(output: impl std::io::Read, pid: Option<String>, package: &str, died: &AtomicBool) {
    for line in BufReader::new(output).lines().map_while(std::result::Result::ok) {
        let Some(entry) = LogLine::parse(&line) else { continue };
        if entry.message.contains(&format!("Process {package} (pid")) && entry.message.contains("has died") {
            died.store(true, Ordering::SeqCst);
        }
        // the app's process, from info up: the framework's debug chatter in
        // the same process (HWUI, EGL, the loader) stays out
        let ours = pid.as_deref() == Some(entry.pid) && matches!(entry.level, "I" | "W" | "E" | "F");
        let crash = matches!(entry.level, "E" | "F")
            && matches!(entry.tag, "AndroidRuntime" | "DEBUG" | "libc" | "crash_dump" | "tombstoned");
        if ours || crash {
            println!("{} {}: {}", entry.level, entry.tag, entry.message);
        }
    }
}

/// One `logcat -v threadtime` line: date, time, pid, tid, level, tag: message.
#[derive(Debug, PartialEq, Eq)]
pub struct LogLine<'a> {
    pub pid: &'a str,
    pub level: &'a str,
    pub tag: &'a str,
    pub message: &'a str,
}

impl<'a> LogLine<'a> {
    pub fn parse(line: &'a str) -> Option<LogLine<'a>> {
        let mut rest = line.trim_start();
        let mut fields = [""; 5];
        for field in &mut fields {
            let end = rest.find(char::is_whitespace)?;
            *field = &rest[..end];
            rest = rest[end..].trim_start();
        }
        let (tag, message) = rest.split_once(':')?;
        Some(LogLine { pid: fields[2], level: fields[4], tag: tag.trim(), message: message.trim_start() })
    }
}

struct AndroidSession {
    logcat: Child,
    died: Arc<AtomicBool>,
    adb: PathBuf,
    serial: String,
    package: String,
}

impl Session for AndroidSession {
    fn wait(&mut self, timeout: Duration) -> Option<Option<i32>> {
        let until = Instant::now() + timeout;
        loop {
            if self.died.load(Ordering::SeqCst) {
                return Some(None);
            }
            if Instant::now() >= until {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn stop(&mut self) {
        let _ = process::run(&self.adb, &["-s", &self.serial, "shell", "am", "force-stop", &self.package], QUICK);
        let _ = self.logcat.kill();
        let _ = self.logcat.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_threadtime_line_reads() {
        let line = "10-10 02:20:01.123  4321  4350 I bunny_ui: window 1080x2400 at 2.625x";
        assert_eq!(
            LogLine::parse(line),
            Some(LogLine { pid: "4321", level: "I", tag: "bunny_ui", message: "window 1080x2400 at 2.625x" })
        );
        let died = "10-10 02:21:00.001   612  1040 I ActivityManager: Process io.bunny.demo_app (pid 4321) has died: fg TOP";
        assert_eq!(LogLine::parse(died).unwrap().tag, "ActivityManager");
        assert_eq!(LogLine::parse("--------- beginning of main"), None);
    }

    #[test]
    fn properties_are_escaped_the_java_way() {
        assert_eq!(escape_property("C:\\Users\\Ada\\sdk"), "C\\:\\\\Users\\\\Ada\\\\sdk");
        assert_eq!(escape_property("Tom & Jerry = 2"), "Tom & Jerry \\= 2");
        assert_eq!(escape_property("Café"), "Caf\\u00E9");
        assert_eq!(escape_property("#1 App"), "\\#1 App");
        assert_eq!(escape_property("🐰"), "\\uD83D\\uDC30");
    }
}
