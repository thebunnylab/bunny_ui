//! `bunny build` on a project `bunny new` writes: the web's site with
//! its files named after their content; on a Mac, the app bundle, signed
//! ad hoc, and its disk image; on Windows, the executable with its
//! resources and its zip; on Linux, the tarball with the `.desktop`
//! entry; and, with `BUNNY_TEST_ANDROID` set on a machine with the
//! Android SDK, NDK and a JDK, the signed App Bundle and APK for
//! arm64-v8a.
//!
//! With `[package.metadata.bunny.updater]` and a key pair from `bunny
//! signer generate`, each desktop package also gets its `.sig` and a
//! `latest.json` that the release contract reads, and `bunny updater
//! merge` joins them.
//!
//! Slow — it builds the framework for release — so it runs on request:
//! `cargo test -p bunny-cli --test build -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf()
}

fn bunny(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_bunny"))
        .args(args)
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", repo().join("target/bunny-build-e2e"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "bunny {args:?} failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    stdout
}

/// The `.sig` beside `package` verifies its bytes with the key pair in
/// `app/keys`, and `out/latest.json` lists it for `target`.
fn signed_for_the_updater(app: &Path, out: &Path, package: &str, target: &str) {
    use bunny_ui_formats::{base64, ed25519::VerifyingKey};
    let public: [u8; 32] =
        base64::decode(fs::read_to_string(app.join("keys/updater.pub")).unwrap().trim()).unwrap().try_into().unwrap();
    let signature = fs::read_to_string(out.join(format!("{package}.sig"))).unwrap();
    let raw: [u8; 64] = base64::decode(signature.trim()).unwrap().try_into().unwrap();
    let bytes = fs::read(out.join(package)).unwrap();
    assert!(VerifyingKey::from_bytes(&public).unwrap().verify(&bytes, &raw), "{package}.sig verifies it");
    let text = fs::read_to_string(out.join("latest.json")).unwrap();
    let manifest: bunny_ui_release::ReleaseManifest = serde_json::from_str(&text).unwrap();
    let asset = manifest.asset_for(target.parse().unwrap()).unwrap_or_else(|| panic!("{target} is not in {text}"));
    assert_eq!(asset.signature, signature.trim());
    assert_eq!(asset.size, Some(bytes.len() as u64));
    assert!(asset.url.starts_with("https://dl.example.com/shipped/"), "{text}");
    assert_eq!(serde_json::to_string_pretty(&manifest).unwrap() + "\n", text, "the contract's own bytes");
}

fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" }
}

