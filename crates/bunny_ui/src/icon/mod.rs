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

use crate::layout::Px;

/// The glyph grid — every drawing lives in a `0..24` square. The
/// rasterizers scale this square onto the destination; the Dom mode
/// writes it as the `viewBox`. One constant, three renderers.
pub const ICON_GRID: Px = 24.0;

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
