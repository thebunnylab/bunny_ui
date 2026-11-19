use std::path::{Path, PathBuf};

use super::*;

fn folder(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("bunny-unpack-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn zip(at: &Path, files: &[(&str, &[u8])]) {
    let files: Vec<(String, Vec<u8>)> = files.iter().map(|(name, bytes)| ((*name).to_owned(), bytes.to_vec())).collect();
    let mut out = Vec::new();
    bunny_ui_formats::zip::write(&mut out, &files).unwrap();
    std::fs::write(at, out).unwrap();
}

#[test]
fn a_zip_goes_over_the_install_and_leaves_what_it_does_not_have() {
    let dir = folder("zip");
    let install = dir.join("App");
    std::fs::create_dir_all(install.join("lib")).unwrap();
    std::fs::write(install.join("app.exe"), b"old").unwrap();
    std::fs::write(install.join("lib").join("old.dll"), b"old").unwrap();
    let archive = dir.join("update.zip");
    zip(&archive, &[("App/app.exe", b"new program"), ("App/lib/new.dll", b"new"), ("App/tool.exe", b"tool")]);
    install_zip(&install, &archive, "app.exe").unwrap();
    assert_eq!(std::fs::read(install.join("app.exe")).unwrap(), b"new program");
    assert_eq!(std::fs::read(install.join("lib").join("new.dll")).unwrap(), b"new");
    assert_eq!(std::fs::read(install.join("tool.exe")).unwrap(), b"tool");
    assert!(install.join("lib").join("old.dll").exists(), "a file the build dropped stays");
    let leftovers: Vec<_> = walk(&install).into_iter().filter(|path| path.to_string_lossy().ends_with(".bunny-new")).collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_zip_without_the_app_changes_nothing() {
    let dir = folder("zip-no-app");
    std::fs::write(dir.join("app.exe"), b"old").unwrap();
    let archive = dir.join("update.zip");
    zip(&archive, &[("App/other.exe", b"x")]);
    assert!(install_zip(&dir, &archive, "app.exe").is_err());
    assert_eq!(std::fs::read(dir.join("app.exe")).unwrap(), b"old");
    assert!(!dir.join("other.exe").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        }
        out.push(path);
    }
    out
}

#[cfg(unix)]
mod tarballs {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use bunny_ui_formats::tar::Entry;
    use bunny_ui_release::Installed;

    use super::{folder, install_tarball};

    fn tarball(at: &Path, entries: &[Entry]) {
        let archive = bunny_ui_formats::tar::write(entries).unwrap();
        std::fs::write(at, bunny_ui_formats::deflate::gzip(&archive)).unwrap();
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_flat_tree_replaces_its_twins_binary_last() {
        let dir = folder("flat");
        let root = dir.join("opt-app");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::write(root.join("app"), b"old").unwrap();
        std::fs::write(root.join("lib").join("old.so"), b"old").unwrap();
        let archive = dir.join("app.tar.gz");
        tarball(&archive, &[
            Entry::folder("app-1.1.0"),
            Entry::file("app-1.1.0/app", 0o755, b"new".to_vec()),
            Entry::file("app-1.1.0/lib/new.so", 0o644, b"lib".to_vec()),
            Entry::file("app-1.1.0/._app", 0o644, b"mac attributes".to_vec()),
        ]);
        let installed = install_tarball(&root.join("app"), &archive, "t1").unwrap();
        assert_eq!(installed, Installed::RelaunchAt(root.join("app")));
        assert_eq!(std::fs::read(root.join("app")).unwrap(), b"new");
        assert_eq!(mode(&root.join("app")), 0o755);
        assert_eq!(std::fs::read(root.join("lib").join("new.so")).unwrap(), b"lib");
        assert!(!root.join("lib").join("old.so").exists(), "a folder is replaced whole");
        assert!(!root.join("._app").exists(), "a Mac's attribute file is not installed");
        assert!(!root.join(".update-t1").exists(), "the staging folder goes");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bunnys_tarball_replaces_the_installed_binary_alone() {
        let dir = folder("prefix");
        // what `install.sh` made: ~/.local/bin/notes
        let bin = dir.join("local").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("notes"), b"old").unwrap();
        std::fs::write(bin.join("other-tool"), b"not ours").unwrap();
        let archive = dir.join("notes-1.1.0-linux-x86_64.tar.gz");
        tarball(&archive, &[
            Entry::folder("notes-1.1.0"),
            Entry::file("notes-1.1.0/bin/notes", 0o755, b"new notes".to_vec()),
            Entry::file("notes-1.1.0/share/applications/io.notes.desktop", 0o644, b"[Desktop Entry]".to_vec()),
            Entry::file("notes-1.1.0/install.sh", 0o755, b"#!/bin/sh".to_vec()),
        ]);
        let installed = install_tarball(&bin.join("notes"), &archive, "t2").unwrap();
        assert_eq!(installed, Installed::RelaunchAt(bin.join("notes")));
        assert_eq!(std::fs::read(bin.join("notes")).unwrap(), b"new notes");
        assert_eq!(mode(&bin.join("notes")), 0o755);
        assert_eq!(std::fs::read(bin.join("other-tool")).unwrap(), b"not ours", "nothing else in bin changes");
        assert!(!bin.join("install.sh").exists() && !bin.join("share").exists());
        let names: Vec<_> = std::fs::read_dir(&bin).unwrap().flatten().map(|entry| entry.file_name()).collect();
        assert_eq!(names.len(), 2, "no file is left beside the binary: {names:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_tarball_without_the_binary_changes_nothing() {
        let dir = folder("no-binary");
        std::fs::write(dir.join("app"), b"old").unwrap();
        let archive = dir.join("x.tar.gz");
        tarball(&archive, &[Entry::file("x/other", 0o755, b"x".to_vec())]);
        let error = install_tarball(&dir.join("app"), &archive, "t3").unwrap_err();
        assert!(error.to_string().contains("no app"), "{error}");
        assert_eq!(std::fs::read(dir.join("app")).unwrap(), b"old");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_entry_that_escapes_is_refused_and_the_staging_goes() {
        let dir = folder("escape");
        let root = dir.join("app-root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("app"), b"old").unwrap();
        let archive = dir.join("x.tar.gz");
        tarball(&archive, &[Entry::file("app", 0o755, b"new".to_vec()), Entry::file("lib/../../evil", 0o644, b"x".to_vec())]);
        let error = install_tarball(&root.join("app"), &archive, "t4").unwrap_err();
        assert!(error.to_string().contains("leaves the install folder"), "{error}");
        assert_eq!(std::fs::read(root.join("app")).unwrap(), b"old");
        assert!(!dir.join("evil").exists());
        assert!(!root.join(".update-t4").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_is_not_gzip_is_refused() {
        let dir = folder("not-gzip");
        std::fs::write(dir.join("x.tar.gz"), b"plain").unwrap();
        assert!(install_tarball(&dir.join("app"), &dir.join("x.tar.gz"), "t5").unwrap_err().to_string().contains("gzip"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
