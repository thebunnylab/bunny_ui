//! The pluggable image boundary — decode and raster of ONE image.
//!
//! Layout is always ours, on every target; what the platform lends is
//! the DECODE and the resampling of pixels: the house raw format in
//! headless, ImageIO on the Mac, the browser on the web. No component
//! API knows which engine is active — [`ImageEngine`] is the only door
//! (a declared boundary: `Rc<dyn ImageEngine>` in the `Runtime`), the
//! exact mirror of the text engine.
//!
//! The engine returns pixels at EXACTLY the physical size the caller
//! asks for. The resample happens once, behind the engine's cache — the
//! compositor and the GPU atlas then consume the SAME bytes, so the two
//! pipelines agree byte for byte. The cost model is the text raster's:
//! a size change re-resamples (rare in real UI); animating an image's
//! size re-rasters every frame — do not.

use std::cell::RefCell;
use std::fmt;
use std::hash::Hasher;
use std::rc::Rc;

use motor::hash::FxHashMap as HashMap;

/// Where an image's pixels come from. The identity (`key`) is computed
/// ONCE at construction — the scene diff, the caches and the wire all
/// compare images by it, never by content.
#[derive(Clone)]
pub enum ImageSource {
    /// Platform-encoded bytes (PNG, JPEG, the house raw format…).
    Bytes { key: u64, bytes: Rc<[u8]> },
    /// The platform's icon for a file path (macOS: the workspace icon).
    FileIcon { key: u64, path: Rc<str> },
}

/// Domain tags folded into the key so the two variants never share an
/// identity by accident.
const BYTES_TAG: u64 = 0x62_6e_79_5f_62_79_74_65; // "bny_byte"
const ICON_TAG: u64 = 0x62_6e_79_5f_69_63_6f_6e; // "bny_icon"

fn fx_hash(tag: u64, bytes: &[u8]) -> u64 {
    let mut hasher = motor::hash::FxHasher::default();
    hasher.write_u64(tag);
    hasher.write_usize(bytes.len());
    hasher.write(bytes);
    hasher.finish()
}

impl ImageSource {
    /// Bytes with a hashed identity. The hash walks the whole blob ONCE
    /// — build the source once (in state or a constant), not per body.
    pub fn from_bytes(bytes: impl Into<Rc<[u8]>>) -> ImageSource {
        let bytes = bytes.into();
        let key = fx_hash(BYTES_TAG, &bytes);
        ImageSource::Bytes { key, bytes }
    }

    /// Bytes with the APP's own identity — the exit for assets that
    /// already carry an id (skips hashing a large blob).
    pub fn bytes_keyed(key: u64, bytes: impl Into<Rc<[u8]>>) -> ImageSource {
        ImageSource::Bytes { key, bytes: bytes.into() }
    }

    /// The cheap identity — what diffs, caches and the wire carry.
    pub fn key(&self) -> u64 {
        match self {
            ImageSource::Bytes { key, .. } | ImageSource::FileIcon { key, .. } => *key,
        }
    }
}

/// The platform's icon for a file path. Headless draws a deterministic
/// checker derived from the path; the web has no file icons in v1 (the
/// engine answers `None` and nothing paints).
pub fn file_icon(path: impl Into<Rc<str>>) -> ImageSource {
    let path = path.into();
    let key = fx_hash(ICON_TAG, path.as_bytes());
    ImageSource::FileIcon { key, path }
}

/// Identity comparison — never the content. Two sources with one key
/// are the same image by contract.
impl PartialEq for ImageSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                ImageSource::Bytes { key, bytes },
                ImageSource::Bytes { key: other_key, bytes: other_bytes },
            ) => key == other_key && bytes.len() == other_bytes.len(),
            (
                ImageSource::FileIcon { path, .. },
                ImageSource::FileIcon { path: other_path, .. },
            ) => path == other_path,
            _ => false,
        }
    }
}

/// Manual on purpose: the derive would spill whole blobs into every
/// `{:?}` of a scene node.
impl fmt::Debug for ImageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageSource::Bytes { key, bytes } => {
                write!(f, "bytes(0x{key:016x}, {}b)", bytes.len())
            }
            ImageSource::FileIcon { path, .. } => write!(f, "file-icon({path})"),
        }
    }
}

