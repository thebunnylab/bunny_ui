//! The geometry half of a glyph: verbs become device-space polylines.
//!
//! Curves flatten by Wang's formula — the segment count is DERIVED
//! from the curve's second differences, so the error is bounded before
//! the first point is emitted, and a gentle arc never pays for a tight
//! one. The tolerance is measured in DEVICE pixels: the same glyph
//! flattens coarser at sixteen and finer at sixty-four, and the eye
//! sees the same drawing at both.

use super::{Rule, Verb};

/// How far the polyline may sit from the true curve, in device px.
/// The error is SYSTEMATIC — a chord always sits inside its arc — so
/// the bound is tight: three hundredths keep the thinnest pen (a two
/// unit stroke on a sixteen pixel icon) within two percent of true.
const TOLERANCE: f64 = 0.03;

/// Sub-scanlines per pixel row. The fill is EXACT along x (intervals,
/// not samples) and quantized along y in steps of `1/ROWS` — at
/// sixteen, the step hides under the same anti-aliasing the tolerance
/// does. One power of two, so the partial coverages sum exactly.
const ROWS: usize = 16;

/// Wang's bound caps the segment count too — a degenerate table of
/// verbs cannot ask for an unbounded fan. High enough that no real
/// curve at icon sizes ever meets it (the clamp would BREAK the bound).
const MAX_SEGMENTS: usize = 128;

/// The grid→device placing: a uniform scale and a shift. Icons never
/// rotate, so the whole transform is three numbers — and the flattening
/// error scales with the SAME factor, which keeps the bound honest.
#[derive(Clone, Copy)]
pub(crate) struct Placing {
    pub scale: f64,
    pub dx: f64,
    pub dy: f64,
}

impl Placing {
    fn apply(&self, x: f32, y: f32) -> (f64, f64) {
        (x as f64 * self.scale + self.dx, y as f64 * self.scale + self.dy)
    }
}

/// Every contour of one path, flattened to device space. Points sit
/// back to back; `contours` marks where each begins and whether a
/// `Close` sealed it (the seal matters to a stroke — the fill closes
/// everything by law).
pub(crate) struct Flattened {
    points: Vec<(f64, f64)>,
    /// `(first point index, sealed)` per contour, in path order.
    contours: Vec<(usize, bool)>,
}

impl Flattened {
    /// The contours, each as `(points, sealed)`.
    pub fn contours(&self) -> impl Iterator<Item = (&[(f64, f64)], bool)> {
        self.contours.iter().enumerate().map(move |(i, &(start, sealed))| {
            let end = self
                .contours
                .get(i + 1)
                .map(|&(next, _)| next)
                .unwrap_or(self.points.len());
            (&self.points[start..end], sealed)
        })
    }
}

/// Flattens one verb table under a placing. Zero-length segments are
/// dropped on the spot — they carry no ink and a stroke would read a
/// direction from them that does not exist.
pub(crate) fn flatten(path: &[Verb], placing: Placing) -> Flattened {
    let mut out = Flattened { points: Vec::new(), contours: Vec::new() };
    // Where the OPEN contour started — both the index and the point,
    // so `Close` can seal and a verb after it can reopen at the seam.
    let mut start: Option<(usize, (f64, f64))> = None;
    let mut current: Option<(f64, f64)> = None;

    for verb in path {
        match *verb {
            Verb::Move(x, y) => {
                let point = placing.apply(x, y);
                begin(&mut out, &mut start, point);
                current = Some(point);
            }
            Verb::Line(x, y) => {
                let Some(from) = reopen(&mut out, &mut start, &mut current) else {
                    continue;
                };
                let to = placing.apply(x, y);
                push(&mut out, from, to);
                current = Some(to);
            }
            Verb::Quad(cx, cy, x, y) => {
                let Some(from) = reopen(&mut out, &mut start, &mut current) else {
                    continue;
                };
                let control = placing.apply(cx, cy);
                let to = placing.apply(x, y);
                let mut last = from;
                for point in quad_points(from, control, to) {
                    push(&mut out, last, point);
                    last = point;
                }
                current = Some(to);
            }
            Verb::Cubic(ax, ay, bx, by, x, y) => {
                let Some(from) = reopen(&mut out, &mut start, &mut current) else {
                    continue;
                };
                let a = placing.apply(ax, ay);
                let b = placing.apply(bx, by);
                let to = placing.apply(x, y);
                let mut last = from;
                for point in cubic_points(from, a, b, to) {
                    push(&mut out, last, point);
                    last = point;
                }
                current = Some(to);
            }
            Verb::Close => {
                if let Some((index, seam)) = start {
                    let last = out.contours.len() - 1;
                    out.contours[last] = (index, true);
                    // the pen returns to the seam (SVG law) — a verb
                    // that follows without a Move draws from there
                    current = Some(seam);
                    start = None;
                }
            }
        }
    }
    // a contour that never grew past its Move carries no ink
    prune_empty(&mut out);
    out
}

