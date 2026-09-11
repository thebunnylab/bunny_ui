//! What a font file says about itself — pure, so the Mac runs the
//! tests. The platform reads a face from a file and says nothing about
//! it, so the family name comes out of the file's own `name` table.

/// FNV-1a over the face's bytes — the file's name under the app.
pub fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, &byte| {
        (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The weight class and the slant a face declares about ITSELF, out of
/// its `OS/2` table — `usWeightClass` (100..=900) and bit 0 of
/// `fsSelection`.
///
/// A family is several files, and the platform reads each one as an
/// unnamed typeface: without this, four files that all say "Geist" are
/// one entry and the last one registered answers for every weight and
/// every slant. The product ships Regular, Medium, SemiBold and Italic
/// under that one family, so the last one was the ITALIC, and the whole
/// workbench rendered oblique on a phone.
///
/// `None` when the table is missing (a face without `OS/2` is answered
/// as an upright 400, by the caller).
pub fn style(bytes: &[u8]) -> Option<(u16, bool)> {
    let table = table(bytes, b"OS/2")?;
    let u16_at = |at: usize| Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]));
    let weight = u16_at(table + 4)?;
    // fsSelection lives at offset 62 in every version of the table
    let italic = u16_at(table + 62)? & 0x0001 != 0;
    Some((weight, italic))
}

/// Where a table starts, by its four-byte tag — a collection answers
/// out of its first face's directory.
fn table(bytes: &[u8], tag: &[u8; 4]) -> Option<usize> {
    let u16_at = |at: usize| Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]));
    let u32_at = |at: usize| {
        Some(u32::from_be_bytes([
            *bytes.get(at)?,
            *bytes.get(at + 1)?,
            *bytes.get(at + 2)?,
            *bytes.get(at + 3)?,
        ]))
    };
    let directory = if bytes.starts_with(b"ttcf") { u32_at(12)? as usize } else { 0 };
    let count = u16_at(directory + 4)? as usize;
    (0..count).find_map(|index| {
        let record = directory + 12 + index * 16;
        (bytes.get(record..record + 4)? == tag).then(|| u32_at(record + 8).map(|at| at as usize))?
    })
}

