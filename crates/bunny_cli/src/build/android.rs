//! `bunny build android`: the app for Google Play and for a direct
//! install — an App Bundle and an APK under `build/android/`, signed with
//! the app's upload key, with the native debug symbols Play Console reads
//! crash reports against.
//!
//! The app's library is built by cargo once per ABI, kept whole for the
//! symbols archive, and stripped for the package; the project's Gradle
//! packs, signs and aligns. The key comes from `android/key.properties`
//! (Flutter's four names: `storeFile`, relative to `android/`,
//! `storePassword`, `keyAlias`, `keyPassword`) or from
//! `BUNNY_ANDROID_KEYSTORE`, `_KEYSTORE_PASSWORD`, `_KEY_ALIAS` and
//! `_KEY_PASSWORD`. Gradle gets it as the properties Android Studio
//! signs with, through the environment — never on a command line.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Info, Log, Options};
use crate::cargo::{self, Target};
use crate::error::{self, Error, Result};
use crate::formats::zip;
use crate::ids;
use crate::platform::android::{self, Toolchain};
use crate::project::Project;
use crate::term;
use crate::toolchains;

const QUICK: Duration = Duration::from_secs(120);

/// Every ABI bunny-ui builds for, and the Rust target of each.
pub const ABIS: &[(&str, &str)] = &[("arm64-v8a", "aarch64-linux-android"), ("x86_64", "x86_64-linux-android")];

/// The upload key a release is signed with.
#[derive(Debug, PartialEq, Eq)]
pub struct Keystore {
    pub file: PathBuf,
    pub store_password: String,
    pub alias: String,
    pub key_password: String,
}

/// What the build needs before it starts — checked before the last
/// package is cleared away.
pub fn check(project: &Project, options: &Options) -> Result<()> {
    project.require_id()?;
    let android = project.platform_dir("android")?;
    Toolchain::find()?;
    let abis = abis(options)?;
    if let Some(rust) = toolchains::rust::detect()
        && let Some((_, missing)) = abis.iter().find(|(_, triple)| !rust.has_target(triple))
    {
        return Err(Error::new(format!("the Rust target {missing} is not installed")).hint(format!("rustup target add {missing}")));
    }
    if options.release {
        let key = keystore(&android)?.ok_or_else(no_key)?;
        if !key.file.is_file() {
            return Err(Error::new(format!("the upload key {} does not exist", key.file.display()))
                .hint("storeFile in android/key.properties is read from the android/ folder"));
        }
    }
    Ok(())
}

/// The ABIs `--abi` names, or every one.
fn abis(options: &Options) -> Result<Vec<(&'static str, &'static str)>> {
    if options.abis.is_empty() {
        return Ok(ABIS.to_vec());
    }
    options
        .abis
        .iter()
        .map(|asked| {
            ABIS.iter().find(|(abi, _)| abi == asked).copied().ok_or_else(|| {
                Error::usage(format!("`{asked}` is not an ABI bunny-ui builds for: arm64-v8a or x86_64"))
            })
        })
        .collect()
}

fn no_key() -> Error {
    Error::new("a release is signed with the app's upload key, and this app has none").hint(
        "make one, and keep it safe — every update is signed with it:\n\
         keytool -genkey -v -keystore ~/upload-keystore.jks -keyalg RSA -keysize 2048 -validity 10000 -alias upload\n\
         then write android/key.properties (keep it out of git):\n\
         storeFile=/Users/you/upload-keystore.jks\n\
         storePassword=…\n\
         keyAlias=upload\n\
         keyPassword=…\n\
         or set BUNNY_ANDROID_KEYSTORE, _KEYSTORE_PASSWORD, _KEY_ALIAS and _KEY_PASSWORD",
    )
}

