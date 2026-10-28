//! Basic shapes reuse the vector resource understood by every paint backend.
//!
//! An ellipse, a dashed or dotted border and a striped ground are all
//! geometry the house traces: a pipeline that composites a traced path
//! paints them, the preview wire carries them, and none of them asked a
//! renderer to learn a new primitive.
//!
//! ## Wiring
//! Layout supplies the final box and inherited ink. The path identity includes
//! both, so retained frames and the preview wire invalidate only changed paint.

use crate::angle::Angle;
use crate::icon::{Cap, Dash, Ink, Paint, Rule, Verb};
use crate::image_engine::ImageSource;
use crate::layout::{Color, Corners, Px, Size};

pub(crate) fn ellipse_source(size: Size, color: Color) -> Option<ImageSource> {
    let (width, height) = (size.width as f32, size.height as f32);
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let (rx, ry) = (width / 2.0, height / 2.0);
    // Four cubic arcs. Scaling the two radii independently preserves an ellipse
    // at non-square sizes; a rounded rectangle would incorrectly produce a pill.
    let kappa = 0.552_284_8;
    let (kx, ky) = (rx * kappa, ry * kappa);
    Some(ImageSource::path(
        vec![
            Verb::Move(width, ry),
            Verb::Cubic(width, ry + ky, rx + kx, height, rx, height),
            Verb::Cubic(rx - kx, height, 0.0, ry + ky, 0.0, ry),
            Verb::Cubic(0.0, ry - ky, rx - kx, 0.0, rx, 0.0),
            Verb::Cubic(rx + kx, 0.0, width, ry - ky, width, ry),
            Verb::Close,
        ],
        Paint::Fill(Rule::NonZero),
        Ink::Solid(color),
        (width, height),
    ))
}

// =============================================================================
// Border styles
// =============================================================================

/// How a border's line is drawn — CSS's `border-style`, the three a
/// design system reaches for.
///
/// The rhythm is the browser's, so a native box and its web twin wear
/// the same dashes: a `Dashed` border of a thin pen has dashes three
/// pens long with two pens of air, a thick one (three points and up)
/// two and one; a `Dotted` border has square dots a pen apart, or round
/// ones from three points up. Each side then stretches its air so the
/// dashes land on both corners, as the browser does — a side too short
/// for two dashes is drawn whole. A rounded box walks its whole outline
/// in one rhythm, which closes on itself without a seam.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum BorderStyle {
    /// One unbroken line — what `.border(…)` has always drawn.
    #[default]
    Solid,
    /// `border-style: dashed`.
    Dashed,
    /// `border-style: dotted`.
    Dotted,
}

impl BorderStyle {
    /// The CSS keyword.
    pub fn css(self) -> &'static str {
        match self {
            BorderStyle::Solid => "solid",
            BorderStyle::Dashed => "dashed",
            BorderStyle::Dotted => "dotted",
        }
    }

    /// The browser's ink and air for a pen this wide, and how the ends
    /// of a dash are cut. `None` for the solid line.
    fn rhythm(self, width: f64) -> Option<(f64, f64, Cap)> {
        let thick = width >= 3.0;
        match self {
            BorderStyle::Solid => None,
            BorderStyle::Dashed if thick => Some((2.0 * width, width, Cap::Butt)),
            BorderStyle::Dashed => Some((3.0 * width, 2.0 * width, Cap::Butt)),
            BorderStyle::Dotted if thick => Some((width, width, Cap::Round)),
            BorderStyle::Dotted => Some((width, width, Cap::Butt)),
        }
    }
}

/// The air between dashes that lands a whole number of them on a line
/// `length` long, as near the asked `gap` as it can — the browser's own
/// rule, so the corners of a dashed box carry ink the way a web page's
/// do. An open line has one gap fewer than dashes (a dash at each end);
/// a closed one as many. `None` when the line is too short for two
/// dashes: then it is drawn solid, which is what a browser shows too.
fn fitted_gap(length: f64, dash: f64, gap: f64, closed: bool) -> Option<f64> {
    if length <= 2.0 * dash || dash + gap <= 0.0 {
        return None;
    }
    let fewer = ((length + gap) / (dash + gap)).floor().max(1.0);
    let more = fewer + 1.0;
    let gaps = |dashes: f64| if closed { dashes } else { dashes - 1.0 };
    let gap_for = |dashes: f64| (length - dashes * dash) / gaps(dashes);
    let (low, high) = (gap_for(fewer), gap_for(more));
    let fitted = if gaps(fewer) <= 0.0 {
        high
    } else if high <= 0.0 || (low - gap).abs() < (high - gap).abs() {
        low
    } else {
        high
    };
    (fitted.is_finite() && fitted >= 0.0).then_some(fitted)
}