/// A resampled image: an RGBA rectangle of STRAIGHT alpha (the house
/// compositor blends straight, on every target), already in PHYSICAL
/// pixels at the size the caller asked for.
pub struct ImageRaster {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Manual for the same reason as the source's.
impl fmt::Debug for ImageRaster {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ImageRaster({}×{})", self.width, self.height)
    }
}

/// The boundary: who decodes and resamples images. Object-safe on
/// purpose — `Rc<dyn ImageEngine>` is the shape that crosses the
/// `Runtime`.
pub trait ImageEngine {
    /// Pixel dimensions of the source — `None` while the platform has
    /// not decoded it (the web decodes asynchronously; broken bytes
    /// stay `None` forever and nothing paints).
    fn intrinsic(&self, source: &ImageSource) -> Option<(u32, u32)>;

    /// The source resampled to EXACTLY `width`×`height` physical px.
    /// `None` = nothing to paint (not decoded yet, zero size, broken).
    fn raster(
        &self,
        source: &ImageSource,
        width: usize,
        height: usize,
    ) -> Option<Rc<ImageRaster>>;
}

// MARK: - RawImages, the default engine

/// How many resampled entries the cache holds before it drops them all.
/// Entries are big (whole bitmaps) — the ceiling is low and the eviction
/// total, like the web text raster cache.
const IMAGE_KEEP: usize = 64;

/// The house engine: decodes only the house raw format (tests inject
/// pixels, never a codec) and draws file icons as a checker derived from
/// the path — deterministic metrics that keep the headless suite
/// byte-stable on any machine. Resampling is nearest-neighbor by
/// integer division: exact, seamless, and free of float drift.
#[derive(Default)]
pub struct RawImages {
    rasters: RefCell<HashMap<(u64, usize, usize), Rc<ImageRaster>>>,
}

/// The house raw format: `"bnyr"`, width u32 LE, height u32 LE, then
/// exactly width×height×4 RGBA bytes. A fixture is six lines of code.
const RAW_MAGIC: &[u8; 4] = b"bnyr";
const RAW_HEADER: usize = 12;

impl RawImages {
    /// Encodes pixels into the house raw format — the fixture helper.
    pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        debug_assert_eq!(rgba.len(), (width * height * 4) as usize);
        let mut out = Vec::with_capacity(RAW_HEADER + rgba.len());
        out.extend_from_slice(RAW_MAGIC);
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(rgba);
        out
    }

    /// The dimensions in the header, when the blob is well-formed.
    fn decode_header(bytes: &[u8]) -> Option<(u32, u32)> {
        if bytes.len() < RAW_HEADER || &bytes[..4] != RAW_MAGIC {
            return None;
        }
        let width = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
        let height = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
        let expected = RAW_HEADER + (width as usize) * (height as usize) * 4;
        (bytes.len() == expected && width > 0 && height > 0).then_some((width, height))
    }

    /// Nearest-neighbor into the destination size — integer division
    /// keeps it deterministic on every machine.
    fn resample(
        bytes: &[u8],
        (src_w, src_h): (u32, u32),
        width: usize,
        height: usize,
    ) -> Vec<u8> {
        let source = &bytes[RAW_HEADER..];
        let (src_w, src_h) = (src_w as usize, src_h as usize);
        let mut rgba = vec![0u8; width * height * 4];
        for y in 0..height {
            let sy = (y * src_h) / height;
            for x in 0..width {
                let sx = (x * src_w) / width;
                let from = (sy * src_w + sx) * 4;
                let to = (y * width + x) * 4;
                rgba[to..to + 4].copy_from_slice(&source[from..from + 4]);
            }
        }
        rgba
    }

    /// The file-icon checker: two colors derived from the identity, in
    /// cells that scale with the destination — recognizable at 16 and
    /// at 64, byte-stable everywhere.
    fn checker(key: u64, width: usize, height: usize) -> Vec<u8> {
        let light = [
            (key >> 16) as u8 | 0x60,
            (key >> 24) as u8 | 0x60,
            (key >> 32) as u8 | 0x60,
            255,
        ];
        let dark = [
            (key >> 40) as u8 & 0x7f,
            (key >> 48) as u8 & 0x7f,
            (key >> 56) as u8 & 0x7f,
            255,
        ];
        let cell = (width.min(height) / 8).max(1);
        let mut rgba = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let color = if (x / cell + y / cell) % 2 == 0 { light } else { dark };
                let to = (y * width + x) * 4;
                rgba[to..to + 4].copy_from_slice(&color);
            }
        }
        rgba
    }
}

