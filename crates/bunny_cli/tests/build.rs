//! `bunny build` on a project `bunny new` writes: the web's site with
//! its files named after their content; on a Mac, the app bundle, signed
//! ad hoc, and its disk image; and, with `BUNNY_TEST_ANDROID` set on a
//! machine with the Android SDK, NDK and a JDK, the signed App Bundle and
//! APK for arm64-v8a.
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

    if cfg!(target_os = "macos") {
        bunny(&app, &["build", "macos", "--build-name", "1.2.0"]);
        let out = app.join("build/macos");
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
