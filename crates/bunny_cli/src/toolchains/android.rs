//! The Android SDK, its NDK and the JDK Gradle runs on — found where
//! Android Studio puts them, or where the environment says.
//!
//! Every function takes an [`Env`] instead of reading the process's
//! environment, so the search is tested on a fake tree.

use std::fs;
use std::path::{Path, PathBuf};

use super::QUICK;
use crate::process;

/// The API level the template compiles and targets.
pub const COMPILE_SDK: u32 = 36;
/// The framework's floor: `WindowInsets.getInsets` and `AImageDecoder`.
pub const MIN_SDK: u32 = 30;
/// The JDK Gradle 9 and its Android plugin need.
pub const MIN_JDK: u32 = 17;

/// The environment the search reads: variables and the home folder.
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub vars: Vec<(String, String)>,
    pub home: Option<PathBuf>,
}

impl Env {
    pub fn current() -> Env {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
        Env { vars: std::env::vars().collect(), home }
    }

    fn var(&self, key: &str) -> Option<&str> {
        self.vars.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str()).filter(|v| !v.is_empty())
    }
}

/// The SDK: `ANDROID_HOME`, then `ANDROID_SDK_ROOT`, then where Android
/// Studio installs it on this kind of host.
pub fn sdk_root(env: &Env) -> Option<PathBuf> {
    let named = ["ANDROID_HOME", "ANDROID_SDK_ROOT"].into_iter().filter_map(|key| env.var(key).map(PathBuf::from));
    named.chain(default_sdk(env)).find(|path| path.is_dir())
}

fn default_sdk(env: &Env) -> Option<PathBuf> {
    if cfg!(windows) {
        return env.var("LOCALAPPDATA").map(|local| Path::new(local).join("Android").join("Sdk"));
    }
    let home = env.home.as_ref()?;
    Some(if cfg!(target_os = "macos") { home.join("Library/Android/sdk") } else { home.join("Android/Sdk") })
}

/// A Native Development Kit: its root, its version, and the folder of
/// its compilers for this host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ndk {
    pub root: PathBuf,
    pub version: String,
    pub bin: PathBuf,
}

impl Ndk {
    /// The compiler driver for `triple` at `api` — what cargo links with:
    /// `aarch64-linux-android30-clang`.
    pub fn clang(&self, triple: &str, api: u32) -> PathBuf {
        let suffix = if cfg!(windows) { ".cmd" } else { "" };
        self.bin.join(format!("{triple}{api}-clang{suffix}"))
    }

    /// One of the LLVM tools it ships (`llvm-ar`, `llvm-nm`, `llvm-strip`).
    pub fn llvm(&self, tool: &str) -> PathBuf {
        let suffix = if cfg!(windows) { ".exe" } else { "" };
        self.bin.join(format!("{tool}{suffix}"))
    }
}

/// The NDK: `ANDROID_NDK_HOME`, then `ANDROID_NDK_ROOT`, then the newest
/// side-by-side one under the SDK.
pub fn ndk(env: &Env, sdk: Option<&Path>) -> Option<Ndk> {
    let named = ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT"].into_iter().filter_map(|key| env.var(key).map(PathBuf::from));
    let side_by_side = sdk.and_then(|sdk| newest_version_dir(&sdk.join("ndk")));
    named.chain(side_by_side).find_map(|root| {
        let bin = prebuilt_bin(&root)?;
        let version = ndk_version(&root).unwrap_or_else(|| {
            root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
        });
        Some(Ndk { root, version, bin })
    })
}

/// The compilers' folder for this host. The NDK ships `darwin-x86_64`
/// for every Mac — universal binaries — so Apple silicon finds it too.
fn prebuilt_bin(root: &Path) -> Option<PathBuf> {
    let prebuilt = root.join("toolchains/llvm/prebuilt");
    let tag = if cfg!(target_os = "macos") {
        "darwin-x86_64"
    } else if cfg!(windows) {
        "windows-x86_64"
    } else {
        "linux-x86_64"
    };
    let bin = prebuilt.join(tag).join("bin");
    bin.is_dir().then_some(bin)
}

