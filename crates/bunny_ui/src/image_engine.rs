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
    /// A vector glyph the HOUSE draws, already tinted. `key` folds the
    /// symbol AND the ink — a re-tint IS a new identity, so the caches,
    /// the GPU atlas and the damage diff work untouched (the contract
    /// the text atlas has kept since day one). No engine ever sees this
    /// variant: [`raster_source`] intercepts it first.
    Symbol { key: u64, symbol: crate::icon::Symbol, color: crate::layout::Color },
    /// A path the app TRACED while the frame ran — the runtime twin of
    /// the glyph: the verbs come from data (a squiggle under a word, a
    /// lane of a commit graph, a sparkline), so nothing about it can be
    /// a `const` table. `verbs` already sit inside their own box, whose
    /// point size is `box_size`, and the key folds geometry, paint and
    /// ink together. No engine ever sees this variant either.
    Path {
        key: u64,
        verbs: Rc<[crate::icon::Verb]>,
        paint: crate::icon::Paint,
        color: crate::layout::Color,
        box_size: (f32, f32),
    },
    /// Any source, seen through a VEIL — what `.opacity(…)` leaves for
    /// the pixel pipelines, where there is no offscreen layer to fade.
    /// The fade rides the identity, so the compositor, the GPU atlas
    /// and the damage diff need to learn nothing: a faded image is
    /// simply another image.
    Faded { key: u64, inner: Rc<ImageSource>, alpha: u8 },
}

