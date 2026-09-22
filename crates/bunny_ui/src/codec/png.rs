//! PNG: chunks → inflate → defilter → straight RGBA. Bit depth 8 for
//! gray, RGB, gray-alpha and RGBA, palette at 1/2/4/8 with `tRNS`,
//! 16-bit by its high byte, and the Adam7 interlace as seven passes
//! of the same road. Everything else answers `None`.

use super::{fits, inflate::inflate_zlib, Image};

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// IHDR only — the cheap answer `intrinsic` wants.
pub fn header(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 33 || bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    fits(width, height).then_some((width, height))
}

/// The seven Adam7 passes: start x, start y, step x, step y.
const ADAM7: [(usize, usize, usize, usize); 7] =
    [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

/// What the header says about the samples.
struct Format {
    depth: u8,
    color: u8,
    channels: usize,
    sample_bytes: usize,
}

impl Format {
    fn new(depth: u8, color: u8) -> Option<Format> {
        let channels = match color {
            0 => 1,
            2 => 3,
            3 => 1,
            4 => 2,
            6 => 4,
            _ => return None,
        };
        if color != 3 && depth != 8 && depth != 16 {
            return None;
        }
        if color == 3 && !matches!(depth, 1 | 2 | 4 | 8) {
            return None;
        }
        Some(Format { depth, color, channels, sample_bytes: if depth == 16 { 2 } else { 1 } })
    }

    fn bits_per_pixel(&self) -> usize {
        self.channels * self.depth as usize
    }

    fn stride(&self, width: usize) -> usize {
        (width * self.bits_per_pixel() + 7) / 8
    }

    /// The filter's "byte to the left" distance.
    fn bpp(&self) -> usize {
        ((self.bits_per_pixel() + 7) / 8).max(1)
    }
}

/// The whole road. `None` on any malformation — the caller remembers
/// the failure and never walks the bytes again.
pub fn decode(bytes: &[u8]) -> Option<Image> {
    let (width, height) = header(bytes)?;
    let format = Format::new(bytes[24], bytes[25])?;
    let interlace = bytes[28];
    if interlace > 1 {
        return None;
    }
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trans: Vec<u8> = Vec::new();
    let mut compressed: Vec<u8> = Vec::new();
    let mut at = 8;
    while at + 8 <= bytes.len() {
        let len =
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let kind = &bytes[at + 4..at + 8];
        let payload = bytes.get(at + 8..at + 8 + len)?;
        match kind {
            b"PLTE" => palette = payload.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            b"tRNS" => trans = payload.to_vec(),
            b"IDAT" => compressed.extend_from_slice(payload),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + len; // len + type + payload + crc
    }
    let raw = inflate_zlib(&compressed)?;
    let (width, height) = (width as usize, height as usize);
    let mut rgba = vec![0u8; width * height * 4];
    let mut cursor = 0usize;
    let passes: &[(usize, usize, usize, usize)] =
        if interlace == 1 { &ADAM7 } else { &[(0, 0, 1, 1)] };
    for &(x0, y0, dx, dy) in passes {
        if x0 >= width || y0 >= height {
            continue;
        }
        let pass_w = (width - x0 + dx - 1) / dx;
        let pass_h = (height - y0 + dy - 1) / dy;
        let rows = defilter(&raw, &mut cursor, pass_w, pass_h, &format)?;
        let stride = format.stride(pass_w);
        for j in 0..pass_h {
            let line = &rows[j * stride..(j + 1) * stride];
            for i in 0..pass_w {
                let px = pixel(line, i, &format, &palette, &trans)?;
                let (x, y) = (x0 + i * dx, y0 + j * dy);
                rgba[(y * width + x) * 4..][..4].copy_from_slice(&px);
            }
        }
    }
    Some(Image { width: width as u32, height: height as u32, rgba })
}

/// One pass's rows, unfiltered — the filter byte leads each row and
/// each of the five filters reads the row above and the byte to the
/// left, both zero at the edges.
fn defilter(
    raw: &[u8],
    cursor: &mut usize,
    width: usize,
    height: usize,
    format: &Format,
) -> Option<Vec<u8>> {
    let stride = format.stride(width);
    let bpp = format.bpp();
    let mut rows: Vec<u8> = vec![0; stride * height];
    let mut previous_start = 0usize;
    for row in 0..height {
        let filter = *raw.get(*cursor)?;
        *cursor += 1;
        let line = raw.get(*cursor..*cursor + stride)?.to_vec();
        *cursor += stride;
        let start = row * stride;
        for index in 0..stride {
            let x = line[index];
            let a = if index >= bpp { rows[start + index - bpp] } else { 0 };
            let b = if row > 0 { rows[previous_start + index] } else { 0 };
            let c = if row > 0 && index >= bpp { rows[previous_start + index - bpp] } else { 0 };
            let value = match filter {
                0 => x,
                1 => x.wrapping_add(a),
                2 => x.wrapping_add(b),
                3 => x.wrapping_add((((a as u16) + (b as u16)) / 2) as u8),
                4 => {
                    let (pa, pb, pc) = {
                        let p = a as i16 + b as i16 - c as i16;
                        ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs())
                    };
                    let predictor = if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    };
                    x.wrapping_add(predictor)
                }
                _ => return None,
            };
            rows[start + index] = value;
        }
        previous_start = start;
    }
    Some(rows)
}