/// The upload key: `android/key.properties`, else the environment.
pub fn keystore(android: &Path) -> Result<Option<Keystore>> {
    let path = android.join("key.properties");
    if path.is_file() {
        let text = fs::read_to_string(&path).map_err(error::at(&path))?;
        let values = properties(&text);
        let get = |key: &str| values.iter().find(|(name, _)| name == key).map(|(_, value)| value.clone());
        let missing = |key: &str| Error::new(format!("android/key.properties has no {key}"));
        let file = PathBuf::from(get("storeFile").ok_or_else(|| missing("storeFile"))?);
        let store_password = get("storePassword").ok_or_else(|| missing("storePassword"))?;
        return Ok(Some(Keystore {
            file: if file.is_absolute() { file } else { android.join(file) },
            key_password: get("keyPassword").unwrap_or_else(|| store_password.clone()),
            store_password,
            alias: get("keyAlias").ok_or_else(|| missing("keyAlias"))?,
        }));
    }
    let var = |name: &str| std::env::var(format!("BUNNY_ANDROID_{name}")).ok().filter(|value| !value.is_empty());
    let (Some(file), Some(store_password), Some(alias)) = (var("KEYSTORE"), var("KEYSTORE_PASSWORD"), var("KEY_ALIAS")) else {
        return Ok(None);
    };
    Ok(Some(Keystore {
        file: PathBuf::from(file),
        key_password: var("KEY_PASSWORD").unwrap_or_else(|| store_password.clone()),
        store_password,
        alias,
    }))
}

/// A Java properties file's `key=value` lines (`:` works too), comments
/// and blank lines left out, `\` escapes read.
fn properties(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim_start)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('!'))
        .filter_map(|line| {
            let split = line.find(['=', ':'])?;
            let unescape = |text: &str| {
                let mut out = String::new();
                let mut chars = text.chars();
                while let Some(c) = chars.next() {
                    match (c, chars.clone().next()) {
                        ('\\', Some(next)) => {
                            out.push(next);
                            chars.next();
                        }
                        _ => out.push(c),
                    }
                }
                out
            };
            Some((unescape(line[..split].trim()), unescape(line[split + 1..].trim())))
        })
        .collect()
}

