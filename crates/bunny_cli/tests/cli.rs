//! The `bunny` binary as a person meets it: the help, the mistakes, and
//! the project `bunny new` writes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bunny(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bunny"))
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .output()
        .expect("the bunny binary runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A fresh, empty folder for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bunny-cli-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_version_and_the_help() {
    let here = scratch("help");
    let out = bunny(&["--version"], &here);
    assert!(out.status.success());
    assert_eq!(text(&out.stdout).trim(), format!("bunny {}", env!("CARGO_PKG_VERSION")));
    let out = bunny(&[], &here);
    assert!(text(&out.stdout).contains("new"), "{}", text(&out.stdout));
    let out = bunny(&["help", "new"], &here);
    assert!(text(&out.stdout).contains("--platforms <LIST>"));
    assert!(!text(&out.stdout).contains("bunny-ui-path"), "a hidden option stays out of the help");
    let out = bunny(&["new", "--help"], &here);
    assert!(out.status.success() && text(&out.stdout).contains("Usage: bunny new <PATH>"));
}

#[test]
fn a_mistake_is_exit_two_with_the_way_out() {
    let here = scratch("mistakes");
    let out = bunny(&["nwe"], &here);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("did you mean `bunny new`?"), "{}", text(&out.stderr));
    let out = bunny(&["new"], &here);
    assert_eq!(out.status.code(), Some(2));
    let out = bunny(&["new", "My-App"], &here);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("lowercase"), "{}", text(&out.stderr));
    let out = bunny(&["new", "app", "--org", "com.x", "--id", "com.x.app"], &here);
    assert_eq!(out.status.code(), Some(2));
    let out = bunny(&["new", "app", "--platforms", "android,tvos"], &here);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("android, ios, macos, web"));
    // a folder with someone else's files is never written into
    fs::create_dir_all(here.join("taken")).unwrap();
    fs::write(here.join("taken/notes.txt"), "mine").unwrap();
    let out = bunny(&["new", "taken"], &here);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(fs::read_dir(here.join("taken")).unwrap().count(), 1);
}

#[test]
fn new_writes_a_project_for_every_platform() {
    let here = scratch("new");
    let out = bunny(&["new", "photo_lab", "--org", "io.bunny"], &here);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let app = here.join("photo_lab");
    let manifest = fs::read_to_string(app.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("name = \"photo_lab\""));
    assert!(manifest.contains("name = \"Photo Lab\""));
    assert!(manifest.contains("id = \"io.bunny.photo_lab\""));
    assert!(manifest.contains(&format!("bunny-ui = {{ version = \"{}\" }}", env!("CARGO_PKG_VERSION"))));
    assert!(fs::read_to_string(app.join("src/main.rs")).unwrap().contains("photo_lab::run()"));
    assert!(fs::read_to_string(app.join("src/lib.rs")).unwrap().contains("bunny_ui::app!(home"));
    assert!(app.join(".gitignore").is_file());
    for platform in ["android", "ios", "macos", "web"] {
        let stamp = fs::read_to_string(app.join(platform).join(".bunny-template")).unwrap();
        assert!(stamp.contains(&format!("template {platform} 1")), "{stamp}");
    }
    assert!(app.join("android/.gitignore").is_file());
    assert!(app.join("android/gradle/wrapper/gradle-wrapper.jar").is_file());
    // the platform files keep their markers: every build fills them
    assert!(fs::read_to_string(app.join("ios/Info.plist")).unwrap().contains("@BUNNY_BUNDLE_ID@"));
    assert!(fs::read_to_string(app.join("web/index.html")).unwrap().contains("@BUNNY_SCRIPTS@"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(app.join("android/gradlew")).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "gradlew is executable");
    }
    // the ids each platform spells its way
    assert!(text(&out.stdout).contains("io.bunny.photo-lab"), "{}", text(&out.stdout));
}

#[test]
fn new_in_an_app_adds_only_what_is_missing() {
    let here = scratch("add");
    assert!(bunny(&["new", "notes", "--platforms", "web"], &here).status.success());
    let app = here.join("notes");
    assert!(!app.join("android").exists());
    let page = app.join("web/index.html");
    fs::write(&page, "my own page").unwrap();
    let out = bunny(&["new", ".", "--platforms", "android,web"], &app);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(app.join("android/app/build.gradle.kts").is_file());
    assert_eq!(fs::read_to_string(&page).unwrap(), "my own page", "an existing folder is left alone");
    let out = bunny(&["new", ".", "--platforms", "web"], &app);
    assert!(out.status.success());
    assert!(text(&out.stdout).contains("already has every platform"));
}

#[test]
fn names_with_quotes_stay_valid_toml() {
    let here = scratch("quotes");
    let out = bunny(&["new", "quoted", "--name", "Ada \"Bunny\" \\ Co"], &here);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let manifest = fs::read_to_string(here.join("quoted/Cargo.toml")).unwrap();
    assert!(manifest.contains(r#"name = "Ada \"Bunny\" \\ Co""#), "{manifest}");
}
