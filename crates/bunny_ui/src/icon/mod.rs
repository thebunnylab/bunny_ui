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

pub mod house;
#[cfg(feature = "svg")]
pub mod parse;
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

/// What the ink of a traced path IS: one colour, or a ramp. The ramp is
/// declared in the path's OWN box (the same [`Gradient`] a box paints
/// behind itself), so it survives every size the path is drawn at — and
/// because a path is pixels in every rendering, all four get the same
/// bytes without one of them learning a new trick.
///
/// [`Gradient`]: crate::layout::Gradient
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ink {
    Solid(crate::layout::Color),
    Ramp(crate::layout::Gradient),
}

impl From<crate::layout::Color> for Ink {
    fn from(color: crate::layout::Color) -> Ink {
        Ink::Solid(color)
    }
}

impl From<crate::layout::Gradient> for Ink {
    fn from(ramp: crate::layout::Gradient) -> Ink {
        Ink::Ramp(ramp)
    }
}

/// One paint over one path. A glyph is a short stack of these, painted
/// in order — most icons are a single stroked path; a badge may fill a
/// disc first and stroke a mark on top.
#[derive(Clone, Copy, Debug)]
pub struct Draw {
    pub paint: Paint,
    pub path: &'static [Verb],
    /// This draw's OWN color. `None` takes the symbol ink — the whole
    /// glyph re-tints with the text around it. `Some` is the drawing's
    /// palette (a crab that is orange in any theme): it rides the
    /// glyph's identity, so the caches never learn a new key.
    pub tint: Option<Color>,
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

/// The `d` of one verb table on the house grid — what the Dom wire
/// carries and the glue hands the browser. ONE encoder, byte-tested,
/// so the geometry contract lives in exactly one place.
pub(crate) fn to_svg_path(path: &[Verb]) -> String {
    use std::fmt::Write;
    // whole grid points print whole — the wire stays short and stable
    fn number(out: &mut String, value: f32) {
        if value == value.trunc() {
            let _ = write!(out, "{}", value as i64);
        } else {
            let _ = write!(out, "{value}");
        }
    }
    fn numbers(out: &mut String, values: &[f32]) {
        for (i, value) in values.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            number(out, *value);
        }
    }
    let mut out = String::new();
    for verb in path {
        match *verb {
            Verb::Move(x, y) => {
                out.push('M');
                numbers(&mut out, &[x, y]);
            }
            Verb::Line(x, y) => {
                out.push('L');
                numbers(&mut out, &[x, y]);
            }
            Verb::Quad(cx, cy, x, y) => {
                out.push('Q');
                numbers(&mut out, &[cx, cy, x, y]);
            }
            Verb::Cubic(ax, ay, bx, by, x, y) => {
                out.push('C');
                numbers(&mut out, &[ax, ay, bx, by, x, y]);
            }
            Verb::Close => out.push('Z'),
        }
    }
    out
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
    forced: bool,
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
        let raster = Rc::new(rasterize(symbol.glyph, color, forced, width, height));
        cache.insert((key, width, height), Rc::clone(&raster));
        Some(raster)
    })
}

/// How many traced paths stay warm. Their own map, away from the
/// glyphs: a diagram that rebuilds its curves every frame must never
/// evict the toolbar's icons.
///
/// One number for the whole page, so it has to hold a FRAME's worth of
/// distinct traces, not a widget's: a mesh of a few hundred contours
/// beside a commit graph beside a sparkline used to overrun 128 and
/// evict itself several times per frame — never a hit, even when the
/// geometry was bit-identical to the frame before.
const TRACE_KEEP: usize = 2048;

/// One warm trace and the tick it was last asked for.
struct Trace {
    raster: Rc<ImageRaster>,
    used: u64,
}

thread_local! {
    /// Traced paths by `(key, physical width, physical height)`, with
    /// the cache's own access clock. The key already folds the
    /// geometry, the paint and the ink — the same contract the glyph
    /// rasters keep. Eviction drops the OLD half, never the table: a
    /// live mark re-traces its moving currents on every step while the
    /// arcs behind them hold one key each — a full clear used to throw
    /// the still layers out with the churn and re-tessellate them a
    /// moment later.
    static TRACES: RefCell<(HashMap<(u64, usize, usize), Trace>, u64)> =
        RefCell::new((HashMap::default(), 0));
}

