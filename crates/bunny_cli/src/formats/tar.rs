//! Tar archives (POSIX ustar), written: what a Linux app is handed out
//! in, gzipped. Each entry keeps its mode, so the binary and the install
//! script stay executable.

/// One entry: a path inside the archive (`/` between its parts), its
/// mode, and its bytes — `None` for a folder.
pub struct Entry {
    pub path: String,
    pub mode: u32,
    pub bytes: Option<Vec<u8>>,
}

impl Entry {
    pub fn folder(path: &str) -> Entry {
        Entry { path: path.trim_end_matches('/').to_string() + "/", mode: 0o755, bytes: None }
    }

    pub fn file(path: &str, mode: u32, bytes: Vec<u8>) -> Entry {
        Entry { path: path.to_string(), mode, bytes: Some(bytes) }
    }
}

/// The archive of `entries`, in their order.
pub fn write(entries: &[Entry]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for entry in entries {
        let (name, prefix) = split(&entry.path).ok_or_else(|| format!("{} is too long a path for a tar", entry.path))?;
        let size = entry.bytes.as_ref().map_or(0, Vec::len);
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        octal(&mut header[100..108], u64::from(entry.mode));
        octal(&mut header[108..116], 0); // uid
        octal(&mut header[116..124], 0); // gid
        octal(&mut header[124..136], size as u64);
        octal(&mut header[136..148], 0); // mtime: the same files, the same archive
        header[156] = if entry.bytes.is_some() { b'0' } else { b'5' };
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
        // the checksum is taken with its own field as spaces
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
        let digits = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(digits.as_bytes());
        out.extend_from_slice(&header);
        if let Some(bytes) = &entry.bytes {
            out.extend_from_slice(bytes);
            out.resize(out.len().div_ceil(512) * 512, 0);
        }
    }
    // the end: two empty records
    out.resize(out.len() + 1024, 0);
    Ok(out)
}

/// A path as ustar holds it: up to 100 bytes of name, and a prefix of up
/// to 155 before the last `/` that keeps the name short enough.
fn split(path: &str) -> Option<(&str, &str)> {
    if path.len() <= 100 {
        return Some((path, ""));
    }
    let cut = path[..path.len().min(156)].rfind('/')?;
    let (prefix, name) = (&path[..cut], &path[cut + 1..]);
    (name.len() <= 100 && !name.is_empty()).then_some((name, prefix))
}

/// An octal number, zero-padded, ending in NUL — the field's width.
fn octal(field: &mut [u8], value: u64) {
    let digits = format!("{value:0width$o}", width = field.len() - 1);
    field[..digits.len()].copy_from_slice(digits.as_bytes());
    field[digits.len()] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_path_moves_its_folders_to_the_prefix() {
        assert_eq!(split("notes/bin/notes"), Some(("notes/bin/notes", "")));
        let long = format!("{}/share/icons/hicolor/512x512/apps/io.bunny.notes.png", "n".repeat(80));
        let (name, prefix) = split(&long).unwrap();
        assert!(name.len() <= 100 && prefix.len() <= 155);
        assert_eq!(format!("{prefix}/{name}"), long);
        assert_eq!(split(&"x".repeat(101)), None, "a name alone longer than 100 has nowhere to go");
    }

    /// The system's tar lists and extracts it, modes kept, where there is
    /// one.
    #[cfg(unix)]
    #[test]
    fn tar_reads_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("bunny-tar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let long = format!("notes/{}/deep.txt", "folder".repeat(20));
        let archive = write(&[
            Entry::folder("notes"),
            Entry::folder("notes/bin"),
            Entry::file("notes/bin/notes", 0o755, b"#!/bin/sh\necho hi\n".to_vec()),
            Entry::file("notes/README", 0o644, b"read me".repeat(100)),
            Entry::file(&long, 0o644, b"deep".to_vec()),
        ])
        .unwrap();
        let path = dir.join("notes.tar.gz");
        std::fs::write(&path, crate::formats::deflate::gzip(&archive)).unwrap();
        let Ok(out) = std::process::Command::new("tar").arg("-xzf").arg(&path).arg("-C").arg(&dir).output() else { return };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let binary = dir.join("notes/bin/notes");
        assert_eq!(std::fs::read(&binary).unwrap(), b"#!/bin/sh\necho hi\n");
        assert_eq!(std::fs::metadata(&binary).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read(dir.join("notes/README")).unwrap(), b"read me".repeat(100));
        assert_eq!(std::fs::read(dir.join(&long)).unwrap(), b"deep");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