/// A dashed or dotted border as ONE traced path in the box's own
/// points: the rhythm fitted side by side (or around a rounded outline
/// at once), then lowered to plain geometry so the sides' different
/// air shares one raster. `None` for a solid border — the compositors
/// keep their exact ring for that.
pub(crate) fn border_source(
    size: Size,
    color: Color,
    width: f64,
    style: BorderStyle,
    corners: Corners,
) -> Option<ImageSource> {
    let (w, h) = (size.width, size.height);
    let (ink, air, cap) = style.rhythm(width)?;
    if !(w > 0.0 && h > 0.0 && width > 0.0) || !w.is_finite() || !h.is_finite() {
        return None;
    }
    let half = width / 2.0;
    let pen = width as f32;
    // a round dot is a dash of no length centred on its slot: half a
    // pen in from where the slot starts
    let dash_of = |length: f64, closed: bool| -> Dash {
        match fitted_gap(length, ink, air, closed) {
            None => Dash::new(length as f32 + 1.0, 0.0),
            Some(gap) if cap == Cap::Round => {
                Dash::new(0.0, (ink + gap) as f32).phase(-(half as f32)).cap(Cap::Round)
            }
            Some(gap) => Dash::new(ink as f32, gap as f32).cap(cap),
        }
    };
    let mut verbs = Vec::new();
    let mut paint = None;
    let mut add = |line: &[Verb], dash: Dash| {
        let (dashed, plain) = crate::icon::undashed(line, pen, dash);
        verbs.extend(dashed);
        paint = Some(plain);
    };
    let corners = corners.clamped(w, h);
    if corners.uniform() == Some(0.0) {
        let (x0, y0, x1, y1) = (half as f32, half as f32, (w - half) as f32, (h - half) as f32);
        let (wf, hf) = (w as f32, h as f32);
        // each side corner to corner, on its own rhythm
        for (from, to, length) in [
            ((0.0, y0), (wf, y0), w),
            ((x1, 0.0), (x1, hf), h),
            ((wf, y1), (0.0, y1), w),
            ((x0, hf), (x0, 0.0), h),
        ] {
            add(&[Verb::Move(from.0, from.1), Verb::Line(to.0, to.1)], dash_of(length, false));
        }
    } else {
        // the pen rides the middle of the ring, so every radius shrinks
        // by half a pen, the way the ring's own inner edge does
        let r = |radius: f64| (radius - half).max(0.0) as f32;
        let (tl, tr, br, bl) =
            (r(corners.top_left), r(corners.top_right), r(corners.bottom_right), r(corners.bottom_left));
        let (x0, y0, x1, y1) = (half as f32, half as f32, (w - half) as f32, (h - half) as f32);
        const KAPPA: f32 = 0.552_284_8;
        let outline = [
            Verb::Move(x0 + tl, y0),
            Verb::Line(x1 - tr, y0),
            Verb::Cubic(x1 - tr + tr * KAPPA, y0, x1, y0 + tr - tr * KAPPA, x1, y0 + tr),
            Verb::Line(x1, y1 - br),
            Verb::Cubic(x1, y1 - br + br * KAPPA, x1 - br + br * KAPPA, y1, x1 - br, y1),
            Verb::Line(x0 + bl, y1),
            Verb::Cubic(x0 + bl - bl * KAPPA, y1, x0, y1 - bl + bl * KAPPA, x0, y1 - bl),
            Verb::Line(x0, y0 + tl),
            Verb::Cubic(x0, y0 + tl - tl * KAPPA, x0 + tl - tl * KAPPA, y0, x0 + tl, y0),
            Verb::Close,
        ];
        let length = crate::icon::length(&outline);
        add(&outline, dash_of(length, true));
    }
    let paint = paint?;
    (!verbs.is_empty()).then(|| {
        ImageSource::path(verbs, paint, Ink::Solid(color), (w as f32, h as f32))
    })
}

// =============================================================================
// Stripes
// =============================================================================