/// Builds the App Bundle and the APK into `out`.
pub fn build(project: &Project, options: &Options, out: &Path, log: &mut Log, info: &mut Info) -> Result<()> {
    check(project, options)?;
    let toolchain = Toolchain::find()?;
    let gradle_dir = project.platform_dir("android")?;
    let package = ids::android_id(project.require_id()?);
    let abis = abis(options)?;

    // the library for each ABI: kept whole for the symbols, stripped for
    // the package (Gradle is told to keep what it is given)
    let jni = project.out_dir("android", "jniLibs", options.release);
    crate::platform::fresh_dir(&jni)?;
    let mut symbols = Vec::new();
    let mut lib_name = String::new();
    for (abi, triple) in &abis {
        let built = cargo::build(&cargo::Build {
            manifest: project.manifest.clone(),
            package: project.package.clone(),
            what: Target::CdylibLib,
            release: options.release,
            profile: None,
            target: Some((*triple).to_string()),
            features: options.features.clone(),
            env: android::cargo_env(project, &toolchain, triple),
            rustc_args: android::page_args(),
            quiet: false,
        })?;
        log.note(&format!("cargo built {} for {triple}", built.artifact.display()));
        android::check_entry(&toolchain, &built.artifact)?;
        let file = built.artifact.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        lib_name = file.trim_start_matches("lib").trim_end_matches(".so").to_string();
        let packed = jni.join(abi).join(&file);
        fs::create_dir_all(jni.join(abi)).map_err(error::at(&jni))?;
        let strip = toolchain.ndk.llvm("llvm-strip");
        let args = [OsStr::new("--strip-unneeded"), OsStr::new("-o"), packed.as_os_str(), built.artifact.as_os_str()];
        let stripped = log.run(&strip.to_string_lossy(), &args, QUICK)?;
        if !stripped.ok() {
            return Err(Error::new(format!("llvm-strip failed: {}", stripped.stderr.trim())));
        }
        let whole = fs::read(&built.artifact).map_err(error::at(&built.artifact))?;
        symbols.push((format!("{abi}/{file}"), whole));
    }
    let abi_list: Vec<&str> = abis.iter().map(|(abi, _)| *abi).collect();
    android::write_properties(&gradle_dir, &[
        ("sdk.dir", toolchain.sdk.display().to_string()),
        ("bunny.applicationId", package.clone()),
        ("bunny.versionName", project.version.clone()),
        ("bunny.versionCode", project.build.to_string()),
        ("bunny.abis", abi_list.join(",")),
        ("bunny.libName", lib_name),
        ("bunny.appLabel", project.name.clone()),
        ("bunny.jniLibsDir", jni.display().to_string()),
    ])?;

    // Gradle packs, signs and aligns; a release with the upload key
    let variant = if options.release { "Release" } else { "Debug" };
    let mut env = Vec::new();
    if options.release {
        let key = keystore(&gradle_dir)?.ok_or_else(no_key)?;
        let property = |name: &str, value: String| (format!("ORG_GRADLE_PROJECT_android.injected.signing.{name}"), value);
        env.extend([
            property("store.file", key.file.display().to_string()),
            property("store.password", key.store_password),
            property("key.alias", key.alias.clone()),
            property("key.password", key.key_password),
        ]);
        info.field("key_alias", &key.alias);
    }
    log.note(&format!("gradlew bundle{variant} assemble{variant}"));
    android::gradle(&gradle_dir, &toolchain, &[&format!("bundle{variant}"), &format!("assemble{variant}")], &env)?;
    let lower = variant.to_lowercase();
    let outputs = gradle_dir.join("app/build/outputs");
    let aab = outputs.join("bundle").join(&lower).join(format!("app-{lower}.aab"));
    let apk = outputs.join("apk").join(&lower).join(format!("app-{lower}.apk"));
    let base = format!("{}-{}", project.package, project.version);
    let unsigned = outputs.join("apk").join(&lower).join(format!("app-{lower}-unsigned.apk"));
    if !apk.is_file() && unsigned.is_file() {
        return Err(Error::new("Gradle packed the app without signing it: the upload key did not reach it")
            .hint("build.log lists the build; android/app/build.gradle.kts must not set its own signingConfig for release"));
    }
    for (from, name) in [(&aab, format!("{base}.aab")), (&apk, format!("{base}.apk"))] {
        if !from.is_file() {
            return Err(Error::new(format!("Gradle finished without {}", from.display())));
        }
        fs::copy(from, out.join(&name)).map_err(error::at(from))?;
        info.file(&name);
    }
    if options.release {
        let mut archive = Vec::new();
        zip::write(&mut archive, &symbols).map_err(|error| Error::new(format!("the symbols archive: {error}")))?;
        fs::write(out.join("native-debug-symbols.zip"), archive).map_err(error::at(out))?;
        info.file("native-debug-symbols.zip");
    }
    info.field("abis", &abi_list.join(" "));
    verify(&toolchain, &out.join(format!("{base}.apk")), log, info)?;
    verify_bundle(&toolchain, &out.join(format!("{base}.aab")), log)?;
    Ok(())
}

/// The App Bundle is signed: Gradle names it the same signed or not, and
/// Play refuses an unsigned one. A bundle is a jar to `jarsigner`.
fn verify_bundle(toolchain: &Toolchain, aab: &Path, log: &mut Log) -> Result<()> {
    let jarsigner = toolchain.jdk.home.join("bin").join(if cfg!(windows) { "jarsigner.exe" } else { "jarsigner" });
    if !jarsigner.is_file() {
        log.note("no jarsigner in the JDK: the App Bundle's signature is not checked");
        return Ok(());
    }
    let out = log.run(&jarsigner.to_string_lossy(), &[OsStr::new("-verify"), aab.as_os_str()], QUICK)?;
    if !out.ok() || !out.stdout.contains("jar verified") {
        return Err(Error::new("the App Bundle is not signed").hint("build.log has jarsigner's answer"));
    }
    Ok(())
}