/// One pixel of an unfiltered row as straight RGBA.
fn pixel(
    line: &[u8],
    x: usize,
    format: &Format,
    palette: &[[u8; 3]],
    trans: &[u8],
) -> Option<[u8; 4]> {
    match format.color {
        3 => {
            let index = match format.depth {
                8 => *line.get(x)? as usize,
                depth => {
                    let per_byte = 8 / depth as usize;
                    let byte = *line.get(x / per_byte)?;
                    let shift = 8 - depth as usize * (x % per_byte + 1);
                    ((byte >> shift) & ((1 << depth) - 1)) as usize
                }
            };
            let [r, g, b] = *palette.get(index)?;
            Some([r, g, b, trans.get(index).copied().unwrap_or(255)])
        }
        color => {
            let px = line.get(x * format.channels * format.sample_bytes..)?;
            let sample = |c: usize| px.get(c * format.sample_bytes).copied();
            Some(match color {
                0 => [sample(0)?, sample(0)?, sample(0)?, 255],
                2 => [sample(0)?, sample(1)?, sample(2)?, 255],
                4 => [sample(0)?, sample(0)?, sample(0)?, sample(1)?],
                6 => [sample(0)?, sample(1)?, sample(2)?, sample(3)?],
                _ => return None,
            })
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in bytes {
            crc ^= byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    pub(crate) fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        let mut body = kind.to_vec();
        body.extend_from_slice(payload);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
        out
    }

    /// A zlib stream of STORED blocks: no compressor needed, and the
    /// inflate still walks the wrapper and the adler tail.
    pub(crate) fn stored_zlib(raw: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let chunks: Vec<&[u8]> = if raw.is_empty() { vec![&[][..]] } else { raw.chunks(65535).collect() };
        for (index, block) in chunks.iter().enumerate() {
            let last = index + 1 == chunks.len();
            out.push(u8::from(last));
            out.extend_from_slice(&(block.len() as u16).to_le_bytes());
            out.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
            out.extend_from_slice(block);
        }
        out.extend_from_slice(&super::super::inflate::adler32(raw).to_be_bytes());
        out
    }

    /// A one-color RGBA8 PNG of `w × h`, filter 0 on every row.
    pub(crate) fn png_rgba(w: u32, h: u32, pixel: [u8; 4]) -> Vec<u8> {
        let mut raw = Vec::new();
        for _ in 0..h {
            raw.push(0);
            for _ in 0..w {
                raw.extend_from_slice(&pixel);
            }
        }
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut png = SIGNATURE.to_vec();
        png.extend(chunk(b"IHDR", &ihdr));
        png.extend(chunk(b"IDAT", &stored_zlib(&raw)));
        png.extend(chunk(b"IEND", &[]));
        png
    }

    #[test]
    fn a_stored_png_decodes_byte_for_byte() {
        let png = png_rgba(4, 3, [10, 20, 30, 200]);
        assert_eq!(header(&png), Some((4, 3)));
        let image = decode(&png).expect("decodes");
        assert_eq!((image.width, image.height), (4, 3));
        assert!(image.rgba.chunks_exact(4).all(|px| px == [10, 20, 30, 200]));
    }

    #[test]
    fn a_paletted_png_reads_its_transparency() {
        // 2×1, 8-bit palette: index 0 opaque red, index 1 half-clear blue
        let raw = [0u8, 0, 1];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 3, 0, 0, 0]);
        let mut png = SIGNATURE.to_vec();
        png.extend(chunk(b"IHDR", &ihdr));
        png.extend(chunk(b"PLTE", &[255, 0, 0, 0, 0, 255]));
        png.extend(chunk(b"tRNS", &[255, 128]));
        png.extend(chunk(b"IDAT", &stored_zlib(&raw)));
        png.extend(chunk(b"IEND", &[]));
        let image = decode(&png).expect("decodes");
        assert_eq!(&image.rgba, &[255, 0, 0, 255, 0, 0, 255, 128]);
    }

    #[test]
    fn corrupt_bytes_fail_clean() {
        assert_eq!(decode(&[0x89, b'P', b'N', b'G', 0, 0, 0, 0, 0]), None);
        assert_eq!(header(b"not a png at all"), None);
        // a good header over garbage data
        let mut png = png_rgba(2, 2, [1, 2, 3, 4]);
        let len = png.len();
        png[len - 20..len - 12].fill(0xAB);
        assert_eq!(decode(&png), None);
    }

    #[test]
    fn a_size_beyond_the_cap_is_refused_at_the_header() {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&65536u32.to_be_bytes());
        ihdr.extend_from_slice(&65536u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut png = SIGNATURE.to_vec();
        png.extend(chunk(b"IHDR", &ihdr));
        assert_eq!(header(&png), None);
    }
}