/// The box a verb table needs, in its own coordinates — the CONTROL
/// hull, which a curve never leaves (the Bezier property). A hull is a
/// few transparent pixels wider than the ink on a bent curve, and it
/// costs one pass over the table instead of a flattening.
pub fn bounds(path: &[Verb]) -> Option<(f64, f64, f64, f64)> {
    let mut box_ = None;
    let mut eat = |x: f32, y: f32| {
        let (x, y) = (x as f64, y as f64);
        match &mut box_ {
            None => box_ = Some((x, y, x, y)),
            Some((min_x, min_y, max_x, max_y)) => {
                *min_x = min_x.min(x);
                *min_y = min_y.min(y);
                *max_x = max_x.max(x);
                *max_y = max_y.max(y);
            }
        }
    };
    for verb in path {
        match *verb {
            Verb::Move(x, y) | Verb::Line(x, y) => eat(x, y),
            Verb::Quad(cx, cy, x, y) => {
                eat(cx, cy);
                eat(x, y);
            }
            Verb::Cubic(ax, ay, bx, by, x, y) => {
                eat(ax, ay);
                eat(bx, by);
                eat(x, y);
            }
            Verb::Close => {}
        }
    }
    box_
}

/// Moves a whole table into its own box — what the painter does once,
/// so the raster below always starts at the origin.
pub(crate) fn shifted(path: &[Verb], dx: f32, dy: f32) -> Vec<Verb> {
    path.iter()
        .map(|verb| match *verb {
            Verb::Move(x, y) => Verb::Move(x + dx, y + dy),
            Verb::Line(x, y) => Verb::Line(x + dx, y + dy),
            Verb::Quad(cx, cy, x, y) => Verb::Quad(cx + dx, cy + dy, x + dx, y + dy),
            Verb::Cubic(ax, ay, bx, by, x, y) => {
                Verb::Cubic(ax + dx, ay + dy, bx + dx, by + dy, x + dx, y + dy)
            }
            Verb::Close => Verb::Close,
        })
        .collect()
}

/// The door `image_engine::raster_source` opens for a traced path —
/// the glyph door's twin, for geometry the app builds while the frame
/// runs.
pub(crate) fn raster_trace(
    key: u64,
    path: &[Verb],
    paint: Paint,
    ink: Ink,
    box_size: (f32, f32),
    width: usize,
    height: usize,
) -> Option<Rc<ImageRaster>> {
    if width == 0 || height == 0 || box_size.0 <= 0.0 || box_size.1 <= 0.0 {
        return None;
    }
    TRACES.with(|cell| {
        let mut cell = cell.borrow_mut();
        let (cache, clock) = &mut *cell;
        *clock += 1;
        let now = *clock;
        if let Some(entry) = cache.get_mut(&(key, width, height)) {
            entry.used = now;
            return Some(Rc::clone(&entry.raster));
        }
        if cache.len() >= TRACE_KEEP {
            // drop the old half: churning curves leave, still ones stay
            let mut stamps: Vec<u64> = cache.values().map(|entry| entry.used).collect();
            stamps.sort_unstable();
            let cutoff = stamps[stamps.len() / 2];
            cache.retain(|_, entry| entry.used > cutoff);
        }
        let raster = Rc::new(rasterize_trace(path, paint, ink, box_size, width, height));
        cache.insert((key, width, height), Trace { raster: Rc::clone(&raster), used: now });
        Some(raster)
    })
}

