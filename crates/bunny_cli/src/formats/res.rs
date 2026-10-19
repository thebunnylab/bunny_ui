//! Windows resources (`.res`), written: what the linker puts into an
//! executable next to its code — the icon Explorer and the taskbar show
//! (group 1, the one the shell's window class loads), the version the
//! file's properties show, and the manifest Windows reads before the
//! first line of the app runs.

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;
/// English (United States), the language every string table here is in.
const LANGUAGE: u16 = 0x0409;

/// What goes into the resources.
pub struct Resources<'a> {
    /// An `.ico` file's bytes, whole.
    pub icon: Option<&'a [u8]>,
    /// `major.minor.patch.build`.
    pub version: [u16; 4],
    /// The version as people read it (`1.4.0-beta.2`).
    pub version_text: &'a str,
    pub product: &'a str,
    pub file_name: &'a str,
    /// The application's reverse-DNS id: the manifest's identity.
    pub id: &'a str,
}

/// The `.res` file.
pub fn write(resources: &Resources) -> Result<Vec<u8>, String> {
    // a .res file opens with an empty entry
    let mut out = Vec::new();
    entry(&mut out, 0, 0, &[]);
    if let Some(ico) = resources.icon {
        let images = icon_images(ico)?;
        let mut group = Vec::new();
        group.extend_from_slice(&0u16.to_le_bytes());
        group.extend_from_slice(&1u16.to_le_bytes());
        group.extend_from_slice(&(images.len() as u16).to_le_bytes());
        for (index, (directory, image)) in images.iter().enumerate() {
            let id = index as u16 + 1;
            entry(&mut out, RT_ICON, id, image);
            // the group's entry: the file's, its last field the icon's id
            group.extend_from_slice(&directory[..12]);
            group.extend_from_slice(&id.to_le_bytes());
        }
        entry(&mut out, RT_GROUP_ICON, 1, &group);
    }
    entry(&mut out, RT_VERSION, 1, &version_info(resources));
    entry(&mut out, RT_MANIFEST, 1, manifest(resources).as_bytes());
    Ok(out)
}

/// One resource: its header — a type and a name, both numbers — and its
/// data, each padded to four bytes.
fn entry(out: &mut Vec<u8>, kind: u16, name: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes()); // the header's size
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&name.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // data version
    let flags: u16 = if kind == 0 { 0 } else { 0x1030 }; // movable, pure, discardable
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(if kind == 0 { 0 } else { LANGUAGE }).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // characteristics
    out.extend_from_slice(data);
    pad(out);
}

fn pad(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// One image of an `.ico`: its 16-byte directory entry and its bytes (a
/// bitmap or a PNG).
type IconImage<'a> = ([u8; 16], &'a [u8]);

/// The images of an `.ico`.
fn icon_images(ico: &[u8]) -> Result<Vec<IconImage<'_>>, String> {
    let u16_at = |at: usize| ico.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| ico.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if u16_at(0) != Some(0) || u16_at(2) != Some(1) {
        return Err(String::from("not an .ico file"));
    }
    let count = u16_at(4).unwrap_or(0) as usize;
    if count == 0 {
        return Err(String::from("an .ico file with no image"));
    }
    (0..count)
        .map(|index| {
            let at = 6 + index * 16;
            let directory: [u8; 16] = ico.get(at..at + 16).and_then(|b| b.try_into().ok()).ok_or("an .ico file cut short")?;
            let (size, offset) = (u32_at(at + 8).unwrap_or(0) as usize, u32_at(at + 12).unwrap_or(0) as usize);
            let image = ico.get(offset..offset + size).ok_or("an .ico image runs off the file")?;
            Ok((directory, image))
        })
        .collect()
}

/// `VS_VERSIONINFO`: the fixed numbers, and the strings Explorer shows.
fn version_info(resources: &Resources) -> Vec<u8> {
    let [major, minor, patch, build] = resources.version.map(u32::from);
    let mut fixed = Vec::with_capacity(52);
    for value in [
        0xFEEF_04BD, // the signature
        0x0001_0000, // the structure's version
        major << 16 | minor,
        patch << 16 | build,
        major << 16 | minor,
        patch << 16 | build,
        0x3F, // the flags that may be set
        0,    // none is
        0x0004_0004, // Windows NT
        1,    // an application
        0,
        0,
        0,
    ] {
        fixed.extend_from_slice(&u32::to_le_bytes(value));
    }
    let strings = [
        ("CompanyName", resources.product),
        ("FileDescription", resources.product),
        ("FileVersion", resources.version_text),
        ("InternalName", resources.file_name.trim_end_matches(".exe")),
        ("OriginalFilename", resources.file_name),
        ("ProductName", resources.product),
        ("ProductVersion", resources.version_text),
    ];
    let table: Vec<Vec<u8>> = strings
        .iter()
        .map(|(key, value)| {
            let text = utf16(value);
            node(key, &text, (text.len() / 2) as u16, 1, &[])
        })
        .collect();
    // 0409 English, 04B0 Unicode
    let string_table = node("040904B0", &[], 0, 1, &table);
    let string_info = node("StringFileInfo", &[], 0, 1, &[string_table]);
    let translation = [0x09, 0x04, 0xB0, 0x04];
    let var = node("Translation", &translation, 4, 0, &[]);
    let var_info = node("VarFileInfo", &[], 0, 1, &[var]);
    node("VS_VERSION_INFO", &fixed, 52, 0, &[string_info, var_info])
}

