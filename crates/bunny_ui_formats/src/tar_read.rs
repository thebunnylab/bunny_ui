//! Tar archives, read: ustar, the GNU long name and the pax path, which
//! cover what `bunny build`, GNU tar, bsdtar and the Rust `tar` crate
//! write.

/// What one entry is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    File(Vec<u8>),
    Folder,
    /// A symbolic link, and the path it points to.
    Link(String),
}

/// One entry read back: its path as the archive spells it, its mode and
/// what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    pub mode: u32,
    pub kind: Kind,
}

/// Every entry of an archive (not gzipped), in its order.
///
/// # Errors
///
/// What is wrong, when a header is malformed, its checksum is wrong, an
/// entry runs past the end, or an entry is a hard link or a device.
pub fn read(archive: &[u8]) -> Result<Vec<Item>, String> {
    let mut items = Vec::new();
    let mut at = 0;
    // the path that a GNU `L` entry or a pax `x` entry gives the next one
    let (mut long_path, mut long_link): (Option<String>, Option<String>) = (None, None);
    while at + 512 <= archive.len() {
        let header = &archive[at..at + 512];
        if header.iter().all(|&byte| byte == 0) {
            break;
        }
        check_sum(header).map_err(|why| format!("the header at byte {at}: {why}"))?;
        let size = number(&header[124..136]).ok_or_else(|| format!("the header at byte {at} has no size"))?;
        let size = usize::try_from(size).map_err(|_| "an entry is too big".to_string())?;
        let data_at = at + 512;
        let data = archive
            .get(data_at..data_at.checked_add(size).ok_or("an entry is too big")?)
            .ok_or_else(|| format!("the entry at byte {at} runs past the end"))?;
        at = data_at + size.div_ceil(512) * 512;
        let flag = header[156];
        match flag {
            b'L' => {
                long_path = Some(text(data));
                continue;
            }
            b'K' => {
                long_link = Some(text(data));
                continue;
            }
            b'x' => {
                for (key, value) in pax(data) {
                    match key.as_str() {
                        "path" => long_path = Some(value),
                        "linkpath" => long_link = Some(value),
                        _ => {}
                    }
                }
                continue;
            }
            // a global pax header holds nothing a path needs
            b'g' => continue,
            _ => {}
        }
        let path = long_path.take().unwrap_or_else(|| {
            let name = text(&header[..100]);
            let prefix = if &header[257..262] == b"ustar" { text(&header[345..500]) } else { String::new() };
            if prefix.is_empty() { name } else { format!("{prefix}/{name}") }
        });
        let link = long_link.take().unwrap_or_else(|| text(&header[157..257]));
        let mode = number(&header[100..108]).unwrap_or(0o644) as u32 & 0o7777;
        let kind = match flag {
            b'0' | 0 | b'7' => {
                if path.ends_with('/') {
                    Kind::Folder
                } else {
                    Kind::File(data.to_vec())
                }
            }
            b'5' => Kind::Folder,
            b'2' => Kind::Link(link),
            b'1' => return Err(format!("{path} is a hard link, which an update does not carry")),
            other => return Err(format!("{path} is an entry of type {:?}, which an update does not carry", char::from(other))),
        };
        items.push(Item { path, mode, kind });
    }
    Ok(items)
}

/// A NUL-ended field as text.
fn text(field: &[u8]) -> String {
    let end = field.iter().position(|&byte| byte == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// An octal field, or the base-256 form that GNU tar writes for a big
/// number (the top bit of the first byte set).
fn number(field: &[u8]) -> Option<u64> {
    if field.first().is_some_and(|&byte| byte & 0x80 != 0) {
        let mut value: u64 = u64::from(field[0] & 0x7F);
        for &byte in &field[1..] {
            value = value.checked_mul(256)?.checked_add(u64::from(byte))?;
        }
        return Some(value);
    }
    let digits: String =
        field.iter().take_while(|&&byte| byte != 0).map(|&byte| char::from(byte)).collect::<String>().trim().to_owned();
    if digits.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(&digits, 8).ok()
}

/// The header's checksum: its bytes summed, with the field as spaces.
fn check_sum(header: &[u8]) -> Result<(), String> {
    let want = number(&header[148..156]).ok_or("the checksum is not a number")?;
    let sum: u64 =
        header.iter().enumerate().map(|(i, &byte)| u64::from(if (148..156).contains(&i) { b' ' } else { byte })).sum();
    if sum == want { Ok(()) } else { Err(format!("the checksum is {want}, the bytes sum to {sum}")) }
}

/// The records of a pax header: `<length> <key>=<value>\n` each.
fn pax(data: &[u8]) -> Vec<(String, String)> {
    let mut records = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let Some(space) = data[at..].iter().position(|&byte| byte == b' ') else { break };
        let Some(length) = std::str::from_utf8(&data[at..at + space]).ok().and_then(|n| n.parse::<usize>().ok()) else {
            break;
        };
        let Some(record) = data.get(at + space + 1..at + length) else { break };
        let record = String::from_utf8_lossy(record);
        if let Some((key, value)) = record.trim_end_matches('\n').split_once('=') {
            records.push((key.to_owned(), value.to_owned()));
        }
        at += length.max(1);
    }
    records
}

#[cfg(test)]
#[path = "tar_read_tests.rs"]
mod tests;