#[test]
#[ignore = "builds the framework for release; run with --ignored"]
fn a_new_project_builds_its_packages() {
    let here = std::env::temp_dir().join(format!("bunny-build-{}", std::process::id()));
    let _ = fs::remove_dir_all(&here);
    fs::create_dir_all(&here).unwrap();
    let facade = repo().join("crates/bunny_ui_facade");
    bunny(&here, &["new", "shipped", "--org", "io.bunny", "--bunny-ui-path", &facade.to_string_lossy()]);
    let app = here.join("shipped");

    bunny(&app, &["build", "web", "--build-number", "12"]);
    let site = app.join("build/web");
    let info = fs::read_to_string(site.join("build-info.json")).unwrap();
    assert!(info.contains("\"platform\": \"web\"") && info.contains("\"build\": \"12\""), "{info}");
    let page = fs::read_to_string(site.join("index.html")).unwrap();
    let wasm = page.split("window.BUNNY_WASM = \"").nth(1).and_then(|rest| rest.split('"').next()).unwrap();
    assert!(wasm.starts_with("shipped.") && wasm.ends_with(".wasm") && site.join(wasm).is_file(), "{page}");
    assert!(fs::read_to_string(site.join("_headers")).unwrap().contains(&format!("/{wasm}\n")));

    // the updater: a key pair, and the table that names it
    bunny(&app, &["signer", "generate"]);
    assert!(fs::read_to_string(app.join(".gitignore")).unwrap().contains("/keys/updater.key"));
    let mut manifest = fs::read_to_string(app.join("Cargo.toml")).unwrap();
    manifest.push_str(
        "\n[package.metadata.bunny.updater]\nfeed = \"https://dl.example.com/shipped/{channel}/latest.json\"\n\
         pubkeys = [\"keys/updater.pub\"]\nurl = \"https://dl.example.com/shipped/{version}/{file}\"\n",
    );
    fs::write(app.join("Cargo.toml"), manifest).unwrap();
    let mut latest = Vec::new();

    if cfg!(target_os = "macos") {
        bunny(&app, &["build", "macos", "--build-name", "1.2.0", "--signing-key", "keys/updater.key"]);
        let out = app.join("build/macos");
        signed_for_the_updater(&app, &out, "Shipped-1.2.0.dmg", &format!("darwin-{}", host_arch()));
        let bundle = out.join("Shipped.app");
        let plist = fs::read_to_string(bundle.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<string>io.bunny.shipped</string>") && plist.contains("<string>1.2.0</string>"), "{plist}");
        assert!(out.join("Shipped-1.2.0.dmg").is_file());
        let verified = Command::new("codesign").args(["--verify", "--strict", "--deep"]).arg(&bundle).status().unwrap();
        assert!(verified.success(), "the bundle's signature verifies");
        let info = fs::read_to_string(out.join("build-info.json")).unwrap();
        assert!(info.contains("\"signed_by\": \"ad hoc\"") || info.contains("Developer ID"), "{info}");
        assert!(fs::read_to_string(out.join("build.log")).unwrap().contains("$ codesign"));
    }

    if cfg!(windows) {
        bunny(&app, &["build", "windows", "--build-number", "9", "--signing-key", "keys/updater.key"]);
        let out = app.join("build/windows");
        let zip = if cfg!(target_arch = "aarch64") { "Shipped-0.1.0-windows-arm64.zip" } else { "Shipped-0.1.0-windows-x64.zip" };
        signed_for_the_updater(&app, &out, zip, &format!("windows-{}", host_arch()));
        latest.push(out.join("latest.json"));
        assert!(out.join("shipped.exe").is_file() && out.join("Shipped-0.1.0-windows-x64.zip").is_file());
        let info = fs::read_to_string(out.join("build-info.json")).unwrap();
        assert!(info.contains("\"subsystem\": \"windows\""), "a release opens no console: {info}");
        assert!(!info.to_ascii_lowercase().contains("vcruntime"), "the C runtime is linked in: {info}");
        if std::env::var_os("BUNNY_WINDOWS_PFX").is_some() {
            assert!(!info.contains("\"signed_by\": \"nobody\""), "the certificate signed it: {info}");
        }
    }

    if cfg!(target_os = "linux") {
        bunny(&app, &["build", "linux", "--signing-key", "keys/updater.key"]);
        let out = app.join("build/linux");
        signed_for_the_updater(&app, &out, &format!("shipped-0.1.0-linux-{}.tar.gz", host_arch()), &format!("linux-{}", host_arch()));
        latest.push(out.join("latest.json"));
        let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
        let archive = out.join(format!("shipped-0.1.0-linux-{arch}.tar.gz"));
        assert!(out.join("shipped").is_file() && archive.is_file());
        let info = fs::read_to_string(out.join("build-info.json")).unwrap();
        assert!(info.contains("libwayland-client"), "{info}");
        let listed = Command::new("tar").arg("-tzf").arg(&archive).output().unwrap();
        let listed = String::from_utf8_lossy(&listed.stdout);
        for entry in ["shipped-0.1.0/bin/shipped", "shipped-0.1.0/share/applications/io.bunny.shipped.desktop", "shipped-0.1.0/install.sh"] {
            assert!(listed.lines().any(|line| line == entry), "{entry} is not in the archive:\n{listed}");
        }
    }

    // the manifests of one version, joined (macOS built 1.2.0 above)
    if !latest.is_empty() {
        let merged = here.join("latest.json");
        let mut args = vec![String::from("updater"), String::from("merge")];
        args.extend(latest.iter().map(|path| path.to_string_lossy().into_owned()));
        args.extend([String::from("-o"), merged.to_string_lossy().into_owned()]);
        bunny(&app, &args.iter().map(String::as_str).collect::<Vec<_>>());
        let text = fs::read_to_string(&merged).unwrap();
        let manifest: bunny_ui_release::ReleaseManifest = serde_json::from_str(&text).unwrap();
        assert_eq!(manifest.platforms.len(), latest.len(), "{text}");
    }

    if std::env::var_os("BUNNY_TEST_ANDROID").is_some() {
        // a throwaway upload key, the way the build's hint makes one
        let keystore = here.join("upload.jks");
        let keytool = std::env::var_os("JAVA_HOME")
            .map(|home| Path::new(&home).join("bin").join(format!("keytool{}", std::env::consts::EXE_SUFFIX)))
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("keytool"));
        let made = Command::new(keytool)
            .args(["-genkeypair", "-storepass", "test1234", "-keypass", "test1234", "-alias", "upload"])
            .args(["-keyalg", "RSA", "-keysize", "2048", "-validity", "10000", "-dname", "CN=bunny test", "-keystore"])
            .arg(&keystore)
            .output()
            .unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        let properties = format!("storeFile={}\nstorePassword=test1234\nkeyAlias=upload\nkeyPassword=test1234\n", keystore.display());
        fs::write(app.join("android/key.properties"), properties.replace('\\', "/")).unwrap();

        bunny(&app, &["build", "android", "--abi", "arm64-v8a", "--build-number", "5"]);
        let out = app.join("build/android");
        for file in ["shipped-0.1.0.aab", "shipped-0.1.0.apk", "native-debug-symbols.zip"] {
            assert!(out.join(file).is_file(), "{file} is missing");
        }
        let info = fs::read_to_string(out.join("build-info.json")).unwrap();
        assert!(info.contains("versionCode='5'") && info.contains("name='io.bunny.shipped'"), "{info}");
    }
    let _ = fs::remove_dir_all(&here);
}
