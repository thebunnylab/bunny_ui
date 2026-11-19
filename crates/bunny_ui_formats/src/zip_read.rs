//! Zip archives, read: stored and deflated entries, their CRC-32 checked,
//! from the central directory. ZIP64 and encrypted entries are refused.

/// One entry read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The path as the archive spells it, with `\` turned into `/`.
    pub path: String,
    /// The Unix mode, when a Unix tool wrote the archive.
    pub mode: Option<u32>,
    /// The bytes; `None` for a folder.
    pub bytes: Option<Vec<u8>>,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Every entry of an archive, in the order of its central directory.
///
/// # Errors
///
/// What is wrong, when the archive has no end record, an entry is ZIP64,
/// encrypted or of an unknown method, or its bytes do not match its CRC.
pub fn read(archive: &[u8]) -> Result<Vec<Item>, String> {
    // the end record: 22 bytes, then a comment of up to 64 KB
    let floor = archive.len().saturating_sub(22 + 0xFFFF);
    let end = (floor..=archive.len().saturating_sub(22))
        .rev()
        .find(|&at| u32_at(archive, at) == Some(0x0605_4b50))
        .ok_or("no end of central directory: not a zip")?;
    let count = u16_at(archive, end + 10).ok_or("a short end record")? as usize;
    let mut at = u32_at(archive, end + 16).ok_or("a short end record")? as usize;
    if count == 0xFFFF || at == 0xFFFF_FFFF {
        return Err("a ZIP64 archive, which an update does not use".into());
    }
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        let short = || "a short central directory".to_string();
        if u32_at(archive, at) != Some(0x0201_4b50) {
            return Err(format!("no central directory entry at byte {at}"));
        }
        let made_by = u16_at(archive, at + 4).ok_or_else(short)?;
        let flags = u16_at(archive, at + 8).ok_or_else(short)?;
        let method = u16_at(archive, at + 10).ok_or_else(short)?;
        let crc = u32_at(archive, at + 16).ok_or_else(short)?;
        let packed = u32_at(archive, at + 20).ok_or_else(short)?;
        let size = u32_at(archive, at + 24).ok_or_else(short)?;
        let name_len = u16_at(archive, at + 28).ok_or_else(short)? as usize;
        let extra_len = u16_at(archive, at + 30).ok_or_else(short)? as usize;
        let comment_len = u16_at(archive, at + 32).ok_or_else(short)? as usize;
        let external = u32_at(archive, at + 38).ok_or_else(short)?;
        let local = u32_at(archive, at + 42).ok_or_else(short)? as usize;
        let name = archive.get(at + 46..at + 46 + name_len).ok_or_else(short)?;
        let path = String::from_utf8_lossy(name).replace('\\', "/");
        at += 46 + name_len + extra_len + comment_len;
        if packed == 0xFFFF_FFFF || size == 0xFFFF_FFFF || local == 0xFFFF_FFFF {
            return Err(format!("{path} is a ZIP64 entry, which an update does not use"));
        }
        if flags & 1 != 0 {
            return Err(format!("{path} is encrypted"));
        }
        // the Unix mode is the top half of the external attributes
        let mode = (made_by >> 8 == 3).then_some((external >> 16) & 0o7777).filter(|&mode| mode != 0);
        if path.ends_with('/') {
            items.push(Item { path, mode, bytes: None });
            continue;
        }
        if u32_at(archive, local) != Some(0x0403_4b50) {
            return Err(format!("{path} has no local header"));
        }
        let skip = 30
            + u16_at(archive, local + 26).ok_or("a short local header")? as usize
            + u16_at(archive, local + 28).ok_or("a short local header")? as usize;
        let data = archive
            .get(local + skip..local + skip + packed as usize)
            .ok_or_else(|| format!("{path} runs past the end"))?;
        let bytes = match method {
            0 => data.to_vec(),
            8 => crate::inflate::inflate(data).ok_or_else(|| format!("{path} does not inflate"))?,
            other => return Err(format!("{path} uses method {other}, not stored or deflated")),
        };
        if bytes.len() != size as usize || crate::zip::crc32(&bytes) != crc {
            return Err(format!("{path} does not match its CRC-32"));
        }
        items.push(Item { path, mode, bytes: Some(bytes) });
    }
    Ok(items)
}

#[cfg(test)]
#[path = "zip_read_tests.rs"]
mod tests;
