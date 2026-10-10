//! A WebAssembly module's export names, read from its bytes — which
//! entry the page will call, and whether the framework's ABI is there.

/// The names in the module's export section, in order. `Err` is a file
/// that is not a wasm module.
pub fn exports(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
        return Err(String::from("not a WebAssembly module"));
    }
    let mut at = 8;
    while at < bytes.len() {
        let id = bytes[at];
        at += 1;
        let size = leb(bytes, &mut at)?;
        let end = at.checked_add(size).filter(|end| *end <= bytes.len()).ok_or("a section runs past the end")?;
        if id == 7 {
            let count = leb(bytes, &mut at)?;
            let mut names = Vec::with_capacity(count.min(4096));
            for _ in 0..count {
                let length = leb(bytes, &mut at)?;
                let name = bytes.get(at..at + length).ok_or("an export name runs past the end")?;
                names.push(String::from_utf8_lossy(name).into_owned());
                at += length + 1; // the name, then the kind byte
                leb(bytes, &mut at)?; // the index
            }
            return Ok(names);
        }
        at = end;
    }
    Ok(Vec::new())
}

/// An unsigned LEB128 number.
fn leb(bytes: &[u8], at: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(*at).ok_or("a number runs past the end")?;
        *at += 1;
        if shift < usize::BITS {
            value |= usize::from(byte & 0x7f) << shift;
        }
        shift += 7;
        if byte < 0x80 {
            return Ok(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A module with two exports, written byte by byte.
    fn module() -> Vec<u8> {
        let mut bytes = b"\0asm\x01\0\0\0".to_vec();
        // type section: one type, () -> ()
        bytes.extend([1, 4, 1, 0x60, 0, 0]);
        // function section: one function of type 0
        bytes.extend([3, 2, 1, 0]);
        // export section: "start" func 0, "memory" mem 0
        let mut exports = vec![2];
        exports.extend([5]);
        exports.extend(b"start");
        exports.extend([0, 0]);
        exports.extend([6]);
        exports.extend(b"memory");
        exports.extend([2, 0]);
        bytes.push(7);
        bytes.push(exports.len() as u8);
        bytes.extend(exports);
        bytes
    }

    #[test]
    fn the_export_names_read() {
        assert_eq!(exports(&module()).unwrap(), vec!["start", "memory"]);
    }

    #[test]
    fn a_file_that_is_not_wasm_is_refused() {
        assert!(exports(b"#!/bin/sh\n").is_err());
        let mut cut = module();
        cut.truncate(cut.len() - 4);
        assert!(exports(&cut).is_err());
    }
}
