//! Linux executables (64-bit little-endian ELF), read: the shared
//! libraries a binary needs (`DT_NEEDED`), which `bunny build linux`
//! turns into the packages each distribution installs them with.

/// The libraries `bytes` names in its dynamic section, in order.
pub fn needed(bytes: &[u8]) -> Result<Vec<String>, String> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or("cut short");
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap_or([0; 4]))).ok_or("cut short");
    let u64_at = |at: usize| bytes.get(at..at + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8]))).ok_or("cut short");
    if bytes.get(..4) != Some(b"\x7fELF") || bytes.get(4) != Some(&2) || bytes.get(5) != Some(&1) {
        return Err(String::from("not a 64-bit little-endian ELF file"));
    }
    let table = u64_at(0x20)? as usize;
    let entry_size = u16_at(0x36)? as usize;
    let count = u16_at(0x38)? as usize;
    let headers: Vec<usize> = (0..count).map(|index| table + index * entry_size).collect();
    // a virtual address, as an offset in the file, through the loaded segments
    let offset = |address: u64| -> Result<usize, String> {
        for &header in &headers {
            if u32_at(header)? == 1 {
                let (file, virt, size) = (u64_at(header + 8)?, u64_at(header + 16)?, u64_at(header + 32)?);
                if address >= virt && address < virt + size {
                    return Ok((address - virt + file) as usize);
                }
            }
        }
        Err(format!("the address {address:#x} is in no loaded segment"))
    };
    let Some(&dynamic) = headers.iter().find(|&&header| u32_at(header) == Ok(2)) else {
        return Ok(Vec::new()); // a static binary needs nothing
    };
    let (start, size) = (u64_at(dynamic + 8)? as usize, u64_at(dynamic + 32)? as usize);
    let entries: Vec<(u64, u64)> = (start..start + size)
        .step_by(16)
        .map_while(|at| Some((u64_at(at).ok()?, u64_at(at + 8).ok()?)))
        .take_while(|(tag, _)| *tag != 0)
        .collect();
    let strings = entries.iter().find(|(tag, _)| *tag == 5).map(|(_, value)| *value).ok_or("no string table")?;
    let strings = offset(strings)?;
    let mut needed = Vec::new();
    for (_, name) in entries.iter().filter(|(tag, _)| *tag == 1) {
        let start = strings + *name as usize;
        let end = bytes.get(start..).and_then(|rest| rest.iter().position(|byte| *byte == 0)).ok_or("a name runs off the file")?;
        needed.push(String::from_utf8_lossy(&bytes[start..start + end]).into_owned());
    }
    Ok(needed)
}

/// The package that installs a library, on Debian and Ubuntu, Fedora, and
/// Arch — `None` for the C library's own, which every system has.
pub fn package(library: &str) -> Option<(&'static str, &'static str, &'static str)> {
    let stem = library.split(".so").next().unwrap_or(library);
    Some(match stem {
        "libc" | "libm" | "libdl" | "libpthread" | "librt" | "libgcc_s" | "libutil" => return None,
        stem if stem.starts_with("ld-linux") => return None,
        "libwayland-client" | "libwayland-cursor" | "libwayland-egl" => ("libwayland-client0", "libwayland-client", "wayland"),
        "libxkbcommon" => ("libxkbcommon0", "libxkbcommon", "libxkbcommon"),
        "libxkbcommon-x11" => ("libxkbcommon-x11-0", "libxkbcommon-x11", "libxkbcommon-x11"),
        "libxcb" => ("libxcb1", "libxcb", "libxcb"),
        "libxcb-xfixes" | "libxcb-xkb" | "libxcb-shm" => ("libxcb1", "libxcb", "libxcb"),
        "libdbus-1" => ("libdbus-1-3", "dbus-libs", "dbus"),
        "libsecret-1" => ("libsecret-1-0", "libsecret", "libsecret"),
        "libglib-2.0" | "libgio-2.0" | "libgobject-2.0" => ("libglib2.0-0", "glib2", "glib2"),
        "libfontconfig" => ("libfontconfig1", "fontconfig", "fontconfig"),
        "libfreetype" => ("libfreetype6", "freetype", "freetype2"),
        "libharfbuzz" => ("libharfbuzz0b", "harfbuzz", "harfbuzz"),
        "libEGL" | "libGLESv2" | "libGL" => ("libegl1", "mesa-libEGL", "libglvnd"),
        "libvulkan" => ("libvulkan1", "vulkan-loader", "vulkan-icd-loader"),
        _ => return Some(("?", "?", "?")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small ELF64 with one loaded segment and a dynamic section naming
    /// two libraries.
    fn sample(libraries: &[&str]) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x400];
        bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
        bytes[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes());
        bytes[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
        bytes[0x38..0x3A].copy_from_slice(&2u16.to_le_bytes());
        // PT_LOAD: the whole file at virtual address 0x10000
        let load = 0x40;
        bytes[load..load + 4].copy_from_slice(&1u32.to_le_bytes());
        bytes[load + 16..load + 24].copy_from_slice(&0x10000u64.to_le_bytes());
        bytes[load + 32..load + 40].copy_from_slice(&0x400u64.to_le_bytes());
        // PT_DYNAMIC at 0x100
        let dynamic = load + 56;
        bytes[dynamic..dynamic + 4].copy_from_slice(&2u32.to_le_bytes());
        bytes[dynamic + 8..dynamic + 16].copy_from_slice(&0x100u64.to_le_bytes());
        bytes[dynamic + 32..dynamic + 40].copy_from_slice(&0x80u64.to_le_bytes());
        // the strings at 0x300 (virtual 0x10300)
        let mut entries = vec![(5u64, 0x10300u64)];
        let mut at = 1;
        for library in libraries {
            entries.push((1, at as u64));
            bytes[0x300 + at..0x300 + at + library.len()].copy_from_slice(library.as_bytes());
            at += library.len() + 1;
        }
        for (index, (tag, value)) in entries.iter().enumerate() {
            let entry = 0x100 + index * 16;
            bytes[entry..entry + 8].copy_from_slice(&tag.to_le_bytes());
            bytes[entry + 8..entry + 16].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn the_needed_libraries_are_read() {
        let needed = needed(&sample(&["libwayland-client.so.0", "libc.so.6"])).unwrap();
        assert_eq!(needed, ["libwayland-client.so.0", "libc.so.6"]);
        assert!(super::needed(b"MZ").is_err());
    }

    #[test]
    fn a_library_is_its_distributions_package() {
        assert_eq!(package("libfreetype.so.6"), Some(("libfreetype6", "freetype", "freetype2")));
        assert_eq!(package("libc.so.6"), None);
        assert_eq!(package("ld-linux-x86-64.so.2"), None);
    }
}