/// One traced path, rasterized. The scale comes from the LONGER side
/// of the box: rounding the short side to whole pixels then cannot
/// stretch the drawing, which is what a tall thin sparkline would
/// show first.
fn rasterize_trace(
    path: &[Verb],
    paint: Paint,
    ink: Ink,
    box_size: (f32, f32),
    width: usize,
    height: usize,
) -> ImageRaster {
    let scale = if box_size.0 >= box_size.1 {
        width as f64 / box_size.0 as f64
    } else {
        height as f64 / box_size.1 as f64
    };
    let placing = vector::Placing { scale, dx: 0.0, dy: 0.0 };
    let mut mask = vector::Mask::new(width, height);
    let flat = vector::flatten(path, placing);
    match paint {
        Paint::Fill(rule) => vector::fill_into(&mut mask, &flat, rule),
        Paint::Stroke { width: pen } => {
            vector::stroke_into(&mut mask, &flat, pen as f64 * scale)
        }
    }
    let mut rgba = vec![0u8; width * height * 4];
    match ink {
        Ink::Solid(color) => paint_mask(&mask, color, &mut rgba, width, height),
        Ink::Ramp(ramp) => {
            // the ramp is resolved against the path's LOGICAL box, so a
            // radius written in points stays that many points at any
            // scale — and the pixel centre is read back through the
            // same scale the coverage was measured at
            let paint = ramp.resolve(crate::layout::Rect {
                origin: crate::layout::Point { x: 0.0, y: 0.0 },
                size: crate::layout::Size {
                    width: box_size.0 as f64,
                    height: box_size.1 as f64,
                },
            });
            paint_ramp(&mask, paint, scale, &mut rgba, width, height);
        }
    }
    ImageRaster { width, height, rgba }
}

