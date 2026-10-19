//! PNG, written and read back: eight-bit RGBA, the rows under the `None`
//! filter and deflated as one zlib stream — the picture `bunny preview
//! --png` leaves on disk, readable by anything that reads a PNG — and
//! read again for `--snapshot`'s goldens, any of the five filters
//! honoured, eight-bit RGBA only, no interlace.

use crate::error::{Error, Result};
use crate::formats::{deflate, inflate, zip};

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

/// The pixels of a PNG `bunny` wrote, or one like it: eight-bit RGBA,
/// not interlaced. Anything else is refused by name — a golden is made
/// by `bunny preview --snapshot`, not edited by hand.
pub fn decode(png: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let (width, height) = size(png).ok_or_else(|| Error::new("not a PNG"))?;
    let (depth, color, interlace) = (png[24], png[25], png[28]);
    if depth != 8 || color != 6 || interlace != 0 {
        return Err(Error::new(format!("a {depth}-bit PNG of color type {color}{}: bunny reads back only the eight-bit RGBA pictures it writes", if interlace != 0 { ", interlaced" } else { "" })));
    }
    let mut compressed = Vec::new();
    let mut at = 8;
    while at + 8 <= png.len() {
        let len = u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]) as usize;
        let kind = &png[at + 4..at + 8];
        let payload = png.get(at + 8..at + 8 + len).ok_or_else(|| Error::new("a PNG chunk ends past the file"))?;
        match kind {
            b"IDAT" => compressed.extend_from_slice(payload),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + len;
    }
    let raw = inflate::inflate_zlib(&compressed).ok_or_else(|| Error::new("the PNG's pixel stream does not inflate"))?;
    let (w, h) = (width as usize, height as usize);
    let stride = w * 4;
    if raw.len() != (stride + 1) * h {
        return Err(Error::new(format!("{} bytes of rows are not {width}×{height} RGBA", raw.len())));
    }
    let mut rgba = vec![0u8; stride * h];
    for y in 0..h {
        let filter = raw[y * (stride + 1)];
        let line = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        let (done, current) = rgba.split_at_mut(y * stride);
        let above = done.get(y.saturating_sub(1) * stride..y.saturating_sub(1) * stride + stride).filter(|_| y > 0);
        let current = &mut current[..stride];
        for x in 0..stride {
            let left = if x >= 4 { current[x - 4] } else { 0 };
            let up = above.map_or(0, |row| row[x]);
            let up_left = if x >= 4 { above.map_or(0, |row| row[x - 4]) } else { 0 };
            let predicted = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => paeth(left, up, up_left),
                other => return Err(Error::new(format!("row {y} uses filter {other}, which is not a PNG filter"))),
            };
            current[x] = line[x].wrapping_add(predicted);
        }
    }
    Ok((width, height, rgba))
}

/// The Paeth predictor: whichever neighbour is nearest the gradient.
fn paeth(left: u8, up: u8, up_left: u8) -> u8 {
    let (a, b, c) = (i16::from(left), i16::from(up), i16::from(up_left));
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        left
    } else if pb <= pc {
        up
    } else {
        up_left
    }
}

/// Adler-32 (RFC 1950), what a zlib stream ends with.
pub(crate) fn adler32(bytes: &[u8]) -> u32 {
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

    /// What was written comes back, pixel for pixel; a filtered row is
    /// undone; the pictures bunny does not write are refused by name.
    #[test]
    fn a_picture_reads_back_and_filters_are_undone() {
        let rgba: Vec<u8> = (0..3 * 2 * 4).map(|i| (i * 37 % 251) as u8).collect();
        let png = encode(3, 2, &rgba).unwrap();
        assert_eq!(decode(&png).unwrap(), (3, 2, rgba.clone()));
        // the same rows under Sub, Up, Average and Paeth, filtered by hand
        let stride = 12;
        let mut raw = Vec::new();
        for (y, filter) in [1u8, 4].into_iter().enumerate() {
            raw.push(filter);
            for x in 0..stride {
                let here = rgba[y * stride + x];
                let left = if x >= 4 { rgba[y * stride + x - 4] } else { 0 };
                let up = if y > 0 { rgba[(y - 1) * stride + x] } else { 0 };
                let up_left = if x >= 4 && y > 0 { rgba[(y - 1) * stride + x - 4] } else { 0 };
                let predicted = match filter {
                    1 => left,
                    4 => paeth(left, up, up_left),
                    _ => unreachable!(),
                };
                raw.push(here.wrapping_sub(predicted));
            }
        }
        let mut zlib = vec![0x78, 0x01];
        zlib.extend(deflate::deflate(&raw));
        zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
        let mut filtered = Vec::new();
        filtered.extend_from_slice(&SIGNATURE);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&3u32.to_be_bytes());
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut filtered, b"IHDR", &ihdr);
        chunk(&mut filtered, b"IDAT", &zlib);
        chunk(&mut filtered, b"IEND", &[]);
        assert_eq!(decode(&filtered).unwrap(), (3, 2, rgba));
        let mut gray = png.clone();
        gray[25] = 0;
        assert!(decode(&gray).unwrap_err().message.contains("color type 0"));
        assert!(decode(b"not a png").is_err());
    }
}