/// Opens a contour at `point`. A `Move` lands here — and so does the
/// first verb after a `Close`, through [`reopen`].
fn begin(out: &mut Flattened, start: &mut Option<(usize, (f64, f64))>, point: (f64, f64)) {
    prune_empty(out);
    *start = Some((out.points.len(), point));
    out.contours.push((out.points.len(), false));
    out.points.push(point);
}

/// The current point a drawing verb continues from. `None` means the
/// table broke the law (a path begins with `Move`) — debug says so,
/// release skips the verb.
fn reopen(
    out: &mut Flattened,
    start: &mut Option<(usize, (f64, f64))>,
    current: &mut Option<(f64, f64)>,
) -> Option<(f64, f64)> {
    let point = (*current)?;
    if start.is_none() {
        // after a Close the next subpath opens AT the seam
        begin(out, start, point);
    }
    debug_assert!(current.is_some(), "a path begins with Move");
    Some(point)
}

/// One segment, unless it is nothing.
fn push(out: &mut Flattened, from: (f64, f64), to: (f64, f64)) {
    if to != from {
        out.points.push(to);
    }
}

/// Drops an open contour that holds only its `Move`.
fn prune_empty(out: &mut Flattened) {
    if let Some(&(first, _)) = out.contours.last() {
        if out.points.len() - first < 2 {
            out.points.truncate(first);
            out.contours.pop();
        }
    }
}

/// Wang's segment count for a quadratic: the second derivative is the
/// constant `2·(p0 − 2c + p2)`, and `n` segments leave at most
/// `|B''| / (8n²)` of gap.
fn quad_segments(p0: (f64, f64), c: (f64, f64), p2: (f64, f64)) -> usize {
    let dx = p0.0 - 2.0 * c.0 + p2.0;
    let dy = p0.1 - 2.0 * c.1 + p2.1;
    let second = 2.0 * dx.hypot(dy);
    segments_for(second)
}

/// Wang's segment count for a cubic: `|B''| ≤ 6·max` of the two second
/// differences.
fn cubic_segments(p0: (f64, f64), a: (f64, f64), b: (f64, f64), p3: (f64, f64)) -> usize {
    let first = (p0.0 - 2.0 * a.0 + b.0).hypot(p0.1 - 2.0 * a.1 + b.1);
    let second = (a.0 - 2.0 * b.0 + p3.0).hypot(a.1 - 2.0 * b.1 + p3.1);
    segments_for(6.0 * first.max(second))
}

/// `n ≥ sqrt(|B''|max / (8·tolerance))` — the bound both curve kinds
/// share once their `|B''|` is known.
fn segments_for(second_max: f64) -> usize {
    let n = (second_max / (8.0 * TOLERANCE)).sqrt().ceil();
    (n as usize).clamp(1, MAX_SEGMENTS)
}

/// The quadratic's points AFTER the start, at uniform parameters.
fn quad_points(
    p0: (f64, f64),
    c: (f64, f64),
    p2: (f64, f64),
) -> impl Iterator<Item = (f64, f64)> {
    let n = quad_segments(p0, c, p2);
    (1..=n).map(move |i| {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        let x = u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p2.0;
        let y = u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p2.1;
        (x, y)
    })
}

/// The cubic's points AFTER the start, at uniform parameters.
fn cubic_points(
    p0: (f64, f64),
    a: (f64, f64),
    b: (f64, f64),
    p3: (f64, f64),
) -> impl Iterator<Item = (f64, f64)> {
    let n = cubic_segments(p0, a, b, p3);
    (1..=n).map(move |i| {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        let x = u * u * u * p0.0
            + 3.0 * u * u * t * a.0
            + 3.0 * u * t * t * b.0
            + t * t * t * p3.0;
        let y = u * u * u * p0.1
            + 3.0 * u * u * t * a.1
            + 3.0 * u * t * t * b.1
            + t * t * t * p3.1;
        (x, y)
    })
}