/// One glyph, rasterized: the [`ICON_GRID`] square scales onto the
/// largest CENTRED square of the destination, every draw piles its
/// coverage in by MAX, and the tint lands once at the end — straight
/// alpha, physical pixels, the same contract text rasters keep.
///
/// `forced` reads the drawing as a MASK: `color` covers every draw and
/// the palette it carries is spent. A glyph with no tint of its own
/// rasterizes to the same bytes either way — the flag can only take
/// away.
fn rasterize(
    glyph: &Glyph,
    color: Color,
    forced: bool,
    width: usize,
    height: usize,
) -> ImageRaster {
    let side = width.min(height) as f64;
    let scale = side / ICON_GRID;
    let placing = vector::Placing {
        scale,
        dx: (width as f64 - side) / 2.0,
        dy: (height as f64 - side) / 2.0,
    };
    // consecutive draws of ONE color pile into a union mask and blend
    // ONCE — a monochrome glyph stays byte for byte what it always
    // was, and a translucent ink never double-blends at a join. A
    // tint change flushes and paints over, in draw order (SVG law).
    let mut rgba = vec![0u8; width * height * 4];
    let mut union = vector::Mask::new(width, height);
    let mut scratch = vector::Mask::new(width, height);
    let mut run_color: Option<Color> = None;
    let flush = |union: &mut vector::Mask, run_color: &mut Option<Color>, rgba: &mut Vec<u8>| {
        let Some(ink) = run_color.take() else { return };
        paint_mask(union, ink, rgba, width, height);
        union.clear();
    };
    for draw in glyph.draws {
        let ink = if forced { color } else { draw.tint.unwrap_or(color) };
        if run_color.is_some() && run_color != Some(ink) {
            flush(&mut union, &mut run_color, &mut rgba);
        }
        run_color = Some(ink);
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
    flush(&mut union, &mut run_color, &mut rgba);
    ImageRaster { width, height, rgba }
}

/// Lays one ink over a finished coverage mask. The rounding is the
/// one `set_covered` applies on the way in, so a glyph rasterized here
/// and a rect painted by the compositor agree on the same edge.
fn paint_mask(mask: &vector::Mask, ink: Color, rgba: &mut [u8], width: usize, height: usize) {
    paint_covered(mask, rgba, width, height, |_, _| ink);
}

/// The same pass with the ink read from a ramp, pixel by pixel. The
/// centre of the pixel is divided back by the raster's scale, so the
/// ramp is sampled in the coordinates it was declared in.
fn paint_ramp(
    mask: &vector::Mask,
    paint: crate::layout::GradientPaint,
    scale: f64,
    rgba: &mut [u8],
    width: usize,
    height: usize,
) {
    paint_covered(mask, rgba, width, height, |x, y| {
        paint.at(crate::layout::Point {
            x: (x as f64 + 0.5) / scale,
            y: (y as f64 + 0.5) / scale,
        })
    });
}

/// Every covered pixel takes the ink the caller names for it. One
/// blend per pixel, never two — the mask already unioned the overlaps.
#[inline]
fn paint_covered(
    mask: &vector::Mask,
    rgba: &mut [u8],
    width: usize,
    height: usize,
    ink: impl Fn(usize, usize) -> Color,
) {
    let Some((x0, y0, x1, y1)) = mask.dirty() else { return };
    debug_assert!(x1 <= width && y1 <= height);
    for y in y0..y1 {
        for x in x0..x1 {
            let coverage = mask.at(x, y);
            if coverage <= 0.0 {
                continue;
            }
            let ink = ink(x, y);
            let alpha = (ink.a as f64 * coverage as f64).round() as u32;
            if alpha == 0 {
                continue;
            }
            let to = (y * width + x) * 4;
            blend_straight(&mut rgba[to..to + 4], ink, alpha);
        }
    }
}

/// Source-over of a straight-alpha ink onto a straight-alpha pixel —
/// the destination may be TRANSPARENT here (the raster's own ground),
/// unlike the bitmap compositor's always-opaque canvas. Over nothing
/// this writes the ink exactly, which is what keeps a monochrome glyph
/// byte-identical to the single-pass raster it always had.
fn blend_straight(pixel: &mut [u8], ink: Color, alpha: u32) {
    let da = pixel[3] as u32;
    if da == 0 {
        pixel[0] = ink.r;
        pixel[1] = ink.g;
        pixel[2] = ink.b;
        pixel[3] = alpha as u8;
        return;
    }
    let keep = da * (255 - alpha) / 255;
    let out_a = alpha + keep;
    let mix = |source: u8, dest: u8| -> u8 {
        ((source as u32 * alpha + dest as u32 * keep + out_a / 2) / out_a) as u8
    };
    pixel[0] = mix(ink.r, pixel[0]);
    pixel[1] = mix(ink.g, pixel[1]);
    pixel[2] = mix(ink.b, pixel[2]);
    pixel[3] = out_a as u8;
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
        Glyph { draws: &[Draw { paint: Paint::Fill(Rule::NonZero), path: SQUARE_PATH, tint: None }] };
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
        let first = raster(1234, &SQUARE, INK, false, 24, 24).unwrap();
        let second = raster(1234, &SQUARE, INK, false, 24, 24).unwrap();
        assert!(Rc::ptr_eq(&first, &second));
    }

    #[test]
    fn a_zero_side_paints_nothing() {
        assert!(raster(9, &SQUARE, INK, false, 0, 24).is_none());
        assert!(raster(9, &SQUARE, INK, false, 24, 0).is_none());
    }

    /// The oldest pin in the book: a glyph that IS a rectangle must
    /// leave exactly the bytes `fill_rect` would — opaque ink inside,
    /// untouched zeros outside, nothing in between.
    #[test]
    fn a_rectangle_glyph_matches_the_house_fill() {
        let raster = rasterize(&SQUARE_GLYPH, INK, false, 24, 24);
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

    /// A tinted draw keeps its own palette in ANY ink; an untinted
    /// one re-tints with the symbol — the two live in one glyph.
    #[test]
    fn a_tinted_draw_keeps_its_palette() {
        const ORANGE: Color = Color { r: 0xF7, g: 0x8C, b: 0x3C, a: 255 };
        const LEFT: &[Verb] = &[
            Verb::Move(2.0, 2.0),
            Verb::Line(11.0, 2.0),
            Verb::Line(11.0, 22.0),
            Verb::Line(2.0, 22.0),
            Verb::Close,
        ];
        const RIGHT: &[Verb] = &[
            Verb::Move(13.0, 2.0),
            Verb::Line(22.0, 2.0),
            Verb::Line(22.0, 22.0),
            Verb::Line(13.0, 22.0),
            Verb::Close,
        ];
        const TWO_TONE: Glyph = Glyph {
            draws: &[
                Draw { paint: Paint::Fill(Rule::NonZero), path: LEFT, tint: Some(ORANGE) },
                Draw { paint: Paint::Fill(Rule::NonZero), path: RIGHT, tint: None },
            ],
        };
        let ink = Color { r: 20, g: 40, b: 60, a: 255 };
        let raster = rasterize(&TWO_TONE, ink, false, 24, 24);
        let pixel = |x: usize, y: usize| {
            let at = (y * 24 + x) * 4;
            [raster.rgba[at], raster.rgba[at + 1], raster.rgba[at + 2], raster.rgba[at + 3]]
        };
        assert_eq!(pixel(5, 12), [ORANGE.r, ORANGE.g, ORANGE.b, 255], "the crab stays orange");
        assert_eq!(pixel(18, 12), [ink.r, ink.g, ink.b, 255], "the plain half takes the ink");
        assert_eq!(pixel(12, 12)[3], 0, "the gap stays air");
    }

    /// The inverse reading: a bar names a STATE, so the palette the
    /// drawing carries is spent and the whole glyph answers one ink.
    #[test]
    fn a_forced_glyph_spends_its_own_palette() {
        const ORANGE: Color = Color { r: 0xF7, g: 0x8C, b: 0x3C, a: 255 };
        const LEFT: &[Verb] = &[
            Verb::Move(2.0, 2.0),
            Verb::Line(11.0, 2.0),
            Verb::Line(11.0, 22.0),
            Verb::Line(2.0, 22.0),
            Verb::Close,
        ];
        const RIGHT: &[Verb] = &[
            Verb::Move(13.0, 2.0),
            Verb::Line(22.0, 2.0),
            Verb::Line(22.0, 22.0),
            Verb::Line(13.0, 22.0),
            Verb::Close,
        ];
        const TWO_TONE: Glyph = Glyph {
            draws: &[
                Draw { paint: Paint::Fill(Rule::NonZero), path: LEFT, tint: Some(ORANGE) },
                Draw { paint: Paint::Fill(Rule::NonZero), path: RIGHT, tint: None },
            ],
        };
        let accent = Color { r: 137, g: 87, b: 229, a: 255 };
        let raster = rasterize(&TWO_TONE, accent, true, 24, 24);
        let pixel = |x: usize, y: usize| {
            let at = (y * 24 + x) * 4;
            [raster.rgba[at], raster.rgba[at + 1], raster.rgba[at + 2], raster.rgba[at + 3]]
        };
        let want = [accent.r, accent.g, accent.b, 255];
        assert_eq!(pixel(5, 12), want, "the half that named a colour gives it up");
        assert_eq!(pixel(18, 12), want, "the plain half is unchanged");
        assert_eq!(pixel(12, 12)[3], 0, "the gap stays air");
    }

    /// A drawing with no palette of its own rasterizes to the SAME
    /// bytes either way — forcing can only take away, so every glyph
    /// the house already ships is untouched by the door.
    #[test]
    fn forcing_a_plain_glyph_changes_no_byte() {
        let ink = Color { r: 20, g: 40, b: 60, a: 255 };
        let plain = rasterize(&SQUARE_GLYPH, ink, false, 24, 24);
        let forced = rasterize(&SQUARE_GLYPH, ink, true, 24, 24);
        assert_eq!(plain.rgba, forced.rgba);
    }

    /// The two readings are two IDENTITIES: the caches, the atlas and
    /// the damage diff compare by key, so one bar slot could otherwise
    /// serve the tree's crab out of a warm tile.
    #[test]
    fn the_forced_reading_is_its_own_identity() {
        let ink = Color { r: 0x89, g: 0x57, b: 0xE5, a: 255 };
        let plain = crate::image_engine::ImageSource::symbol(SQUARE, ink);
        let forced = crate::image_engine::ImageSource::symbol_forced(SQUARE, ink);
        assert_ne!(plain.key(), forced.key(), "one glyph, two readings, two keys");
        assert_eq!(
            forced.key(),
            crate::image_engine::ImageSource::symbol_forced(SQUARE, ink).key(),
            "and the forced key is stable"
        );
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
            Glyph { draws: &[Draw { paint: Paint::Fill(Rule::NonZero), path: CIRCLE_PATH, tint: None }] };
        let side = 48;
        let raster = rasterize(&CIRCLE, INK, false, side, side);
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

    // MARK: - The traced path (what the app builds while the frame runs)

    #[test]
    fn one_rasterizer_answers_the_table_and_the_trace() {
        // the glyph door and the runtime door share the same stone: a
        // 24 unit square drawn as a const glyph and the SAME verbs
        // traced into a 24 point box must land byte for byte
        let glyph = rasterize(&SQUARE_GLYPH, INK, false, 24, 24);
        let trace = rasterize_trace(
            SQUARE_PATH,
            Paint::Fill(Rule::NonZero),
            Ink::Solid(INK),
            (ICON_GRID as f32, ICON_GRID as f32),
            24,
            24,
        );
        assert_eq!(glyph.rgba, trace.rgba);
    }

    #[test]
    fn a_traced_pen_rides_the_line_it_was_given() {
        // a horizontal pen of 4 units across a 40x12 box: the band is
        // ink, a row well above it is not
        const LINE: &[Verb] = &[Verb::Move(2.0, 6.0), Verb::Line(38.0, 6.0)];
        let raster =
            rasterize_trace(LINE, Paint::Stroke { width: 4.0 }, Ink::Solid(INK), (40.0, 12.0), 40, 12);
        let alpha = |x: usize, y: usize| raster.rgba[(y * 40 + x) * 4 + 3];
        assert_eq!(alpha(20, 6), 255, "the middle of the band is solid");
        assert_eq!(alpha(20, 0), 0, "three units above it, nothing");
        assert_eq!(alpha(20, 11), 0, "and nothing below it either");
        // the round cap rides PAST the first point by half a pen — which
        // is exactly why the painter pads the box before it rasterizes
        assert!((1..255).contains(&alpha(0, 6)), "the cap softens at the edge");
    }

    #[test]
    fn a_traced_path_fills_with_a_ramp_that_survives_the_scale() {
        use crate::layout::{Gradient, Point, Rect, Size, UnitPoint};

        // a filled band across a 40x12 box, red at the leading edge and
        // blue at the trailing one
        const BAND: &[Verb] = &[
            Verb::Move(0.0, 0.0),
            Verb::Line(40.0, 0.0),
            Verb::Line(40.0, 12.0),
            Verb::Line(0.0, 12.0),
            Verb::Close,
        ];
        const RED: Color = Color { r: 220, g: 30, b: 30, a: 255 };
        const BLUE: Color = Color { r: 30, g: 30, b: 220, a: 255 };
        let ramp = Gradient::linear(RED, BLUE)
            .direction(UnitPoint::LEADING, UnitPoint::TRAILING);
        let fill = Paint::Fill(Rule::NonZero);
        let box_ = (40.0f32, 12.0f32);

        let traced = |scale: usize| {
            rasterize_trace(BAND, fill, Ink::Ramp(ramp), box_, 40 * scale, 12 * scale)
        };
        let at = |raster: &ImageRaster, x: usize, y: usize| {
            let to = (y * raster.width + x) * 4;
            Color {
                r: raster.rgba[to],
                g: raster.rgba[to + 1],
                b: raster.rgba[to + 2],
                a: raster.rgba[to + 3],
            }
        };

        let one = traced(1);
        // the ramp runs: red on the left, blue on the right, and the
        // middle belongs to neither
        let left = at(&one, 1, 6);
        let right = at(&one, 38, 6);
        let middle = at(&one, 20, 6);
        assert!(left.r > left.b, "the leading edge is red: {left:?}");
        assert!(right.b > right.r, "the trailing edge is blue: {right:?}");
        assert!(middle.r < left.r && middle.b < right.b, "and the middle mixes: {middle:?}");

        // the ORACLE is the ramp's own formula — the one the shaders
        // repeat — read in the coordinates the path was declared in
        let paint = ramp.resolve(Rect {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size { width: 40.0, height: 12.0 },
        });
        for x in [1usize, 9, 20, 31, 38] {
            let want = paint.at(Point { x: x as f64 + 0.5, y: 6.5 });
            let got = at(&one, x, 6);
            assert_eq!((got.r, got.g, got.b), (want.r, want.g, want.b), "at x={x}");
        }

        // and it survives the scale: the ramp is declared in the box's
        // OWN proportions, so twice the pixels is the same picture —
        // which is why every rendering gets the same bytes for free
        let two = traced(2);
        assert_eq!(two.width, 80);
        for x in [1usize, 9, 20, 31, 38] {
            let want = paint.at(Point { x: x as f64 + 0.5, y: 6.5 });
            // the physical pixel whose CENTRE is nearest that logical
            // point: 2x + 0 has centre (2x + 0.5) / 2 = x + 0.25
            let got = at(&two, x * 2, 12);
            let near = |a: u8, b: u8| a.abs_diff(b) <= 4;
            assert!(
                near(got.r, want.r) && near(got.g, want.g) && near(got.b, want.b),
                "at x={x}: {got:?} against {want:?}",
            );
        }

        // a flat ink is untouched: the same path through the old door
        // is byte for byte the raster it always was
        let flat = rasterize_trace(BAND, fill, Ink::Solid(RED), box_, 40, 12);
        assert!(flat.rgba.chunks(4).all(|pixel| pixel[3] == 0
            || (pixel[0], pixel[1], pixel[2]) == (RED.r, RED.g, RED.b)));
        assert_ne!(flat.rgba, one.rgba, "and a ramp is not a flat fill");
    }

    #[test]
    fn a_pen_put_down_and_lifted_at_one_point_is_a_dot() {
        // the dab a sketch draws: a segment of no length at all. The
        // distance field answers the POINT, so the round cap is the
        // whole mark — and the box the painter pads around it holds it
        const DAB: &[Verb] = &[Verb::Move(6.0, 6.0), Verb::Line(6.0, 6.0)];
        let raster =
            rasterize_trace(DAB, Paint::Stroke { width: 8.0 }, Ink::Solid(INK), (12.0, 12.0), 12, 12);
        let alpha = |x: usize, y: usize| raster.rgba[(y * 12 + x) * 4 + 3];
        assert_eq!(alpha(6, 6), 255, "the middle is solid");
        assert_eq!(alpha(0, 0), 0, "the corner is outside the disc");
        assert!((1..255).contains(&alpha(6, 2)), "and the rim softens");
    }

    #[test]
    fn a_traced_identity_folds_geometry_paint_and_ink() {
        use crate::image_engine::ImageSource;
        const A: &[Verb] = &[Verb::Move(0.0, 0.0), Verb::Line(10.0, 10.0)];
        const B: &[Verb] = &[Verb::Move(0.0, 0.0), Verb::Line(10.0, 9.0)];
        let pen = Paint::Stroke { width: 2.0 };
        let box_ = (12.0, 12.0);
        let key = |verbs: &'static [Verb], paint, ink: Ink| {
            ImageSource::path(verbs.to_vec(), paint, ink, box_).key()
        };
        let ink = Ink::Solid(INK);
        assert_eq!(key(A, pen, ink), key(A, pen, ink), "the same drawing, the same key");
        assert_ne!(key(A, pen, ink), key(B, pen, ink), "one moved point moves it");
        assert_ne!(
            key(A, pen, ink),
            key(A, Paint::Fill(Rule::NonZero), ink),
            "the paint rides the identity"
        );
        assert_ne!(
            key(A, pen, ink),
            key(A, pen, Ink::Solid(Color { r: 200, g: 10, b: 10, a: 255 })),
            "and so does the ink"
        );
        // and a ramp is a different tile from either of its ends, and
        // from another ramp with the same ends pointing elsewhere
        let ramp = crate::layout::Gradient::linear(INK, Color { r: 200, g: 10, b: 10, a: 255 });
        assert_ne!(key(A, pen, ink), key(A, pen, Ink::Ramp(ramp)));
        assert_ne!(
            key(A, pen, Ink::Ramp(ramp)),
            key(
                A,
                pen,
                Ink::Ramp(ramp.direction(
                    crate::layout::UnitPoint::LEADING,
                    crate::layout::UnitPoint::TRAILING,
                )),
            ),
            "the ramp's own geometry rides it too"
        );
    }

    #[test]
    fn the_control_hull_holds_every_curve() {
        // a cubic that bulges up: the hull answers the CONTROLS, which
        // the curve never leaves
        const ARC: &[Verb] =
            &[Verb::Move(0.0, 10.0), Verb::Cubic(0.0, 0.0, 20.0, 0.0, 20.0, 10.0)];
        assert_eq!(bounds(ARC), Some((0.0, 0.0, 20.0, 10.0)));
        assert_eq!(bounds(&[]), None, "an empty table has no box");
    }

    #[test]
    fn a_shifted_table_moves_whole() {
        const CURVE: &[Verb] = &[
            Verb::Move(1.0, 2.0),
            Verb::Quad(3.0, 4.0, 5.0, 6.0),
            Verb::Cubic(7.0, 8.0, 9.0, 10.0, 11.0, 12.0),
            Verb::Close,
        ];
        let moved = shifted(CURVE, -1.0, -2.0);
        assert_eq!(
            moved,
            vec![
                Verb::Move(0.0, 0.0),
                Verb::Quad(2.0, 2.0, 4.0, 4.0),
                Verb::Cubic(6.0, 6.0, 8.0, 8.0, 10.0, 10.0),
                Verb::Close,
            ]
        );
    }
}
