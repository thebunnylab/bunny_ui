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

/// The grid→device placing: a turn about a pivot, a uniform scale and
/// a shift. A turn and a uniform scale keep every length's ratio, so
/// the flattening error scales with the SAME factor the drawing does,
/// which keeps the bound honest.
#[derive(Clone, Copy)]
pub(crate) struct Placing {
    pub scale: f64,
    pub dx: f64,
    pub dy: f64,
    /// Applied FIRST, in the table's own units: a glyph turns about the
    /// centre of its grid, whatever size it lands at.
    pub spin: Spin,
}

impl Placing {
    fn apply(&self, x: f32, y: f32) -> (f64, f64) {
        let (x, y) = self.spin.apply(x as f64, y as f64);
        (x * self.scale + self.dx, y * self.scale + self.dy)
    }
}

/// A turn about one point, clockwise on the y-down screen — the way
/// CSS's `rotate()` and SwiftUI's `rotationEffect` read an angle.
#[derive(Clone, Copy)]
pub(crate) struct Spin {
    cos: f64,
    sin: f64,
    cx: f64,
    cy: f64,
}

impl Spin {
    /// No turn at all. It is exact, not merely close: `x·1 − y·0` is
    /// `x` to the last bit, so an upright glyph keeps the bytes it
    /// always had.
    pub const NONE: Spin = Spin { cos: 1.0, sin: 0.0, cx: 0.0, cy: 0.0 };

