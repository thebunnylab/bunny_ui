//! A real disk image, made and mounted by `hdiutil`.

use std::path::Path;
use std::process::Command;

use bunny_ui_release::Installed;

use super::install_dmg;

fn bundle(root: &Path, program: &[u8], extra: Option<&str>) {
    let macos = root.join("Contents").join("MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::write(macos.join("app"), program).unwrap();
    std::fs::write(root.join("Contents").join("Info.plist"), b"<plist/>").unwrap();
    if let Some(extra) = extra {
        std::fs::write(root.join("Contents").join(extra), b"x").unwrap();
    }
}

#[test]
fn a_disk_image_replaces_the_bundle_in_place() {
    let dir = std::env::temp_dir().join(format!("bunny-dmg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let source = dir.join("source");
    bundle(&source.join("App.app"), b"new build", Some("new-resource"));
    std::os::unix::fs::symlink("/Applications", source.join("Applications")).unwrap();
    let dmg = dir.join("App-1.1.0.dmg");
    let made = Command::new("hdiutil")
        .args(["create", "-quiet", "-fs", "HFS+", "-format", "UDZO", "-volname", "App", "-srcfolder"])
        .arg(&source)
        .arg(&dmg)
        .status()
        .unwrap();
    assert!(made.success(), "hdiutil create");
    let installed_at = dir.join("Applications").join("App.app");
    bundle(&installed_at, b"old build", Some("old-resource"));

    assert_eq!(install_dmg(&installed_at, &dmg).unwrap(), Installed::ReadyOnRestart);
    assert_eq!(std::fs::read(installed_at.join("Contents/MacOS/app")).unwrap(), b"new build");
    assert!(installed_at.join("Contents/new-resource").exists());
    assert!(!installed_at.join("Contents/old-resource").exists(), "what the new build dropped goes");
    assert!(!installed_at.join("App.app").exists(), "a replace, not a copy inside");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_that_is_not_a_disk_image_is_refused() {
    let dir = std::env::temp_dir().join(format!("bunny-dmg-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("x.dmg"), b"not an image").unwrap();
    let error = install_dmg(&dir.join("App.app"), &dir.join("x.dmg")).unwrap_err();
    assert!(error.to_string().contains("hdiutil"), "{error}");
    std::fs::remove_dir_all(&dir).unwrap();
}