/// Bands of one ink across a box, at an angle — CSS's
/// `repeating-linear-gradient` with hard stops, the ground a design
/// lays under "unknown" or "planned": `width` of `ink`, then air to the
/// next band, one band every `period` points.
///
/// The angle reads as CSS reads it: the direction the bands REPEAT in,
/// `0°` up and clockwise from there — `45°` walks to the top right, so
/// the bands lean the other way. The bands are laid out exactly where
/// the browser lays them, from the start of the line CSS draws through
/// the box's centre, so a striped row and its web twin start their
/// first band at the same point. Under the bands is whatever the box
/// already paints: pair it with `.background_color(…)` for a ground.
///
/// ```ignore
/// // `repeating-linear-gradient(135deg, #e9ecef 0 4px, transparent 4px 8px)`
/// row.background_stripes(Stripes::new(Angle::degrees(135.0), 4.0, 8.0, Color::hex(0xe9ecef)))
/// ```
///
/// The house traces the bands, so every pipeline paints the same
/// pixels; the element lowering hands the browser the gradient itself.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Stripes {
    pub angle: Angle,
    pub width: Px,
    pub period: Px,
    pub ink: Color,
}

impl Stripes {
    /// `width` of `ink` every `period` points, repeating along `angle`.
    pub fn new(angle: Angle, width: Px, period: Px, ink: Color) -> Stripes {
        Stripes { angle, width, period, ink }
    }
}

/// The bands of `stripes` over a box of `size`, as one traced fill.
/// `None` when they paint nothing (no band, no period, no box).
pub(crate) fn stripes_source(size: Size, stripes: &Stripes) -> Option<ImageSource> {
    let (w, h) = (size.width, size.height);
    let Stripes { width, period, ink, .. } = *stripes;
    if !(w > 0.0 && h > 0.0 && width > 0.0 && period > 0.0) || !(w * h).is_finite() {
        return None;
    }
    let radians = if stripes.angle.is_zero() { 0.0 } else { stripes.angle.as_radians() };
    let (sin, cos) = radians.sin_cos();
    // CSS's gradient line: through the centre, long enough that its
    // perpendiculars at the two ends touch the far corners
    let direction = (sin, -cos);
    let length = (w * sin).abs() + (h * cos).abs();
    let start = (w / 2.0 - direction.0 * length / 2.0, h / 2.0 - direction.1 * length / 2.0);
    let along = |(x, y): (f64, f64)| (x - start.0) * direction.0 + (y - start.1) * direction.1;
    let box_ = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let mut verbs = Vec::new();
    let mut band = 0.0;
    while band < length {
        // a band as wide as its period leaves no air: one band, the box
        let to = if width >= period { length } else { (band + width).min(length) };
        let from = band;
        band += period;
        // the box, cut to the slab `from ≤ along ≤ to`
        let cut = clip_half(&clip_half(&box_, |p| along(p) - from), |p| to - along(p));
        let Some(&(x, y)) = cut.first() else { continue };
        verbs.push(Verb::Move(x as f32, y as f32));
        verbs.extend(cut[1..].iter().map(|&(x, y)| Verb::Line(x as f32, y as f32)));
        verbs.push(Verb::Close);
        if width >= period {
            break;
        }
    }
    (!verbs.is_empty()).then(|| {
        ImageSource::path(verbs, Paint::Fill(Rule::NonZero), Ink::Solid(ink), (w as f32, h as f32))
    })
}

