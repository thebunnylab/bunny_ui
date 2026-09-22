//! The linux image engine: the codecs of the house
//! ([`bunny_ui::codec`] — PNG and JPEG in safe Rust, this platform has
//! no OS codec and the C ones speak setjmp, a road Rust cannot walk),
//! a bilinear resample, and file icons from the freedesktop icon
//! themes on disk (PNG sizes), with a procedural document glyph as
//! the floor when a theme offers nothing. A JPEG the codec refuses by
//! name (arithmetic, lossless, CMYK …) says so once on stderr and
//! paints nothing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use bunny_ui::codec::{self, png, Image as Png};
use bunny_ui::image_engine::{ImageEngine, ImageRaster, ImageSource, FILE_ICON_SIZE};

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
                        && let Some(png) = png::decode(&bytes)
                    {
                        return Some(png);
                    }
                }
            }
        }
    }
    let flat = format!("{}/{name}.png", ROOTS[1]);
    std::fs::read(flat).ok().and_then(|bytes| png::decode(&bytes))
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
                let key = *key;
                (key, Box::new(move || match codec::kind(&bytes) {
                    // a JPEG the codec refuses says why, once per key —
                    // the failure is remembered and never walked again
                    Some(codec::Kind::Jpeg) => match codec::jpeg::decode(&bytes) {
                        Ok(image) => Some(image),
                        Err(why) => {
                            eprintln!("bunny_ui_linux: a jpeg was refused — {why:?} (image {key:#x})");
                            None
                        }
                    },
                    _ => codec::decode(&bytes),
                }))
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
            ImageSource::Bytes { bytes, .. } => codec::header(bytes),
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
        idat.extend_from_slice(&bunny_ui::codec::inflate::adler32(&raw).to_be_bytes());
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        out.extend_from_slice(&chunk(b"IHDR", &ihdr));
        out.extend_from_slice(&chunk(b"IDAT", &idat));
        out.extend_from_slice(&chunk(b"IEND", &[]));
        out
    }

    fn source(bytes: Vec<u8>) -> ImageSource {
        ImageSource::Bytes { key: bytes.iter().map(|&b| b as u64).sum(), bytes: bytes.into() }
    }



    /// The JPEG road of the engine: the size from the header, the
    /// pixels from the codec, at the picture's own size and resampled.
    #[test]
    fn a_jpeg_answers_its_size_and_its_pixels() {
        let bytes = include_bytes!("../../bunny_ui/tests/fixtures/codec/base_420.jpg");
        let engine = LinuxImageEngine::new();
        let src = source(bytes.to_vec());
        assert_eq!(engine.intrinsic(&src), Some((33, 21)));
        let raster = engine.raster(&src, 33, 21).expect("decodes");
        let expected = include_bytes!("../../bunny_ui/tests/fixtures/codec/base_420.rgba");
        let worst = raster.rgba.iter().zip(expected.iter()).map(|(a, b)| a.abs_diff(*b)).max();
        assert!(worst.is_some_and(|w| w <= 2), "within two steps of libjpeg: {worst:?}");
        let half = engine.raster(&src, 16, 10).expect("resamples");
        assert_eq!(half.rgba.len(), 16 * 10 * 4);
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
