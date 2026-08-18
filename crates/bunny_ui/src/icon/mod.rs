//! Vector glyphs — resolution-independent drawings the house paints.
//!
//! A glyph is a RECIPE, never pixels: verbs on a fixed grid, plus the
//! paint that turns contours into ink. The house rasterizes the recipe
//! at the exact physical size a frame asks for — crisp at sixteen,
//! crisp at sixty-four — and every pipeline consumes the SAME bytes,
//! so the backends agree byte for byte by construction.
//!
//! The data is `const`-friendly on purpose: a shipped icon is a static
//! table the compiler lays out, with zero startup cost and zero parse.

pub(crate) mod vector;

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use crate::image_engine::ImageRaster;
use crate::layout::{Color, Px};
use motor::hash::FxHashMap as HashMap;

/// The glyph grid — every drawing lives in a `0..24` square. The
/// rasterizers scale this square onto the destination; the Dom mode
/// writes it as the `viewBox`. One constant, three renderers.
pub const ICON_GRID: Px = 24.0;

/// A glyph beside 13pt body text wants SIXTEEN points — the number the
/// house wrote by hand for file icons before the symbol existed. Every
/// other font follows the same ratio.
const ICON_RATIO: Px = 1.25;

/// The natural side of an icon under a font, in points. Derived from
/// `FontSpec::size` — pure data, identical on every target — never
/// from engine metrics, and rounded to a whole point so a stem lands
/// on a pixel edge at one and at two scale.
pub fn natural_size(font: &crate::text_engine::FontSpec) -> Px {
    (font.size * ICON_RATIO).round()
}

/// One drawing instruction, in grid units. Coordinates are `f32`: the
/// grid is small, the table is `const`, and half the bytes reach twice
/// as many glyphs into a cache line.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Verb {
    /// Start a new contour at this point.
    Move(f32, f32),
    /// A straight segment to this point.
    Line(f32, f32),
    /// A quadratic curve: control, then end.
    Quad(f32, f32, f32, f32),
    /// A cubic curve: two controls, then end.
    Cubic(f32, f32, f32, f32, f32, f32),
    /// Seal the contour back to its start. A stroke draws the closing
    /// segment; a fill closes every contour anyway (SVG law).
    Close,
}

/// Which side of a self-crossing contour counts as INSIDE.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Rule {
    /// Winding count ≠ 0 — SVG's default.
    NonZero,
    /// Odd crossing count — the rule that cuts holes.
    EvenOdd,
}

/// How a contour becomes ink.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Paint {
    /// Cover the inside.
    Fill(Rule),
    /// Ride the contour with a pen this wide (grid units). Caps and
    /// joins are ROUND — the one shape the house kernel draws exactly,
    /// and the only one the icon world uses.
    Stroke { width: f32 },
}

/// One paint over one path. A glyph is a short stack of these, painted
/// in order — most icons are a single stroked path; a badge may fill a
/// disc first and stroke a mark on top.
#[derive(Clone, Copy, Debug)]
pub struct Draw {
    pub paint: Paint,
    pub path: &'static [Verb],
}

/// A whole drawing on the [`ICON_GRID`]. This is the type an app's
/// converted icons declare as `const` — the house set in this crate is
/// built from the same stone.
#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub draws: &'static [Draw],
}

/// A glyph with the identity that caches it. House symbols and an
/// app's converted icons are the SAME type, side by side in one call —
/// there is no registry to install and no name to look up at runtime:
/// the name becomes a `u64` while the compiler is still running.
#[derive(Clone, Copy)]
pub struct Symbol {
    /// What the caches, the atlas and the damage diff carry. Distinct
    /// drawings MUST NOT share it — [`Symbol::new`] guarantees that.
    pub key: u64,
    /// The name, for `Debug` and the wire's readability.
    pub name: &'static str,
    /// The drawing itself.
    pub glyph: &'static Glyph,
}

/// The domain tag folded into every symbol key, so a glyph can never
/// collide with an image identity by accident — the image module's
/// `BYTES_TAG`/`ICON_TAG` do the same job on their side.
const SYMBOL_TAG: u64 = 0x62_6e_79_5f_73_79_6d_62; // "bny_symb"

impl Symbol {
    /// The name is hashed ONCE, at compile time. Nothing hashes while
    /// a frame runs.
    pub const fn new(name: &'static str, glyph: &'static Glyph) -> Symbol {
        Symbol { key: fnv1a(SYMBOL_TAG, name.as_bytes()), name, glyph }
    }
}

