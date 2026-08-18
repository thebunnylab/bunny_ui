//! The geometry half of a glyph: verbs become device-space polylines.
//!
//! Curves flatten by Wang's formula — the segment count is DERIVED
//! from the curve's second differences, so the error is bounded before
//! the first point is emitted, and a gentle arc never pays for a tight
//! one. The tolerance is measured in DEVICE pixels: the same glyph
//! flattens coarser at sixteen and finer at sixty-four, and the eye
//! sees the same drawing at both.

use super::Verb;

/// How far the polyline may sit from the true curve, in device px.
/// A tenth of a pixel disappears under the coverage anti-aliasing.
const TOLERANCE: f64 = 0.1;

/// Wang's bound caps the segment count too — a degenerate table of
/// verbs cannot ask for an unbounded fan.
const MAX_SEGMENTS: usize = 64;

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
            (4..=12).contains(&small),
            "a 16px quarter circle wants a handful, took {small}"
        );
        let big = cubic_segments((320.0, 0.0), (320.0, 176.0), (176.0, 320.0), (0.0, 320.0));
        assert!(
            (16..=MAX_SEGMENTS).contains(&big),
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
}