/// Domain tags folded into the key so the two variants never share an
/// identity by accident.
const BYTES_TAG: u64 = 0x62_6e_79_5f_62_79_74_65; // "bny_byte"
const ICON_TAG: u64 = 0x62_6e_79_5f_69_63_6f_6e; // "bny_icon"
const PATH_TAG: u64 = 0x62_6e_79_5f_70_61_74_68; // "bny_path"
const FADE_TAG: u64 = 0x62_6e_79_5f_66_61_64_65; // "bny_fade"

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

    /// A tinted glyph — built at PLACEMENT, where the ink is known.
    /// One 64-bit mix per icon per frame: the symbol's key is already
    /// well spread, the tint only has to move it somewhere unique.
    pub fn symbol(symbol: crate::icon::Symbol, color: crate::layout::Color) -> ImageSource {
        let packed = ((color.r as u64) << 24)
            | ((color.g as u64) << 16)
            | ((color.b as u64) << 8)
            | color.a as u64;
        let mut key = symbol.key ^ packed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        key = key.wrapping_mul(0xff51_afd7_ed55_8ccd);
        key ^= key >> 33;
        ImageSource::Symbol { key, symbol, color }
    }

    /// A traced path — built at PAINT, where the geometry is known.
    /// The hash walks the table ONCE per call: a few dozen numbers,
    /// which is the price of an identity for something that has no
    /// name. Keep the tables short and the frame never notices.
    pub fn path(
        verbs: impl Into<Rc<[crate::icon::Verb]>>,
        paint: crate::icon::Paint,
        color: crate::layout::Color,
        box_size: (f32, f32),
    ) -> ImageSource {
        use crate::icon::{Paint, Verb};
        let verbs = verbs.into();
        let mut hasher = motor::hash::FxHasher::default();
        hasher.write_u64(PATH_TAG);
        hasher.write_usize(verbs.len());
        let number = |hasher: &mut motor::hash::FxHasher, value: f32| {
            hasher.write_u32(value.to_bits())
        };
        for verb in verbs.iter() {
            match *verb {
                Verb::Move(x, y) => {
                    hasher.write_u8(0);
                    number(&mut hasher, x);
                    number(&mut hasher, y);
                }
                Verb::Line(x, y) => {
                    hasher.write_u8(1);
                    number(&mut hasher, x);
                    number(&mut hasher, y);
                }
                Verb::Quad(cx, cy, x, y) => {
                    hasher.write_u8(2);
                    for value in [cx, cy, x, y] {
                        number(&mut hasher, value);
                    }
                }
                Verb::Cubic(ax, ay, bx, by, x, y) => {
                    hasher.write_u8(3);
                    for value in [ax, ay, bx, by, x, y] {
                        number(&mut hasher, value);
                    }
                }
                Verb::Close => hasher.write_u8(4),
            }
        }
        match paint {
            Paint::Fill(rule) => {
                hasher.write_u8(5);
                hasher.write_u8(rule as u8);
            }
            Paint::Stroke { width } => {
                hasher.write_u8(6);
                number(&mut hasher, width);
            }
        }
        hasher.write_u32(
            (color.r as u32) << 24
                | (color.g as u32) << 16
                | (color.b as u32) << 8
                | color.a as u32,
        );
        number(&mut hasher, box_size.0);
        number(&mut hasher, box_size.1);
        ImageSource::Path { key: hasher.finish(), verbs, paint, color, box_size }
    }

    /// The same source behind a veil. `1.0` gives the source back
    /// untouched — a fade that changes nothing must not cost an
    /// identity, or every cache would hold the picture twice.
    pub fn faded(&self, opacity: f64) -> ImageSource {
        let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        if alpha == 255 {
            return self.clone();
        }
        // a veil over a veil multiplies, and only the OUTER one stays:
        // the identity of a stack of fades is one number
        let (inner, alpha) = match self {
            ImageSource::Faded { inner, alpha: under, .. } => (
                Rc::clone(inner),
                ((*under as u32 * alpha as u32 + 127) / 255) as u8,
            ),
            other => (Rc::new(other.clone()), alpha),
        };
        let mut hasher = motor::hash::FxHasher::default();
        hasher.write_u64(FADE_TAG);
        hasher.write_u64(inner.key());
        hasher.write_u8(alpha);
        ImageSource::Faded { key: hasher.finish(), inner, alpha }
    }

    /// The cheap identity — what diffs, caches and the wire carry.
    pub fn key(&self) -> u64 {
        match self {
            ImageSource::Bytes { key, .. }
            | ImageSource::FileIcon { key, .. }
            | ImageSource::Symbol { key, .. }
            | ImageSource::Path { key, .. }
            | ImageSource::Faded { key, .. } => *key,
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
            (
                ImageSource::Symbol { key, .. },
                ImageSource::Symbol { key: other_key, .. },
            )
            | (
                ImageSource::Path { key, .. },
                ImageSource::Path { key: other_key, .. },
            )
            | (
                ImageSource::Faded { key, .. },
                ImageSource::Faded { key: other_key, .. },
            ) => key == other_key,
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
            ImageSource::Symbol { symbol, color, .. } => write!(
                f,
                "symbol({}, #{:02x}{:02x}{:02x}{:02x})",
                symbol.name, color.r, color.g, color.b, color.a
            ),
            ImageSource::Path { key, verbs, .. } => {
                write!(f, "path(0x{key:016x}, {} verbs)", verbs.len())
            }
            ImageSource::Faded { inner, alpha, .. } => {
                write!(f, "faded({inner:?}, {alpha})")
            }
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

/// The ONE door every pipeline asks for pixels through. A vector glyph
/// never reaches the platform: the house rasterizes it, so the CPU
/// compositor, the GPU atlas and the browser canvas consume literally
/// the same bytes — parity by construction, not by agreement. Anything
/// else is the platform's to decode.
pub fn raster_source(
    engine: &dyn ImageEngine,
    source: &ImageSource,
    width: usize,
    height: usize,
) -> Option<Rc<ImageRaster>> {
    match source {
        ImageSource::Symbol { key, symbol, color } => {
            crate::icon::raster(*key, symbol, *color, width, height)
        }
        ImageSource::Path { key, verbs, paint, color, box_size } => {
            crate::icon::raster_trace(*key, verbs, *paint, *color, *box_size, width, height)
        }
        ImageSource::Faded { key, inner, alpha } => {
            fade_raster(*key, engine, inner, *alpha, width, height)
        }
        _ => engine.raster(source, width, height),
    }
}

/// The intrinsic twin. A glyph has no natural pixel size — the grid
/// square stands in, the way [`FILE_ICON_SIZE`] stands in for the
/// workspace icons; the normal use is the icon view, which sizes off
/// the FONT and never asks.
pub fn intrinsic_of(engine: &dyn ImageEngine, source: &ImageSource) -> Option<(u32, u32)> {
    match source {
        ImageSource::Symbol { .. } => {
            let grid = crate::icon::ICON_GRID as u32;
            Some((grid, grid))
        }
        // a traced path IS its box — the painter sized it from the
        // geometry, so there is nothing to resample against
        ImageSource::Path { box_size, .. } => {
            Some((box_size.0.round() as u32, box_size.1.round() as u32))
        }
        // a veil never changes a size
        ImageSource::Faded { inner, .. } => intrinsic_of(engine, inner),
        _ => engine.intrinsic(source),
    }
}

/// How many faded copies stay warm. A veil is a rare thing on a
/// picture — the crossfade the modifier exists for lands on glyphs,
/// which are small.
const FADE_KEEP: usize = 64;

thread_local! {
    static FADED: RefCell<HashMap<(u64, usize, usize), Rc<ImageRaster>>> =
        RefCell::new(HashMap::default());
}

/// The source's own pixels with the veil multiplied in — straight
/// alpha, so the fade is one multiply per pixel and the chroma never
/// moves.
fn fade_raster(
    key: u64,
    engine: &dyn ImageEngine,
    inner: &ImageSource,
    alpha: u8,
    width: usize,
    height: usize,
) -> Option<Rc<ImageRaster>> {
    if let Some(hit) = FADED.with(|cache| cache.borrow().get(&(key, width, height)).cloned()) {
        return Some(hit);
    }
    let source = raster_source(engine, inner, width, height)?;
    let mut rgba = source.rgba.clone();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[3] = ((pixel[3] as u32 * alpha as u32 + 127) / 255) as u8;
    }
    let faded = Rc::new(ImageRaster { width: source.width, height: source.height, rgba });
    FADED.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= FADE_KEEP {
            cache.clear();
        }
        cache.insert((key, width, height), Rc::clone(&faded));
    });
    Some(faded)
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
            ImageSource::Symbol { .. }
            | ImageSource::Path { .. }
            | ImageSource::Faded { .. } => {
                // the door intercepts what the house draws before any
                // engine — a regression at a call site should be LOUD
                debug_assert!(false, "a house drawing never reaches an engine");
                None
            }
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
            ImageSource::Symbol { .. }
            | ImageSource::Path { .. }
            | ImageSource::Faded { .. } => {
                debug_assert!(false, "a house drawing never reaches an engine");
                return None;
            }
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

    // MARK: - The veil (dor 25)

    #[test]
    fn a_veil_multiplies_the_pixels_and_moves_the_identity() {
        use crate::icon::house;
        use crate::layout::Color;

        let engine = RawImages::default();
        let ink = Color { r: 255, g: 255, b: 255, a: 255 };
        let solid = ImageSource::symbol(house::CLOSE, ink);
        let half = solid.faded(0.5);
        assert_ne!(solid.key(), half.key(), "a fade is a new identity");
        assert_eq!(solid.faded(1.0).key(), solid.key(), "and no fade is no cost");

        let full = raster_source(&engine, &solid, 24, 24).expect("the glyph rasterizes");
        let faded = raster_source(&engine, &half, 24, 24).expect("so does the veil over it");
        assert_eq!(full.width, faded.width);
        let alphas = |raster: &ImageRaster| -> Vec<u8> {
            raster.rgba.chunks_exact(4).map(|pixel| pixel[3]).collect()
        };
        let (before, after) = (alphas(&full), alphas(&faded));
        assert!(before.iter().any(|alpha| *alpha > 200), "the mark is there to fade");
        for (solid, faded) in before.iter().zip(&after) {
            assert_eq!(*faded, ((*solid as u32 * 128 + 127) / 255) as u8);
        }
    }

    #[test]
    fn a_veil_over_a_veil_is_one_veil() {
        use crate::icon::house;
        use crate::layout::Color;

        let source = ImageSource::symbol(house::CHECK, Color::BLACK);
        let twice = source.faded(0.5).faded(0.5);
        match &twice {
            ImageSource::Faded { inner, alpha, .. } => {
                assert_eq!(inner.key(), source.key(), "the stack never grows");
                assert_eq!(*alpha, 64, "the fades multiply");
            }
            other => panic!("a fade must stay a fade: {other:?}"),
        }
    }
}