/// Identity comparison, like every source in the image world.
impl PartialEq for Symbol {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Symbol({})", self.name)
    }
}

/// FNV-1a, seeded by the domain tag — the one hash `std` lets a
/// `const fn` run.
const fn fnv1a(tag: u64, bytes: &[u8]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let tag_bytes = tag.to_be_bytes();
    let mut i = 0;
    while i < tag_bytes.len() {
        hash ^= tag_bytes[i] as u64;
        hash = hash.wrapping_mul(PRIME);
        i += 1;
    }
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(PRIME);
        i += 1;
    }
    hash
}

// MARK: - The house rasterizer

/// How many tinted rasters stay warm before the cache drops them all —
/// the same total-eviction shape the image caches use. Icons are small
/// (a 32×32 tile is four kilobytes); the ceiling is generous.
const ICON_KEEP: usize = 256;

thread_local! {
    /// Rasters by `(tinted key, physical width, physical height)`. The
    /// tint lives INSIDE the key (see `ImageSource::symbol`), so a
    /// theme flip is a cache miss by construction, never a stale hit.
    static RASTERS: RefCell<HashMap<(u64, usize, usize), Rc<ImageRaster>>> =
        RefCell::new(HashMap::default());
}

/// The door `image_engine::raster_source` opens for a symbol: the
/// house rasterizes the glyph — no platform in the loop, so wasm, mac
/// and headless produce literally the same bytes.
pub(crate) fn raster(
    key: u64,
    symbol: &Symbol,
    color: Color,
    width: usize,
    height: usize,
) -> Option<Rc<ImageRaster>> {
    if width == 0 || height == 0 {
        return None;
    }
    RASTERS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(raster) = cache.get(&(key, width, height)) {
            return Some(Rc::clone(raster));
        }
        if cache.len() >= ICON_KEEP {
            cache.clear();
        }
        let raster = Rc::new(rasterize(symbol.glyph, color, width, height));
        cache.insert((key, width, height), Rc::clone(&raster));
        Some(raster)
    })
}

