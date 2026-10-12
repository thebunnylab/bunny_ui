//! PNG, written: eight-bit RGBA, the rows under the `None` filter and
//! deflated as one zlib stream — the picture `bunny preview --png` leaves
//! on disk, readable by anything that reads a PNG.

use crate::error::{Error, Result};
use crate::formats::{deflate, zip};

/// The eight bytes every PNG starts with.
pub const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// `rgba` — `width × height` pixels of four bytes, rows top down — as a
/// PNG. The wrong number of bytes is refused by name.
pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let row = width as usize * 4;
    if rgba.len() != row * height as usize {
        return Err(Error::new(format!("{} bytes are not {width}×{height} RGBA pixels", rgba.len())));
    }
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for line in rgba.chunks_exact(row) {
        raw.push(0); // the None filter
        raw.extend_from_slice(line);
    }
    let mut zlib = vec![0x78, 0x01];
    zlib.extend(deflate::deflate(&raw));
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // eight bits a sample, RGBA, deflate, adaptive filters, no interlace
    let mut out = Vec::with_capacity(zlib.len() + 64);
    out.extend_from_slice(&SIGNATURE);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

/// The width and height a PNG's header declares — what a test reads back.
pub fn size(png: &[u8]) -> Option<(u32, u32)> {
    if png.get(..8)? != SIGNATURE {
        return None;
    }
    let ihdr = png.get(16..24)?;
    Some((u32::from_be_bytes(ihdr[0..4].try_into().ok()?), u32::from_be_bytes(ihdr[4..8].try_into().ok()?)))
}

/// One chunk: its length, its kind, its data, and the CRC of the last two.
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = zip::crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Adler-32 (RFC 1950), what a zlib stream ends with.
fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 bytes fit before the sums can overflow a u32
    for run in bytes.chunks(5552) {
        for &byte in run {
            a += u32::from(byte);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-by-one picture: the signature, the header's fields, every
    /// chunk's CRC, zlib's header and the rows' adler at the stream's end.
    #[test]
    fn a_picture_is_a_png_with_its_chunks_and_checksums() {
        let png = encode(2, 1, &[255, 0, 0, 255, 0, 0, 255, 128]).unwrap();
        assert_eq!(&png[..8], &SIGNATURE);
        assert_eq!(size(&png), Some((2, 1)));
        assert_eq!(&png[24..29], &[8, 6, 0, 0, 0]);
        assert_eq!(&png[29..33], &zip::crc32(&png[12..29]).to_be_bytes(), "IHDR's CRC covers its kind and data");
        let idat_len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let zlib = &png[41..41 + idat_len];
        assert_eq!(&zlib[..2], &[0x78, 0x01]);
        let raw = [0u8, 255, 0, 0, 255, 0, 0, 255, 128];
        assert_eq!(&zlib[zlib.len() - 4..], &adler32(&raw).to_be_bytes());
        assert_eq!(&png[png.len() - 12..], &[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]);
        assert!(encode(2, 2, &[0; 4]).is_err(), "the wrong number of bytes is refused");
        assert_eq!(size(b"not a png at all, not at all"), None);
    }

    #[test]
    fn adler_is_the_one_zlib_checks() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}