/// A convex polygon cut to the side where `inside` is not negative —
/// one step of Sutherland and Hodgman.
fn clip_half(polygon: &[(f64, f64)], inside: impl Fn((f64, f64)) -> f64) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(polygon.len() + 1);
    for (i, &a) in polygon.iter().enumerate() {
        let b = polygon[(i + 1) % polygon.len()];
        let (da, db) = (inside(a), inside(b));
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(source: &ImageSource, width: usize, height: usize) -> std::rc::Rc<crate::image_engine::ImageRaster> {
        let engine = crate::image_engine::RawImages::default();
        crate::image_engine::raster_source(&engine, source, width, height).expect("a traced path rasterizes")
    }

    fn alpha(raster: &crate::image_engine::ImageRaster, x: usize, y: usize) -> u8 {
        raster.rgba[(y * raster.width + x) * 4 + 3]
    }

    const INK: Color = Color { r: 10, g: 20, b: 30, a: 255 };

    #[test]
    fn the_gap_lands_a_dash_on_both_ends_of_a_side() {
        // twenty long, dashes of three: four dashes and three gaps of
        // 2.67 beat five dashes and four gaps of 1.25 for an asked two
        let gap = fitted_gap(20.0, 3.0, 2.0, false).expect("room for dashes");
        assert!((gap - 8.0 / 3.0).abs() < 1e-9, "{gap}");
        // a closed outline has as many gaps as dashes
        let closed = fitted_gap(20.0, 3.0, 2.0, true).expect("room for dashes");
        assert!((4.0 * 3.0 + 4.0 * closed - 20.0).abs() < 1e-9, "{closed}");
        // too short for two dashes: the side is drawn whole
        assert_eq!(fitted_gap(5.0, 3.0, 2.0, false), None);
    }

    #[test]
    fn the_rhythm_is_the_browsers_for_thin_and_thick_pens() {
        assert_eq!(BorderStyle::Dashed.rhythm(1.0), Some((3.0, 2.0, Cap::Butt)));
        assert_eq!(BorderStyle::Dashed.rhythm(4.0), Some((8.0, 4.0, Cap::Butt)));
        assert_eq!(BorderStyle::Dotted.rhythm(1.0), Some((1.0, 1.0, Cap::Butt)));
        assert_eq!(BorderStyle::Dotted.rhythm(3.0), Some((3.0, 3.0, Cap::Round)));
        assert_eq!(BorderStyle::Solid.rhythm(1.0), None);
        assert_eq!(BorderStyle::Dashed.css(), "dashed");
    }

    #[test]
    fn a_dashed_border_inks_its_corners_and_leaves_air_between() {
        let size = Size { width: 20.0, height: 10.0 };
        let source = border_source(size, INK, 1.0, BorderStyle::Dashed, Corners::ZERO)
            .expect("a dashed border is a traced path");
        let raster = raster(&source, 20, 10);
        for (x, y) in [(0, 0), (19, 0), (0, 9), (19, 9)] {
            assert_eq!(alpha(&raster, x, y), 255, "corner ({x}, {y})");
        }
        // the top side: a dash of three, then 2.67 of air
        assert_eq!(alpha(&raster, 2, 0), 255);
        assert_eq!(alpha(&raster, 4, 0), 0, "air");
        // the middle is not the border's
        assert_eq!(alpha(&raster, 10, 5), 0);
        assert!(border_source(size, INK, 1.0, BorderStyle::Solid, Corners::ZERO).is_none());
    }

    #[test]
    fn a_rounded_dotted_border_walks_its_whole_outline() {
        let size = Size { width: 40.0, height: 24.0 };
        let source = border_source(size, INK, 4.0, BorderStyle::Dotted, Corners::all(8.0))
            .expect("a dotted border is a traced path");
        let ImageSource::Path { paint, .. } = &source else { panic!("{source:?}") };
        assert!(matches!(paint, Paint::Stroke { .. }), "round dots ride the round pen");
        let raster = raster(&source, 40, 24);
        // the first dot sits half a pen in from where the straight top starts
        assert!(alpha(&raster, 8, 1) > 200, "a dot on the top edge");
        assert_eq!(alpha(&raster, 20, 12), 0, "the middle is not the border's");
    }

    #[test]
    fn bands_start_where_css_starts_the_gradient_line() {
        let size = Size { width: 8.0, height: 8.0 };
        // `0deg` repeats UP: the first band is at the bottom
        let up = stripes_source(size, &Stripes::new(Angle::ZERO, 2.0, 4.0, INK)).expect("bands");
        let raster_up = raster(&up, 8, 8);
        assert_eq!(alpha(&raster_up, 3, 7), 255, "the bottom row is the first band");
        assert_eq!(alpha(&raster_up, 3, 5), 0, "then air");
        assert_eq!(alpha(&raster_up, 3, 3), 255, "then the next band, two rows up");
        // a band as wide as its period is the whole box
        let whole = stripes_source(size, &Stripes::new(Angle::degrees(45.0), 4.0, 4.0, INK)).expect("bands");
        let raster_whole = raster(&whole, 8, 8);
        assert!(raster_whole.rgba.chunks(4).all(|pixel| pixel[3] == 255));
        assert!(stripes_source(size, &Stripes::new(Angle::ZERO, 0.0, 4.0, INK)).is_none());
    }
}