/// `Pkg.Revision` out of the NDK's `source.properties`.
fn ndk_version(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join("source.properties")).ok()?;
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "Pkg.Revision").then(|| value.trim().to_string())
    })
}

/// The subfolder whose name is the highest version (`27.0.12077973` over
/// `26.3.11579264`), compared number by number.
fn newest_version_dir(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .max_by_key(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().split('.').map(|part| part.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>())
                .unwrap_or_default()
        })
}

fn exe(name: &str, windows_extension: &str) -> String {
    if cfg!(windows) { format!("{name}{windows_extension}") } else { name.to_string() }
}

pub fn adb(sdk: &Path) -> PathBuf {
    sdk.join("platform-tools").join(exe("adb", ".exe"))
}

pub fn emulator(sdk: &Path) -> PathBuf {
    sdk.join("emulator").join(exe("emulator", ".exe"))
}

pub fn sdkmanager(sdk: &Path) -> PathBuf {
    sdk.join("cmdline-tools/latest/bin").join(exe("sdkmanager", ".bat"))
}

/// The platform the template compiles against is installed.
pub fn has_platform(sdk: &Path) -> bool {
    sdk.join("platforms").join(format!("android-{COMPILE_SDK}")).is_dir()
}

/// The SDK's license has been accepted — until it is, Gradle refuses to
/// download what it is missing.
pub fn licenses_accepted(sdk: &Path) -> bool {
    sdk.join("licenses/android-sdk-license").is_file()
}

/// A JDK: its home and its major version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jdk {
    pub home: PathBuf,
    pub version: u32,
}

/// The JDK Gradle will run on: `JAVA_HOME`, then the one bundled with
/// Android Studio, then the system's (`java_home` on macOS, `java` on
/// the PATH). The first that answers is the one.
pub fn jdk(env: &Env) -> Option<Jdk> {
    let java = exe("java", ".exe");
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = env.var("JAVA_HOME") {
        candidates.push(Path::new(home).join("bin").join(&java));
    }
    for studio in studio_jbr(env) {
        candidates.push(studio.join("bin").join(&java));
    }
    if cfg!(target_os = "macos")
        && let Ok(out) = process::run("/usr/libexec/java_home", &["-v", "17+"], QUICK)
        && out.ok()
    {
        candidates.push(Path::new(out.stdout.trim()).join("bin/java"));
    }
    if let Some(on_path) = process::which("java") {
        candidates.push(on_path);
    }
    candidates.iter().filter(|path| path.is_file()).find_map(|path| java_info(path))
}

/// Where Android Studio keeps its bundled runtime on this kind of host.
fn studio_jbr(env: &Env) -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        vec![PathBuf::from("/Applications/Android Studio.app/Contents/jbr/Contents/Home")]
    } else if cfg!(windows) {
        vec![PathBuf::from(r"C:\Program Files\Android\Android Studio\jbr")]
    } else {
        let mut list = vec![PathBuf::from("/opt/android-studio/jbr")];
        if let Some(home) = &env.home {
            list.push(home.join("android-studio/jbr"));
        }
        list
    }
}

/// Asks a `java` its home and version. A macOS stub with no JDK behind
/// it answers with an error, and counts as none.
fn java_info(java: &Path) -> Option<Jdk> {
    let out = process::run(java, &["-XshowSettings:properties", "-version"], QUICK).ok().filter(process::Output::ok)?;
    parse_java_properties(&out.stderr)
}

pub fn parse_java_properties(text: &str) -> Option<Jdk> {
    let field = |name: &str| {
        text.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == name).then(|| value.trim().to_string())
        })
    };
    let home = PathBuf::from(field("java.home")?);
    let spec = field("java.specification.version")?;
    // "1.8" for Java 8, "17" from Java 9 on
    let version = match spec.strip_prefix("1.") {
        Some(old) => old.parse().ok()?,
        None => spec.parse().ok()?,
    };
    Some(Jdk { home, version })
}