/// A coverage mask: one `0..=1` per pixel. Draws of one glyph pile in
/// by MAX — same ink, so overlap is union, and a translucent tint can
/// never double-blend where a join or a second draw crosses itself.
pub(crate) struct Mask {
    width: usize,
    height: usize,
    values: Vec<f32>,
}

impl Mask {
    pub fn new(width: usize, height: usize) -> Mask {
        Mask { width, height, values: vec![0.0; width * height] }
    }

    pub fn clear(&mut self) {
        self.values.fill(0.0);
    }

    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.values[y * self.width + x]
    }

    fn max_at(&mut self, x: usize, y: usize, coverage: f32) {
        let value = &mut self.values[y * self.width + x];
        *value = value.max(coverage);
    }

    /// Folds another draw's mask in.
    pub fn merge_max(&mut self, other: &Mask) {
        for (value, more) in self.values.iter_mut().zip(&other.values) {
            *value = value.max(*more);
        }
    }
}

/// Fills the inside of `flat` into `mask` under `rule`. Every contour
/// closes (SVG law — the seal only matters to a stroke). The walk is
/// interval-exact in x: crossings on each sub-scanline sort, the rule
/// picks the inside runs, and each run adds its exact overlap with
/// each pixel. Sub-rows partition y, so the parts sum to true area
/// with only the vertical quantization left.
pub(crate) fn fill_into(mask: &mut Mask, flat: &Flattened, rule: Rule) {
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    let mut row: Vec<f32> = vec![0.0; mask.width];
    for y in 0..mask.height {
        row.fill(0.0);
        let mut touched = false;
        for sub in 0..ROWS {
            let sample = y as f64 + (sub as f64 + 0.5) / ROWS as f64;
            crossings.clear();
            for (points, _) in flat.contours() {
                let count = points.len();
                for i in 0..count {
                    let (x0, y0) = points[i];
                    let (x1, y1) = points[(i + 1) % count];
                    // half-open in y: a vertex on the sample counts once
                    let (dir, top, bottom, tx, bx) = if y1 > y0 {
                        (1, y0, y1, x0, x1)
                    } else if y0 > y1 {
                        (-1, y1, y0, x1, x0)
                    } else {
                        continue; // horizontal — its neighbors cross
                    };
                    if sample >= top && sample < bottom {
                        let t = (sample - top) / (bottom - top);
                        crossings.push((tx + t * (bx - tx), dir));
                    }
                }
            }
            if crossings.is_empty() {
                continue;
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            // walk the crossings; the rule decides where inside begins
            let mut winding = 0;
            let mut entered = 0.0;
            for &(x, dir) in &crossings {
                let was_inside = match rule {
                    Rule::NonZero => winding != 0,
                    Rule::EvenOdd => winding % 2 != 0,
                };
                winding += dir;
                let is_inside = match rule {
                    Rule::NonZero => winding != 0,
                    Rule::EvenOdd => winding % 2 != 0,
                };
                if !was_inside && is_inside {
                    entered = x;
                } else if was_inside && !is_inside {
                    touched |= add_run(&mut row, entered, x);
                }
            }
        }
        if touched {
            for x in 0..mask.width {
                if row[x] > 0.0 {
                    mask.max_at(x, y, row[x].min(1.0));
                }
            }
        }
    }
}

/// Adds one inside run `[from, to]` of one sub-scanline into the row:
/// each pixel takes its exact overlap, scaled by the sub-row's share.
fn add_run(row: &mut [f32], from: f64, to: f64) -> bool {
    let width = row.len() as f64;
    let from = from.max(0.0);
    let to = to.min(width);
    if to <= from {
        return false;
    }
    let share = 1.0 / ROWS as f64;
    let first = from.floor() as usize;
    let last = (to.ceil() as usize).min(row.len());
    for x in first..last {
        let cover = (to.min(x as f64 + 1.0) - from.max(x as f64)).max(0.0);
        row[x] += (cover * share) as f32;
    }
    true
}

/// Strokes `flat` into `mask` with a pen `width` wide (device px).
/// The whole geometry is DISTANCE: a pixel's coverage is the house
/// kernel `(width/2 − distance + 0.5).clamp(0, 1)` against the nearest
/// segment — which hands out round caps and round joins for free (the
/// near-end of a segment IS a disc), and the MAX accumulation makes
/// self-overlap union, never a double blend. A sealed contour strokes
/// its closing segment; an open one wears its caps.
pub(crate) fn stroke_into(mask: &mut Mask, flat: &Flattened, width: f64) {
    let half = width / 2.0;
    // pixels whose CENTER sits within the kernel's reach of the
    // segment — one pixel of slack keeps the cut clearly outside
    let reach = half + 1.5;
    for (points, sealed) in flat.contours() {
        let count = points.len();
        let segments = if sealed { count } else { count - 1 };
        for i in 0..segments {
            let a = points[i];
            let b = points[(i + 1) % count];
            let x0 = ((a.0.min(b.0) - reach).floor().max(0.0)) as usize;
            let y0 = ((a.1.min(b.1) - reach).floor().max(0.0)) as usize;
            let x1 = ((a.0.max(b.0) + reach).ceil().max(0.0) as usize).min(mask.width);
            let y1 = ((a.1.max(b.1) + reach).ceil().max(0.0) as usize).min(mask.height);
            for y in y0..y1 {
                for x in x0..x1 {
                    let center = (x as f64 + 0.5, y as f64 + 0.5);
                    let distance = segment_distance(center, a, b);
                    let coverage = (half - distance + 0.5).clamp(0.0, 1.0);
                    if coverage > 0.0 {
                        mask.max_at(x, y, coverage as f32);
                    }
                }
            }
        }
    }
}

/// Distance from a point to one segment — the stroke's whole geometry,
/// and the test bench's ruler.
pub(crate) fn segment_distance(
    (px, py): (f64, f64),
    (ax, ay): (f64, f64),
    (bx, by): (f64, f64),
) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let length2 = dx * dx + dy * dy;
    if length2 == 0.0 {
        return (px - ax).hypot(py - ay);
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / length2).clamp(0.0, 1.0);
    (px - ax - t * dx).hypot(py - ay - t * dy)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: Placing = Placing { scale: 1.0, dx: 0.0, dy: 0.0 };

    fn polyline_distance(point: (f64, f64), points: &[(f64, f64)]) -> f64 {
        points
            .windows(2)
            .map(|pair| segment_distance(point, pair[0], pair[1]))
            .fold(f64::INFINITY, f64::min)
    }

    #[test]
    fn a_line_flattens_to_its_two_ends() {
        let flat = flatten(&[Verb::Move(1.0, 2.0), Verb::Line(21.0, 2.0)], ONE);
        let all: Vec<_> = flat.contours().collect();
        assert_eq!(all.len(), 1);
        let (points, sealed) = all[0];
        assert_eq!(points, &[(1.0, 2.0), (21.0, 2.0)]);
        assert!(!sealed);
    }

    #[test]
    fn a_close_seals_the_contour() {
        let flat = flatten(
            &[
                Verb::Move(2.0, 2.0),
                Verb::Line(22.0, 2.0),
                Verb::Line(12.0, 20.0),
                Verb::Close,
            ],
            ONE,
        );
        let all: Vec<_> = flat.contours().collect();
        assert_eq!(all.len(), 1);
        let (points, sealed) = all[0];
        // the seal is a FLAG, not a repeated point — consumers wrap
        assert_eq!(points.len(), 3);
        assert!(sealed);
    }

    #[test]
    fn a_verb_after_close_opens_at_the_seam() {
        let flat = flatten(
            &[
                Verb::Move(2.0, 2.0),
                Verb::Line(10.0, 2.0),
                Verb::Close,
                Verb::Line(2.0, 10.0),
            ],
            ONE,
        );
        let all: Vec<_> = flat.contours().collect();
        assert_eq!(all.len(), 2);
        // SVG law: after Close the pen sits on the seam again
        assert_eq!(all[1].0, &[(2.0, 2.0), (2.0, 10.0)]);
        assert!(!all[1].1);
    }

    #[test]
    fn two_moves_make_two_contours() {
        let flat = flatten(
            &[
                Verb::Move(0.0, 0.0),
                Verb::Line(4.0, 0.0),
                Verb::Move(0.0, 8.0),
                Verb::Line(4.0, 8.0),
            ],
            ONE,
        );
        assert_eq!(flat.contours().count(), 2);
    }

    #[test]
    fn a_lonely_move_carries_no_ink() {
        let flat = flatten(&[Verb::Move(3.0, 3.0)], ONE);
        assert_eq!(flat.contours().count(), 0);
        let flat = flatten(
            &[Verb::Move(3.0, 3.0), Verb::Move(5.0, 5.0), Verb::Line(9.0, 5.0)],
            ONE,
        );
        assert_eq!(flat.contours().count(), 1);
    }

    #[test]
    fn zero_length_segments_are_dropped() {
        let flat = flatten(
            &[Verb::Move(5.0, 5.0), Verb::Line(5.0, 5.0), Verb::Line(10.0, 5.0)],
            ONE,
        );
        let all: Vec<_> = flat.contours().collect();
        assert_eq!(all[0].0.len(), 2);
    }

    /// A quarter circle as the standard cubic (kappa control points),
    /// scaled up hard: every sampled true point stays within TOLERANCE
    /// of the polyline.
    #[test]
    fn a_cubic_stays_within_the_bound() {
        const KAPPA: f32 = 0.552_284_75;
        let radius = 12.0_f32;
        let path = [
            Verb::Move(radius, 0.0),
            Verb::Cubic(radius, radius * KAPPA, radius * KAPPA, radius, 0.0, radius),
        ];
        let placing = Placing { scale: 20.0, dx: 0.0, dy: 0.0 };
        let flat = flatten(&path, placing);
        let all: Vec<_> = flat.contours().collect();
        let points = all[0].0;
        let r = radius as f64 * 20.0;
        let k = KAPPA as f64;
        for i in 0..=200 {
            let t = i as f64 / 200.0;
            let u = 1.0 - t;
            let x = u * u * u * r + 3.0 * u * u * t * r + 3.0 * u * t * t * (r * k);
            let y = 3.0 * u * u * t * (r * k) + 3.0 * u * t * t * r + t * t * t * r;
            let gap = polyline_distance((x, y), points);
            assert!(gap <= TOLERANCE, "t={t}: gap {gap} beyond the bound");
        }
    }

    #[test]
    fn a_quad_stays_within_the_bound() {
        let path = [Verb::Move(0.0, 20.0), Verb::Quad(12.0, -20.0, 24.0, 20.0)];
        let placing = Placing { scale: 8.0, dx: 3.0, dy: 5.0 };
        let flat = flatten(&path, placing);
        let all: Vec<_> = flat.contours().collect();
        let points = all[0].0;
        let p0 = (3.0, 20.0 * 8.0 + 5.0);
        let c = (12.0 * 8.0 + 3.0, -20.0 * 8.0 + 5.0);
        let p2 = (24.0 * 8.0 + 3.0, 20.0 * 8.0 + 5.0);
        for i in 0..=200 {
            let t = i as f64 / 200.0;
            let u = 1.0 - t;
            let x = u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p2.0;
            let y = u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p2.1;
            let gap = polyline_distance((x, y), points);
            assert!(gap <= TOLERANCE, "t={t}: gap {gap} beyond the bound");
        }
    }

    /// The bound is thrifty as well as safe: a small arc must not fan
    /// out, a big one must not stay coarse — and a future "improvement"
    /// must not silently double the work.
    #[test]
    fn the_segment_count_is_derived_not_guessed() {
        let small = cubic_segments((16.0, 0.0), (16.0, 8.8), (8.8, 16.0), (0.0, 16.0));
        assert!(
            (8..=20).contains(&small),
            "a 16px quarter circle wants a handful, took {small}"
        );
        let big = cubic_segments((320.0, 0.0), (320.0, 176.0), (176.0, 320.0), (0.0, 320.0));
        assert!(
            (32..MAX_SEGMENTS).contains(&big),
            "a 320px quarter circle wants real segments, took {big}"
        );
        assert!(big > small, "the count follows the size");
    }

    #[test]
    fn a_degenerate_curve_is_one_segment() {
        // controls on the chord: |B''| = 0 — a straight line in curve's
        // clothing takes exactly one segment
        let n = cubic_segments((0.0, 0.0), (8.0, 8.0), (16.0, 16.0), (24.0, 24.0));
        assert_eq!(n, 1);
    }

    #[test]
    fn the_placing_lands_where_it_says() {
        let placing = Placing { scale: 2.0, dx: 10.0, dy: 20.0 };
        let flat = flatten(&[Verb::Move(0.0, 0.0), Verb::Line(24.0, 24.0)], placing);
        let all: Vec<_> = flat.contours().collect();
        assert_eq!(all[0].0, &[(10.0, 20.0), (58.0, 68.0)]);
    }

    fn filled(path: &[Verb], rule: Rule, side: usize) -> Mask {
        let mut mask = Mask::new(side, side);
        fill_into(&mut mask, &flatten(path, ONE), rule);
        mask
    }

    #[test]
    fn an_axis_rectangle_fills_exactly() {
        // integer edges: x is interval-exact and the sixteen sub-rows
        // sum to exactly one — full pixels read 1.0, not almost
        let path = [
            Verb::Move(3.0, 4.0),
            Verb::Line(13.0, 4.0),
            Verb::Line(13.0, 11.0),
            Verb::Line(3.0, 11.0),
            Verb::Close,
        ];
        let mask = filled(&path, Rule::NonZero, 16);
        for y in 0..16 {
            for x in 0..16 {
                let inside = (3..13).contains(&x) && (4..11).contains(&y);
                let want = if inside { 1.0 } else { 0.0 };
                assert_eq!(mask.at(x, y), want, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn an_open_contour_fills_closed() {
        // no seal — the fill closes it anyway (SVG law)
        let open = [
            Verb::Move(2.0, 2.0),
            Verb::Line(12.0, 2.0),
            Verb::Line(12.0, 12.0),
            Verb::Line(2.0, 12.0),
        ];
        let mask = filled(&open, Rule::NonZero, 14);
        assert_eq!(mask.at(7, 7), 1.0);
        assert_eq!(mask.at(3, 3), 1.0);
    }

    /// Two concentric rings wound the SAME way: even-odd cuts the
    /// hole, non-zero paints it solid.
    #[test]
    fn a_donut_obeys_both_rules() {
        let rings = [
            Verb::Move(1.0, 1.0),
            Verb::Line(15.0, 1.0),
            Verb::Line(15.0, 15.0),
            Verb::Line(1.0, 15.0),
            Verb::Close,
            Verb::Move(5.0, 5.0),
            Verb::Line(11.0, 5.0),
            Verb::Line(11.0, 11.0),
            Verb::Line(5.0, 11.0),
            Verb::Close,
        ];
        let even = filled(&rings, Rule::EvenOdd, 16);
        assert_eq!(even.at(8, 8), 0.0, "even-odd cuts the hole");
        assert_eq!(even.at(3, 8), 1.0, "the ring stays inked");
        let solid = filled(&rings, Rule::NonZero, 16);
        assert_eq!(solid.at(8, 8), 1.0, "non-zero fills through");
    }

    /// The inner ring wound the OTHER way: now non-zero cuts the hole
    /// too — the winding direction is read, not assumed.
    #[test]
    fn a_reversed_inner_ring_holes_nonzero_too() {
        let rings = [
            Verb::Move(1.0, 1.0),
            Verb::Line(15.0, 1.0),
            Verb::Line(15.0, 15.0),
            Verb::Line(1.0, 15.0),
            Verb::Close,
            Verb::Move(5.0, 5.0),
            Verb::Line(5.0, 11.0),
            Verb::Line(11.0, 11.0),
            Verb::Line(11.0, 5.0),
            Verb::Close,
        ];
        let mask = filled(&rings, Rule::NonZero, 16);
        assert_eq!(mask.at(8, 8), 0.0);
        assert_eq!(mask.at(3, 8), 1.0);
    }

    /// Where a contour overlaps itself the winding reaches two — the
    /// coverage must still read ONE, never a double blend.
    #[test]
    fn a_self_overlap_never_exceeds_one() {
        let bowtie = [
            Verb::Move(2.0, 2.0),
            Verb::Line(12.0, 2.0),
            Verb::Line(12.0, 12.0),
            Verb::Line(6.0, 12.0),
            Verb::Line(6.0, 6.0),
            Verb::Line(9.0, 6.0),
            Verb::Line(9.0, 9.0),
            Verb::Line(2.0, 9.0),
            Verb::Close,
        ];
        let mask = filled(&bowtie, Rule::NonZero, 14);
        // (7,7) sits inside both loops of the spiral: winding two
        assert_eq!(mask.at(7, 7), 1.0);
        for y in 0..14 {
            for x in 0..14 {
                assert!(mask.at(x, y) <= 1.0);
            }
        }
    }

    /// The independent referee: the textbook winding-number point test,
    /// sharing NO code with the fill's crossing walk.
    fn winding_at(point: (f64, f64), flat: &Flattened) -> i32 {
        let (px, py) = point;
        let mut winding = 0;
        for (points, _) in flat.contours() {
            let count = points.len();
            for i in 0..count {
                let (ax, ay) = points[i];
                let (bx, by) = points[(i + 1) % count];
                let cross = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
                if ay <= py {
                    if by > py && cross > 0.0 {
                        winding += 1;
                    }
                } else if by <= py && cross < 0.0 {
                    winding -= 1;
                }
            }
        }
        winding
    }

    /// The oracle that catches what portraits cannot: for a bag of
    /// shapes, the mask must agree with a brute-force 16×16 point count
    /// per pixel. The gate is DERIVED, not shrugged: the fill is
    /// interval-exact in x while the referee samples sixteen points, so
    /// they may part by at most 1/16 per pixel — everything beyond is a
    /// sign, an order or a parity gone wrong.
    #[test]
    fn the_coverage_matches_a_brute_force_count() {
        const KAPPA: f32 = 0.552_284_75;
        let diamond = vec![
            Verb::Move(8.0, 0.5),
            Verb::Line(15.5, 8.0),
            Verb::Line(8.0, 15.5),
            Verb::Line(0.5, 8.0),
            Verb::Close,
        ];
        let triangle = vec![
            Verb::Move(1.3, 14.2),
            Verb::Line(14.8, 12.9),
            Verb::Line(3.1, 1.6),
            Verb::Close,
        ];
        let rings = vec![
            Verb::Move(1.0, 1.0),
            Verb::Line(15.0, 1.0),
            Verb::Line(15.0, 15.0),
            Verb::Line(1.0, 15.0),
            Verb::Close,
            Verb::Move(4.5, 4.5),
            Verb::Line(11.5, 4.5),
            Verb::Line(11.5, 11.5),
            Verb::Line(4.5, 11.5),
            Verb::Close,
        ];
        let circle = vec![
            Verb::Move(15.0, 8.0),
            Verb::Cubic(15.0, 8.0 + 7.0 * KAPPA, 8.0 + 7.0 * KAPPA, 15.0, 8.0, 15.0),
            Verb::Cubic(8.0 - 7.0 * KAPPA, 15.0, 1.0, 8.0 + 7.0 * KAPPA, 1.0, 8.0),
            Verb::Cubic(1.0, 8.0 - 7.0 * KAPPA, 8.0 - 7.0 * KAPPA, 1.0, 8.0, 1.0),
            Verb::Cubic(8.0 + 7.0 * KAPPA, 1.0, 15.0, 8.0 - 7.0 * KAPPA, 15.0, 8.0),
            Verb::Close,
        ];
        let cases: [(&str, &[Verb], Rule); 5] = [
            ("diamond", &diamond, Rule::NonZero),
            ("triangle", &triangle, Rule::NonZero),
            ("rings even-odd", &rings, Rule::EvenOdd),
            ("rings non-zero", &rings, Rule::NonZero),
            ("circle", &circle, Rule::NonZero),
        ];
        for (label, path, rule) in cases {
            let flat = flatten(path, ONE);
            let mut mask = Mask::new(16, 16);
            fill_into(&mut mask, &flat, rule);
            for y in 0..16 {
                for x in 0..16 {
                    let mut hits = 0;
                    for sy in 0..16 {
                        for sx in 0..16 {
                            let point = (
                                x as f64 + (sx as f64 + 0.5) / 16.0,
                                y as f64 + (sy as f64 + 0.5) / 16.0,
                            );
                            let winding = winding_at(point, &flat);
                            let inside = match rule {
                                Rule::NonZero => winding != 0,
                                Rule::EvenOdd => winding % 2 != 0,
                            };
                            if inside {
                                hits += 1;
                            }
                        }
                    }
                    let reference = hits as f32 / 256.0;
                    let gap = (mask.at(x, y) - reference).abs();
                    assert!(
                        gap <= 1.0 / 16.0 + 1e-4,
                        "{label} ({x},{y}): mask {} vs referee {reference}",
                        mask.at(x, y),
                    );
                }
            }
        }
    }

    fn stroked(path: &[Verb], width: f64, side: usize) -> Mask {
        let mut mask = Mask::new(side, side);
        stroke_into(&mut mask, &flatten(path, ONE), width);
        mask
    }

    /// The stroke's oracle is its own definition: for every pixel, the
    /// kernel of the brute-force MIN distance over every segment. The
    /// bounding boxes in `stroke_into` may skip pixels — never drop
    /// ink — so the two must agree to the LAST BIT.
    #[test]
    fn the_stroke_matches_the_field_it_claims() {
        let path = [
            Verb::Move(2.5, 3.1),
            Verb::Line(12.7, 2.4),
            Verb::Line(13.9, 12.2),
            Verb::Line(4.1, 13.6),
            Verb::Line(2.2, 7.7),
        ];
        let width = 2.6;
        let flat = flatten(&path, ONE);
        let mask = stroked(&path, width, 16);
        let points: Vec<_> = flat.contours().next().unwrap().0.to_vec();
        for y in 0..16 {
            for x in 0..16 {
                let center = (x as f64 + 0.5, y as f64 + 0.5);
                let nearest = points
                    .windows(2)
                    .map(|pair| segment_distance(center, pair[0], pair[1]))
                    .fold(f64::INFINITY, f64::min);
                let want = (width / 2.0 - nearest + 0.5).clamp(0.0, 1.0) as f32;
                assert_eq!(mask.at(x, y), want, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn a_stroked_line_wears_round_caps() {
        let path = [Verb::Move(4.0, 8.0), Verb::Line(12.0, 8.0)];
        let mask = stroked(&path, 3.0, 16);
        // past the endpoint along the axis: still under the cap's disc
        assert!(mask.at(12, 8) > 0.9, "the cap reaches past the end");
        // past the disc: bare
        assert_eq!(mask.at(14, 8), 0.0);
        // the cap is ROUND: on the axis the shoulder fades…
        let shoulder = mask.at(13, 8);
        assert!(
            shoulder > 0.0 && shoulder < 1.0,
            "the shoulder anti-aliases, read {shoulder}"
        );
        // …and the diagonal corner a SQUARE cap would ink stays bare
        assert_eq!(mask.at(13, 9), 0.0);
    }

    #[test]
    fn a_sealed_triangle_strokes_its_closing_edge() {
        let open = [
            Verb::Move(2.0, 2.0),
            Verb::Line(14.0, 2.0),
            Verb::Line(8.0, 13.0),
        ];
        let sealed = [
            Verb::Move(2.0, 2.0),
            Verb::Line(14.0, 2.0),
            Verb::Line(8.0, 13.0),
            Verb::Close,
        ];
        // midpoint of the closing edge (2,2)-(8,13)
        let probe = (5, 7);
        let bare = stroked(&open, 2.0, 16);
        let inked = stroked(&sealed, 2.0, 16);
        assert_eq!(bare.at(probe.0, probe.1), 0.0, "no seal, no closing edge");
        assert!(inked.at(probe.0, probe.1) > 0.9, "the seal draws it");
    }

    #[test]
    fn a_sharp_join_stays_at_one() {
        // a hairpin — the join region is covered by both segments
        let path = [
            Verb::Move(3.0, 3.0),
            Verb::Line(13.0, 3.0),
            Verb::Line(3.0, 5.0),
        ];
        let mask = stroked(&path, 2.5, 16);
        for y in 0..16 {
            for x in 0..16 {
                assert!(mask.at(x, y) <= 1.0, "({x},{y}) overflowed the union");
            }
        }
        // the elbow itself is solid
        assert_eq!(mask.at(12, 3), 1.0);
    }

    #[test]
    fn merge_max_takes_the_union() {
        let mut a = Mask::new(4, 1);
        let mut b = Mask::new(4, 1);
        a.max_at(0, 0, 0.8);
        a.max_at(1, 0, 0.2);
        b.max_at(1, 0, 0.9);
        a.merge_max(&b);
        assert_eq!(a.at(0, 0), 0.8);
        assert_eq!(a.at(1, 0), 0.9);
        assert_eq!(a.at(2, 0), 0.0);
    }
}
