//! The linux image engine: a PNG decoder written in the house, safe
//! Rust from the first byte — this platform has no OS codec, and the
//! C ones speak setjmp, a road Rust cannot walk. The inflate below is
//! the whole of RFC 1951 the format needs: stored, fixed and dynamic
//! blocks, the 32K window, the adler check.
//!
//! File icons come from the freedesktop icon themes on disk (PNG
//! sizes), with a procedural document glyph as the floor when a theme
//! offers nothing. JPEG is deferred until a fixture demands it — the
//! examples and the apps speak PNG.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use bunny_ui::image_engine::{ImageEngine, ImageRaster, ImageSource};

// MARK: - inflate (RFC 1951, by hand)

struct BitReader<'a> {
    bytes: &'a [u8],
    at: usize,
    bit: u32,
    value: u32,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> BitReader<'a> {
        BitReader { bytes, at: 0, bit: 0, value: 0 }
    }

    fn take(&mut self, count: u32) -> Option<u32> {
        while self.bit < count {
            let byte = *self.bytes.get(self.at)? as u32;
            self.at += 1;
            self.value |= byte << self.bit;
            self.bit += 8;
        }
        let out = self.value & ((1 << count) - 1);
        self.value >>= count;
        self.bit -= count;
        Some(out)
    }

    fn align(&mut self) {
        self.value = 0;
        self.bit = 0;
    }
}