/// The AVDs the emulator can boot.
pub fn avds(emulator: &Path) -> Vec<String> {
    match process::run(emulator, &["-list-avds"], QUICK) {
        Ok(out) if out.ok() => parse_avds(&out.stdout),
        _ => Vec::new(),
    }
}

/// AVD names, one per line; the emulator mixes in log lines
/// (`INFO    | …`), which no AVD name looks like.
pub fn parse_avds(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| line.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
        .map(String::from)
        .collect()
}

/// The system image `doctor` suggests: the host's own architecture runs
/// fast, a foreign one crawls.
pub fn suggested_image() -> String {
    let abi = if cfg!(target_arch = "aarch64") { "arm64-v8a" } else { "x86_64" };
    format!("system-images;android-{COMPILE_SDK};google_apis;{abi}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("bunny-android-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn ndk_at(root: &Path, revision: &str) {
        let tag = if cfg!(target_os = "macos") {
            "darwin-x86_64"
        } else if cfg!(windows) {
            "windows-x86_64"
        } else {
            "linux-x86_64"
        };
        fs::create_dir_all(root.join("toolchains/llvm/prebuilt").join(tag).join("bin")).unwrap();
        fs::write(root.join("source.properties"), format!("Pkg.Desc = Android NDK\nPkg.Revision = {revision}\n")).unwrap();
    }

    #[test]
    fn the_sdk_and_the_newest_ndk_are_found() {
        let root = tree("sdk");
        let sdk = root.join("sdk");
        ndk_at(&sdk.join("ndk/26.3.11579264"), "26.3.11579264");
        ndk_at(&sdk.join("ndk/27.0.12077973"), "27.0.12077973");
        // a folder with no compilers for this host is not an NDK
        fs::create_dir_all(sdk.join("ndk/9.9.9")).unwrap();
        let env = Env { vars: vec![(String::from("ANDROID_HOME"), sdk.to_string_lossy().into_owned())], home: None };
        assert_eq!(sdk_root(&env), Some(sdk.clone()));
        let found = ndk(&env, Some(&sdk)).unwrap();
        assert_eq!(found.version, "27.0.12077973");
        assert!(found.clang("aarch64-linux-android", MIN_SDK).ends_with(if cfg!(windows) {
            "aarch64-linux-android30-clang.cmd"
        } else {
            "aarch64-linux-android30-clang"
        }));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_environment_wins_over_the_sdk() {
        let root = tree("env");
        ndk_at(&root.join("pinned"), "28.1.1");
        ndk_at(&root.join("sdk/ndk/29.0.0"), "29.0.0");
        let env = Env {
            vars: vec![(String::from("ANDROID_NDK_HOME"), root.join("pinned").to_string_lossy().into_owned())],
            home: None,
        };
        assert_eq!(ndk(&env, Some(&root.join("sdk"))).unwrap().version, "28.1.1");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_found_is_none() {
        let env = Env { vars: Vec::new(), home: Some(tree("empty")) };
        assert_eq!(sdk_root(&env), None);
        assert_eq!(ndk(&env, None), None);
    }

    #[test]
    fn java_reports_its_home_and_version() {
        let text = "Property settings:\n    java.home = /opt/jbr\n    java.specification.version = 21\n    java.vendor = JetBrains\n";
        assert_eq!(parse_java_properties(text), Some(Jdk { home: PathBuf::from("/opt/jbr"), version: 21 }));
        let old = "    java.home = /usr/lib/jvm/8\n    java.specification.version = 1.8\n";
        assert_eq!(parse_java_properties(old).map(|jdk| jdk.version), Some(8));
    }

    #[test]
    fn avd_names_without_the_log_lines() {
        let text = "INFO    | Storing crashdata in: /tmp/x\nPixel_9_API_36\nbunny-36\n\n";
        assert_eq!(parse_avds(text), vec!["Pixel_9_API_36", "bunny-36"]);
    }
}
