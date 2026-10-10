//! Zip archives, written: each file deflated (RFC 1951), or stored when
//! deflating would not shrink it — what Play Console reads native debug
//! symbols from, what a Windows app is handed out in.

use std::io::Write;

/// The CRC-32 of `bytes` (IEEE 802.3, the one zip uses).
pub fn crc32(bytes: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (index, entry) in table.iter_mut().enumerate() {
            let mut value = index as u32;
            for _ in 0..8 {
                value = if value & 1 == 1 { 0xEDB8_8320 ^ (value >> 1) } else { value >> 1 };
            }
            *entry = value;
        }
        table
    });
    !bytes.iter().fold(!0u32, |crc, byte| table[((crc ^ u32::from(*byte)) & 0xFF) as usize] ^ (crc >> 8))
}

/// A zip with `files` — each a path inside the archive, with `/` between
/// its parts, and its bytes.
pub fn write(out: &mut impl Write, files: &[(String, Vec<u8>)]) -> std::io::Result<()> {
    let mut central = Vec::new();
    let mut offset = 0u32;
    for (name, bytes) in files {
        let crc = crc32(bytes);
        let size = u32::try_from(bytes.len()).map_err(|_| std::io::Error::other(format!("{name} is too big for a zip")))?;
        let deflated = super::deflate::deflate(bytes);
        let (method, data): (u16, &[u8]) = if deflated.len() < bytes.len() { (8, &deflated) } else { (0, bytes) };
        let packed = data.len() as u32;
        let name_len = name.len() as u16;
        // the local header, then the bytes
        let mut local = Vec::with_capacity(30 + name.len());
        local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        local.extend_from_slice(&20u16.to_le_bytes()); // version needed
        local.extend_from_slice(&0x0800u16.to_le_bytes()); // names are UTF-8
        local.extend_from_slice(&method.to_le_bytes());
        local.extend_from_slice(&DOS_TIME.to_le_bytes());
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&packed.to_le_bytes());
        local.extend_from_slice(&size.to_le_bytes());
        local.extend_from_slice(&name_len.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name.as_bytes());
        out.write_all(&local)?;
        out.write_all(data)?;
        // its entry in the central directory
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&DOS_TIME.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&packed.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&[0u8; 6]); // extra, comment, disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
        offset = offset
            .checked_add(local.len() as u32 + packed)
            .ok_or_else(|| std::io::Error::other("the zip outgrew 4 GB"))?;
    }
    out.write_all(&central)?;
    let count = files.len() as u16;
    let mut end = Vec::with_capacity(22);
    end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    end.extend_from_slice(&[0u8; 4]); // disks
    end.extend_from_slice(&count.to_le_bytes());
    end.extend_from_slice(&count.to_le_bytes());
    end.extend_from_slice(&(central.len() as u32).to_le_bytes());
    end.extend_from_slice(&offset.to_le_bytes());
    end.extend_from_slice(&0u16.to_le_bytes()); // comment
    out.write_all(&end)
}

/// 1 January 1980, midnight — the first moment a zip can say. Every
/// entry carries it, so the same files make the same archive.
const DOS_TIME: u32 = (1 << 5 | 1) << 16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checksum_is_zips() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
    }

    #[test]
    fn an_archive_lists_its_files_where_unzip_looks() {
        let mut bytes = Vec::new();
        let files = vec![
            (String::from("arm64-v8a/libapp.so"), b"\x7fELF one".to_vec()),
            (String::from("x86_64/libapp.so"), b"\x7fELF two".to_vec()),
        ];
        write(&mut bytes, &files).unwrap();
        // the end record points at the central directory, which names both
        let end = bytes.len() - 22;
        assert_eq!(&bytes[end..end + 4], &0x0605_4b50u32.to_le_bytes());
        assert_eq!(u16::from_le_bytes([bytes[end + 10], bytes[end + 11]]), 2);
        let central = u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap()) as usize;
        assert_eq!(&bytes[central..central + 4], &0x0201_4b50u32.to_le_bytes());
        let name_len = u16::from_le_bytes([bytes[central + 28], bytes[central + 29]]) as usize;
        assert_eq!(&bytes[central + 46..central + 46 + name_len], b"arm64-v8a/libapp.so");
        // and the first file's bytes follow its local header — stored:
        // eight bytes do not deflate smaller
        assert_eq!(&bytes[30 + 19..30 + 19 + 8], b"\x7fELF one");
    }

    /// The system's unzip opens it and finds every file whole, where
    /// there is one.
    #[cfg(unix)]
    #[test]
    fn unzip_reads_it() {
        let dir = std::env::temp_dir().join(format!("bunny-zip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = b"a deflated file, a deflated file, ".repeat(300);
        let mut bytes = Vec::new();
        write(&mut bytes, &[(String::from("Notes/notes.exe"), big.clone()), (String::from("Notes/tiny.txt"), b"hi".to_vec())]).unwrap();
        let path = dir.join("app.zip");
        std::fs::write(&path, &bytes).unwrap();
        let Ok(out) = std::process::Command::new("unzip").arg("-p").arg(&path).arg("Notes/notes.exe").output() else { return };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(out.stdout, big);
        let tested = std::process::Command::new("unzip").arg("-tq").arg(&path).output().unwrap();
        assert!(tested.status.success());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