/// A canonical Huffman table: code lengths in, symbol lookup out.
struct Huffman {
    /// counts[len] and offsets into `symbols`, the canonical walk.
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Huffman {
        let mut counts = [0u16; 16];
        for &length in lengths {
            counts[length as usize] += 1;
        }
        counts[0] = 0;
        let mut offsets = [0u16; 16];
        for length in 1..16 {
            offsets[length] = offsets[length - 1] + counts[length - 1];
        }
        let mut symbols = vec![0u16; lengths.iter().filter(|&&l| l != 0).count()];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                symbols[offsets[length as usize] as usize] = symbol as u16;
                offsets[length as usize] += 1;
            }
        }
        Huffman { counts, symbols }
    }

    /// One symbol off the stream — deflate codes arrive MSB-first
    /// inside the LSB-first bit soup, so the walk goes bit by bit.
    fn decode(&self, bits: &mut BitReader) -> Option<u16> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for length in 1..16 {
            code |= bits.take(1)? as i32;
            let count = self.counts[length] as i32;
            if code - first < count {
                return Some(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        None
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
    13, 13,
];

/// zlib in, raw bytes out. `None` on any malformation — the caller
/// remembers the failure and never walks the bytes again.
fn inflate_zlib(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 6 {
        return None;
    }
    let cmf = bytes[0] as u32;
    let flg = bytes[1] as u32;
    // deflate method, window sane, header checksum, no preset dict
    if cmf & 0x0F != 8 || (cmf * 256 + flg) % 31 != 0 || flg & 0x20 != 0 {
        return None;
    }
    let deflate = &bytes[2..];
    let mut bits = BitReader::new(deflate);
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = bits.take(1)?;
        match bits.take(2)? {
            0 => {
                // stored: aligned, LEN + one's complement
                bits.align();
                let at = bits.at;
                let len = u16::from_le_bytes([*deflate.get(at)?, *deflate.get(at + 1)?]) as usize;
                let nlen =
                    u16::from_le_bytes([*deflate.get(at + 2)?, *deflate.get(at + 3)?]) as usize;
                if len != !nlen & 0xFFFF {
                    return None;
                }
                let data = deflate.get(at + 4..at + 4 + len)?;
                out.extend_from_slice(data);
                bits.at = at + 4 + len;
            }
            kind @ (1 | 2) => {
                let (literals, distances);
                if kind == 1 {
                    // the fixed trees, straight from the RFC
                    let mut lengths = [0u8; 288];
                    lengths[..144].fill(8);
                    lengths[144..256].fill(9);
                    lengths[256..280].fill(7);
                    lengths[280..].fill(8);
                    literals = Huffman::new(&lengths);
                    distances = Huffman::new(&[5u8; 30]);
                } else {
                    const ORDER: [usize; 19] =
                        [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                    let hlit = bits.take(5)? as usize + 257;
                    let hdist = bits.take(5)? as usize + 1;
                    let hclen = bits.take(4)? as usize + 4;
                    let mut code_lengths = [0u8; 19];
                    for &slot in ORDER.iter().take(hclen) {
                        code_lengths[slot] = bits.take(3)? as u8;
                    }
                    let decoder = Huffman::new(&code_lengths);
                    let mut lengths = vec![0u8; hlit + hdist];
                    let mut at = 0;
                    while at < lengths.len() {
                        let symbol = decoder.decode(&mut bits)?;
                        match symbol {
                            0..=15 => {
                                lengths[at] = symbol as u8;
                                at += 1;
                            }
                            16 => {
                                let previous = *lengths.get(at.checked_sub(1)?)?;
                                for _ in 0..bits.take(2)? + 3 {
                                    *lengths.get_mut(at)? = previous;
                                    at += 1;
                                }
                            }
                            17 => at += bits.take(3)? as usize + 3,
                            18 => at += bits.take(7)? as usize + 11,
                            _ => return None,
                        }
                    }
                    if at > lengths.len() {
                        return None;
                    }
                    literals = Huffman::new(&lengths[..hlit]);
                    distances = Huffman::new(&lengths[hlit..]);
                }
                loop {
                    let symbol = literals.decode(&mut bits)?;
                    match symbol {
                        0..=255 => out.push(symbol as u8),
                        256 => break,
                        257..=285 => {
                            let slot = symbol as usize - 257;
                            let length = LENGTH_BASE[slot] as usize
                                + bits.take(LENGTH_EXTRA[slot] as u32)? as usize;
                            let dist_symbol = distances.decode(&mut bits)? as usize;
                            if dist_symbol >= 30 {
                                return None;
                            }
                            let distance = DIST_BASE[dist_symbol] as usize
                                + bits.take(DIST_EXTRA[dist_symbol] as u32)? as usize;
                            let start = out.len().checked_sub(distance)?;
                            // the window copy may overlap itself — the
                            // repeat IS the feature
                            for offset in 0..length {
                                let byte = out[start + offset];
                                out.push(byte);
                            }
                        }
                        _ => return None,
                    }
                }
            }
            _ => return None,
        }
        if last == 1 {
            break;
        }
    }
    // the adler tail seals the stream
    let at = 2 + bits.at + if bits.bit >= 8 { 0 } else { 0 };
    let tail = bytes.get(at..at + 4);
    if let Some(tail) = tail {
        let want = u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]);
        if adler32(&out) != want {
            return None;
        }
    }
    Some(out)
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in bytes.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

// MARK: - PNG

struct Png {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// IHDR only — the cheap answer `intrinsic` wants.
fn png_header(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 33 || bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    (width > 0 && height > 0).then_some((width, height))
}

/// The whole road: chunks → inflate → defilter → straight RGBA.
/// Supported: bit depth 8 for gray/rgb/gray-alpha/rgba, palette at
/// 1/2/4/8 with tRNS, 16-bit by its high byte. Interlace is refused —
/// nothing in the house emits it.
fn png_decode(bytes: &[u8]) -> Option<Png> {
    let (width, height) = png_header(bytes)?;
    let depth = bytes[24];
    let color = bytes[25];
    let interlace = bytes[28];
    if interlace != 0 {
        return None;
    }
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trans: Vec<u8> = Vec::new();
    let mut compressed: Vec<u8> = Vec::new();
    let mut at = 8;
    while at + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
            as usize;
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
    let channels: usize = match color {
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
    let sample_bytes = if depth == 16 { 2 } else { 1 };
    let bits_per_pixel = channels * depth as usize;
    let stride = (width as usize * bits_per_pixel + 7) / 8;
    let bpp = ((bits_per_pixel + 7) / 8).max(1);
    let mut rows: Vec<u8> = vec![0; stride * height as usize];
    let mut previous_start = 0usize;
    let mut cursor = 0usize;
    for row in 0..height as usize {
        let filter = *raw.get(cursor)?;
        cursor += 1;
        let line = raw.get(cursor..cursor + stride)?.to_vec();
        cursor += stride;
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
    // to straight RGBA
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for row in 0..height as usize {
        let line = &rows[row * stride..(row + 1) * stride];
        for x in 0..width as usize {
            let out = &mut rgba[(row * width as usize + x) * 4..][..4];
            match color {
                3 => {
                    let index = match depth {
                        8 => line[x] as usize,
                        _ => {
                            let per_byte = 8 / depth as usize;
                            let byte = line[x / per_byte];
                            let shift = 8 - depth as usize * (x % per_byte + 1);
                            ((byte >> shift) & ((1 << depth) - 1)) as usize
                        }
                    };
                    let [r, g, b] = *palette.get(index)?;
                    out.copy_from_slice(&[
                        r,
                        g,
                        b,
                        trans.get(index).copied().unwrap_or(255),
                    ]);
                }
                _ => {
                    let px = &line[x * channels * sample_bytes..];
                    let sample = |c: usize| px[c * sample_bytes];
                    match color {
                        0 => out.copy_from_slice(&[sample(0), sample(0), sample(0), 255]),
                        2 => out.copy_from_slice(&[sample(0), sample(1), sample(2), 255]),
                        4 => out.copy_from_slice(&[sample(0), sample(0), sample(0), sample(1)]),
                        6 => out.copy_from_slice(&[sample(0), sample(1), sample(2), sample(3)]),
                        _ => return None,
                    }
                }
            }
        }
    }
    Some(Png { width, height, rgba })
}

// MARK: - resample (bilinear, straight alpha)

fn resample(source: &Png, width: usize, height: usize) -> Vec<u8> {
    if source.width as usize == width && source.height as usize == height {
        return source.rgba.clone();
    }
    let mut out = vec![0u8; width * height * 4];
    let sw = source.width as f64;
    let sh = source.height as f64;
    for y in 0..height {
        let v = ((y as f64 + 0.5) * sh / height as f64 - 0.5).clamp(0.0, sh - 1.0);
        let y0 = v.floor() as usize;
        let y1 = (y0 + 1).min(source.height as usize - 1);
        let fy = v - y0 as f64;
        for x in 0..width {
            let u = ((x as f64 + 0.5) * sw / width as f64 - 0.5).clamp(0.0, sw - 1.0);
            let x0 = u.floor() as usize;
            let x1 = (x0 + 1).min(source.width as usize - 1);
            let fx = u - x0 as f64;
            let sample = |sx: usize, sy: usize, c: usize| {
                source.rgba[(sy * source.width as usize + sx) * 4 + c] as f64
            };
            for c in 0..4 {
                let top = sample(x0, y0, c) * (1.0 - fx) + sample(x1, y0, c) * fx;
                let bottom = sample(x0, y1, c) * (1.0 - fx) + sample(x1, y1, c) * fx;
                out[(y * width + x) * 4 + c] = (top * (1.0 - fy) + bottom * fy).round() as u8;
            }
        }
    }
    out
}

// MARK: - file icons (freedesktop themes, PNG sizes)

const FILE_ICON_SIZE: u32 = 32;

/// Extension → the freedesktop icon names worth trying, best first.
fn icon_names(path: &str) -> &'static [&'static str] {
    let name = path.rsplit('/').next().unwrap_or(path);
    if path.ends_with('/') || !name.contains('.') {
        return &["folder", "text-x-generic"];
    }
    match name.rsplit('.').next().unwrap_or("") {
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" => &["image-x-generic"],
        "zip" | "tar" | "gz" | "xz" | "zst" => &["package-x-generic"],
        "sh" | "exe" | "bin" => &["application-x-executable"],
        "md" | "txt" | "toml" | "json" | "yaml" | "lock" => &["text-x-generic"],
        _ => &["text-x-generic"],
    }
}

/// Walks the installed themes for a raster icon of roughly the wanted
/// size. Themes are inconsistent (the cursor lesson again) — the walk
/// is candidates × sizes × sections, first hit wins.
fn theme_icon(name: &str) -> Option<Png> {
    const ROOTS: [&str; 2] = ["/usr/share/icons", "/usr/share/pixmaps"];
    const THEMES: [&str; 4] = ["Adwaita", "Yaru", "hicolor", "HighContrast"];
    const SIZES: [&str; 4] = ["32x32", "48x48", "64x64", "24x24"];
    const SECTIONS: [&str; 4] = ["mimetypes", "places", "mimes", "apps"];
    for theme in THEMES {
        for size in SIZES {
            for section in SECTIONS {
                for order in
                    [format!("{size}/{section}"), format!("{section}/{size}")]
                {
                    let path = format!("{}/{theme}/{order}/{name}.png", ROOTS[0]);
                    if let Ok(bytes) = std::fs::read(&path)
                        && let Some(png) = png_decode(&bytes)
                    {
                        return Some(png);
                    }
                }
            }
        }
    }
    let flat = format!("{}/{name}.png", ROOTS[1]);
    std::fs::read(flat).ok().and_then(|bytes| png_decode(&bytes))
}

/// The floor: a plain document glyph — sheet, folded corner — so a
/// file icon always answers even on a theme-less box.
fn fallback_icon() -> Png {
    const S: usize = FILE_ICON_SIZE as usize;
    let mut rgba = vec![0u8; S * S * 4];
    let sheet = [148u8, 158, 168, 255];
    let fold = [190u8, 198, 206, 255];
    let (left, right, top, bottom) = (6, S - 6, 3, S - 3);
    let fold_size = 8;
    for y in top..bottom {
        for x in left..right {
            let in_fold_cut = x >= right - fold_size && y < top + fold_size;
            let diagonal = (right - x) + (y - top) == fold_size;
            let px = &mut rgba[(y * S + x) * 4..][..4];
            if in_fold_cut {
                if diagonal || (right - x) + (y - top) < fold_size {
                    px.copy_from_slice(&fold);
                }
            } else {
                px.copy_from_slice(&sheet);
            }
        }
    }
    Png { width: S as u32, height: S as u32, rgba }
}

// MARK: - the engine

/// Decoded sources by key; `None` remembers a failure so corrupt bytes
/// decode exactly once. Resamples cache by (key, w, h) and evict
/// WHOLE at 64 entries — the twins' doctrine.
pub struct LinuxImageEngine {
    decoded: RefCell<HashMap<u64, Option<Rc<Png>>>>,
    resampled: RefCell<HashMap<(u64, usize, usize), Rc<ImageRaster>>>,
}

impl LinuxImageEngine {
    pub fn new() -> LinuxImageEngine {
        LinuxImageEngine { decoded: RefCell::new(HashMap::new()), resampled: RefCell::new(HashMap::new()) }
    }

    fn decoded_of(&self, source: &ImageSource) -> Option<Rc<Png>> {
        let (key, decode): (u64, Box<dyn FnOnce() -> Option<Png>>) = match source {
            ImageSource::Bytes { key, bytes } => {
                let bytes = Rc::clone(bytes);
                (*key, Box::new(move || png_decode(&bytes)))
            }
            ImageSource::FileIcon { key, path } => {
                let path = Rc::clone(path);
                (*key, Box::new(move || {
                    let png = icon_names(&path)
                        .iter()
                        .find_map(|name| theme_icon(name))
                        .unwrap_or_else(fallback_icon);
                    Some(png)
                }))
            }
            _ => return None,
        };
        if let Some(known) = self.decoded.borrow().get(&key) {
            return known.clone();
        }
        let fresh = decode().map(Rc::new);
        self.decoded.borrow_mut().insert(key, fresh.clone());
        fresh
    }

    #[cfg(test)]
    fn resample_cache_len(&self) -> usize {
        self.resampled.borrow().len()
    }
}

impl ImageEngine for LinuxImageEngine {
    fn intrinsic(&self, source: &ImageSource) -> Option<(u32, u32)> {
        match source {
            ImageSource::Bytes { bytes, .. } => png_header(bytes),
            ImageSource::FileIcon { .. } => Some((FILE_ICON_SIZE, FILE_ICON_SIZE)),
            _ => None,
        }
    }

    fn raster(
        &self,
        source: &ImageSource,
        width: usize,
        height: usize,
    ) -> Option<Rc<ImageRaster>> {
        if width == 0 || height == 0 {
            return None;
        }
        let key = match source {
            ImageSource::Bytes { key, .. } | ImageSource::FileIcon { key, .. } => *key,
            _ => return None,
        };
        if let Some(hit) = self.resampled.borrow().get(&(key, width, height)) {
            return Some(Rc::clone(hit));
        }
        let decoded = self.decoded_of(source)?;
        let raster =
            Rc::new(ImageRaster { width, height, rgba: resample(&decoded, width, height) });
        let mut cache = self.resampled.borrow_mut();
        if cache.len() >= 64 {
            // the whole shelf clears at once — no clock, no ranking
            cache.clear();
        }
        cache.insert((key, width, height), Rc::clone(&raster));
        Some(raster)
    }
}

// MARK: - tests

#[cfg(test)]
mod tests {
    use super::*;

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc: u32 = 0xffff_ffff;
        for byte in bytes {
            crc ^= *byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut body = kind.to_vec();
        body.extend_from_slice(payload);
        let mut out = (payload.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
        out
    }

    /// The example's own generator: stored deflate blocks are valid
    /// zlib and need no compressor.
    fn png_rgba(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut raw = Vec::new();
        for y in 0..height {
            raw.push(0);
            for x in 0..width {
                raw.extend_from_slice(&pixel(x, y));
            }
        }
        let mut idat = vec![0x78, 0x01, 0x01];
        idat.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        idat.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
        idat.extend_from_slice(&raw);
        idat.extend_from_slice(&adler32(&raw).to_be_bytes());
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        out.extend_from_slice(&chunk(b"IHDR", &ihdr));
        out.extend_from_slice(&chunk(b"IDAT", &idat));
        out.extend_from_slice(&chunk(b"IEND", &[]));
        out
    }

    fn source(bytes: Vec<u8>) -> ImageSource {
        ImageSource::Bytes { key: bytes.iter().map(|&b| b as u64).sum(), bytes: bytes.into() }
    }

    #[test]
    fn the_embedded_png_decodes_byte_for_byte() {
        let bytes = png_rgba(4, 3, |x, y| [x as u8 * 10, y as u8 * 20, 7, 255]);
        let png = png_decode(&bytes).expect("a valid png decodes");
        assert_eq!((png.width, png.height), (4, 3));
        assert_eq!(&png.rgba[0..4], &[0, 0, 7, 255]);
        assert_eq!(&png.rgba[(2 * 4 + 3) * 4..][..4], &[30, 40, 7, 255]);
    }

    #[test]
    fn a_paletted_png_reads_its_transparency() {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 3, 0, 0, 0]); // 8-bit palette
        let raw = [0u8, 0, 1]; // filter none, indexes 0 and 1
        let mut idat = vec![0x78, 0x01, 0x01];
        idat.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        idat.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
        idat.extend_from_slice(&raw);
        idat.extend_from_slice(&adler32(&raw).to_be_bytes());
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(&chunk(b"IHDR", &ihdr));
        bytes.extend_from_slice(&chunk(b"PLTE", &[255, 0, 0, 0, 255, 0]));
        bytes.extend_from_slice(&chunk(b"tRNS", &[128]));
        bytes.extend_from_slice(&chunk(b"IDAT", &idat));
        bytes.extend_from_slice(&chunk(b"IEND", &[]));
        let png = png_decode(&bytes).expect("palette + tRNS decodes");
        assert_eq!(&png.rgba[0..4], &[255, 0, 0, 128], "index 0 carries its tRNS alpha");
        assert_eq!(&png.rgba[4..8], &[0, 255, 0, 255], "index 1 is opaque");
    }

    #[test]
    fn a_real_theme_icon_walks_the_full_inflate() {
        // real Adwaita/Yaru PNGs carry dynamic-huffman streams — the
        // road the stored-block fixtures never touch. Skips quietly on
        // a theme-less box.
        if let Some(png) = theme_icon("text-x-generic").or_else(|| theme_icon("folder")) {
            assert!(png.width > 0 && png.height > 0);
            assert_eq!(png.rgba.len(), (png.width * png.height * 4) as usize);
        }
    }

    #[test]
    fn corrupt_bytes_fail_clean_and_only_decode_once() {
        let engine = LinuxImageEngine::new();
        let bad = source(vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert!(engine.raster(&bad, 8, 8).is_none());
        assert!(engine.raster(&bad, 8, 8).is_none(), "the failure is remembered");
        let ImageSource::Bytes { key, .. } = &bad else { unreachable!() };
        assert_eq!(
            engine.decoded.borrow().get(key).map(|slot| slot.is_none()),
            Some(true),
            "the failure lives in the cache — the bytes never decode twice"
        );
    }

    #[test]
    fn the_resample_cache_evicts_whole_at_its_cap() {
        let engine = LinuxImageEngine::new();
        let bytes = png_rgba(4, 4, |_, _| [1, 2, 3, 255]);
        let image = source(bytes);
        for size in 1..=70usize {
            let _ = engine.raster(&image, size, size);
        }
        assert!(engine.resample_cache_len() <= 64, "the shelf cleared at least once");
    }

    #[test]
    fn straight_alpha_survives_the_road() {
        let bytes = png_rgba(4, 4, |x, _| if x < 2 { [59, 130, 246, 0] } else { [59, 130, 246, 255] });
        let engine = LinuxImageEngine::new();
        let raster = engine.raster(&source(bytes), 4, 4).unwrap();
        assert_eq!(&raster.rgba[0..4], &[59, 130, 246, 0], "transparent keeps its RGB — straight");
        assert_eq!(&raster.rgba[3 * 4..][..4], &[59, 130, 246, 255]);
    }

    #[test]
    fn a_file_icon_always_answers_at_its_size() {
        let engine = LinuxImageEngine::new();
        let icon = ImageSource::FileIcon { key: 42, path: "src/main.rs".into() };
        assert_eq!(engine.intrinsic(&icon), Some((32, 32)));
        let raster = engine.raster(&icon, 32, 32).expect("theme or the procedural floor");
        assert_eq!((raster.width, raster.height), (32, 32));
        assert!(raster.rgba.chunks_exact(4).any(|px| px[3] > 0), "there is ink");
    }

    #[test]
    fn the_bilinear_resample_interpolates() {
        let png = Png { width: 2, height: 1, rgba: vec![0, 0, 0, 255, 100, 0, 0, 255] };
        let out = resample(&png, 4, 1);
        assert_eq!(out.len(), 16);
        assert!(out[4] > 0 && out[4] < 100, "the middle samples between the poles");
    }
}
