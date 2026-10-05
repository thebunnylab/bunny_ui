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
    ///
    /// `forced` spends the drawing's own palette: every draw takes
    /// `color`, whatever tint it declares. It rides the key like the
    /// ink does, so the two readings of one glyph are two identities.
    Symbol {
        key: u64,
        symbol: crate::icon::Symbol,
        color: crate::layout::Color,
        forced: bool,
    },
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
        ink: crate::icon::Ink,
        box_size: (f32, f32),
    },
    /// Pixels the APP already holds: an RGBA buffer it filled itself,
    /// straight through the one image door. The app owns what the bytes
    /// mean (a decoded frame, a computed field, a heat map); the house
    /// only resamples and uploads them, so every tier draws it the way
    /// it draws any other image. No engine sees this variant either —
    /// there is nothing to decode.
    Rgba { key: u64, size: (u32, u32), rgba: Rc<[u8]> },
    /// Immutable GPU frame. The platform imports its owned payload directly;
    /// CPU/headless renderers never download it implicitly. The producer must
    /// publish only completed frames and retain resources through this owner.
    Native { key: u64, size: (u32, u32), payload: std::sync::Arc<dyn std::any::Any + Send + Sync> },
    /// A picture whose BYTES change every frame and whose identity does
    /// not — a camera, a decoded video, a chart redrawing itself. `key`
    /// is the SLOT: the texture a tier keeps for it. `generation` moves
    /// with every new frame and rides the equality, so a new frame is
    /// damage while the slot stays the same texture. The GPU scales it
    /// to its box with a linear sampler; nothing is resampled on the
    /// CPU, nothing is minted per frame, and the shared atlas never
    /// hears of it. [`ImageFeed`] is the handle that mints these.
    Feed { key: u64, generation: u64, size: (u32, u32), format: PixelFormat, bytes: Rc<[u8]> },
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
const RGBA_TAG: u64 = 0x62_6e_79_5f_72_67_62_61; // "bny_rgba"
const FEED_TAG: u64 = 0x626e_795f_6665_6564; // "bny_feed"

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

    /// Pixels the app filled ITSELF — straight RGBA, `width × height × 4`,
    /// row major, straight alpha (what the house compositor blends
    /// everywhere). A decoded video frame, a computed field, a heat map:
    /// the app owns the arithmetic, the house resamples and uploads.
    ///
    /// The identity is the app's, like [`ImageSource::bytes_keyed`] —
    /// hashing a megapixel every frame would cost more than the upload
    /// it saves. Give a buffer that CHANGES a new key (a frame counter
    /// is enough); give a still one a constant, and every tier caches it
    /// like any other picture.
    pub fn rgba(key: u64, size: (u32, u32), rgba: impl Into<Rc<[u8]>>) -> ImageSource {
        let rgba = rgba.into();
        debug_assert_eq!(
            rgba.len(),
            (size.0 as usize) * (size.1 as usize) * 4,
            "an RGBA buffer is width × height × 4 bytes"
        );
        ImageSource::Rgba { key: key ^ RGBA_TAG, size, rgba }
    }

    /// One frame of a feed, for an app that keeps its own slots and
    /// counts its own frames — [`ImageFeed`] does both for everyone
    /// else. `generation` must climb with every new frame of the slot;
    /// the bytes are straight RGBA, `width × height × 4`.
    pub fn feed(
        key: FeedKey,
        generation: u64,
        size: (u32, u32),
        rgba: impl Into<Rc<[u8]>>,
    ) -> ImageSource {
        let bytes = rgba.into();
        debug_assert_eq!(
            bytes.len(),
            PixelFormat::Rgba8.bytes_for(size),
            "an RGBA feed frame is width × height × 4 bytes"
        );
        ImageSource::Feed { key: key.0, generation, size, format: PixelFormat::Rgba8, bytes }
    }

    /// A tinted glyph — built at PLACEMENT, where the ink is known.
    /// One 64-bit mix per icon per frame: the symbol's key is already
    /// well spread, the tint only has to move it somewhere unique.
    ///
    /// A draw that declares its OWN tint keeps it: the crab stays
    /// orange under any ink, which is what a file-type set is for.
    /// [`ImageSource::symbol_forced`] is the other reading.
    pub fn symbol(symbol: crate::icon::Symbol, color: crate::layout::Color) -> ImageSource {
        ImageSource::inked(symbol, color, false)
    }

    /// The same glyph read as a MASK: this ink, and nothing else. Every
    /// draw takes `color`, and the palette the drawing carries is spent.
    ///
    /// A monochrome bar wants this. On a stripe the ink IS the state —
    /// dim while the panel sleeps, accent while it is open — and a slot
    /// that borrows a file-type glyph so the set grows without new art
    /// would otherwise be the one icon on the bar that answers nothing.
    ///
    /// It is the exact inverse of the per-draw tint, and both readings
    /// are right: a tree names a language and wants its colours; a bar
    /// names a state and wants one.
    pub fn symbol_forced(
        symbol: crate::icon::Symbol,
        color: crate::layout::Color,
    ) -> ImageSource {
        ImageSource::inked(symbol, color, true)
    }

    /// The one mix both readings go through. `forced` sits above the
    /// packed colour, so an unforced glyph keeps the identity it has
    /// always had and the forced twin can never land on it.
    fn inked(
        symbol: crate::icon::Symbol,
        color: crate::layout::Color,
        forced: bool,
    ) -> ImageSource {
        let packed = ((forced as u64) << 32)
            | ((color.r as u64) << 24)
            | ((color.g as u64) << 16)
            | ((color.b as u64) << 8)
            | color.a as u64;
        let mut key = symbol.key ^ packed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        key = key.wrapping_mul(0xff51_afd7_ed55_8ccd);
        key ^= key >> 33;
        ImageSource::Symbol { key, symbol, color, forced }
    }

    /// A traced path — built at PAINT, where the geometry is known.
    /// The hash walks the table ONCE per call: a few dozen numbers,
    /// which is the price of an identity for something that has no
    /// name. Keep the tables short and the frame never notices.
    pub fn path(
        verbs: impl Into<Rc<[crate::icon::Verb]>>,
        paint: crate::icon::Paint,
        ink: crate::icon::Ink,
        box_size: (f32, f32),
    ) -> ImageSource {
        use crate::icon::{Ink, Paint, Verb};
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
        // the ink is part of the identity, exactly like a symbol's
        // tint: a path repainted through another ramp is another tile
        let colour = |hasher: &mut motor::hash::FxHasher, color: crate::layout::Color| {
            hasher.write_u32(
                (color.r as u32) << 24
                    | (color.g as u32) << 16
                    | (color.b as u32) << 8
                    | color.a as u32,
            );
        };
        match ink {
            Ink::Solid(color) => {
                hasher.write_u8(7);
                colour(&mut hasher, color);
            }
            Ink::Ramp(ramp) => {
                hasher.write_u8(8);
                match ramp {
                    crate::layout::Gradient::Linear { start, end, from, to } => {
                        hasher.write_u8(0);
                        for value in [start.x, start.y, end.x, end.y] {
                            hasher.write_u64(value.to_bits());
                        }
                        colour(&mut hasher, from);
                        colour(&mut hasher, to);
                    }
                    crate::layout::Gradient::Radial {
                        center,
                        start,
                        end,
                        aspect,
                        inner,
                        outer,
                    } => {
                        hasher.write_u8(1);
                        for value in
                            [center.x, center.y, start, end.unwrap_or(f64::NAN), aspect]
                        {
                            hasher.write_u64(value.to_bits());
                        }
                        colour(&mut hasher, inner);
                        colour(&mut hasher, outer);
                    }
                }
            }
        }
        number(&mut hasher, box_size.0);
        number(&mut hasher, box_size.1);
        ImageSource::Path { key: hasher.finish(), verbs, paint, ink, box_size }
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
        // a feed's key is its SLOT: the veil over frame 40 must not be
        // the veil over frame 41, or the faded cache would show a stale
        // picture through it
        if let ImageSource::Feed { generation, .. } = &*inner {
            hasher.write_u64(*generation);
        }
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
            | ImageSource::Native { key, .. }
            | ImageSource::Rgba { key, .. }
            | ImageSource::Feed { key, .. }
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
            ) => key == other_key,
            (
                ImageSource::Native { key, size, .. },
                ImageSource::Native { key: other_key, size: other_size, .. },
            ) => key == other_key && size == other_size,
            // the slot AND the frame: two generations of one feed are two
            // images to the damage diff, one texture to the tier
            (
                ImageSource::Feed { key, generation, size, .. },
                ImageSource::Feed {
                    key: other_key,
                    generation: other_generation,
                    size: other_size,
                    ..
                },
            ) => key == other_key && generation == other_generation && size == other_size,
            (
                ImageSource::Rgba { key, .. },
                ImageSource::Rgba { key: other_key, .. },
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
            ImageSource::Symbol { symbol, color, forced, .. } => write!(
                f,
                "symbol({}, #{:02x}{:02x}{:02x}{:02x}{})",
                symbol.name,
                color.r,
                color.g,
                color.b,
                color.a,
                // only the forced reading says so: every print that
                // came before this door keeps the words it had
                if *forced { ", forced" } else { "" }
            ),
            ImageSource::Path { key, verbs, .. } => {
                write!(f, "path(0x{key:016x}, {} verbs)", verbs.len())
            }
            ImageSource::Native { key, size, .. } => write!(f, "native(0x{key:016x}, {}×{})", size.0, size.1),
            ImageSource::Rgba { key, size, .. } => {
                write!(f, "rgba(0x{key:016x}, {}×{})", size.0, size.1)
            }
            ImageSource::Feed { key, generation, size, .. } => {
                write!(f, "feed(0x{key:016x}, {}×{} #{generation})", size.0, size.1)
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

    /// Lets go of every cached decode and raster: the next frame decodes
    /// again what it still shows. The door a memory warning opens — the
    /// caches are a convenience, and a system that asks for its memory
    /// back gets them before it takes the app. The default keeps nothing
    /// to let go.
    fn drop_caches(&self) {}

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
/// Nearest-neighbor into the destination size — integer division keeps
/// it deterministic on every machine, which is what lets the CPU and the
/// GPU tiers assert byte equality.
fn resample_rgba(source: &[u8], (src_w, src_h): (u32, u32), width: usize, height: usize) -> Vec<u8> {
    let (src_w, src_h) = (src_w as usize, src_h as usize);
    let mut rgba = vec![0u8; width * height * 4];
    if src_w == 0 || src_h == 0 {
        return rgba;
    }
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

pub fn raster_source(
    engine: &dyn ImageEngine,
    source: &ImageSource,
    width: usize,
    height: usize,
) -> Option<Rc<ImageRaster>> {
    match source {
        ImageSource::Native { .. } => None,
        ImageSource::Symbol { key, symbol, color, forced } => {
            crate::icon::raster(*key, symbol, *color, *forced, width, height)
        }
        ImageSource::Path { key, verbs, paint, ink, box_size } => {
            crate::icon::raster_trace(*key, verbs, *paint, *ink, *box_size, width, height)
        }
        // the app's own pixels: no decode, no platform — resampled to
        // the box the frame asked for and handed on
        ImageSource::Rgba { size, rgba, .. } => {
            if width == 0 || height == 0 || size.0 == 0 || size.1 == 0 {
                return None;
            }
            Some(Rc::new(ImageRaster {
                width,
                height,
                rgba: resample_rgba(rgba, *size, width, height),
            }))
        }
        // a feed scales BILINEAR, like the sampler the GPU tiers read it
        // with, once per generation and size — the damage replay asks
        // several times a frame
        ImageSource::Feed { key, generation, size, format, bytes } => {
            if width == 0 || height == 0 || size.0 == 0 || size.1 == 0 {
                return None;
            }
            Some(feed_raster(*key, *generation, *size, *format, bytes, width, height))
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
        // the app declared its own box when it handed the pixels over
        ImageSource::Rgba { size, .. }
        | ImageSource::Native { size, .. }
        | ImageSource::Feed { size, .. } => Some(*size),
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

// MARK: - Feeds: a picture whose bytes change and whose identity stays

/// The pixel layout of a feed's bytes. RGBA8 today; a planar layout
/// (NV12, I420) would be named here, with an upload arm per ground and
/// a conversion in the shader — the enum is open for it.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// Straight RGBA, 8 bits a channel, row major: `width × height × 4`.
    Rgba8,
}

impl PixelFormat {
    /// How many bytes `size` pixels take in this layout.
    pub fn bytes_for(self, size: (u32, u32)) -> usize {
        match self {
            PixelFormat::Rgba8 => size.0 as usize * size.1 as usize * 4,
        }
    }
}

/// The identity of one feed — the SLOT a tier keeps a texture for. An
/// app that numbers its own slots builds one with [`FeedKey::new`]; the
/// keys [`ImageFeed::new`] mints keep the top bit set, so the two never
/// meet while the app's numbers stay below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FeedKey(u64);

impl FeedKey {
    /// A slot of the app's own numbering.
    pub fn new(slot: u64) -> FeedKey {
        FeedKey(slot ^ FEED_TAG)
    }

    /// The number the tiers know the slot by.
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// The slots `ImageFeed::new` mints, counted once for the process — a
/// feed made on any thread never shares a slot with another.
static NEXT_FEED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct FeedFrame {
    generation: u64,
    size: (u32, u32),
    bytes: Rc<[u8]>,
}

/// The app's end of a feed: a picture whose BYTES change every frame
/// and whose identity does not — a camera, a decoded video, a chart
/// that redraws itself. Mint one per picture and keep it; `push` the
/// bytes as they arrive; draw it like any image:
///
/// ```ignore
/// let camera = ImageFeed::new();                 // once, in state
/// camera.push((640, 360), rgba);                 // per frame
/// image(&camera).resizable().aspect_ratio(ContentMode::Fill)
/// painter.image(rect, &camera);
/// ```
///
/// Every tier keeps ONE texture per feed, sized to the picture,
/// replaces its bytes in place when the generation moves, and scales it
/// to its box on the GPU with a linear sampler. No resample on the CPU,
/// no texture per frame, and the shared atlas never hears of it. The
/// CPU oracle scales it bilinear, so the two roads agree within a step.
#[derive(Clone)]
pub struct ImageFeed {
    key: FeedKey,
    latest: Rc<RefCell<FeedFrame>>,
}

impl Default for ImageFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageFeed {
    /// An empty feed with a slot of its own. Nothing paints until the
    /// first `push`.
    pub fn new() -> ImageFeed {
        let serial = NEXT_FEED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        ImageFeed {
            key: FeedKey((serial | (1 << 63)) ^ FEED_TAG),
            latest: Rc::new(RefCell::new(FeedFrame {
                generation: 0,
                size: (0, 0),
                bytes: Rc::from(Vec::new()),
            })),
        }
    }

    /// The next frame: straight RGBA, `width × height × 4`. Answers the
    /// generation it became. One `Rc` move — no hash, no copy.
    pub fn push(&self, size: (u32, u32), rgba: impl Into<Rc<[u8]>>) -> u64 {
        let bytes = rgba.into();
        debug_assert_eq!(
            bytes.len(),
            PixelFormat::Rgba8.bytes_for(size),
            "an RGBA feed frame is width × height × 4 bytes"
        );
        let mut latest = self.latest.borrow_mut();
        latest.generation = latest.generation.wrapping_add(1);
        latest.size = size;
        latest.bytes = bytes;
        latest.generation
    }

    /// The slot.
    pub fn key(&self) -> FeedKey {
        self.key
    }

    /// How many frames were pushed — zero while the feed is empty.
    pub fn generation(&self) -> u64 {
        self.latest.borrow().generation
    }

    /// The picture's size, once something was pushed.
    pub fn size(&self) -> Option<(u32, u32)> {
        let latest = self.latest.borrow();
        (latest.generation != 0).then_some(latest.size)
    }

    /// The newest frame as a source — what `image(&feed)` and
    /// `painter.image(rect, &feed)` take through `Into`.
    pub fn source(&self) -> ImageSource {
        let latest = self.latest.borrow();
        ImageSource::Feed {
            key: self.key.0,
            generation: latest.generation,
            size: latest.size,
            format: PixelFormat::Rgba8,
            bytes: Rc::clone(&latest.bytes),
        }
    }
}

impl From<&ImageFeed> for ImageSource {
    fn from(feed: &ImageFeed) -> ImageSource {
        feed.source()
    }
}

impl From<ImageFeed> for ImageSource {
    fn from(feed: ImageFeed) -> ImageSource {
        feed.source()
    }
}

/// How many feed rasters stay warm — one per feed and size on screen.
/// A feed is a camera or a video: a handful on a screen, never a crowd.
const FEED_KEEP: usize = 32;

struct FeedRaster {
    generation: u64,
    raster: Rc<ImageRaster>,
}

thread_local! {
    static FEEDS: RefCell<HashMap<(u64, usize, usize), FeedRaster>> =
        RefCell::new(HashMap::default());
}

/// The feed's newest frame at one physical size, bilinear — once per
/// generation and size. The CPU surface replays a command once per
/// damage rect, and a video frame under three rects must cost one
/// resample, not three.
fn feed_raster(
    key: u64,
    generation: u64,
    size: (u32, u32),
    format: PixelFormat,
    bytes: &[u8],
    width: usize,
    height: usize,
) -> Rc<ImageRaster> {
    let slot = (key, width, height);
    let warm = FEEDS.with(|cache| {
        cache.borrow().get(&slot).and_then(|feed| {
            (feed.generation == generation).then(|| Rc::clone(&feed.raster))
        })
    });
    if let Some(raster) = warm {
        return raster;
    }
    let rgba = match format {
        PixelFormat::Rgba8 => resample_rgba_bilinear(bytes, size, width, height),
    };
    let raster = Rc::new(ImageRaster { width, height, rgba });
    FEEDS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= FEED_KEEP && !cache.contains_key(&slot) {
            cache.clear();
        }
        cache.insert(slot, FeedRaster { generation, raster: Rc::clone(&raster) });
    });
    raster
}

/// Bilinear into the destination size, every channel on its own over
/// straight alpha — the arithmetic a GPU's linear sampler does over the
/// same texels, in 16.16 fixed point so every machine agrees. At 1:1 it
/// is a copy, byte for byte; a source shorter than its size paints
/// nothing rather than reading past its end.
fn resample_rgba_bilinear(
    source: &[u8],
    (src_w, src_h): (u32, u32),
    width: usize,
    height: usize,
) -> Vec<u8> {
    let (src_w, src_h) = (src_w as usize, src_h as usize);
    if src_w == 0 || src_h == 0 || source.len() < src_w * src_h * 4 {
        return vec![0u8; width * height * 4];
    }
    if src_w == width && src_h == height {
        return source[..width * height * 4].to_vec();
    }
    // destination pixel `i` of `n` samples the source of `m` at
    // (i + ½)·m/n − ½: the texel below, the one after it, and the
    // weight between them in 8 bits — clamped at both edges
    let axis = |n: usize, m: usize| -> Vec<(usize, usize, u32)> {
        (0..n)
            .map(|i| {
                let centred = (((2 * i as u64 + 1) * m as u64) << 16) / (2 * n as u64);
                let fixed = centred.saturating_sub(1 << 15);
                let low = ((fixed >> 16) as usize).min(m - 1);
                let high = (low + 1).min(m - 1);
                let weight = ((fixed & 0xffff) >> 8) as u32;
                (low, high, weight)
            })
            .collect()
    };
    let columns = axis(width, src_w);
    let rows = axis(height, src_h);
    let mut rgba = vec![0u8; width * height * 4];
    for (y, &(y0, y1, wy)) in rows.iter().enumerate() {
        let (row0, row1) = (y0 * src_w, y1 * src_w);
        for (x, &(x0, x1, wx)) in columns.iter().enumerate() {
            let at = |row: usize, col: usize| &source[(row + col) * 4..(row + col) * 4 + 4];
            let (p00, p01, p10, p11) = (at(row0, x0), at(row0, x1), at(row1, x0), at(row1, x1));
            let to = (y * width + x) * 4;
            for channel in 0..4 {
                let top = p00[channel] as u32 * (256 - wx) + p01[channel] as u32 * wx;
                let bottom = p10[channel] as u32 * (256 - wx) + p11[channel] as u32 * wx;
                rgba[to + channel] = ((top * (256 - wy) + bottom * wy + (1 << 15)) >> 16) as u8;
            }
        }
    }
    rgba
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
        size: (u32, u32),
        width: usize,
        height: usize,
    ) -> Vec<u8> {
        resample_rgba(&bytes[RAW_HEADER..], size, width, height)
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
            | ImageSource::Native { .. }
            | ImageSource::Rgba { .. }
            | ImageSource::Feed { .. }
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
            | ImageSource::Native { .. }
            | ImageSource::Rgba { .. }
            | ImageSource::Feed { .. }
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

    /// The app's own pixels land exactly where the same pixels land
    /// after a trip through the house's encoded format — same door, same
    /// resample, so every tier draws the two identically.
    #[test]
    fn the_apps_own_pixels_match_the_encoded_road() {
        let pixels: &[u8] = &[255, 0, 0, 255, 0, 0, 255, 255];
        let engine = RawImages::default();
        let direct = ImageSource::rgba(7, (2, 1), pixels);
        assert_eq!(intrinsic_of(&engine, &direct), Some((2, 1)));
        for (width, height) in [(2, 1), (4, 2), (1, 1)] {
            let ours = raster_source(&engine, &direct, width, height).expect("pixels");
            let encoded = raster_source(&engine, &two_by_one(), width, height).expect("pixels");
            assert_eq!(ours.rgba, encoded.rgba, "{width}x{height} resamples the same");
        }
        // the identity is the app's, and a new key IS a new image
        assert_eq!(direct, ImageSource::rgba(7, (2, 1), pixels));
        assert_ne!(direct, ImageSource::rgba(8, (2, 1), pixels));
        // no engine ever sees it — the door answers first
        assert_eq!(format!("{direct:?}"), format!("rgba(0x{:016x}, 2×1)", direct.key()));
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

    // MARK: - Feeds

    #[test]
    fn a_feed_changes_its_bytes_but_not_its_identity() {
        let feed = ImageFeed::new();
        assert_eq!(feed.generation(), 0);
        assert_eq!(feed.size(), None, "nothing was pushed yet");
        let empty = feed.source();
        assert!(
            raster_source(&RawImages::default(), &empty, 4, 4).is_none(),
            "an empty feed paints nothing"
        );

        assert_eq!(feed.push((2, 1), vec![255, 0, 0, 255, 0, 0, 255, 255]), 1);
        let first = feed.source();
        assert_eq!(feed.push((2, 1), vec![0, 255, 0, 255, 0, 0, 255, 255]), 2);
        let second = feed.source();
        // the slot is the identity the tiers keep a texture by…
        assert_eq!(first.key(), second.key());
        assert_eq!(first.key(), feed.key().raw());
        // …and the generation is what makes a new frame a new image to
        // the damage diff
        assert_ne!(first, second);
        assert_eq!(second, feed.source());
        assert_eq!(feed.size(), Some((2, 1)));
        assert_eq!(intrinsic_of(&RawImages::default(), &second), Some((2, 1)));
        assert_eq!(
            format!("{second:?}"),
            format!("feed(0x{:016x}, 2×1 #2)", feed.key().raw())
        );
        // two feeds never share a slot, and an app's own slot never
        // meets a minted one
        assert_ne!(ImageFeed::new().key(), feed.key());
        assert_ne!(FeedKey::new(1).raw(), ImageFeed::new().key().raw());
        assert_eq!(ImageSource::feed(FeedKey::new(9), 3, (1, 1), vec![1, 2, 3, 4]).key(), FeedKey::new(9).raw());
    }

    #[test]
    fn a_feed_resamples_bilinear_and_is_exact_at_one_to_one() {
        let engine = RawImages::default();
        let feed = ImageFeed::new();
        // red | blue, one row
        feed.push((2, 1), vec![255, 0, 0, 255, 0, 0, 255, 255]);
        let exact = raster_source(&engine, &feed.source(), 2, 1).expect("pixels");
        assert_eq!(exact.rgba, vec![255, 0, 0, 255, 0, 0, 255, 255], "1:1 is a copy");
        // four across: the two outer pixels sit on their texels, the two
        // inner ones a quarter of the way into the other — never nearest
        let wide = raster_source(&engine, &feed.source(), 4, 1).expect("pixels");
        assert_eq!(&wide.rgba[0..4], &[255, 0, 0, 255], "the left edge clamps to red");
        assert_eq!(&wide.rgba[12..16], &[0, 0, 255, 255], "the right edge clamps to blue");
        let (second, third) = (&wide.rgba[4..8], &wide.rgba[8..12]);
        assert!(second[0] > 128 && second[2] < 128, "mostly red: {second:?}");
        assert!(third[0] < 128 && third[2] > 128, "mostly blue: {third:?}");
        assert_eq!(second[0] + second[2], third[0] + third[2], "the ramp is symmetric");
        // down: the one pixel is the middle of the row
        let narrow = raster_source(&engine, &feed.source(), 1, 1).expect("pixels");
        assert_eq!(&narrow.rgba[..], &[128, 0, 128, 255]);
        // the same generation at the same size is the same allocation —
        // the damage replay never resamples twice
        let again = raster_source(&engine, &feed.source(), 4, 1).expect("pixels");
        assert!(Rc::ptr_eq(&wide, &again));
        feed.push((2, 1), vec![0, 255, 0, 255, 0, 255, 0, 255]);
        let fresh = raster_source(&engine, &feed.source(), 4, 1).expect("pixels");
        assert!(!Rc::ptr_eq(&wide, &fresh), "a new generation is a new raster");
        assert_eq!(&fresh.rgba[0..4], &[0, 255, 0, 255]);
    }

    #[test]
    fn a_veil_over_a_feed_follows_the_generation() {
        let engine = RawImages::default();
        let feed = ImageFeed::new();
        feed.push((1, 1), vec![255, 255, 255, 255]);
        let first = feed.source().faded(0.5);
        feed.push((1, 1), vec![0, 0, 0, 255]);
        let second = feed.source().faded(0.5);
        assert_ne!(first.key(), second.key(), "the veil's identity moves with the frame");
        let shown = raster_source(&engine, &second, 1, 1).expect("pixels");
        assert_eq!(&shown.rgba[..], &[0, 0, 0, 128], "the veil shows the NEW frame");
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
