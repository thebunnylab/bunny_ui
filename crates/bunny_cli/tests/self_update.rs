//! An app that `bunny build` makes updates itself, on a Mac.
//!
//! `bunny new` writes the app, and `bunny signer generate` its key pair.
//! The app turns on the `updater` feature, with a feed of files. Version
//! 1.0.0 is built and installed in a folder of the test. Version 1.1.0 is
//! built and published to the feed. Then 1.0.0 runs, and the test waits:
//!
//! 1. the first check, twenty seconds after launch, finds 1.1.0;
//! 2. a click installs it: the download, the hash, the signature, the
//!    disk image mounted and copied over the running bundle;
//! 3. a click restarts it, and the process that comes up is 1.1.0.
//!
//! The clicks come from the app itself: a few lines that the test adds
//! to its code, which click the update's control when it offers a step.
//!
//! Slow — two release builds and a launch — so it runs on request:
//! `cargo test -p bunny-cli --test self_update -- --ignored`.

#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf()
}

fn bunny(dir: &Path, args: &[&str], key: Option<&str>) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bunny"));
    command
        .args(args)
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", repo().join("target/bunny-self-update-e2e"))
        .env("NO_COLOR", "1")
        .env_remove("BUNNY_MACOS_SIGN")
        .env_remove("BUNNY_CHANNEL");
    if let Some(key) = key {
        command.env("BUNNY_SIGNING_KEY", key);
    }
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "bunny {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// What the test adds to the app: it notes which version ran, and clicks
/// the update's control for the person, to install and then to restart.
const DRIVER: &str = r#"
fn drive() {
    let Ok(folder) = std::env::var("BUNNY_SELF_UPDATE_E2E") else { return };
    let _ = std::fs::write(format!("{folder}/ran-{}", env!("CARGO_PKG_VERSION")), "");
    bunny_ui::task::spawn(async move {
        loop {
            bunny_ui::task::sleep(std::time::Duration::from_millis(250)).await;
            match bunny_ui::updater::current() {
                bunny_ui::updater::UpdateState::Available(_) => bunny_ui::updater::click(),
                bunny_ui::updater::UpdateState::ReadyToRestart { .. } => bunny_ui::updater::click(),
                bunny_ui::updater::UpdateState::Failed => {
                    let _ = std::fs::write(format!("{folder}/failed-{}", env!("CARGO_PKG_VERSION")), "");
                    return;
                }
                _ => {}
            }
        }
    })
    .detach();
}
"#;

fn set_version(app: &Path, version: &str) {
    let manifest = app.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    let line = text.lines().find(|line| line.starts_with("version = ")).unwrap().to_owned();
    fs::write(&manifest, text.replacen(&line, &format!("version = \"{version}\""), 1)).unwrap();
}

fn version_of(bundle: &Path) -> String {
    let out = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleShortVersionString"])
        .arg(bundle.join("Contents/Info.plist"))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Ends every process that runs from `bundle`, when the test ends.
struct Stop(PathBuf);

impl Drop for Stop {
    fn drop(&mut self) {
        let _ = Command::new("pkill").arg("-f").arg(self.0.join("Contents/MacOS")).status();
    }
}

#[test]
#[ignore = "two release builds and a launch; run with --ignored"]
fn a_built_app_updates_itself() {
    let here = std::env::temp_dir().join(format!("bunny-self-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&here);
    fs::create_dir_all(&here).unwrap();
    let facade = repo().join("crates/bunny_ui_facade");
    bunny(&here, &["new", "upd8", "--org", "io.bunny", "--bunny-ui-path", &facade.to_string_lossy()], None);
    let app = here.join("upd8");
    bunny(&app, &["signer", "generate"], None);
    let key = fs::read_to_string(app.join("keys/updater.key")).unwrap().trim().to_owned();

    let feed = here.join("feed");
    let manifest = app.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    let line = text.lines().find(|line| line.starts_with("bunny-ui = {")).unwrap().to_owned();
    let text = text.replacen(&line, &line.replacen(" }", ", features = [\"updater\"] }", 1), 1);
    let table = format!(
        "\n[package.metadata.bunny.updater]\nfeed = \"file://{0}/{{channel}}/latest.json\"\npubkeys = [\"keys/updater.pub\"]\nurl = \"file://{0}/{{version}}/{{file}}\"\n",
        feed.display()
    );
    fs::write(&manifest, text + &table).unwrap();
    let lib = app.join("src/lib.rs");
    let code = fs::read_to_string(&lib).unwrap();
    let code = code.replacen("fn home() -> impl View {", "fn home() -> impl View {\n    drive();", 1) + DRIVER;
    fs::write(&lib, code).unwrap();

    // 1.0.0, installed where the test runs it
    set_version(&app, "1.0.0");
    bunny(&app, &["build", "macos"], Some(&key));
    let installed = here.join("Applications/Upd8.app");
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    assert!(Command::new("ditto").arg(app.join("build/macos/Upd8.app")).arg(&installed).status().unwrap().success());

    // 1.1.0, published to the feed
    set_version(&app, "1.1.0");
    bunny(&app, &["build", "macos"], Some(&key));
    let out = app.join("build/macos");
    fs::create_dir_all(feed.join("stable")).unwrap();
    fs::create_dir_all(feed.join("1.1.0")).unwrap();
    for file in ["Upd8-1.1.0.dmg", "Upd8-1.1.0.dmg.sig"] {
        fs::copy(out.join(file), feed.join("1.1.0").join(file)).unwrap();
    }
    fs::copy(out.join("latest.json"), feed.join("stable/latest.json")).unwrap();

    // 1.0.0 runs, and the test waits for 1.1.0 to come up in its place
    let marks = here.join("marks");
    fs::create_dir_all(&marks).unwrap();
    let cache = PathBuf::from(std::env::var("HOME").unwrap()).join("Library/Caches/upd8");
    let _ = fs::remove_dir_all(&cache);
    let _stop = Stop(installed.clone());
    let mut first = Command::new(installed.join("Contents/MacOS/upd8"))
        .env("BUNNY_SELF_UPDATE_E2E", &marks)
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !marks.join("ran-1.1.0").exists() {
        assert!(!marks.join("failed-1.0.0").exists(), "the update failed:\n{}", fs::read_to_string(cache.join("update.log")).unwrap_or_default());
        assert!(start.elapsed() < Duration::from_secs(120), "1.1.0 never came up");
        std::thread::sleep(Duration::from_millis(250));
    }
    assert!(marks.join("ran-1.0.0").exists());
    // 1.0.0 quits after it starts 1.1.0
    assert!(first.wait().unwrap().success());
    assert_eq!(version_of(&installed), "1.1.0", "the bundle in place is the new one");
    let verified = Command::new("codesign").args(["--verify", "--strict", "--deep"]).arg(&installed).status().unwrap();
    assert!(verified.success(), "the bundle's signature holds after the copy");
    let kept: Vec<_> = fs::read_dir(cache.join("updates")).map(|dir| dir.flatten().collect()).unwrap_or_default();
    assert!(kept.is_empty(), "the download goes once it is in place: {kept:?}");
    assert!(!cache.join("update.log").exists(), "nothing failed");
    drop(_stop);
    let _ = fs::remove_dir_all(&here);
    let _ = fs::remove_dir_all(&cache);
}