/// The intrinsic size of a file icon, in points. System icons are
/// multi-representation — a fixed contract stands in for a size that
/// does not exist; the normal use is `.resizable()` plus a frame.
pub const FILE_ICON_SIZE: u32 = 32;

impl ImageEngine for RawImages {
    fn intrinsic(&self, source: &ImageSource) -> Option<(u32, u32)> {
        match source {
            ImageSource::Bytes { bytes, .. } => RawImages::decode_header(bytes),
            ImageSource::FileIcon { .. } => Some((FILE_ICON_SIZE, FILE_ICON_SIZE)),
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
        let cache_key = (source.key(), width, height);
        if let Some(raster) = self.rasters.borrow().get(&cache_key) {
            return Some(Rc::clone(raster));
        }
        let rgba = match source {
            ImageSource::Bytes { bytes, .. } => {
                let dimensions = RawImages::decode_header(bytes)?;
                RawImages::resample(bytes, dimensions, width, height)
            }
            ImageSource::FileIcon { key, .. } => RawImages::checker(*key, width, height),
        };
        let raster = Rc::new(ImageRaster { width, height, rgba });
        let mut rasters = self.rasters.borrow_mut();
        if rasters.len() >= IMAGE_KEEP {
            rasters.clear();
        }
        rasters.insert(cache_key, Rc::clone(&raster));
        Some(raster)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_by_one() -> ImageSource {
        // red then blue, 2×1
        ImageSource::from_bytes(RawImages::encode(
            2,
            1,
            &[255, 0, 0, 255, 0, 0, 255, 255],
        ))
    }

    #[test]
    fn the_raw_format_round_trips() {
        let source = two_by_one();
        assert_eq!(RawImages::default().intrinsic(&source), Some((2, 1)));
        let raster = RawImages::default().raster(&source, 2, 1).expect("pixels");
        assert_eq!(&raster.rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(&raster.rgba[4..], &[0, 0, 255, 255]);
    }

    #[test]
    fn broken_bytes_decode_to_nothing() {
        let broken = ImageSource::from_bytes(&b"not an image"[..]);
        assert_eq!(RawImages::default().intrinsic(&broken), None);
        assert!(RawImages::default().raster(&broken, 8, 8).is_none());
    }

    #[test]
    fn the_resample_is_deterministic_nearest() {
        let source = two_by_one();
        let engine = RawImages::default();
        let raster = engine.raster(&source, 4, 2).expect("pixels");
        // left half red, right half blue, both rows equal
        assert_eq!(&raster.rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(&raster.rgba[8..12], &[0, 0, 255, 255]);
        let again = engine.raster(&source, 4, 2).expect("pixels");
        assert!(Rc::ptr_eq(&raster, &again), "the cache returns the same allocation");
    }

    #[test]
    fn a_file_icon_checkers_by_identity() {
        let engine = RawImages::default();
        let one = engine.raster(&file_icon("src/main.rs"), 16, 16).expect("pixels");
        let same = RawImages::default()
            .raster(&file_icon("src/main.rs"), 16, 16)
            .expect("pixels");
        let other = engine.raster(&file_icon("src/lib.rs"), 16, 16).expect("pixels");
        assert_eq!(one.rgba, same.rgba, "same path, same pixels, any engine");
        assert_ne!(one.rgba, other.rgba, "distinct paths read distinct");
    }

    #[test]
    fn identity_compares_and_debug_stays_small() {
        let source = two_by_one();
        assert_eq!(source, source.clone());
        assert_ne!(source, ImageSource::from_bytes(&b"bnyr\x01\x00\x00\x00\x01\x00\x00\x00AAAA"[..]));
        let printed = format!("{source:?}");
        assert!(printed.starts_with("bytes(0x"), "{printed}");
        assert!(printed.len() < 40, "debug never spills content: {printed}");
        assert_eq!(format!("{:?}", file_icon("a.rs")), "file-icon(a.rs)");
    }

    #[test]
    fn the_keyed_exit_skips_hashing() {
        let keyed = ImageSource::bytes_keyed(7, RawImages::encode(1, 1, &[1, 2, 3, 4]));
        assert_eq!(keyed.key(), 7);
    }
}