/// One block of the version tree: its length, the length of its value
/// (characters for text, bytes for binary), its type, its key, its
/// value, and its children — each starting on a four-byte boundary.
fn node(key: &str, value: &[u8], value_length: u16, kind: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0, 0];
    out.extend_from_slice(&value_length.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&utf16(key));
    pad(&mut out);
    out.extend_from_slice(value);
    for child in children {
        pad(&mut out);
        out.extend_from_slice(child);
    }
    let length = out.len() as u16;
    out[..2].copy_from_slice(&length.to_le_bytes());
    out
}

/// Text as Windows keeps it: UTF-16, little-endian, ending in a NUL.
fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
}

/// The manifest: per-monitor DPI, the Windows 10 and 11 behavior, the
/// common controls of version 6, UTF-8 as the code page, long paths, and
/// no elevation.
pub fn manifest(resources: &Resources) -> String {
    let [major, minor, patch, build] = resources.version;
    let id: String = resources.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-').collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="{id}" version="{major}.{minor}.{patch}.{build}"/>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#
    )
}

/// `1.4.0-beta.2` and build 42 as the four numbers Windows keeps.
pub fn version_numbers(version: &str, build: u64) -> [u16; 4] {
    let core = version.split(['-', '+']).next().unwrap_or("0");
    let mut numbers = core.split('.').map(|part| part.parse::<u16>().unwrap_or(0));
    [numbers.next().unwrap_or(0), numbers.next().unwrap_or(0), numbers.next().unwrap_or(0), build.min(u64::from(u16::MAX)) as u16]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resources(icon: Option<&[u8]>) -> Resources<'_> {
        Resources { icon, version: [1, 4, 0, 42], version_text: "1.4.0", product: "Notes", file_name: "notes.exe", id: "io.bunny.notes" }
    }

    /// The entries of a `.res`, as (type, name, data length).
    fn entries(bytes: &[u8]) -> Vec<(u16, u16, usize)> {
        let mut at = 0;
        let mut found = Vec::new();
        while at + 32 <= bytes.len() {
            let size = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let header = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            let kind = u16::from_le_bytes([bytes[at + 10], bytes[at + 11]]);
            let name = u16::from_le_bytes([bytes[at + 14], bytes[at + 15]]);
            found.push((kind, name, size));
            at += header + size.div_ceil(4) * 4;
        }
        assert_eq!(at, bytes.len(), "the entries tile the file");
        found
    }

    #[test]
    fn the_resources_are_a_res_file() {
        // an .ico with one 1×1 PNG image
        let png = b"\x89PNG\r\n\x1a\nfake";
        let mut ico = vec![0, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 32, 0];
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(png);
        let bytes = write(&resources(Some(&ico))).unwrap();
        let found = entries(&bytes);
        assert_eq!(found[0], (0, 0, 0), "the empty entry first");
        assert_eq!(found[1], (RT_ICON, 1, png.len()));
        assert_eq!(found[2], (RT_GROUP_ICON, 1, 6 + 14));
        assert_eq!(found[3].0, RT_VERSION);
        assert_eq!(found[4].0, RT_MANIFEST);
        assert!(write(&resources(Some(b"not an icon"))).is_err());
        assert_eq!(entries(&write(&resources(None)).unwrap()).len(), 3);
    }

    #[test]
    fn the_version_tree_nests_by_length() {
        let info = version_info(&resources(None));
        assert_eq!(u16::from_le_bytes([info[0], info[1]]) as usize, info.len());
        assert_eq!(u16::from_le_bytes([info[2], info[3]]), 52, "the fixed numbers");
        // the fixed numbers, after the key and its padding
        let fixed = 6 + "VS_VERSION_INFO".len() * 2 + 2 + 2;
        assert_eq!(&info[fixed..fixed + 4], &0xFEEF_04BDu32.to_le_bytes());
        assert_eq!(&info[fixed + 8..fixed + 12], &(1u32 << 16 | 4).to_le_bytes());
        assert!(info.windows(utf16("ProductName").len()).any(|window| window == utf16("ProductName")));
    }

    #[test]
    fn a_version_becomes_four_numbers() {
        assert_eq!(version_numbers("1.4.0-beta.2", 42), [1, 4, 0, 42]);
        assert_eq!(version_numbers("2", 7), [2, 0, 0, 7]);
        assert!(manifest(&resources(None)).contains("version=\"1.4.0.42\""));
    }
}
