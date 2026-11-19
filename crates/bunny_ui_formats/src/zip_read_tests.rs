use super::*;

#[test]
fn what_the_writer_wrote_reads_back() {
    let files = vec![
        ("App/App.exe".to_string(), b"MZ the program ".repeat(300)),
        ("App/notes.txt".to_string(), b"short".to_vec()),
        ("App/empty".to_string(), Vec::new()),
    ];
    let mut archive = Vec::new();
    crate::zip::write(&mut archive, &files).unwrap();
    let items = read(&archive).unwrap();
    assert_eq!(items.len(), 3);
    for ((name, bytes), item) in files.iter().zip(&items) {
        assert_eq!(&item.path, name);
        assert_eq!(item.bytes.as_ref(), Some(bytes));
    }
}

#[test]
fn a_broken_archive_says_what_broke() {
    assert!(read(b"not a zip at all").unwrap_err().contains("not a zip"));
    let mut archive = Vec::new();
    crate::zip::write(&mut archive, &[("a".to_string(), b"stored".to_vec())]).unwrap();
    // the stored bytes start after the 30-byte header and the name
    archive[31] ^= 0xFF;
    assert!(read(&archive).unwrap_err().contains("CRC-32"));
}

/// What the system's `zip` writes, folders and modes too, reads back.
#[cfg(unix)]
#[test]
fn the_systems_zip_reads_back() {
    if std::process::Command::new("zip").arg("-v").output().is_err() {
        return; // no zip on this machine
    }
    let dir = std::env::temp_dir().join(format!("bunny-zip-read-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("App").join("lib")).unwrap();
    std::fs::write(dir.join("App").join("app"), b"program ".repeat(4000)).unwrap();
    std::fs::write(dir.join("App").join("lib").join("x.dll"), b"x").unwrap();
    let status = std::process::Command::new("zip")
        .current_dir(&dir)
        .args(["-q", "-r", "app.zip", "App"])
        .status()
        .unwrap();
    assert!(status.success());
    let items = read(&std::fs::read(dir.join("app.zip")).unwrap()).unwrap();
    let program = items.iter().find(|item| item.path == "App/app").unwrap();
    assert_eq!(program.bytes.as_deref(), Some(&b"program ".repeat(4000)[..]));
    assert!(program.mode.is_some(), "a Unix zip keeps the mode");
    assert!(items.iter().any(|item| item.path == "App/lib/" && item.bytes.is_none()));
    std::fs::remove_dir_all(&dir).unwrap();
}
