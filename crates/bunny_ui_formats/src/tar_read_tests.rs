use super::*;
use crate::tar::{Entry, write};

#[test]
fn what_the_writer_wrote_reads_back() {
    let long = format!("{}/share/icons/hicolor/512x512/apps/io.bunny.notes.png", "n".repeat(80));
    let archive = write(&[
        Entry::folder("notes"),
        Entry::file("notes/bin/notes", 0o755, b"\x7fELF binary".to_vec()),
        Entry::file(&long, 0o644, vec![9; 1500]),
        Entry::file("notes/empty", 0o600, Vec::new()),
    ])
    .unwrap();
    let items = read(&archive).unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(items[0], Item { path: "notes/".into(), mode: 0o755, kind: Kind::Folder });
    assert_eq!(items[1], Item { path: "notes/bin/notes".into(), mode: 0o755, kind: Kind::File(b"\x7fELF binary".to_vec()) });
    assert_eq!(items[2].path, long, "the prefix comes back in front of the name");
    assert_eq!(items[2].kind, Kind::File(vec![9; 1500]));
    assert_eq!(items[3].kind, Kind::File(Vec::new()));
}

#[test]
fn a_broken_archive_says_what_broke() {
    let mut archive = write(&[Entry::file("a", 0o644, b"abc".to_vec())]).unwrap();
    archive[0] = b'b';
    assert!(read(&archive).unwrap_err().contains("checksum"));
    let archive = write(&[Entry::file("a", 0o644, vec![1; 2000])]).unwrap();
    assert!(read(&archive[..1024]).unwrap_err().contains("past the end"));
}

#[test]
fn a_hard_link_is_refused() {
    let mut archive = write(&[Entry::file("a", 0o644, Vec::new())]).unwrap();
    archive[156] = b'1';
    // the checksum again, with the new type
    archive[148..156].copy_from_slice(b"        ");
    let sum: u32 = archive[..512].iter().map(|&byte| u32::from(byte)).sum();
    archive[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    assert!(read(&archive).unwrap_err().contains("hard link"));
}

#[test]
fn a_gzip_of_the_writer_and_of_the_system_inflates() {
    let archive = write(&[Entry::file("a/b", 0o644, b"hello ".repeat(5000))]).unwrap();
    let packed = crate::deflate::gzip(&archive);
    assert_eq!(crate::inflate::gunzip(&packed).as_deref(), Some(&archive[..]));
    let mut broken = packed.clone();
    let at = broken.len() - 6;
    broken[at] ^= 1;
    assert_eq!(crate::inflate::gunzip(&broken), None, "a wrong CRC is refused");
    #[cfg(unix)]
    {
        use std::io::Write as _;
        let mut gzip = std::process::Command::new("gzip")
            .args(["-9", "-c"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        gzip.stdin.take().unwrap().write_all(&archive).unwrap();
        let system = gzip.wait_with_output().unwrap().stdout;
        assert_eq!(crate::inflate::gunzip(&system).as_deref(), Some(&archive[..]));
    }
}

/// What the system's tar writes, long names and links too, reads back.
#[cfg(unix)]
#[test]
fn the_systems_tar_reads_back() {
    let dir = std::env::temp_dir().join(format!("bunny-tar-read-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let tree = dir.join("app");
    let deep = tree.join("share").join("x".repeat(90)).join("y".repeat(60));
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(tree.join("app"), b"binary").unwrap();
    std::fs::write(deep.join("icon.png"), vec![3; 700]).unwrap();
    std::os::unix::fs::symlink("app", tree.join("link")).unwrap();
    let archive = dir.join("app.tar.gz");
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&dir)
        .arg("app")
        .status()
        .unwrap();
    assert!(status.success());
    let items = read(&crate::inflate::gunzip(&std::fs::read(&archive).unwrap()).unwrap()).unwrap();
    // a Mac's tar adds `._name` entries for extended attributes: match the
    // whole last part, not a suffix
    let find = |end: &str| {
        items.iter().find(|item| item.path.trim_end_matches('/').rsplit('/').next() == Some(end)).cloned()
    };
    assert_eq!(items.iter().find(|item| item.path == "app/app").unwrap().kind, Kind::File(b"binary".to_vec()));
    let icon = find("icon.png").unwrap();
    assert!(icon.path.len() > 160, "a path past ustar's 100 + 155: {}", icon.path);
    assert_eq!(icon.kind, Kind::File(vec![3; 700]));
    assert_eq!(find("link").unwrap().kind, Kind::Link("app".into()));
    std::fs::remove_dir_all(&dir).unwrap();
}