/// One glyph, rasterized: the [`ICON_GRID`] square scales onto the
/// largest CENTRED square of the destination, every draw piles its
/// coverage in by MAX, and the tint lands once at the end — straight
/// alpha, physical pixels, the same contract text rasters keep.
fn rasterize(glyph: &Glyph, color: Color, width: usize, height: usize) -> ImageRaster {
    let side = width.min(height) as f64;
    let scale = side / ICON_GRID;
    let placing = vector::Placing {
        scale,
        dx: (width as f64 - side) / 2.0,
        dy: (height as f64 - side) / 2.0,
    };
    let mut union = vector::Mask::new(width, height);
    let mut scratch = vector::Mask::new(width, height);
    for draw in glyph.draws {
        scratch.clear();
        let flat = vector::flatten(draw.path, placing);
        match draw.paint {
            Paint::Fill(rule) => vector::fill_into(&mut scratch, &flat, rule),
            Paint::Stroke { width: pen } => {
                vector::stroke_into(&mut scratch, &flat, pen as f64 * scale)
            }
        }
        union.merge_max(&scratch);
    }
    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let coverage = union.at(x, y);
            if coverage <= 0.0 {
                continue;
            }
            // the same rounding `set_covered` applies on the way in
            let alpha = (color.a as f64 * coverage as f64).round() as u8;
            if alpha == 0 {
                continue;
            }
            let to = (y * width + x) * 4;
            rgba[to] = color.r;
            rgba[to + 1] = color.g;
            rgba[to + 2] = color.b;
            rgba[to + 3] = alpha;
        }
    }
    ImageRaster { width, height, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE_PATH: &[Verb] = &[
        Verb::Move(4.0, 4.0),
        Verb::Line(20.0, 4.0),
        Verb::Line(20.0, 20.0),
        Verb::Line(4.0, 20.0),
        Verb::Close,
    ];
    const SQUARE_GLYPH: Glyph =
        Glyph { draws: &[Draw { paint: Paint::Fill(Rule::NonZero), path: SQUARE_PATH }] };
    const SQUARE: Symbol = Symbol::new("test.square", &SQUARE_GLYPH);

    const INK: Color = Color { r: 20, g: 40, b: 60, a: 255 };

    #[test]
    fn a_symbol_key_is_stable_and_tagged() {
        // the const hash runs while the compiler does — twice the same
        // name, twice the same key; a different name moves it
        const AGAIN: Symbol = Symbol::new("test.square", &SQUARE_GLYPH);
        assert_eq!(SQUARE.key, AGAIN.key);
        const OTHER: Symbol = Symbol::new("test.circle", &SQUARE_GLYPH);
        assert_ne!(SQUARE.key, OTHER.key);
        // pinned: the key must never drift between builds or targets —
        // caches, the atlas and the Dom wire all carry it
        assert_eq!(SQUARE.key, 0x9330_9594_d5a1_ed53, "0x{:016x}", SQUARE.key);
    }

    #[test]
    fn the_tint_rides_the_source_key() {
        let black = crate::image_engine::ImageSource::symbol(SQUARE, INK);
        let same = crate::image_engine::ImageSource::symbol(SQUARE, INK);
        let white = crate::image_engine::ImageSource::symbol(
            SQUARE,
            Color { r: 255, g: 255, b: 255, a: 255 },
        );
        assert_eq!(black.key(), same.key());
        assert_ne!(black.key(), white.key(), "a re-tint IS a new identity");
        assert_ne!(black.key(), SQUARE.key, "the tinted key leaves the bare one");
    }

    #[test]
    fn the_glyph_cache_returns_the_same_allocation() {
        let first = raster(1234, &SQUARE, INK, 24, 24).unwrap();
        let second = raster(1234, &SQUARE, INK, 24, 24).unwrap();
        assert!(Rc::ptr_eq(&first, &second));
    }

    #[test]
    fn a_zero_side_paints_nothing() {
        assert!(raster(9, &SQUARE, INK, 0, 24).is_none());
        assert!(raster(9, &SQUARE, INK, 24, 0).is_none());
    }

    /// The oldest pin in the book: a glyph that IS a rectangle must
    /// leave exactly the bytes `fill_rect` would — opaque ink inside,
    /// untouched zeros outside, nothing in between.
    #[test]
    fn a_rectangle_glyph_matches_the_house_fill() {
        let raster = rasterize(&SQUARE_GLYPH, INK, 24, 24);
        for y in 0..24 {
            for x in 0..24 {
                let want: [u8; 4] = if (4..20).contains(&x) && (4..20).contains(&y) {
                    [INK.r, INK.g, INK.b, 255]
                } else {
                    [0, 0, 0, 0]
                };
                let at = (y * 24 + x) * 4;
                assert_eq!(&raster.rgba[at..at + 4], &want, "pixel ({x},{y})");
            }
        }
    }

    /// A circle of four cubics against the house pill — the corner
    /// kernel of `fill_rect` at radius = half the box. The two ramps
    /// are cousins, not twins — analytic area against distance offset,
    /// and the fill quantizes y in sixteenths — so the gate is a bound
    /// with a derivation, not a shrug: ~0.05 of kernel divergence near
    /// the diagonals plus ~0.05 of sub-row error on the near-horizontal
    /// crowns, and almost every pixel far tighter than either.
    #[test]
    fn a_circle_glyph_agrees_with_the_house_pill() {
        const KAPPA: f32 = 0.552_284_75;
        const K: f32 = 12.0 * KAPPA;
        const CIRCLE_PATH: &[Verb] = &[
            Verb::Move(24.0, 12.0),
            Verb::Cubic(24.0, 12.0 + K, 12.0 + K, 24.0, 12.0, 24.0),
            Verb::Cubic(12.0 - K, 24.0, 0.0, 12.0 + K, 0.0, 12.0),
            Verb::Cubic(0.0, 12.0 - K, 12.0 - K, 0.0, 12.0, 0.0),
            Verb::Cubic(12.0 + K, 0.0, 24.0, 12.0 - K, 24.0, 12.0),
            Verb::Close,
        ];
        const CIRCLE: Glyph =
            Glyph { draws: &[Draw { paint: Paint::Fill(Rule::NonZero), path: CIRCLE_PATH }] };
        let side = 48;
        let raster = rasterize(&CIRCLE, INK, side, side);
        let radius = side as f64 / 2.0;
        let mut beyond_two = 0usize;
        for y in 0..side {
            for x in 0..side {
                // the house corner kernel, radius = half the square
                let distance =
                    (x as f64 + 0.5 - radius).hypot(y as f64 + 0.5 - radius);
                let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
                let want = (255.0 * coverage).round() as i32;
                let got = raster.rgba[(y * side + x) * 4 + 3] as i32;
                let delta = (want - got).abs();
                assert!(delta <= 26, "pixel ({x},{y}): pill {want} vs glyph {got}");
                if delta > 2 {
                    beyond_two += 1;
                }
            }
        }
        let share = beyond_two as f64 / (side * side) as f64;
        assert!(share < 0.08, "{share} of pixels beyond two steps");
    }
}