/// The APK checked the way Play checks it: signed, every shared object on
/// a 16 KB page boundary, and the names and numbers it says it has.
fn verify(toolchain: &Toolchain, apk: &Path, log: &mut Log, info: &mut Info) -> Result<()> {
    let Some(tools) = build_tools(&toolchain.sdk) else {
        log.note("no build-tools in the SDK: the APK is not checked");
        return Ok(());
    };
    let java = [("JAVA_HOME", toolchain.jdk.home.as_os_str())];
    let apksigner = tools.join(if cfg!(windows) { "apksigner.bat" } else { "apksigner" });
    let signed = log.run_with(&apksigner.to_string_lossy(), &[OsStr::new("verify"), apk.as_os_str()], &java, QUICK)?;
    if !signed.ok() {
        return Err(Error::new(format!("the APK's signature does not verify: {}", signed.stderr.trim())));
    }
    let zipalign = tools.join(if cfg!(windows) { "zipalign.exe" } else { "zipalign" });
    let args = [OsStr::new("-c"), OsStr::new("-P"), OsStr::new("16"), OsStr::new("4"), apk.as_os_str()];
    let aligned = log.run(&zipalign.to_string_lossy(), &args, QUICK)?;
    if !aligned.ok() {
        return Err(Error::new("the APK's shared objects are not aligned to 16 KB pages, which Play requires")
            .hint("build.log has zipalign's answer"));
    }
    let aapt2 = tools.join(if cfg!(windows) { "aapt2.exe" } else { "aapt2" });
    let badging = log.run(&aapt2.to_string_lossy(), &[OsStr::new("dump"), OsStr::new("badging"), apk.as_os_str()], QUICK)?;
    if let Some(line) = badging.stdout.lines().find(|line| line.starts_with("package:")) {
        info.field("badging", line.trim_start_matches("package:").trim());
    }
    println!("{}", term::ok("Signed, aligned to 16 KB pages"));
    Ok(())
}

/// The newest `build-tools/<version>` of the SDK.
fn build_tools(sdk: &Path) -> Option<PathBuf> {
    let mut versions: Vec<(Vec<u32>, PathBuf)> = fs::read_dir(sdk.join("build-tools"))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            let numbers: Option<Vec<u32>> = name.split('.').map(|part| part.parse().ok()).collect();
            Some((numbers?, path))
        })
        .collect();
    versions.sort();
    versions.pop().map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_properties_file_reads_the_java_way() {
        let text = "# the upload key\nstoreFile=keys/upload.jks\nstorePassword = s3cr=t\n  keyAlias:upload\n! old\nkeyPassword=a\\:b\n";
        let values = properties(text);
        assert_eq!(
            values,
            [
                (String::from("storeFile"), String::from("keys/upload.jks")),
                (String::from("storePassword"), String::from("s3cr=t")),
                (String::from("keyAlias"), String::from("upload")),
                (String::from("keyPassword"), String::from("a:b")),
            ]
        );
    }

    #[test]
    fn the_key_file_is_found_from_the_android_folder() {
        let dir = std::env::temp_dir().join(format!("bunny-key-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("key.properties"), "storeFile=upload.jks\nstorePassword=one\nkeyAlias=upload\n").unwrap();
        let key = keystore(&dir).unwrap().unwrap();
        assert_eq!(key.file, dir.join("upload.jks"));
        assert_eq!(key.key_password, "one", "the key's password defaults to the store's");
        fs::write(dir.join("key.properties"), "storePassword=one\n").unwrap();
        assert!(keystore(&dir).is_err(), "a file without storeFile says so");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_abis_are_named_or_all() {
        let mut options = Options::default();
        assert_eq!(abis(&options).unwrap().len(), 2);
        options.abis = vec![String::from("x86_64")];
        assert_eq!(abis(&options).unwrap(), [("x86_64", "x86_64-linux-android")]);
        options.abis = vec![String::from("armeabi-v7a")];
        assert!(abis(&options).is_err());
    }
}