    /// `radians` clockwise about `(cx, cy)`.
    pub fn about(radians: f64, cx: f64, cy: f64) -> Spin {
        if radians == 0.0 {
            return Spin::NONE;
        }
        let (sin, cos) = radians.sin_cos();
        Spin { cos, sin, cx, cy }
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let (dx, dy) = (x - self.cx, y - self.cy);
        (dx * self.cos - dy * self.sin + self.cx, dx * self.sin + dy * self.cos + self.cy)
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
    // did a drawing verb touch the OPEN contour? SVG strokes a subpath
    // that was drawn to, never a lone `Move`
    let mut drawn = false;

    for verb in path {
        match *verb {
            Verb::Move(x, y) => {
                let point = placing.apply(x, y);
                begin(&mut out, &mut start, &mut drawn, point);
                current = Some(point);
            }
            Verb::Line(x, y) => {
                let Some(from) = reopen(&mut out, &mut start, &mut drawn, &mut current) else {
                    continue;
                };
                let to = placing.apply(x, y);
                push(&mut out, from, to);
                current = Some(to);
            }
            Verb::Quad(cx, cy, x, y) => {
                let Some(from) = reopen(&mut out, &mut start, &mut drawn, &mut current) else {
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
                let Some(from) = reopen(&mut out, &mut start, &mut drawn, &mut current) else {
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
    prune_empty(&mut out, drawn);
    out
}

/// Opens a contour at `point`. A `Move` lands here — and so does the
/// first verb after a `Close`, through [`reopen`].
fn begin(
    out: &mut Flattened,
    start: &mut Option<(usize, (f64, f64))>,
    drawn: &mut bool,
    point: (f64, f64),
) {
    prune_empty(out, *drawn);
    *drawn = false;
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
    drawn: &mut bool,
    current: &mut Option<(f64, f64)>,
) -> Option<(f64, f64)> {
    let point = (*current)?;
    if start.is_none() {
        // after a Close the next subpath opens AT the seam
        begin(out, start, drawn, point);
    }
    debug_assert!(current.is_some(), "a path begins with Move");
    // whatever verb asked for the current point is about to draw
    *drawn = true;
    Some(point)
}

/// One segment, unless it is nothing.
fn push(out: &mut Flattened, from: (f64, f64), to: (f64, f64)) {
    if to != from {
        out.points.push(to);
    }
}

/// Drops a contour that holds only its `Move` and was never DRAWN to.
/// One that WAS drawn to — a pen put down and lifted at the same
/// point — survives as a single point, and the round cap makes it a
/// dot. That is the rule SVG names in so many words: a subpath of a
/// lone `moveto` is not stroked; a `moveto` and a line back to the
/// same place is.
fn prune_empty(out: &mut Flattened, drawn: bool) {
    if drawn {
        return;
    }
    if let Some(&(first, _)) = out.contours.last()
        && out.points.len() - first < 2
    {
        out.points.truncate(first);
        out.contours.pop();
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
    /// The half-open box of pixels that were ever written, `(x0, y0,
    /// x1, y1)`. A traced path's box is the whole drawing while its
    /// ink is a thread through it — the ink is what the blend must
    /// walk, never the box.
    dirty: Option<(usize, usize, usize, usize)>,
}

impl Mask {
    pub fn new(width: usize, height: usize) -> Mask {
        Mask { width, height, values: vec![0.0; width * height], dirty: None }
    }

    pub fn clear(&mut self) {
        self.values.fill(0.0);
        self.dirty = None;
    }

    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.values[y * self.width + x]
    }

    /// What was touched — `None` when nothing was.
    pub fn dirty(&self) -> Option<(usize, usize, usize, usize)> {
        self.dirty
    }

    fn touch(&mut self, x: usize, y: usize) {
        match &mut self.dirty {
            None => self.dirty = Some((x, y, x + 1, y + 1)),
            Some((x0, y0, x1, y1)) => {
                *x0 = (*x0).min(x);
                *y0 = (*y0).min(y);
                *x1 = (*x1).max(x + 1);
                *y1 = (*y1).max(y + 1);
            }
        }
    }

    fn max_at(&mut self, x: usize, y: usize, coverage: f32) {
        let value = &mut self.values[y * self.width + x];
        *value = value.max(coverage);
        self.touch(x, y);
    }

    /// Folds another draw's mask in.
    pub fn merge_max(&mut self, other: &Mask) {
        let Some((x0, y0, x1, y1)) = other.dirty else { return };
        for y in y0..y1 {
            for x in x0..x1 {
                let more = other.values[y * other.width + x];
                if more > 0.0 {
                    self.max_at(x, y, more);
                }
            }
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
            for (x, coverage) in row.iter().copied().enumerate().take(mask.width) {
                if coverage > 0.0 {
                    mask.max_at(x, y, coverage.min(1.0));
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
    for (x, coverage) in row.iter_mut().enumerate().take(last).skip(first) {
        let cover = (to.min(x as f64 + 1.0) - from.max(x as f64)).max(0.0);
        *coverage += (cover * share) as f32;
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
        let segments = match (sealed, count) {
            // a contour that is only a point: the round cap IS the mark
            (_, 1) => 1,
            (true, _) => count,
            (false, _) => count - 1,
        };
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

// MARK: - Dashes

/// One run of ink a dash pattern leaves on a contour: the points it
/// rides, in device space, and the direction it set off in — a dot
/// (`on` of zero) has a single point and still needs a heading for a
/// square cap.
pub(crate) struct Run {
    points: Vec<(f64, f64)>,
    heading: (f64, f64),
}

/// Walks every contour of `flat` with a dash pattern, in device px:
/// `on` of ink, `off` of air, the walk starting `phase` into the
/// pattern (SVG's `stroke-dashoffset`). The pattern restarts on every
/// contour, as SVG says it does, and runs ON across the joints of one —
/// a dash that meets a corner bends around it instead of stopping
/// there, so the rhythm a reader sees is the length along the line.
pub(crate) fn dash_runs(flat: &Flattened, on: f64, off: f64, phase: f64) -> Vec<Run> {
    let (on, off) = (on.max(0.0), off.max(0.0));
    let period = on + off;
    let mut runs = Vec::new();
    // a NaN period answers no to both: no pattern, no runs
    if period <= 0.0 || !period.is_finite() {
        return runs;
    }
    for (points, sealed) in flat.contours() {
        let count = points.len();
        let segments = match (sealed, count) {
            (_, 0 | 1) => 0,
            (true, _) => count,
            (false, _) => count - 1,
        };
        let at = phase.rem_euclid(period);
        // the start of a period is ink, even when the ink has no length:
        // that is where a dot lands
        let (mut inked, mut left) =
            if at < on || at == 0.0 { (true, on - at) } else { (false, period - at) };
        let mut open: Option<Run> = None;
        let mut heading = (1.0, 0.0);
        let mut last = points[0];
        for i in 0..segments {
            let a = points[i];
            let b = points[(i + 1) % count];
            let length = (b.0 - a.0).hypot(b.1 - a.1);
            if length <= 0.0 {
                continue;
            }
            heading = ((b.0 - a.0) / length, (b.1 - a.1) / length);
            let point = |t: f64| (a.0 + heading.0 * t, a.1 + heading.1 * t);
            let mut travelled = 0.0;
            while travelled < length {
                if inked && open.is_none() {
                    open = Some(Run { points: vec![point(travelled)], heading });
                }
                if inked && left <= 0.0 {
                    // a dot lands where it falls and the air resumes
                    runs.extend(open.take());
                    inked = false;
                    left = off;
                    continue;
                }
                let step = (length - travelled).min(left);
                travelled += step;
                left -= step;
                if inked && let Some(run) = &mut open {
                    run.points.push(point(travelled));
                }
                if left <= 1e-9 {
                    if inked {
                        runs.extend(open.take());
                        (inked, left) = (false, off);
                    } else {
                        (inked, left) = (true, on);
                    }
                }
            }
            last = b;
        }
        // a dot the pattern puts on the contour's very end
        if inked && on <= 0.0 && open.is_none() && segments > 0 {
            open = Some(Run { points: vec![last], heading });
        }
        runs.extend(open.take());
    }
    runs
}

/// The runs as open contours — what the round pen of [`stroke_into`]
/// rides: each run wears round caps and a dot becomes a disc.
pub(crate) fn runs_as_contours(runs: &[Run]) -> Flattened {
    let mut out = Flattened { points: Vec::new(), contours: Vec::new() };
    for run in runs {
        out.contours.push((out.points.len(), false));
        out.points.extend_from_slice(&run.points);
    }
    out
}

/// The OUTLINE of every run under a flat pen `width` wide — the closed
/// polygons a fill rule turns into ink. A butt end stops square at the
/// run's own end; a square end goes on half a pen further, which is
/// SVG's `square` cap and the corner of a dotted CSS border. Joints
/// inside a run meet at their mitre, cut to a bevel past SVG's default
/// limit of four.
///
/// The fill is exact along x and sub-sampled along y, the same kernel
/// every filled glyph rides, so a dash's flat end lands on its true
/// fraction of a pixel — the round pen could only ever end in a disc.
pub(crate) fn runs_outline(runs: &[Run], width: f64, extend: bool) -> Flattened {
    const MITER_LIMIT: f64 = 4.0;
    let half = width / 2.0;
    let mut out = Flattened { points: Vec::new(), contours: Vec::new() };
    for run in runs {
        let mut points = run.points.clone();
        if points.len() == 1 {
            // a dot: nothing under a butt end, a square under a square one
            if !extend {
                continue;
            }
            let (x, y) = points[0];
            let (hx, hy) = run.heading;
            points = vec![(x - hx * half, y - hy * half), (x + hx * half, y + hy * half)];
        } else if extend {
            let first = heading_of(points[0], points[1]);
            let last = heading_of(points[points.len() - 2], points[points.len() - 1]);
            points[0] = (points[0].0 - first.0 * half, points[0].1 - first.1 * half);
            let end = points.len() - 1;
            points[end] = (points[end].0 + last.0 * half, points[end].1 + last.1 * half);
        }
        let count = points.len();
        // the left side forward, then the right side back
        let mut left = Vec::with_capacity(count + 2);
        let mut right = Vec::with_capacity(count + 2);
        for i in 0..count {
            let normal_of = |a: (f64, f64), b: (f64, f64)| {
                let (hx, hy) = heading_of(a, b);
                (-hy, hx)
            };
            let before = (i > 0).then(|| normal_of(points[i - 1], points[i]));
            let after = (i + 1 < count).then(|| normal_of(points[i], points[i + 1]));
            let (x, y) = points[i];
            match (before, after) {
                (Some(n), None) | (None, Some(n)) => {
                    left.push((x + n.0 * half, y + n.1 * half));
                    right.push((x - n.0 * half, y - n.1 * half));
                }
                (Some(n0), Some(n1)) => {
                    let (mx, my) = (n0.0 + n1.0, n0.1 + n1.1);
                    let length = mx.hypot(my);
                    let cos = if length > 1e-12 { (mx * n0.0 + my * n0.1) / length } else { 0.0 };
                    if cos * MITER_LIMIT < 1.0 {
                        // too sharp for a mitre: the bevel's two corners
                        for n in [n0, n1] {
                            left.push((x + n.0 * half, y + n.1 * half));
                            right.push((x - n.0 * half, y - n.1 * half));
                        }
                    } else {
                        let reach = half / cos;
                        let (ux, uy) = (mx / length, my / length);
                        left.push((x + ux * reach, y + uy * reach));
                        right.push((x - ux * reach, y - uy * reach));
                    }
                }
                (None, None) => {}
            }
        }
        out.contours.push((out.points.len(), true));
        out.points.extend(left);
        out.points.extend(right.into_iter().rev());
    }
    out
}

fn heading_of(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let length = (b.0 - a.0).hypot(b.1 - a.1);
    if length <= 0.0 { (1.0, 0.0) } else { ((b.0 - a.0) / length, (b.1 - a.1) / length) }
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

    const ONE: Placing = Placing { scale: 1.0, dx: 0.0, dy: 0.0, spin: Spin::NONE };

    fn polyline_distance(point: (f64, f64), points: &[(f64, f64)]) -> f64 {
        points
            .windows(2)
            .map(|pair| segment_distance(point, pair[0], pair[1]))
            .fold(f64::INFINITY, f64::min)
    }

    fn run_lengths(runs: &[Run]) -> Vec<f64> {
        runs.iter()
            .map(|run| {
                run.points.windows(2).map(|pair| (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1)).sum()
            })
            .collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn a_dash_walks_its_rhythm_along_the_line() {
        let flat = flatten(&[Verb::Move(0.0, 0.0), Verb::Line(20.0, 0.0)], ONE);
        let runs = dash_runs(&flat, 4.0, 2.0, 0.0);
        let starts: Vec<f64> = runs.iter().map(|run| run.points[0].0).collect();
        assert_eq!(starts, vec![0.0, 6.0, 12.0, 18.0], "a dash every six, from the start");
        let lengths = run_lengths(&runs);
        assert!(close(lengths[0], 4.0) && close(lengths[2], 4.0));
        assert!(close(lengths[3], 2.0), "the line ends inside the last dash: {lengths:?}");
    }

    #[test]
    fn the_phase_starts_the_walk_inside_the_pattern() {
        let flat = flatten(&[Verb::Move(0.0, 0.0), Verb::Line(20.0, 0.0)], ONE);
        // five into a pattern of six: one point of air, then a dash
        let runs = dash_runs(&flat, 4.0, 2.0, 5.0);
        assert!(close(runs[0].points[0].0, 1.0), "{:?}", runs[0].points);
        // a negative phase walks back into the pattern, as SVG says
        let back = dash_runs(&flat, 4.0, 2.0, -1.0);
        assert!(close(back[0].points[0].0, 1.0), "{:?}", back[0].points);
    }

    #[test]
    fn a_dash_bends_around_a_corner_instead_of_stopping() {
        let flat = flatten(
            &[Verb::Move(0.0, 0.0), Verb::Line(10.0, 0.0), Verb::Line(10.0, 10.0)],
            ONE,
        );
        let runs = dash_runs(&flat, 15.0, 5.0, 0.0);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].points, vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0)]);
        assert!(close(run_lengths(&runs)[0], 15.0));
    }

    #[test]
    fn a_dash_measures_a_curve_by_its_length() {
        // a quarter of a circle of radius ten: 5π long
        let k = 10.0 * 0.552_284_8;
        let flat = flatten(&[Verb::Move(10.0, 0.0), Verb::Cubic(10.0, k, k, 10.0, 0.0, 10.0)], ONE);
        let runs = dash_runs(&flat, 3.0, 1.0, 0.0);
        let inked: f64 = run_lengths(&runs).iter().sum();
        let whole = 5.0 * std::f64::consts::PI;
        // whole dashes of three every four, and a share of the last
        let expected = (whole / 4.0).floor() * 3.0 + (whole % 4.0).min(3.0);
        assert!((inked - expected).abs() < 0.05, "{inked} against {expected}");
    }

    #[test]
    fn a_dash_of_no_length_is_a_dot_on_every_beat() {
        let flat = flatten(&[Verb::Move(0.0, 0.0), Verb::Line(20.0, 0.0)], ONE);
        let runs = dash_runs(&flat, 0.0, 5.0, 0.0);
        let dots: Vec<(f64, f64)> = runs.iter().map(|run| run.points[0]).collect();
        assert!(runs.iter().all(|run| run.points.len() == 1), "every run is one point");
        assert_eq!(dots, vec![(0.0, 0.0), (5.0, 0.0), (10.0, 0.0), (15.0, 0.0), (20.0, 0.0)]);
    }

    #[test]
    fn the_pattern_restarts_on_every_contour() {
        let flat = flatten(
            &[
                Verb::Move(0.0, 0.0),
                Verb::Line(5.0, 0.0),
                Verb::Move(0.0, 4.0),
                Verb::Line(5.0, 4.0),
            ],
            ONE,
        );
        let runs = dash_runs(&flat, 3.0, 3.0, 0.0);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[1].points[0], (0.0, 4.0), "the second contour starts on ink");
    }

    #[test]
    fn a_butt_end_stops_square_and_a_square_end_goes_half_a_pen_on() {
        let flat = flatten(&[Verb::Move(2.0, 5.0), Verb::Line(6.0, 5.0)], ONE);
        let runs = dash_runs(&flat, 10.0, 0.0, 0.0);
        let butt = runs_outline(&runs, 2.0, false);
        let (points, sealed) = butt.contours().next().expect("one outline");
        assert!(sealed);
        let xs = points.iter().map(|p| p.0);
        let ys = points.iter().map(|p| p.1);
        assert_eq!((xs.clone().fold(f64::MAX, f64::min), xs.fold(f64::MIN, f64::max)), (2.0, 6.0));
        assert_eq!((ys.clone().fold(f64::MAX, f64::min), ys.fold(f64::MIN, f64::max)), (4.0, 6.0));
        let square = runs_outline(&runs, 2.0, true);
        let (points, _) = square.contours().next().expect("one outline");
        let xs = points.iter().map(|p| p.0);
        assert_eq!((xs.clone().fold(f64::MAX, f64::min), xs.fold(f64::MIN, f64::max)), (1.0, 7.0));
        // a dot has no length: nothing under a butt end, a square under a square one
        let dot = dash_runs(&flat, 0.0, 10.0, 0.0);
        assert_eq!(runs_outline(&dot, 2.0, false).contours().count(), 0);
        assert_eq!(runs_outline(&dot, 2.0, true).contours().count(), 1);
    }

    #[test]
    fn a_turn_moves_every_point_about_its_pivot() {
        let quarter = Placing {
            scale: 1.0,
            dx: 0.0,
            dy: 0.0,
            spin: Spin::about(std::f64::consts::FRAC_PI_2, 12.0, 12.0),
        };
        let flat = flatten(&[Verb::Move(12.0, 4.0), Verb::Line(20.0, 4.0)], quarter);
        let (points, _) = flat.contours().next().expect("one contour");
        // clockwise on the y-down screen: right of the pivot goes below it
        assert!((points[0].0 - 20.0).abs() < 1e-9 && (points[0].1 - 12.0).abs() < 1e-9, "{points:?}");
        assert!((points[1].0 - 20.0).abs() < 1e-9 && (points[1].1 - 20.0).abs() < 1e-9, "{points:?}");
        // no turn is exact, not merely close
        assert_eq!(Spin::NONE.apply(3.25, -7.5), (3.25, -7.5));
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

    #[test]
    fn a_subpath_drawn_back_to_its_own_start_survives_as_a_point() {
        // SVG's own rule, both halves: a lone `Move` is not stroked...
        let stray = flatten(&[Verb::Move(3.0, 3.0), Verb::Line(9.0, 3.0), Verb::Move(1.0, 1.0)], ONE);
        assert_eq!(stray.contours().count(), 1, "the trailing Move drew nothing");
        // ...and a `Move` with a line back to the same place IS
        let dab = flatten(&[Verb::Move(3.0, 3.0), Verb::Line(3.0, 3.0)], ONE);
        let all: Vec<_> = dab.contours().collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, &[(3.0, 3.0)], "one point, and the cap makes it a dot");
    }

    /// A quarter circle as the standard cubic (kappa control points),
    /// scaled up hard: every sampled true point stays within TOLERANCE
    /// of the polyline.
    #[test]
    fn a_cubic_stays_within_the_bound() {
        const KAPPA: f32 = 0.552_284_8;
        let radius = 12.0_f32;
        let path = [
            Verb::Move(radius, 0.0),
            Verb::Cubic(radius, radius * KAPPA, radius * KAPPA, radius, 0.0, radius),
        ];
        let placing = Placing { scale: 20.0, dx: 0.0, dy: 0.0, spin: Spin::NONE };
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
        let placing = Placing { scale: 8.0, dx: 3.0, dy: 5.0, spin: Spin::NONE };
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
        let placing = Placing { scale: 2.0, dx: 10.0, dy: 20.0, spin: Spin::NONE };
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
        const KAPPA: f32 = 0.552_284_8;
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