/// The family name (`nameID` 1) out of a TrueType or OpenType face's
/// `name` table; a collection answers its first face's.
pub fn family_name(bytes: &[u8]) -> Option<String> {
    let u16_at = |at: usize| Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]));
    let table = table(bytes, b"name")?;
    let count = u16_at(table + 2)? as usize;
    let strings = table + u16_at(table + 4)? as usize;
    let mut fallback = None;
    for index in 0..count {
        let record = table + 6 + index * 12;
        let platform = u16_at(record)?;
        let encoding = u16_at(record + 2)?;
        let name_id = u16_at(record + 6)?;
        if name_id != 1 {
            continue;
        }
        let length = u16_at(record + 8)? as usize;
        let start = strings + u16_at(record + 10)? as usize;
        let raw = bytes.get(start..start + length)?;
        match (platform, encoding) {
            // Windows, Unicode BMP: UTF-16BE — the one every face carries
            (3, 1) | (0, _) => {
                let units: Vec<u16> =
                    raw.chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect();
                return Some(String::from_utf16_lossy(&units));
            }
            // Macintosh Roman: close enough to ASCII for a family name
            (1, 0) if fallback.is_none() => {
                fallback = Some(raw.iter().map(|&byte| byte as char).collect::<String>());
            }
            _ => {}
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A face with one `name` table: the family "Bunny Sans" in both
    /// encodings.
    fn face_with_names() -> Vec<u8> {
        let family_utf16: Vec<u8> =
            "Bunny Sans".encode_utf16().flat_map(|unit| unit.to_be_bytes()).collect();
        let family_mac = b"Bunny Sans".to_vec();
        let mut name = Vec::new();
        name.extend_from_slice(&0u16.to_be_bytes()); // format
        name.extend_from_slice(&2u16.to_be_bytes()); // count
        name.extend_from_slice(&(6u16 + 2 * 12).to_be_bytes()); // string offset
        for (platform, encoding, length, offset) in [
            (1u16, 0u16, family_mac.len() as u16, 0u16),
            (3, 1, family_utf16.len() as u16, family_mac.len() as u16),
        ] {
            for value in [platform, encoding, 0, 1, length, offset] {
                name.extend_from_slice(&value.to_be_bytes());
            }
        }
        name.extend_from_slice(&family_mac);
        name.extend_from_slice(&family_utf16);
        let mut face = Vec::new();
        face.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        face.extend_from_slice(&1u16.to_be_bytes()); // one table
        face.extend_from_slice(&[0u8; 6]);
        face.extend_from_slice(b"name");
        face.extend_from_slice(&0u32.to_be_bytes());
        face.extend_from_slice(&28u32.to_be_bytes()); // offset: 12 + 16
        face.extend_from_slice(&(name.len() as u32).to_be_bytes());
        face.extend_from_slice(&name);
        face
    }

    #[test]
    fn the_family_name_comes_out_of_the_face() {
        assert_eq!(family_name(&face_with_names()).as_deref(), Some("Bunny Sans"));
        assert_eq!(family_name(b"not a face"), None);
        assert_eq!(family_name(&[]), None);
    }

    #[test]
    fn a_collection_reads_its_first_face() {
        let face = face_with_names();
        let mut collection = Vec::new();
        collection.extend_from_slice(b"ttcf");
        collection.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        collection.extend_from_slice(&1u32.to_be_bytes());
        collection.extend_from_slice(&16u32.to_be_bytes()); // the face starts after this header
        // the face's table offsets are relative to the file, so they move
        let mut moved = face.clone();
        let table_offset = 28u32 + 16;
        moved[12 + 8..12 + 12].copy_from_slice(&table_offset.to_be_bytes());
        collection.extend_from_slice(&moved);
        assert_eq!(family_name(&collection).as_deref(), Some("Bunny Sans"));
    }

    /// A face carrying an `OS/2` table beside its `name` one: the two
    /// tables a shipped face is read for.
    fn face_with_style(weight: u16, italic: bool) -> Vec<u8> {
        let plain = face_with_names();
        // the `name` table's own bytes, which move by one table record
        let name = &plain[28..];
        let mut os2 = vec![0u8; 78];
        os2[0..2].copy_from_slice(&5u16.to_be_bytes()); // version
        os2[4..6].copy_from_slice(&weight.to_be_bytes()); // usWeightClass
        os2[62..64].copy_from_slice(&u16::from(italic).to_be_bytes()); // fsSelection
        let directory = 12 + 2 * 16;
        let mut face = Vec::new();
        face.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        face.extend_from_slice(&2u16.to_be_bytes()); // two tables
        face.extend_from_slice(&[0u8; 6]);
        // the records are sorted by tag, as a real face's are
        face.extend_from_slice(b"OS/2");
        face.extend_from_slice(&0u32.to_be_bytes());
        face.extend_from_slice(&(directory as u32).to_be_bytes());
        face.extend_from_slice(&(os2.len() as u32).to_be_bytes());
        face.extend_from_slice(b"name");
        face.extend_from_slice(&0u32.to_be_bytes());
        face.extend_from_slice(&((directory + os2.len()) as u32).to_be_bytes());
        face.extend_from_slice(&(name.len() as u32).to_be_bytes());
        face.append(&mut os2);
        face.extend_from_slice(name);
        face
    }

    /// The whole reason the style is read: four files of one family, and
    /// the italic among them must not answer for the other three.
    #[test]
    fn a_face_says_its_weight_and_its_slant() {
        assert_eq!(style(&face_with_style(400, false)), Some((400, false)));
        assert_eq!(style(&face_with_style(600, false)), Some((600, false)));
        assert_eq!(style(&face_with_style(400, true)), Some((400, true)));
        // the family still reads, with a second table in the directory
        assert_eq!(family_name(&face_with_style(400, true)).as_deref(), Some("Bunny Sans"));
        // a face without the table says nothing, and the caller assumes
        // the upright 400 the platform would
        assert_eq!(style(&face_with_names()), None);
        assert_eq!(style(b"not a face"), None);
    }

    #[test]
    fn the_hash_names_the_file() {
        assert_ne!(fnv64(b"one face"), fnv64(b"another"));
        assert_eq!(fnv64(b""), 0xcbf2_9ce4_8422_2325);
    }
}
