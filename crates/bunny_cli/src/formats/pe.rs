//! Windows executables (PE32+), read: what `bunny build windows` checks
//! in the binary it made — the subsystem it opens as, the DLLs it needs
//! (no C runtime DLL when the runtime is linked in), and whether its
//! resources are there.

/// What a Windows executable says about itself.
#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    /// 2 opens a window and no console; 3 opens a console.
    pub subsystem: u16,
    /// The DLLs it imports, as written (`KERNEL32.dll`).
    pub imports: Vec<String>,
    /// It carries resources: an icon, a version, a manifest.
    pub resources: bool,
}

pub const GUI: u16 = 2;
pub const CONSOLE: u16 = 3;

/// Reads `bytes` as a 64-bit Windows executable.
pub fn read(bytes: &[u8]) -> Result<Image, String> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or("cut short");
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or("cut short");
    if bytes.get(..2) != Some(b"MZ") {
        return Err(String::from("not a Windows executable"));
    }
    let pe = u32_at(0x3C)? as usize;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err(String::from("no PE signature"));
    }
    let coff = pe + 4;
    let sections = u16_at(coff + 2)? as usize;
    let optional = coff + 20;
    let optional_size = u16_at(coff + 16)? as usize;
    if u16_at(optional)? != 0x20B {
        return Err(String::from("not a 64-bit executable"));
    }
    let subsystem = u16_at(optional + 68)?;
    // the data directories: 1 is the import table, 2 the resources
    let directory = |index: usize| -> Result<(u32, u32), String> {
        let at = optional + 112 + index * 8;
        Ok((u32_at(at)?, u32_at(at + 4)?))
    };
    let (imports_rva, _) = directory(1)?;
    let (resources_rva, resources_size) = directory(2)?;
    // a virtual address, as an offset in the file, through the sections
    let table = optional + optional_size;
    let offset = |rva: u32| -> Result<usize, String> {
        for index in 0..sections {
            let header = table + index * 40;
            let size = u32_at(header + 8)?.max(u32_at(header + 16)?);
            let address = u32_at(header + 12)?;
            if rva >= address && rva < address + size {
                return Ok((rva - address + u32_at(header + 20)?) as usize);
            }
        }
        Err(format!("the address {rva:#x} is in no section"))
    };
    let mut imports = Vec::new();
    if imports_rva != 0 {
        let mut descriptor = offset(imports_rva)?;
        loop {
            let name_rva = u32_at(descriptor + 12)?;
            if name_rva == 0 {
                break;
            }
            let start = offset(name_rva)?;
            let end = bytes[start..].iter().position(|byte| *byte == 0).map_or(bytes.len(), |end| start + end);
            imports.push(String::from_utf8_lossy(&bytes[start..end]).into_owned());
            descriptor += 20;
        }
    }
    Ok(Image { subsystem, imports, resources: resources_rva != 0 && resources_size != 0 })
}

/// Whether the C runtime is a DLL the executable needs — the one a
/// machine without the Visual C++ Redistributable does not have.
pub fn needs_c_runtime(image: &Image) -> bool {
    image.imports.iter().any(|dll| {
        let dll = dll.to_ascii_lowercase();
        dll.starts_with("vcruntime") || dll.starts_with("msvcp") || dll.starts_with("api-ms-win-crt")
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A small PE32+ with one section holding the import table — the
    /// shape a linker writes, with only the fields this reads.
    pub fn sample(subsystem: u16, dlls: &[&str]) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x400];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        bytes[0x40..0x44].copy_from_slice(b"PE\0\0");
        let coff = 0x44;
        bytes[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
        bytes[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
        bytes[coff + 16..coff + 18].copy_from_slice(&240u16.to_le_bytes());
        let optional = coff + 20;
        bytes[optional..optional + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
        bytes[optional + 68..optional + 70].copy_from_slice(&subsystem.to_le_bytes());
        // the import directory at RVA 0x1000; resources at 0x1800
        bytes[optional + 120..optional + 124].copy_from_slice(&0x1000u32.to_le_bytes());
        bytes[optional + 124..optional + 128].copy_from_slice(&40u32.to_le_bytes());
        bytes[optional + 128..optional + 132].copy_from_slice(&0x1800u32.to_le_bytes());
        bytes[optional + 132..optional + 136].copy_from_slice(&16u32.to_le_bytes());
        // one section: RVA 0x1000, 0x1000 bytes, at file offset 0x200
        let section = optional + 240;
        bytes[section..section + 6].copy_from_slice(b".idata");
        bytes[section + 8..section + 12].copy_from_slice(&0x1000u32.to_le_bytes());
        bytes[section + 12..section + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        bytes[section + 16..section + 20].copy_from_slice(&0x200u32.to_le_bytes());
        bytes[section + 20..section + 24].copy_from_slice(&0x200u32.to_le_bytes());
        // the descriptors, each naming a DLL written after them
        let mut name_at = 0x200 + (dlls.len() + 1) * 20;
        for (index, dll) in dlls.iter().enumerate() {
            let descriptor = 0x200 + index * 20;
            let rva = 0x1000 + (name_at - 0x200) as u32;
            bytes[descriptor + 12..descriptor + 16].copy_from_slice(&rva.to_le_bytes());
            bytes[name_at..name_at + dll.len()].copy_from_slice(dll.as_bytes());
            name_at += dll.len() + 1;
        }
        bytes
    }

    #[test]
    fn the_subsystem_and_the_dlls_are_read() {
        let image = read(&sample(GUI, &["KERNEL32.dll", "USER32.dll"])).unwrap();
        assert_eq!(image, Image { subsystem: GUI, imports: vec![String::from("KERNEL32.dll"), String::from("USER32.dll")], resources: true });
        assert!(!needs_c_runtime(&image));
        let dynamic = read(&sample(CONSOLE, &["VCRUNTIME140.dll", "KERNEL32.dll"])).unwrap();
        assert_eq!(dynamic.subsystem, CONSOLE);
        assert!(needs_c_runtime(&dynamic));
        assert!(read(b"\x7fELF").is_err());
    }
}
