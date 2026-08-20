//! CPU rasterizer — 100% std, the same one for all four targets.
//!
//! Paints a [`DisplayList`] into an RGBA [`Bitmap`]. It is the portable
//! capital of the first pixel: on the Mac the buffer becomes `CGImage`,
//! on the web `putImageData`, on Android `Bitmap` — the platform backend
//! only blits. GPU arrives when the benchmark says so; the interface
//! (the display list) does not change.
//!
//! Snapping happens HERE, once, when converting logical coordinates into
//! pixels: edges rounded in device space (neighbors that converge to the
//! same column close without a gap), a documented and localized decision
//! — never scattered through the layout.
//!
//! Text enters through the frame's [`TextEngine`]: the engine rasterizes
//! the line into an RGBA rectangle of straight alpha and the compositor
//! here blends it into the bitmap — a single compositing path for the
//! house pixel font and for the Mac's CoreText (and for the web's
//! canvas, one day).

use crate::image_engine::{ImageEngine, RawImages, raster_source};
use crate::glass::{Lens, Pyramid};
use crate::layout::{
    Color, Corners, DisplayList, DrawCommand, GlassPaint, GradientPaint, Point, Rect,
};
use crate::text_engine::{PixelFont, TextEngine, TextRaster};

/// An RGBA buffer (one `0xRRGGBBAA` `u32` per pixel, rows top to
/// bottom) — what the platform backend blits into the window.
pub struct Bitmap {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
    /// Clip stack in physical px (intersections already resolved) — `set`
    /// checks the top; fill, stroke and text respect it for free.
    clip: Vec<ClipEntry>,
    /// The stack top, MIRRORED flat: `set` runs per pixel and must not
    /// pay a `Vec` deref there. An empty stack mirrors as the open
    /// sentinel, so the hot path is four integer compares, always.
    top_cut: (i64, i64, i64, i64),
    top_round: Option<ClipRound>,
}

/// The cut of an empty clip stack — everything passes.
const OPEN_CUT: (i64, i64, i64, i64) = (i64::MIN, i64::MIN, i64::MAX, i64::MAX);

/// One open clip: the hard rectangular cut every clip has always been,
/// and the curve that softens it — kept in its OWN box, because an
/// outer rect can trim the cut without moving the corner it rounds.
#[derive(Clone, Copy)]
struct ClipEntry {
    cut: (i64, i64, i64, i64),
    round: Option<ClipRound>,
}

/// The curve of a rounded clip, in physical px. The radius is clamped
/// the way `fill_rect` clamps it — the background and the cut that
/// follows it must ramp the SAME arc, or a seam is born between a box
/// and its own corner.
#[derive(Clone, Copy)]
struct ClipRound {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
    radii: Corners,
}

impl ClipRound {
    /// `None` when all four are square — the same door `fill_rect`
    /// keeps, so a hair of a radius stays the straight clip byte for
    /// byte.
    fn new((x0, y0, x1, y1): (i64, i64, i64, i64), corner_radius: Corners) -> Option<ClipRound> {
        let radii = corner_radius.clamped((x1 - x0) as f64, (y1 - y0) as f64);
        (!radii.is_zero()).then_some(ClipRound {
            x0,
            y0,
            x1,
            y1,
            radii,
        })
    }

    /// The columns of this row the curve leaves whole — a span fills
    /// between them in one go and blends only the two ends.
    fn straight(&self, y: i64) -> (i64, i64) {
        let radii = self.radii;
        let left = corner_at(y, self.y0, self.y1, radii.top_left, radii.bottom_left);
        let right = corner_at(y, self.y0, self.y1, radii.top_right, radii.bottom_right);
        if left.is_none() && right.is_none() {
            (i64::MIN, i64::MAX)
        } else {
            (self.x0 + corner_reach(left), self.x1 - corner_reach(right))
        }
    }

    /// `fill_rect`'s corner kernel, word for word — now asking each
    /// side for the corner THIS row meets.
    #[inline]
    fn coverage(&self, x: i64, y: i64) -> f64 {
        let radii = self.radii;
        let left = corner_at(y, self.y0, self.y1, radii.top_left, radii.bottom_left);
        let right = corner_at(y, self.y0, self.y1, radii.top_right, radii.bottom_right);
        let (radius, center_x, center_y) = match (left, right) {
            (Some((radius, center_y)), _) if x < self.x0 + radius.ceil() as i64 => {
                (radius, self.x0 as f64 + radius, center_y)
            }
            (_, Some((radius, center_y))) if x >= self.x1 - radius.ceil() as i64 => {
                (radius, self.x1 as f64 - radius, center_y)
            }
            _ => return 1.0,
        };
        let distance = (x as f64 + 0.5 - center_x).hypot(y as f64 + 0.5 - center_y);
        (radius - distance + 0.5).clamp(0.0, 1.0)
    }
}

/// The corner a row meets on one side of the box: its radius and the
/// centre of its arc. `None` = the row runs straight to that edge,
/// which is what a square corner gives every row it owns.
///
/// It is the single place the four radii turn into the two the loops
/// below need — a row can meet a rounded corner on one side and a
/// square one on the other, and each side answers for itself.
#[inline]
fn corner_at(y: i64, y0: i64, y1: i64, top: f64, bottom: f64) -> Option<(f64, f64)> {
    if top > 0.0 && y < y0 + top.ceil() as i64 {
        Some((top, y0 as f64 + top))
    } else if bottom > 0.0 && y >= y1 - bottom.ceil() as i64 {
        Some((bottom, y1 as f64 - bottom))
    } else {
        None
    }
}

/// How far into the box a corner reaches, in whole pixels.
#[inline]
fn corner_reach(corner: Option<(f64, f64)>) -> i64 {
    corner.map_or(0, |(radius, _)| radius.ceil() as i64)
}

fn pack(color: Color) -> u32 {
    ((color.r as u32) << 24) | ((color.g as u32) << 16) | ((color.b as u32) << 8) | color.a as u32
}

/// Exact division by 255 with rounding — the integer-compositing
/// classic: `round(x / 255)` without float.
fn div255(x: u32) -> u32 {
    let x = x + 128;
    (x + (x >> 8)) >> 8
}

/// Source-over of `src` onto `dst` (both `0xRRGGBBAA`, straight alpha):
/// `out = src·sa + dst·(1−sa)` per channel. Fast paths: opaque
/// overwrites, invisible does not touch.
fn blend_px(src: u32, dst: u32) -> u32 {
    let sa = src & 0xFF;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let inv = 255 - sa;
    let channel = |shift: u32| {
        let s = (src >> shift) & 0xFF;
        let d = (dst >> shift) & 0xFF;
        div255(s * sa + d * inv)
    };
    let a = sa + div255((dst & 0xFF) * inv);
    (channel(24) << 24) | (channel(16) << 16) | (channel(8) << 8) | a
}

/// Straight-alpha RGBA from bytes that were blended onto a TRANSPARENT
/// ground. `blend_px` leaves `rgb = colour x coverage` there, which is
/// the premultiplied convention; `ImageData` and a canvas blit both
/// read straight. Wherever alpha is 255 this changes nothing, which is
/// why only islands ever need it.
pub fn unpremultiplied(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for pixel in out.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
    out
}

impl Bitmap {
    pub fn new(width: usize, height: usize, background: Color) -> Self {
        Bitmap {
            width,
            height,
            pixels: vec![pack(background); width * height],
            clip: Vec::new(),
            top_cut: OPEN_CUT,
            top_round: None,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// The raw bytes, for the backend's blit.
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    pub fn pixel(&self, x: usize, y: usize) -> Option<u32> {
        (x < self.width && y < self.height).then(|| self.pixels[y * self.width + x])
    }

    /// `R,G,B,A` bytes per pixel, row by row — the format platform blits
    /// expect with no endianness argument.
    pub fn to_rgba_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.pixels.len() * 4);
        for pixel in &self.pixels {
            bytes.push((pixel >> 24) as u8);
            bytes.push((pixel >> 16) as u8);
            bytes.push((pixel >> 8) as u8);
            bytes.push(*pixel as u8);
        }
        bytes
    }

    #[inline(always)]
    fn set(&mut self, x: i64, y: i64, color: u32) {
        if x < 0 || y < 0 || (x as usize) >= self.width || (y as usize) >= self.height {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.top_cut;
        if x < cx0 || y < cy0 || x >= cx1 || y >= cy1 {
            return;
        }
        let mut color = color;
        // the curve bites four small corner squares and nothing else —
        // everywhere else this is one predicted branch on a hot field
        if let Some(round) = &self.top_round {
            let coverage = round.coverage(x, y);
            if coverage < 1.0 {
                let alpha = ((color & 0xFF) as f64 * coverage).round() as u32;
                if alpha == 0 {
                    return;
                }
                color = (color & !0xFF) | alpha;
            }
        }
        let index = y as usize * self.width + x as usize;
        self.pixels[index] = blend_px(color, self.pixels[index]);
    }

    /// Refreshes the flat mirror after a push or a pop.
    fn sync_top(&mut self) {
        match self.clip.last() {
            Some(entry) => {
                self.top_cut = entry.cut;
                self.top_round = entry.round;
            }
            None => {
                self.top_cut = OPEN_CUT;
                self.top_round = None;
            }
        }
    }

    /// Pushes a clip: the RECT intersects the open cut (exact integer
    /// boxes, as always) and the CURVE is the innermost one declared —
    /// a clip with no radius of its own INHERITS the curve already
    /// cutting, so a scroll region inside a rounded island keeps the
    /// island's corners.
    fn push_clip(&mut self, rect: Rect, corner_radius: Corners) {
        let snapped = Self::snap(rect);
        let round = ClipRound::new(snapped, corner_radius);
        let (x0, y0, x1, y1) = snapped;
        let entry = match self.clip.last().copied() {
            Some(top) => {
                let (cx0, cy0, cx1, cy1) = top.cut;
                ClipEntry {
                    cut: (x0.max(cx0), y0.max(cy0), x1.min(cx1), y1.min(cy1)),
                    round: round.or(top.round),
                }
            }
            None => ClipEntry { cut: snapped, round },
        };
        self.clip.push(entry);
        self.sync_top();
    }

    fn pop_clip(&mut self) {
        self.clip.pop();
        self.sync_top();
    }

    /// Pushes an ALREADY-physical clip (the damage replay) — same stack,
    /// no snapping, intersected with the current top like any clip.
    fn push_clip_physical(&mut self, rect: DamageRect) {
        let entry = match self.clip.last().copied() {
            Some(top) => {
                let (cx0, cy0, cx1, cy1) = top.cut;
                ClipEntry {
                    cut: (rect.0.max(cx0), rect.1.max(cy0), rect.2.min(cx1), rect.3.min(cy1)),
                    // a damage rect never bends — it inherits the curve
                    round: top.round,
                }
            }
            None => ClipEntry { cut: rect, round: None },
        };
        self.clip.push(entry);
        self.sync_top();
    }

    /// Overwrites the rect with `color` — a CLEAR, not a wash: alpha is
    /// written as-is, no blending (clearing with a translucent color
    /// must not accumulate over frames).
    fn clear_rect(&mut self, rect: DamageRect, color: Color) {
        let packed = pack(color);
        let x0 = rect.0.clamp(0, self.width as i64) as usize;
        let x1 = rect.2.clamp(0, self.width as i64) as usize;
        let y0 = rect.1.clamp(0, self.height as i64) as usize;
        let y1 = rect.3.clamp(0, self.height as i64) as usize;
        for y in y0..y1 {
            self.pixels[y * self.width + x0..y * self.width + x1].fill(packed);
        }
    }

    /// A soft halo OUTSIDE the rounded rect: quadratic falloff over
    /// `radius` px, measured from the ROUNDED edge (distance to the
    /// rect's core shrunk by `corner_radius`, minus the corner radius —
    /// the classic signed distance). The notch BEHIND a rounded corner
    /// gets shadow too: it is outside the shape, so it belongs to the
    /// halo, not to the backdrop.
    fn shadow_rect(&mut self, rect: Rect, color: Color, radius: f64, corner_radius: Corners) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        let reach = radius.max(1.0);
        let reach_px = reach.round() as i64;
        let radii = corner_radius.clamped((x1 - x0) as f64, (y1 - y0) as f64);
        // which corner a pixel belongs to — the box's own midpoint
        // splits it in four, so the halo bends by the SAME arc the
        // shape it hides behind was filled with
        let (mid_x, mid_y) = ((x0 + x1) as f64 / 2.0, (y0 + y1) as f64 / 2.0);
        let corner_of = |px: f64, py: f64| match (px < mid_x, py < mid_y) {
            (true, true) => radii.top_left,
            (false, true) => radii.top_right,
            (false, false) => radii.bottom_right,
            (true, false) => radii.bottom_left,
        };
        let corner_px = radii.max().ceil() as i64;
        let (cx0, cy0, cx1, cy1) = self.clip_box();
        let from_y = (y0 - reach_px).max(cy0);
        let to_y = (y1 + reach_px).min(cy1);
        for y in from_y..to_y {
            // rows across the straight middle skip the interior: only
            // the bands outside the rect can hold shadow there
            let straight_row = y >= y0 + corner_px && y < y1 - corner_px;
            let ranges: [(i64, i64); 2] = if straight_row {
                [((x0 - reach_px).max(cx0), x0.min(cx1)), (x1.max(cx0), (x1 + reach_px).min(cx1))]
            } else {
                [((x0 - reach_px).max(cx0), (x1 + reach_px).min(cx1)), (0, 0)]
            };
            let py = y as f64 + 0.5;
            for (from, to) in ranges {
                for x in from..to {
                    let px = x as f64 + 0.5;
                    let corner = corner_of(px, py);
                    let dx = px - px.clamp(x0 as f64 + corner, x1 as f64 - corner);
                    let dy = py - py.clamp(y0 as f64 + corner, y1 as f64 - corner);
                    let distance = if dx == 0.0 || dy == 0.0 {
                        dx.abs() + dy.abs() - corner
                    } else {
                        dx.hypot(dy) - corner
                    };
                    if distance <= 0.0 || distance >= reach {
                        continue;
                    }
                    let strength = 1.0 - distance / reach;
                    self.set_covered(x, y, color, strength * strength);
                }
            }
        }
    }

    /// The liquid-glass pane: it READS the pixels already in this
    /// bitmap, blurs them through the pyramid, bends them at the rim
    /// and writes the result back over the same box.
    ///
    /// The only paint that samples its own target, which is why the
    /// pyramid is built from a copy of the neighbourhood: the write of
    /// one pixel must never move the blur of the next.
    fn glass_pane(&mut self, rect: Rect, glass: GlassPaint, corner_radius: Corners) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.clip_box();
        if x0 >= cx1 || x1 <= cx0 || y0 >= cy1 || y1 <= cy0 {
            return;
        }
        let radii = corner_radius.clamped((x1 - x0) as f64, (y1 - y0) as f64);
        let lens = Lens {
            rect: (x0 as f64, y0 as f64, x1 as f64, y1 as f64),
            radii,
            glass,
            viewport: (self.width as f64, self.height as f64),
        };
        let pyramid =
            Pyramid::build(&self.pixels, self.width, self.height, lens.area(), lens.levels());
        for y in y0.max(cy0)..y1.min(cy1) {
            for x in x0.max(cx0)..x1.min(cx1) {
                if let Some((color, coverage)) = lens.shade(&pyramid, x, y) {
                    self.set_covered(x, y, color, coverage);
                }
            }
        }
    }

    /// A two-stop ramp inside the shape [`Bitmap::fill_rect`] covers:
    /// the same coverage — straight spans full, corners on the circle
    /// ramp — with the color resolved per pixel. A ramp whose two
    /// colors are equal paints exactly what the flat fill paints.
    fn fill_gradient(&mut self, rect: Rect, paint: GradientPaint, corner_radius: Corners) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.clip_box();
        if x0 >= cx1 || x1 <= cx0 || y0 >= cy1 || y1 <= cy0 {
            return;
        }
        // the ellipse ignores the box's corner — the GPU's wire traded
        // that slot for the aspect, and the two must drop it TOGETHER
        // (a rounded wash clips through `.clipped()`)
        let corner_radius = match paint {
            GradientPaint::Radial { aspect, .. } if aspect != 1.0 => Corners::ZERO,
            _ => corner_radius,
        };
        let radii = corner_radius.clamped((x1 - x0) as f64, (y1 - y0) as f64);
        for y in y0.max(cy0)..y1.min(cy1) {
            let py = y as f64 + 0.5;
            let left = corner_at(y, y0, y1, radii.top_left, radii.bottom_left);
            let right = corner_at(y, y0, y1, radii.top_right, radii.bottom_right);
            let (left_reach, right_reach) = (corner_reach(left), corner_reach(right));
            for x in x0.max(cx0)..x1.min(cx1) {
                let px = x as f64 + 0.5;
                let coverage = match (left, right) {
                    (Some((radius, center_y)), _) if x < x0 + left_reach => {
                        let distance = (px - (x0 as f64 + radius)).hypot(py - center_y);
                        (radius - distance + 0.5).clamp(0.0, 1.0)
                    }
                    (_, Some((radius, center_y))) if x >= x1 - right_reach => {
                        let distance = (px - (x1 as f64 - radius)).hypot(py - center_y);
                        (radius - distance + 0.5).clamp(0.0, 1.0)
                    }
                    _ => 1.0,
                };
                if coverage <= 0.0 {
                    continue;
                }
                self.set_covered(x, y, paint.at(Point { x: px, y: py }), coverage);
            }
        }
    }

    /// Edges rounded in device px — the single point of snapping.
    fn snap(rect: Rect) -> (i64, i64, i64, i64) {
        let x0 = rect.origin.x.round() as i64;
        let y0 = rect.origin.y.round() as i64;
        let x1 = (rect.origin.x + rect.size.width).round() as i64;
        let y1 = (rect.origin.y + rect.size.height).round() as i64;
        (x0, y0, x1, y1)
    }

    /// The clip top (or the surface box) — primitives CLAMP their loops
    /// to it before iterating: a big rect under a small clip must cost
    /// the clip, not the rect. Always intersected with the surface, so a
    /// span clamped to it can write the row slice directly.
    fn clip_box(&self) -> (i64, i64, i64, i64) {
        let surface = (0, 0, self.width as i64, self.height as i64);
        match self.clip.last().copied() {
            Some(entry) => {
                let (x0, y0, x1, y1) = entry.cut;
                (x0.max(surface.0), y0.max(surface.1), x1.min(surface.2), y1.min(surface.3))
            }
            None => surface,
        }
    }

    /// Paints one horizontal span with the clip applied — opaque colors
    /// write the row slice in one go, translucent ones blend per pixel.
    fn span(&mut self, y: i64, from: i64, to: i64, color: Color, packed: u32) {
        let (cx0, cy0, cx1, cy1) = self.clip_box();
        if y < cy0 || y >= cy1 {
            return;
        }
        let from = from.max(cx0);
        let to = to.min(cx1);
        if from >= to {
            return;
        }
        if color.a == 255 {
            // a row crossing no corner keeps the one-shot fill; a row
            // that does keeps it for its MIDDLE and blends the two ends
            let (straight_from, straight_to) = match &self.top_round {
                Some(round) => round.straight(y),
                None => (i64::MIN, i64::MAX),
            };
            let middle_from = from.max(straight_from).min(to);
            let middle_to = to.min(straight_to).max(middle_from);
            for x in from..middle_from {
                self.set(x, y, packed);
            }
            let row = y as usize * self.width;
            self.pixels[row + middle_from as usize..row + middle_to as usize].fill(packed);
            for x in middle_to..to {
                self.set(x, y, packed);
            }
        } else {
            for x in from..to {
                self.set(x, y, packed);
            }
        }
    }

    /// One anti-aliased pixel: `coverage` scales the color's alpha.
    fn set_covered(&mut self, x: i64, y: i64, color: Color, coverage: f64) {
        let alpha = (color.a as f64 * coverage).round() as u32;
        if alpha == 0 {
            return;
        }
        let packed = ((color.r as u32) << 24)
            | ((color.g as u32) << 16)
            | ((color.b as u32) << 8)
            | alpha;
        self.set(x, y, packed);
    }

    /// Fill with optional corners: straight rows paint as wide spans;
    /// corner pixels get circle-coverage anti-aliasing (one hypot per
    /// corner pixel — the corner square is tiny, the curve comes out
    /// smooth). `corner_radius: Corners::ZERO` reproduces the straight rectangle
    /// byte for byte.
    fn fill_rect(&mut self, rect: Rect, color: Color, corner_radius: Corners) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.clip_box();
        if x0 >= cx1 || x1 <= cx0 || y0 >= cy1 || y1 <= cy0 {
            return;
        }
        let packed = pack(color);
        let radii = corner_radius.clamped((x1 - x0) as f64, (y1 - y0) as f64);
        if radii.is_zero() {
            for y in y0.max(cy0)..y1.min(cy1) {
                self.span(y, x0, x1, color, packed);
            }
            return;
        }
        for y in y0.max(cy0)..y1.min(cy1) {
            let left = corner_at(y, y0, y1, radii.top_left, radii.bottom_left);
            let right = corner_at(y, y0, y1, radii.top_right, radii.bottom_right);
            let (left_reach, right_reach) = (corner_reach(left), corner_reach(right));
            // the straight middle: the whole row when both corners of
            // this row are square, which is the plain rectangle again
            self.span(y, x0 + left_reach, x1 - right_reach, color, packed);
            let py = y as f64 + 0.5;
            if let Some((radius, center_y)) = left {
                let center_x = x0 as f64 + radius;
                for x in x0.max(cx0)..(x0 + left_reach).min(cx1) {
                    let distance = (x as f64 + 0.5 - center_x).hypot(py - center_y);
                    self.set_covered(x, y, color, (radius - distance + 0.5).clamp(0.0, 1.0));
                }
            }
            if let Some((radius, center_y)) = right {
                let center_x = x1 as f64 - radius;
                for x in (x1 - right_reach).max(cx0)..x1.min(cx1) {
                    let distance = (x as f64 + 0.5 - center_x).hypot(py - center_y);
                    self.set_covered(x, y, color, (radius - distance + 0.5).clamp(0.0, 1.0));
                }
            }
        }
    }

    /// A frame inward from the edge. With `corner_radius` the border
    /// FOLLOWS the curve — an anti-aliased ring between the outer circle
    /// and the inner one (outer minus `width`); a straight bar cutting
    /// across the arc is exactly the bug this kills. `0.0` keeps the
    /// four straight non-overlapping bars byte for byte (a translucent
    /// border cannot blend a corner twice).
    fn stroke_rect(&mut self, rect: Rect, color: Color, width: f64, corner_radius: Corners) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.clip_box();
        if x0 >= cx1 || x1 <= cx0 || y0 >= cy1 || y1 <= cy0 {
            return;
        }
        let packed = pack(color);
        let thickness = width.max(1.0).round() as i64;
        let radii = corner_radius.clamped((x1 - x0) as f64, (y1 - y0) as f64);
        if radii.is_zero() {
            let top_end = (y0 + thickness).min(y1);
            let bottom_start = (y1 - thickness).max(top_end);
            let left_end = (x0 + thickness).min(x1);
            let right_start = (x1 - thickness).max(left_end);
            for y in y0.max(cy0)..top_end.min(cy1) {
                self.span(y, x0, x1, color, packed);
            }
            for y in bottom_start.max(cy0)..y1.min(cy1) {
                self.span(y, x0, x1, color, packed);
            }
            for y in top_end.max(cy0)..bottom_start.min(cy1) {
                self.span(y, x0, left_end, color, packed);
                self.span(y, right_start, x1, color, packed);
            }
            return;
        }
        for y in y0.max(cy0)..y1.min(cy1) {
            let left = corner_at(y, y0, y1, radii.top_left, radii.bottom_left);
            let right = corner_at(y, y0, y1, radii.top_right, radii.bottom_right);
            if left.is_none() && right.is_none() {
                // straight sides between the corners
                self.span(y, x0, x0 + thickness, color, packed);
                self.span(y, x1 - thickness, x1, color, packed);
                continue;
            }
            let (left_reach, right_reach) = (corner_reach(left), corner_reach(right));
            // the straight middle of the horizontal bars — a square
            // corner lets it run all the way to that edge
            let bar_row = y < y0 + thickness || y >= y1 - thickness;
            if bar_row {
                self.span(y, x0 + left_reach, x1 - right_reach, color, packed);
            }
            // a SQUARE corner beside a rounded one keeps its straight
            // bar for the rows the arc spends on the other side
            if !bar_row {
                if left.is_none() {
                    self.span(y, x0, x0 + thickness, color, packed);
                }
                if right.is_none() {
                    self.span(y, x1 - thickness, x1, color, packed);
                }
            }
            let py = y as f64 + 0.5;
            let ring = |radius: f64, distance: f64| {
                let inner = (radius - thickness as f64).max(0.0);
                ((radius - distance + 0.5).clamp(0.0, 1.0)
                    - (inner - distance + 0.5).clamp(0.0, 1.0))
                    .clamp(0.0, 1.0)
            };
            if let Some((radius, center_y)) = left {
                let center_x = x0 as f64 + radius;
                for x in x0.max(cx0)..(x0 + left_reach).min(cx1) {
                    let distance = (x as f64 + 0.5 - center_x).hypot(py - center_y);
                    self.set_covered(x, y, color, ring(radius, distance));
                }
            }
            if let Some((radius, center_y)) = right {
                let center_x = x1 as f64 - radius;
                for x in (x1 - right_reach).max(cx0)..x1.min(cx1) {
                    let distance = (x as f64 + 0.5 - center_x).hypot(py - center_y);
                    self.set_covered(x, y, color, ring(radius, distance));
                }
            }
        }
    }

    /// Composites a line rasterized by the engine at the logical origin,
    /// snapped ONCE (the raster already comes in physical pixels).
    fn composite_text(&mut self, origin_x: f64, origin_y: f64, scale: usize, raster: &TextRaster) {
        self.composite_rgba(origin_x, origin_y, scale, raster.width, raster.height, &raster.rgba);
    }

    /// Source-over blit of ANY straight-alpha RGBA rectangle (text
    /// rasters, images) at the logical origin, snapped ONCE — the
    /// pixels already come in physical size.
    fn composite_rgba(
        &mut self,
        origin_x: f64,
        origin_y: f64,
        scale: usize,
        width: usize,
        height: usize,
        rgba: &[u8],
    ) {
        let base_x = (origin_x * scale as f64).round() as i64;
        let base_y = (origin_y * scale as f64).round() as i64;
        // clamp the loop to the clip: a long line under a small damage
        // rect must cost the visible slice, not the line
        let (cx0, cy0, cx1, cy1) = self.clip_box();
        let row_first = (cy0 - base_y).max(0) as usize;
        let row_last = ((cy1 - base_y).max(0) as usize).min(height);
        let col_first = (cx0 - base_x).max(0) as usize;
        let col_last = ((cx1 - base_x).max(0) as usize).min(width);
        for row in row_first..row_last {
            for col in col_first..col_last {
                let index = (row * width + col) * 4;
                let alpha = rgba[index + 3];
                if alpha == 0 {
                    continue;
                }
                let packed = ((rgba[index] as u32) << 24)
                    | ((rgba[index + 1] as u32) << 16)
                    | ((rgba[index + 2] as u32) << 8)
                    | alpha as u32;
                self.set(base_x + col as i64, base_y + row as i64, packed);
            }
        }
    }
}

/// The physical pixel count of one logical rect edge — the SAME
/// rounding on every pipeline (CPU compositor, GPU atlas, canvas), so
/// the engine rasters once and the bytes agree everywhere.
pub fn physical_extent(length: f64, scale: usize) -> usize {
    (length * scale as f64).round().max(0.0) as usize
}

fn scale_rect(rect: Rect, scale: f64) -> Rect {
    Rect {
        origin: crate::layout::Point { x: rect.origin.x * scale, y: rect.origin.y * scale },
        size: crate::layout::Size {
            width: rect.size.width * scale,
            height: rect.size.height * scale,
        },
    }
}

/// Paints the list in order — whoever comes later paints on top. Text
/// comes from the house pixel font (the default engine).
pub fn rasterize(display: &DisplayList, width: usize, height: usize, background: Color) -> Bitmap {
    rasterize_scaled(display, width, height, 1, background)
}

/// Like [`rasterize`], but with `width`/`height` in PHYSICAL pixels and
/// the display list's logical coordinates multiplied by `scale` — the
/// retina path (the backend asks the window for its scale factor).
pub fn rasterize_scaled(
    display: &DisplayList,
    width: usize,
    height: usize,
    scale: usize,
    background: Color,
) -> Bitmap {
    rasterize_with(display, width, height, scale, background, &PixelFont, &RawImages::default())
}

/// The full path: paints the list with the frame's [`TextEngine`] and
/// [`ImageEngine`] — it is what the `Runtime` calls (the house engines
/// in headless, the platform's on a shell).
pub fn rasterize_with(
    display: &DisplayList,
    width: usize,
    height: usize,
    scale: usize,
    background: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
) -> Bitmap {
    let mut bitmap = Bitmap::new(width, height, background);
    let factor = scale as f64;
    for command in display.iter() {
        match command {
            DrawCommand::FillRect { rect, color, corner_radius } => {
                bitmap.fill_rect(scale_rect(*rect, factor), *color, corner_radius * factor)
            }
            DrawCommand::Gradient { rect, paint, corner_radius } => bitmap.fill_gradient(
                scale_rect(*rect, factor),
                paint.scaled(factor),
                corner_radius * factor,
            ),
            DrawCommand::Backdrop { rect, glass, corner_radius } => bitmap.glass_pane(
                scale_rect(*rect, factor),
                glass.scaled(factor),
                corner_radius * factor,
            ),
            DrawCommand::StrokeRect { rect, color, width, corner_radius } => bitmap
                .stroke_rect(
                    scale_rect(*rect, factor),
                    *color,
                    width * factor,
                    corner_radius * factor,
                ),
            DrawCommand::Shadow { rect, radius, color, corner_radius } => bitmap.shadow_rect(
                scale_rect(*rect, factor),
                *color,
                radius * factor,
                corner_radius * factor,
            ),
            DrawCommand::TextLine { origin, content, range, color, font } => {
                let slice = &content[range.0..range.1];
                if let Some(raster) = text.raster_line(slice, font, *color, scale) {
                    bitmap.composite_text(origin.x, origin.y, scale, &raster);
                }
            }
            DrawCommand::Image { rect, source } => {
                let width = physical_extent(rect.size.width, scale);
                let height = physical_extent(rect.size.height, scale);
                if let Some(raster) = raster_source(images, source, width, height) {
                    bitmap.composite_rgba(
                        rect.origin.x,
                        rect.origin.y,
                        scale,
                        raster.width,
                        raster.height,
                        &raster.rgba,
                    );
                }
            }
            DrawCommand::PushClip { rect, corner_radius } => {
                bitmap.push_clip(scale_rect(*rect, factor), corner_radius * factor)
            }
            DrawCommand::PopClip => bitmap.pop_clip(),
        }
    }
    bitmap
}

// MARK: - Surface (incremental repaint by damage regions)

/// A physical damage rect: `[x0, y0, x1, y1)` in device pixels, already
/// clamped to the surface.
pub type DamageRect = (i64, i64, i64, i64);

fn intersect(a: DamageRect, b: DamageRect) -> Option<DamageRect> {
    let rect = (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3));
    (rect.0 < rect.2 && rect.1 < rect.3).then_some(rect)
}

fn union(a: DamageRect, b: DamageRect) -> DamageRect {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

fn touches(a: DamageRect, b: DamageRect) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

/// A retained paint target: the previous frame's bitmap, its display
/// list, and the physical bounds of every command. A new frame DIFFS
/// against the retained list (common prefix + common suffix — a hover
/// changes one or two commands in the middle), clears only the damaged
/// rects and replays the commands that intersect them, clipped. Pixels
/// come out byte-identical to a full repaint; the cost follows the
/// CHANGE, not the window.
pub struct Surface {
    bitmap: Bitmap,
    scale: usize,
    background: Color,
    /// The list painted on the bitmap (the previous frame). Empty at
    /// birth — the first frame damages the whole surface.
    display: DisplayList,
    /// Physical bounds per command, aligned with `display`. `None` for
    /// clip commands (they paint nothing). Text keeps a SUPERSET box
    /// (measure + slack) — a superset is always safe: clear covers it
    /// and replay repaints it.
    bounds: Vec<Option<DamageRect>>,
    /// A virgin surface damages EVERYTHING on its first frame — the
    /// window has no pixels yet, background included.
    primed: bool,
    /// The surface's own measure cache: text bounds in the diff hit warm
    /// entries instead of re-shaping (typing alternates content — the
    /// same aging rule as the layout cache).
    cache: crate::text_engine::MeasureCache,
    /// The PERSISTENT RGBA mirror of the bitmap — the bytes the backend
    /// blits (`R,G,B,A` per pixel). Synced lazily, damage-only: a hover
    /// converts one row, never the window.
    rgba: Vec<u8>,
    /// Damage waiting to be synced into the mirror — drained by
    /// [`Surface::rgba`].
    rgba_pending: Vec<DamageRect>,
}

impl Surface {
    /// `width`/`height` in PHYSICAL pixels. `background` is the clear
    /// color — damaged rects are overwritten with it before replay
    /// (alpha is written as-is, not blended: it is a clear, not a wash).
    pub fn new(width: usize, height: usize, scale: usize, background: Color) -> Self {
        Surface {
            bitmap: Bitmap::new(width, height, background),
            scale,
            background,
            display: DisplayList::default(),
            bounds: Vec::new(),
            primed: false,
            cache: crate::text_engine::MeasureCache::default(),
            rgba: vec![0; width * height * 4],
            rgba_pending: vec![(0, 0, width as i64, height as i64)],
        }
    }

    pub fn bitmap(&self) -> &Bitmap {
        &self.bitmap
    }

    fn whole(&self) -> DamageRect {
        (0, 0, self.bitmap.width as i64, self.bitmap.height as i64)
    }

    /// The superset bounds of one command under `clip` (physical), or
    /// `None` when it paints nothing / lands outside the clip.
    fn command_bounds(
        &self,
        command: &DrawCommand,
        clip: Option<DamageRect>,
        text: &dyn TextEngine,
    ) -> Option<DamageRect> {
        let factor = self.scale as f64;
        let raw = match command {
            DrawCommand::FillRect { rect, .. }
            | DrawCommand::StrokeRect { rect, .. }
            | DrawCommand::Backdrop { rect, .. }
            | DrawCommand::Gradient { rect, .. } => Bitmap::snap(scale_rect(*rect, factor)),
            DrawCommand::Shadow { rect, radius, .. } => {
                let (x0, y0, x1, y1) = Bitmap::snap(scale_rect(*rect, factor));
                let reach = (radius * factor).max(1.0).round() as i64;
                (x0 - reach, y0 - reach, x1 + reach, y1 + reach)
            }
            DrawCommand::TextLine { origin, content, range, font, .. } => {
                let metrics = self.cache.get_or_measure(&content[range.0..range.1], font, text);
                let x = (origin.x * factor).round() as i64;
                let y = (origin.y * factor).round() as i64;
                // +2px of slack per edge: measure vs raster rounding
                // never crosses it, and a superset is always safe
                (
                    x - 2,
                    y - 2,
                    x + (metrics.width * factor).ceil() as i64 + 2,
                    y + (metrics.height() * factor).ceil() as i64 + 2,
                )
            }
            // the destination rect is the whole truth — no slack needed
            DrawCommand::Image { rect, .. } => Bitmap::snap(scale_rect(*rect, factor)),
            // a clip that CHANGES must damage everything it governs —
            // its own box is the safe superset (the curve and the
            // nested cuts only ever REMOVE coverage). Identical clips
            // in the prefix and suffix contribute nothing, as ever.
            DrawCommand::PushClip { rect, .. } => Bitmap::snap(scale_rect(*rect, factor)),
            DrawCommand::PopClip => return None,
        };
        let clipped = match clip {
            Some(clip) => intersect(raw, clip)?,
            None => raw,
        };
        intersect(clipped, self.whole())
    }

    /// Walks `commands[..upto]` keeping the physical clip stack; calls
    /// `visit(index, clip_at_index)` for every command in `range`.
    fn walk_clips(
        &self,
        commands: &[DrawCommand],
        upto: usize,
        mut visit: impl FnMut(usize, Option<DamageRect>),
    ) {
        let factor = self.scale as f64;
        let mut stack: Vec<DamageRect> = Vec::new();
        for (index, command) in commands[..upto].iter().enumerate() {
            visit(index, stack.last().copied());
            match command {
                DrawCommand::PushClip { rect, .. } => {
                    let snapped = Bitmap::snap(scale_rect(*rect, factor));
                    let top = match stack.last().copied() {
                        Some(top) => {
                            intersect(snapped, top).unwrap_or((snapped.0, snapped.1, snapped.0, snapped.1))
                        }
                        None => snapped,
                    };
                    stack.push(top);
                }
                DrawCommand::PopClip => {
                    stack.pop();
                }
                _ => {}
            }
        }
    }

    /// Paints the new frame incrementally and returns the damaged rects
    /// (physical) — what the backend needs to blit. An identical list
    /// returns no damage and touches no pixel.
    pub fn frame(
        &mut self,
        display: DisplayList,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) -> Vec<DamageRect> {
        self.cache.begin_frame();
        let old = self.display.as_slice();
        let new = display.as_slice();

        // the diff: common prefix + common suffix; the middle is the change
        let mut prefix = 0;
        while prefix < old.len() && prefix < new.len() && old[prefix] == new[prefix] {
            prefix += 1;
        }
        if prefix == old.len() && prefix == new.len() {
            self.display = display;
            return Vec::new();
        }
        let mut suffix = 0;
        while suffix < old.len() - prefix
            && suffix < new.len() - prefix
            && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix]
        {
            suffix += 1;
        }

        // bounds of the NEW list: prefix and suffix reuse the retained
        // boxes (identical command ⇒ identical pixels ⇒ identical box);
        // only the middle is computed (and only it can pay a measure)
        let mut new_bounds: Vec<Option<DamageRect>> = Vec::with_capacity(new.len());
        new_bounds.extend_from_slice(&self.bounds[..prefix.min(self.bounds.len())]);
        self.walk_clips(new, new.len() - suffix, |index, clip| {
            if index >= prefix {
                new_bounds.push(self.command_bounds(&new[index], clip, text));
            }
        });
        new_bounds.extend_from_slice(&self.bounds[self.bounds.len() - suffix..]);
        debug_assert_eq!(new_bounds.len(), new.len(), "one box per command");

        // damage = old middle boxes ∪ new middle boxes
        let mut candidates: Vec<DamageRect> = Vec::new();
        candidates.extend(self.bounds[prefix..self.bounds.len() - suffix].iter().flatten());
        candidates.extend(new_bounds[prefix..new.len() - suffix].iter().flatten());

        // a pane of glass READS what is under it: a change anywhere in
        // its reach changes the pane, even when the pane's own command
        // did not move. Panes that answer for each other settle by
        // repeating the sweep — stacked glass is one pane over another
        let mut panes: Vec<usize> = Vec::new();
        loop {
            let mut grew = false;
            for (index, command) in new.iter().enumerate() {
                let DrawCommand::Backdrop { glass, .. } = command else { continue };
                let (Some(bounds), false) = (new_bounds[index], panes.contains(&index)) else {
                    continue;
                };
                let reach = (glass.reach() * self.scale as f64).ceil() as i64;
                let watch =
                    (bounds.0 - reach, bounds.1 - reach, bounds.2 + reach, bounds.3 + reach);
                if candidates.iter().any(|rect| touches(watch, *rect)) {
                    candidates.push(bounds);
                    panes.push(index);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }

        // greedy merge of touching rects; too many leftovers collapse
        // into one box (clears beat bookkeeping at that point)
        let mut damage: Vec<DamageRect> = Vec::new();
        'outer: for candidate in candidates {
            for slot in &mut damage {
                if touches(*slot, candidate) {
                    *slot = union(*slot, candidate);
                    continue 'outer;
                }
            }
            damage.push(candidate);
        }
        loop {
            let before = damage.len();
            let mut merged: Vec<DamageRect> = Vec::new();
            'fold: for rect in damage {
                for slot in &mut merged {
                    if touches(*slot, rect) {
                        *slot = union(*slot, rect);
                        continue 'fold;
                    }
                }
                merged.push(rect);
            }
            damage = merged;
            if damage.len() == before {
                break;
            }
        }
        if damage.len() > 8 {
            let all = damage.iter().copied().reduce(union).expect("non-empty damage");
            damage = vec![all];
        }
        if !self.primed {
            // first frame: the window has no pixels at all — blit it whole
            self.primed = true;
            damage = vec![self.whole()];
        }

        // clear + clipped replay: paint commands skip when their box
        // misses the rect; clip commands ALWAYS run (the stack must
        // stay balanced no matter what is skipped)
        let factor = self.scale as f64;
        for &rect in &damage {
            self.bitmap.clear_rect(rect, self.background);
            self.bitmap.push_clip_physical(rect);
            for (index, command) in new.iter().enumerate() {
                match command {
                    DrawCommand::PushClip { rect, corner_radius } => {
                        self.bitmap.push_clip(scale_rect(*rect, factor), corner_radius * factor);
                        continue;
                    }
                    DrawCommand::PopClip => {
                        self.bitmap.pop_clip();
                        continue;
                    }
                    _ => {}
                }
                let hits = new_bounds[index].is_some_and(|bounds| touches(bounds, rect));
                if !hits {
                    continue;
                }
                match command {
                    DrawCommand::FillRect { rect, color, corner_radius } => self
                        .bitmap
                        .fill_rect(scale_rect(*rect, factor), *color, corner_radius * factor),
                    DrawCommand::Gradient { rect, paint, corner_radius } => {
                        self.bitmap.fill_gradient(
                            scale_rect(*rect, factor),
                            paint.scaled(factor),
                            corner_radius * factor,
                        )
                    }
                    DrawCommand::Backdrop { rect, glass, corner_radius } => {
                        self.bitmap.glass_pane(
                            scale_rect(*rect, factor),
                            glass.scaled(factor),
                            corner_radius * factor,
                        )
                    }
                    DrawCommand::StrokeRect { rect, color, width, corner_radius } => {
                        self.bitmap.stroke_rect(
                            scale_rect(*rect, factor),
                            *color,
                            width * factor,
                            corner_radius * factor,
                        )
                    }
                    DrawCommand::Shadow { rect, radius, color, corner_radius } => {
                        self.bitmap.shadow_rect(
                            scale_rect(*rect, factor),
                            *color,
                            radius * factor,
                            corner_radius * factor,
                        )
                    }
                    DrawCommand::TextLine { origin, content, range, color, font } => {
                        let slice = &content[range.0..range.1];
                        if let Some(raster) = text.raster_line(slice, font, *color, self.scale) {
                            self.bitmap.composite_text(origin.x, origin.y, self.scale, &raster);
                        }
                    }
                    DrawCommand::Image { rect: image_rect, source } => {
                        let width = physical_extent(image_rect.size.width, self.scale);
                        let height = physical_extent(image_rect.size.height, self.scale);
                        if let Some(raster) = raster_source(images, source, width, height) {
                            self.bitmap.composite_rgba(
                                image_rect.origin.x,
                                image_rect.origin.y,
                                self.scale,
                                raster.width,
                                raster.height,
                                &raster.rgba,
                            );
                        }
                    }
                    DrawCommand::PushClip { .. } | DrawCommand::PopClip => unreachable!(),
                }
            }
            self.bitmap.pop_clip();
        }

        self.display = display;
        self.bounds = new_bounds;
        self.rgba_pending.extend(damage.iter().copied());
        damage
    }

    /// The RGBA mirror, synced: pending damage converts in place (a
    /// hover converts one row of bytes, never the window) and the whole
    /// buffer is returned for the backend to blit from — persistent, so
    /// the backend can also present PARTIALLY from the same pointer.
    pub fn rgba(&mut self) -> &[u8] {
        let width = self.bitmap.width;
        for &(x0, y0, x1, y1) in &self.rgba_pending {
            let x0 = x0.clamp(0, width as i64) as usize;
            let x1 = x1.clamp(0, width as i64) as usize;
            let y0 = y0.clamp(0, self.bitmap.height as i64) as usize;
            let y1 = y1.clamp(0, self.bitmap.height as i64) as usize;
            for y in y0..y1 {
                let row = y * width;
                for x in x0..x1 {
                    let pixel = self.bitmap.pixels[row + x];
                    let out = (row + x) * 4;
                    self.rgba[out] = (pixel >> 24) as u8;
                    self.rgba[out + 1] = (pixel >> 16) as u8;
                    self.rgba[out + 2] = (pixel >> 8) as u8;
                    self.rgba[out + 3] = pixel as u8;
                }
            }
        }
        self.rgba_pending.clear();
        &self.rgba
    }
}

#[cfg(test)]
mod tests {

    /// An island clears to NOTHING, and `blend_px` over nothing leaves
    /// the colour already multiplied by its own coverage: a half-alpha
    /// red lands as (128, 0, 0, 128). Those bytes go to `putImageData`,
    /// which reads them as STRAIGHT by specification — so the browser
    /// multiplies by the alpha a second time and the pane composites at
    /// a quarter instead of a half.
    ///
    /// The pixel tiers never saw it: over an OPAQUE background the
    /// alpha is 255 and the two conventions are the same number. Only a
    /// transparent destination tells them apart, and only islands have
    /// one.
    #[test]
    fn an_island_over_nothing_carries_its_alpha_home() {
        let mut display = crate::layout::DisplayList::default();
        display.push(crate::layout::DrawCommand::FillRect {
            rect: crate::layout::Rect {
                origin: crate::layout::Point { x: 0.0, y: 0.0 },
                size: crate::layout::Size { width: 4.0, height: 4.0 },
            },
            color: crate::layout::Color::rgba(255, 0, 0, 128),
            corner_radius: crate::layout::Corners::ZERO,
        });
        let bitmap = crate::raster::rasterize_with(
            &display,
            4,
            4,
            1,
            crate::layout::Color::rgba(0, 0, 0, 0),
            &crate::text_engine::PixelFont,
            &crate::image_engine::RawImages::default(),
        );
        let straight = crate::raster::unpremultiplied(&bitmap.to_rgba_bytes());
        assert_eq!(
            &straight[..4],
            &[255, 0, 0, 128],
            "a half-covered red is FULL red at half alpha, not half red"
        );
        // and an opaque pixel is untouched: the two conventions agree
        // wherever alpha is 255, which is why this never bit the window
        let opaque = crate::raster::unpremultiplied(&[10, 20, 30, 255]);
        assert_eq!(opaque, vec![10, 20, 30, 255]);
    }
    use super::*;
    use crate::layout::{DrawCommand, Glass, Point};

    /// Draws the ascii portrait of a bitmap patch — a readable golden.
    fn portrait(bitmap: &Bitmap, x: usize, y: usize, w: usize, h: usize, ink: u32) -> String {
        let mut out = String::new();
        for row in y..y + h {
            for col in x..x + w {
                out.push(if bitmap.pixel(col, row) == Some(ink) { '#' } else { '.' });
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn the_glyph_cell_respects_the_layout_metrics() {
        let mut display = DisplayList::default();
        display.push(DrawCommand::TextLine {
            origin: Point { x: 0.0, y: 0.0 },
            content: std::sync::Arc::from("1"),
            range: (0, 1),
            color: Color::BLACK,
            font: crate::text_engine::FontSpec::DEFAULT,
        });
        let bitmap = rasterize(&display, 8, 16, Color::WHITE);

        let ink = super::pack(Color::BLACK);
        let picture = portrait(&bitmap, 0, 0, 8, 16, ink);
        // 3px of vertical slack on top, a 10px glyph, the rest clean
        assert!(picture.lines().take(3).all(|line| !line.contains('#')));
        assert!(picture.lines().nth(13).is_some_and(|line| !line.contains('#')));
        assert!(picture.contains('#'), "the glyph body has ink:\n{picture}");
    }

    #[test]
    fn fill_and_stroke_land_on_snapped_edges() {
        let mut display = DisplayList::default();
        display.push(DrawCommand::FillRect {
            rect: Rect {
                origin: Point { x: 2.0, y: 2.0 },
                size: crate::layout::Size { width: 4.0, height: 4.0 },
            },
            color: Color::FILL,
            corner_radius: Corners::ZERO,
        });
        let bitmap = rasterize(&display, 8, 8, Color::WHITE);

        let fill = super::pack(Color::FILL);
        let white = super::pack(Color::WHITE);
        assert_eq!(bitmap.pixel(2, 2), Some(fill));
        assert_eq!(bitmap.pixel(5, 5), Some(fill), "edge [2,6): 5 is the last one inside");
        assert_eq!(bitmap.pixel(6, 6), Some(white), "6 is already outside");
        assert_eq!(bitmap.pixel(1, 1), Some(white));
    }

    #[test]
    fn source_over_blends_the_veil_exactly() {
        // a veil of blue at 128/255 over white: each channel comes out of
        // div255, an exact integer value — no "almost"
        let mut display = DisplayList::default();
        display.push(DrawCommand::FillRect {
            rect: Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: crate::layout::Size { width: 1.0, height: 1.0 },
            },
            color: Color { r: 0, g: 0, b: 255, a: 128 },
            corner_radius: Corners::ZERO,
        });
        let bitmap = rasterize(&display, 1, 1, Color::WHITE);

        // r = g = round(255·127/255) = 127; b = 255; a = 128 + 127 = 255
        assert_eq!(bitmap.pixel(0, 0), Some(0x7F7F_FFFF));
    }

    #[test]
    fn zero_alpha_is_a_no_op_and_full_alpha_overwrites() {
        let rect = Rect {
            origin: Point { x: 0.0, y: 0.0 },
            size: crate::layout::Size { width: 1.0, height: 1.0 },
        };
        let mut display = DisplayList::default();
        display.push(DrawCommand::FillRect {
            rect,
            color: Color { r: 9, g: 9, b: 9, a: 0 },
            corner_radius: Corners::ZERO,
        });
        display.push(DrawCommand::FillRect {
            rect,
            color: Color { r: 1, g: 2, b: 3, a: 255 },
            corner_radius: Corners::ZERO,
        });
        let bitmap = rasterize(&display, 1, 1, Color::WHITE);

        assert_eq!(bitmap.pixel(0, 0), Some(super::pack(Color { r: 1, g: 2, b: 3, a: 255 })));
    }

    #[test]
    fn corner_radius_insets_the_scanlines() {
        let mut display = DisplayList::default();
        display.push(DrawCommand::FillRect {
            rect: Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: crate::layout::Size { width: 8.0, height: 8.0 },
            },
            color: Color::BLACK,
            corner_radius: Corners::all(3.0),
        });
        let bitmap = rasterize(&display, 8, 8, Color::WHITE);

        let ink = super::pack(Color::BLACK);
        let white = super::pack(Color::WHITE);
        assert_eq!(bitmap.pixel(0, 0), Some(white), "corner inset");
        assert_eq!(bitmap.pixel(7, 7), Some(white), "opposite corner inset");
        assert_eq!(bitmap.pixel(4, 0), Some(ink), "middle of the top edge painted");
        assert_eq!(bitmap.pixel(0, 4), Some(ink), "middle of the left edge painted");
        assert_eq!(bitmap.pixel(4, 4), Some(ink), "center painted");
    }

    #[test]
    fn stroke_width_never_double_blends_the_corners() {
        // translucent frame: corner and edge have the SAME value — every
        // border pixel blended exactly once
        let mut display = DisplayList::default();
        display.push(DrawCommand::StrokeRect {
            rect: Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: crate::layout::Size { width: 6.0, height: 6.0 },
            },
            color: Color { r: 0, g: 0, b: 0, a: 128 },
            width: 2.0,
            corner_radius: Corners::ZERO,
        });
        let bitmap = rasterize(&display, 6, 6, Color::WHITE);

        assert_eq!(bitmap.pixel(0, 0), bitmap.pixel(3, 0), "corner == top");
        assert_eq!(bitmap.pixel(0, 3), bitmap.pixel(3, 0), "side == top");
        assert_eq!(bitmap.pixel(3, 3), Some(super::pack(Color::WHITE)), "interior untouched");
    }

    // MARK: - Surface (damage)

    use crate::layout::Size;

    fn fill(x: f64, y: f64, w: f64, h: f64, color: Color) -> DrawCommand {
        DrawCommand::FillRect {
            rect: Rect { origin: Point { x, y }, size: Size { width: w, height: h } },
            color,
            corner_radius: Corners::ZERO,
        }
    }

    /// The RGBA of one pixel, unpacked.
    fn channels(bitmap: &Bitmap, x: usize, y: usize) -> (u8, u8, u8, u8) {
        let pixel = bitmap.pixel(x, y).expect("inside the bitmap");
        (
            (pixel >> 24) as u8,
            (pixel >> 16) as u8,
            (pixel >> 8) as u8,
            pixel as u8,
        )
    }

    fn gradient(rect: Rect, paint: crate::layout::GradientPaint, corner: f64) -> DrawCommand {
        DrawCommand::Gradient { rect, paint, corner_radius: Corners::all(corner) }
    }

    #[test]
    fn a_flat_ramp_paints_what_a_flat_fill_paints() {
        // the coverage of a gradient IS the fill's: same shape, same
        // anti-aliased corners — only the color moves
        let box_rect = Rect {
            origin: Point { x: 6.0, y: 4.0 },
            size: Size { width: 40.0, height: 30.0 },
        };
        let ink = Color::hex(0x3B82F6);
        let mut ramp = DisplayList::default();
        ramp.push(gradient(
            box_rect,
            crate::layout::Gradient::radial(ink, ink).resolve(box_rect),
            9.0,
        ));
        let mut flat = DisplayList::default();
        flat.push(DrawCommand::FillRect { rect: box_rect, color: ink, corner_radius: Corners::all(9.0) });
        assert_eq!(
            rasterize(&ramp, 60, 40, Color::CANVAS).pixels(),
            rasterize(&flat, 60, 40, Color::CANVAS).pixels(),
            "one ramp with one color is a fill"
        );
    }

    #[test]
    fn a_radial_ramp_runs_from_the_centre_outwards() {
        let box_rect = Rect {
            origin: Point::ZERO,
            size: Size { width: 40.0, height: 40.0 },
        };
        let inner = Color::hex(0xFF0000);
        let outer = Color::hex(0x0000FF);
        let mut display = DisplayList::default();
        display.push(gradient(
            box_rect,
            crate::layout::Gradient::radial(inner, outer).resolve(box_rect),
            0.0,
        ));
        let bitmap = rasterize(&display, 40, 40, Color::CANVAS);
        let (r, g, b, _) = channels(&bitmap, 20, 20);
        assert!(r > 240 && b < 16, "the centre is the inner color: {r},{g},{b}");
        let (r, _, b, _) = channels(&bitmap, 0, 0);
        assert!(b > 240 && r < 16, "the farthest corner is the outer one");
        // and the ramp only ever moves one way along a radius
        let mut last = 0;
        for x in 20..40 {
            let (_, _, blue, _) = channels(&bitmap, x, 20);
            assert!(blue >= last, "the ramp never turns back at x={x}");
            last = blue;
        }
    }

    #[test]
    fn a_linear_ramp_runs_along_its_line() {
        let box_rect = Rect {
            origin: Point::ZERO,
            size: Size { width: 20.0, height: 40.0 },
        };
        let from = Color::hex(0x000000);
        let to = Color::hex(0xFFFFFF);
        let mut display = DisplayList::default();
        display.push(gradient(
            box_rect,
            crate::layout::Gradient::linear(from, to).resolve(box_rect),
            0.0,
        ));
        let bitmap = rasterize(&display, 20, 40, Color::CANVAS);
        let (top, _, _, _) = channels(&bitmap, 10, 0);
        let (middle, _, _, _) = channels(&bitmap, 10, 20);
        let (bottom, _, _, _) = channels(&bitmap, 10, 39);
        assert!(top < 16, "it starts at the from color: {top}");
        assert!(bottom > 240, "and ends at the to color: {bottom}");
        assert!((middle as i16 - 128).abs() <= 8, "half way is half way: {middle}");
        // the line runs down, not across
        assert_eq!(channels(&bitmap, 2, 20), channels(&bitmap, 17, 20));
    }

    #[test]
    fn a_ramp_that_fades_out_keeps_its_hue() {
        // fading to a transparent BLACK drags the ramp through grey;
        // fading to the same color with no alpha keeps it clean
        let box_rect = Rect {
            origin: Point::ZERO,
            size: Size { width: 40.0, height: 8.0 },
        };
        let violet = Color::hex(0x8B5CF6);
        let mut display = DisplayList::default();
        display.push(gradient(
            box_rect,
            crate::layout::Gradient::linear(violet, violet.fade())
                .direction(crate::layout::UnitPoint::LEADING, crate::layout::UnitPoint::TRAILING)
                .resolve(box_rect),
            0.0,
        ));
        let bitmap = rasterize(&display, 40, 8, Color::WHITE);
        let (r, g, b, _) = channels(&bitmap, 20, 4);
        // halfway over white: the hue survives (red still leads blue's
        // neighbourhood, green stays the lowest channel)
        assert!(g < r && g < b, "the violet is still violet: {r},{g},{b}");
    }

    fn line(x: f64, y: f64, text: &str, color: Color) -> DrawCommand {
        DrawCommand::TextLine {
            origin: Point { x, y },
            content: std::sync::Arc::from(text),
            range: (0, text.len()),
            color,
            font: crate::text_engine::FontSpec::DEFAULT,
        }
    }

    fn list(commands: Vec<DrawCommand>) -> DisplayList {
        let mut display = DisplayList::default();
        for command in commands {
            display.push(command);
        }
        display
    }

    /// A hover-like frame sequence: panel + two rows + text; the second
    /// frame swaps ONE row color, the third swaps it back.
    fn hover_frames() -> Vec<DisplayList> {
        let row = |y: f64, color: Color| fill(8.0, y, 104.0, 20.0, color);
        let base = Color::hex(0xEEEEF2);
        let hot = Color::hex(0xD8DCE6);
        let panel = fill(0.0, 0.0, 120.0, 80.0, Color::WHITE);
        vec![
            list(vec![
                panel.clone(),
                row(8.0, base),
                row(32.0, base),
                line(12.0, 10.0, "alpha", Color::BLACK),
                line(12.0, 34.0, "beta", Color::BLACK),
            ]),
            list(vec![
                panel.clone(),
                row(8.0, base),
                row(32.0, hot),
                line(12.0, 10.0, "alpha", Color::BLACK),
                line(12.0, 34.0, "beta", Color::BLACK),
            ]),
            list(vec![
                panel,
                row(8.0, base),
                row(32.0, base),
                line(12.0, 10.0, "alpha", Color::BLACK),
                line(12.0, 34.0, "beta", Color::BLACK),
            ]),
        ]
    }

    #[test]
    fn surface_matches_the_full_repaint_byte_for_byte() {
        // the oracle: every incremental frame == the one-shot raster of
        // the same list, pixel by pixel — including clips and text
        let sequences: Vec<Vec<DisplayList>> = vec![
            hover_frames(),
            // scrolled clip region: content shifts under the clip
            vec![
                list(vec![
                    fill(0.0, 0.0, 120.0, 80.0, Color::WHITE),
                    DrawCommand::PushClip {
                        rect: Rect {
                            origin: Point { x: 0.0, y: 16.0 },
                            size: Size { width: 120.0, height: 48.0 },
                        },
                        corner_radius: Corners::ZERO,
                    },
                    line(8.0, 18.0, "one", Color::BLACK),
                    line(8.0, 40.0, "two", Color::BLACK),
                    DrawCommand::PopClip,
                ]),
                list(vec![
                    fill(0.0, 0.0, 120.0, 80.0, Color::WHITE),
                    DrawCommand::PushClip {
                        rect: Rect {
                            origin: Point { x: 0.0, y: 16.0 },
                            size: Size { width: 120.0, height: 48.0 },
                        },
                        corner_radius: Corners::ZERO,
                    },
                    line(8.0, 8.0, "one", Color::BLACK),
                    line(8.0, 30.0, "two", Color::BLACK),
                    line(8.0, 52.0, "three", Color::BLACK),
                    DrawCommand::PopClip,
                ]),
            ],
        ];
        for frames in sequences {
            let mut surface = Surface::new(120, 80, 1, Color::CANVAS);
            for frame in frames {
                let oracle = rasterize(&frame, 120, 80, Color::CANVAS);
                surface.frame(frame, &PixelFont, &RawImages::default());
                assert_eq!(
                    surface.bitmap().pixels(),
                    oracle.pixels(),
                    "incremental and full repaint must match byte for byte"
                );
            }
        }
    }

    #[test]
    fn an_identical_frame_damages_nothing() {
        let mut surface = Surface::new(120, 80, 1, Color::CANVAS);
        let frames = hover_frames();
        surface.frame(frames[0].clone(), &PixelFont, &RawImages::default());
        let damage = surface.frame(frames[0].clone(), &PixelFont, &RawImages::default());
        assert!(damage.is_empty(), "same list, no damage: {damage:?}");
    }

    #[test]
    fn a_rounded_stroke_follows_the_curve_and_leaves_the_square_corner_empty() {
        let mut display = DisplayList::default();
        display.push(DrawCommand::StrokeRect {
            rect: Rect {
                origin: Point { x: 10.0, y: 10.0 },
                size: Size { width: 40.0, height: 40.0 },
            },
            color: Color::BLACK,
            width: 2.0,
            corner_radius: Corners::all(12.0),
        });
        let bitmap = rasterize(&display, 60, 60, Color::WHITE);

        let white = super::pack(Color::WHITE);
        let ink = |x: usize, y: usize| bitmap.pixel(x, y) != Some(white);
        assert!(!ink(11, 11), "the square corner is OUTSIDE the curve — no straight bar");
        // the arc: points at ~radius distance from the corner center (22, 22)
        assert!(ink(22, 10), "top bar starts after the corner");
        assert!(ink(13, 14), "the ring passes through the diagonal of the arc");
        assert!(ink(10, 22), "left bar starts after the corner");
        assert!(!ink(16, 16), "inside the ring is empty (border, not fill)");
        assert!(!ink(30, 30), "the middle stays untouched");
    }

    #[test]
    fn a_rounded_shadow_fills_the_notch_behind_the_corner() {
        // a rounded panel recedes at the corner; the notch belongs to
        // the halo, not to the backdrop
        let mut display = DisplayList::default();
        display.push(DrawCommand::Shadow {
            rect: Rect {
                origin: Point { x: 20.0, y: 20.0 },
                size: Size { width: 30.0, height: 30.0 },
            },
            radius: 8.0,
            color: Color::rgba(0, 0, 0, 200),
            corner_radius: Corners::all(10.0),
        });
        let bitmap = rasterize(&display, 70, 70, Color::WHITE);

        let white = super::pack(Color::WHITE);
        assert_ne!(
            bitmap.pixel(21, 21),
            Some(white),
            "the notch behind the rounded corner holds shadow"
        );
        assert_eq!(bitmap.pixel(35, 35), Some(white), "the middle of the shape stays clean");
        assert_eq!(bitmap.pixel(30, 24), Some(white), "inside the rounded edge stays clean");
    }

    #[test]
    fn a_shadow_falls_off_outside_and_never_touches_the_inside() {
        let mut display = DisplayList::default();
        display.push(DrawCommand::Shadow {
            rect: Rect {
                origin: Point { x: 20.0, y: 20.0 },
                size: Size { width: 20.0, height: 20.0 },
            },
            radius: 10.0,
            color: Color::rgba(0, 0, 0, 200),
            corner_radius: Corners::ZERO,
        });
        let bitmap = rasterize(&display, 60, 60, Color::WHITE);

        let white = super::pack(Color::WHITE);
        assert_eq!(bitmap.pixel(30, 30), Some(white), "the inside belongs to the view");
        let near = bitmap.pixel(30, 18).unwrap() & 0xFF00_0000;
        let far = bitmap.pixel(30, 12).unwrap() & 0xFF00_0000;
        assert!(near < 0xFF00_0000, "right at the edge the halo darkens");
        assert!(far > near, "farther out the halo fades (quadratic falloff)");
        assert_eq!(bitmap.pixel(30, 5), Some(white), "past the radius, nothing");
        // corners fade too (euclidean distance, no square halo artifact)
        assert!(bitmap.pixel(16, 16).unwrap() & 0xFF00_0000 > near, "corner is softer than edge");
    }

    #[test]
    fn shadow_damage_covers_the_halo() {
        // toggling a shadow must damage the halo box, not just the frame
        let mut surface = Surface::new(80, 80, 1, Color::CANVAS);
        let with = |shadow: bool| {
            let mut display = DisplayList::default();
            if shadow {
                display.push(DrawCommand::Shadow {
                    rect: Rect {
                        origin: Point { x: 30.0, y: 30.0 },
                        size: Size { width: 20.0, height: 20.0 },
                    },
                    radius: 8.0,
                    color: Color::rgba(0, 0, 0, 90),
                    corner_radius: Corners::ZERO,
                });
            }
            display.push(fill(30.0, 30.0, 20.0, 20.0, Color::WHITE));
            display
        };
        surface.frame(with(false), &PixelFont, &RawImages::default());
        let damage = surface.frame(with(true), &PixelFont, &RawImages::default());
        let oracle = rasterize(&with(true), 80, 80, Color::CANVAS);
        assert_eq!(surface.bitmap().pixels(), oracle.pixels(), "golden with shadow");
        let (x0, y0, x1, y1) = damage[0];
        assert!(x0 <= 22 && y0 <= 22 && x1 >= 58 && y1 >= 58, "halo box damaged: {damage:?}");
    }

    #[test]
    fn the_rgba_mirror_stays_true_through_partial_syncs() {
        // the mirror must equal a full conversion after every frame,
        // even though only damaged rects are converted
        let mut surface = Surface::new(120, 80, 1, Color::CANVAS);
        for frame in hover_frames() {
            surface.frame(frame, &PixelFont, &RawImages::default());
            let full = surface.bitmap().to_rgba_bytes();
            assert_eq!(surface.rgba(), &full[..], "mirror == full conversion");
        }
    }

    #[test]
    fn a_hover_swap_damages_only_the_row() {
        let mut surface = Surface::new(120, 80, 1, Color::CANVAS);
        let frames = hover_frames();
        let first = surface.frame(frames[0].clone(), &PixelFont, &RawImages::default());
        assert_eq!(first, vec![(0, 0, 120, 80)], "first frame damages the whole surface");
        let damage = surface.frame(frames[1].clone(), &PixelFont, &RawImages::default());
        assert_eq!(damage.len(), 1, "one row changed, one rect: {damage:?}");
        let (x0, y0, x1, y1) = damage[0];
        // the changed row lives at (8, 32)–(112, 52); text slack may pad
        assert!(x0 >= 8 && y0 >= 28 && x1 <= 114 && y1 <= 56, "row-sized damage: {damage:?}");
    }

    #[test]
    fn an_image_swap_damages_only_its_rect() {
        use crate::image_engine::ImageSource;
        let paint = |seed: u8| {
            // opaque pixels: the compare below reads them back verbatim
            let source =
                ImageSource::from_bytes(RawImages::encode(2, 2, &[seed, seed, seed, 255].repeat(4)));
            let mut display = DisplayList::default();
            display.push(DrawCommand::FillRect {
                rect: Rect {
                    origin: Point { x: 0.0, y: 0.0 },
                    size: crate::layout::Size { width: 100.0, height: 40.0 },
                },
                color: Color::CANVAS,
                corner_radius: Corners::ZERO,
            });
            display.push(DrawCommand::Image {
                rect: Rect {
                    origin: Point { x: 10.0, y: 10.0 },
                    size: crate::layout::Size { width: 16.0, height: 16.0 },
                },
                source,
            });
            display
        };
        let mut surface = Surface::new(100, 40, 1, Color::CANVAS);
        surface.frame(paint(60), &PixelFont, &RawImages::default());
        let damage = surface.frame(paint(200), &PixelFont, &RawImages::default());
        assert_eq!(damage, vec![(10, 10, 26, 26)], "only the image rect repaints");
        // and the pixels landed: the new seed shows at the center
        assert_eq!(surface.bitmap().pixel(18, 18), Some(0xC8C8_C8FF), "seed 200 everywhere");
    }

    fn clip(x: f64, y: f64, w: f64, h: f64, radius: f64) -> DrawCommand {
        DrawCommand::PushClip {
            rect: Rect {
                origin: Point { x, y },
                size: Size { width: w, height: h },
            },
            corner_radius: Corners::all(radius),
        }
    }

    /// The frozen degenerate: a radius-zero clip must leave EXACTLY
    /// the picture the straight clip always left.
    #[test]
    fn a_clip_without_a_radius_is_the_clip_it_always_was() {
        let display = list(vec![
            clip(2.0, 2.0, 8.0, 8.0, 0.0),
            fill(0.0, 0.0, 12.0, 12.0, Color::BLACK),
            DrawCommand::PopClip,
        ]);
        let bitmap = rasterize(&display, 12, 12, Color::WHITE);
        let picture = portrait(&bitmap, 0, 0, 12, 12, super::pack(Color::BLACK));
        assert_eq!(
            picture,
            "\
............
............
..########..
..########..
..########..
..########..
..########..
..########..
..########..
..########..
............
............
"
        );
    }

    /// The pain the front came to kill: a child that paints its own
    /// background under a rounded clip loses its corner to the curve.
    #[test]
    fn a_rounded_clip_eats_the_child_corner() {
        let display = list(vec![
            clip(1.0, 1.0, 14.0, 14.0, 5.0),
            fill(1.0, 1.0, 14.0, 14.0, Color::BLACK),
            DrawCommand::PopClip,
        ]);
        let bitmap = rasterize(&display, 16, 16, Color::WHITE);
        let black = super::pack(Color::BLACK);
        let white = super::pack(Color::WHITE);
        // the dead corner is untouched canvas
        assert_eq!(bitmap.pixel(1, 1), Some(white), "the notch stays canvas");
        // the straight edges hold their ink
        assert_eq!(bitmap.pixel(8, 1), Some(black));
        assert_eq!(bitmap.pixel(1, 8), Some(black));
        assert_eq!(bitmap.pixel(8, 8), Some(black));
        // and the pixel ON the arc (coverage 0.55 by the kernel) is
        // neither empty nor full — real anti-aliasing, not a threshold
        let arc = bitmap.pixel(2, 2).unwrap();
        let alpha_like = arc != black && arc != white;
        assert!(alpha_like, "the arc blends: 0x{arc:08x}");
    }

    /// Text funnels through the same door — a glyph crossing the
    /// corner is cut smoothly, no square survivor.
    #[test]
    fn text_under_a_rounded_clip_loses_its_corner() {
        let radius = 6.0;
        let display = list(vec![
            clip(0.0, 0.0, 24.0, 24.0, radius),
            fill(0.0, 0.0, 24.0, 24.0, Color::FILL),
            line(0.0, 0.0, "88", Color::BLACK),
            DrawCommand::PopClip,
        ]);
        let bitmap = rasterize(&display, 24, 24, Color::WHITE);
        let white = super::pack(Color::WHITE);
        // the notch (outside the arc) shows canvas even where the
        // glyph would have inked; one pixel in, the arc already blends
        assert_eq!(bitmap.pixel(0, 0), Some(white));
        assert_ne!(bitmap.pixel(1, 1), Some(white), "the arc blends at (1,1)");
        // inside the curve the glyph paints
        let inked = (4..20)
            .flat_map(|y| (4..20).map(move |x| (x, y)))
            .any(|(x, y)| bitmap.pixel(x, y) == Some(super::pack(Color::BLACK)));
        assert!(inked, "the glyph body survives inside the curve");
    }

    /// An image blit funnels through the same door too.
    #[test]
    fn an_image_under_a_rounded_clip_is_cut_smoothly() {
        let source = crate::image_engine::ImageSource::from_bytes(RawImages::encode(4, 4, &[0x40u8; 64]));
        let display = list(vec![
            clip(0.0, 0.0, 12.0, 12.0, 4.0),
            DrawCommand::Image {
                rect: Rect {
                    origin: Point { x: 0.0, y: 0.0 },
                    size: Size { width: 12.0, height: 12.0 },
                },
                source,
            },
            DrawCommand::PopClip,
        ]);
        let bitmap = rasterize(&display, 12, 12, Color::WHITE);
        let white = super::pack(Color::WHITE);
        assert_eq!(bitmap.pixel(0, 0), Some(white), "the notch stays canvas");
        assert_ne!(bitmap.pixel(6, 6), Some(white), "the body lands");
        // the corner pixel on the arc is a BLEND of image over canvas
        let arc = bitmap.pixel(1, 1).unwrap();
        assert_ne!(arc, white, "the arc took some image");
        assert_ne!(arc & 0xFF, 0x00);
    }

    /// The composition rule: rects intersect all the way down, the
    /// innermost curve wins, and a rect-only clip INHERITS the curve
    /// already cutting (the scroll-inside-a-card case).
    #[test]
    fn the_innermost_curve_wins_and_the_rects_still_cut() {
        // a rounded island, then a straight inner clip: the inner cut
        // trims the right half away, but the island's curve still eats
        // the top-left corner of what remains
        let display = list(vec![
            clip(0.0, 0.0, 16.0, 16.0, 5.0),
            clip(0.0, 0.0, 8.0, 16.0, 0.0),
            fill(0.0, 0.0, 16.0, 16.0, Color::BLACK),
            DrawCommand::PopClip,
            DrawCommand::PopClip,
        ]);
        let bitmap = rasterize(&display, 16, 16, Color::WHITE);
        let black = super::pack(Color::BLACK);
        let white = super::pack(Color::WHITE);
        assert_eq!(bitmap.pixel(0, 0), Some(white), "the island curve holds");
        assert_eq!(bitmap.pixel(4, 4), Some(black), "inside both cuts");
        assert_eq!(bitmap.pixel(9, 4), Some(white), "the straight cut trims the right");
        // a deeper ROUNDED clip replaces the curve: its own corner
        // bends, the outer's corner has no say inside it
        let nested = list(vec![
            clip(0.0, 0.0, 16.0, 16.0, 7.0),
            clip(4.0, 4.0, 12.0, 12.0, 3.0),
            fill(0.0, 0.0, 16.0, 16.0, Color::BLACK),
            DrawCommand::PopClip,
            DrawCommand::PopClip,
        ]);
        let bitmap = rasterize(&nested, 16, 16, Color::WHITE);
        assert_eq!(bitmap.pixel(4, 4), Some(white), "the inner curve bends its own corner");
        assert_eq!(bitmap.pixel(10, 10), Some(black));
    }

    /// The oracle covers the curve: an incremental frame under a
    /// rounded clip stays byte-identical to the one-shot raster —
    /// including the replay plumbing that must pass the radius.
    #[test]
    fn the_rounded_clip_survives_the_incremental_oracle() {
        let frames = vec![
            list(vec![
                fill(0.0, 0.0, 40.0, 40.0, Color::WHITE),
                clip(4.0, 4.0, 32.0, 32.0, 8.0),
                fill(4.0, 4.0, 32.0, 32.0, Color::BLACK),
                DrawCommand::PopClip,
            ]),
            // the content re-tints under the same curve
            list(vec![
                fill(0.0, 0.0, 40.0, 40.0, Color::WHITE),
                clip(4.0, 4.0, 32.0, 32.0, 8.0),
                fill(4.0, 4.0, 32.0, 32.0, Color::FILL),
                DrawCommand::PopClip,
            ]),
            // the RADIUS itself changes: the clip lands in the middle
            // and must damage everything it governs
            list(vec![
                fill(0.0, 0.0, 40.0, 40.0, Color::WHITE),
                clip(4.0, 4.0, 32.0, 32.0, 2.0),
                fill(4.0, 4.0, 32.0, 32.0, Color::FILL),
                DrawCommand::PopClip,
            ]),
            // and collapses back to the straight cut
            list(vec![
                fill(0.0, 0.0, 40.0, 40.0, Color::WHITE),
                clip(4.0, 4.0, 32.0, 32.0, 0.0),
                fill(4.0, 4.0, 32.0, 32.0, Color::FILL),
                DrawCommand::PopClip,
            ]),
        ];
        let mut surface = Surface::new(40, 40, 1, Color::CANVAS);
        for frame in frames {
            let oracle = rasterize(&frame, 40, 40, Color::CANVAS);
            surface.frame(frame, &PixelFont, &RawImages::default());
            assert_eq!(
                surface.bitmap().pixels(),
                oracle.pixels(),
                "golden: incremental == full under the curve"
            );
        }
    }

    // MARK: - Liquid glass

    fn size(width: f64, height: f64) -> crate::layout::Size {
        crate::layout::Size { width, height }
    }

    fn box_at(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect { origin: Point { x, y }, size: size(width, height) }
    }

    /// A scene of vertical stripes — the pattern a blur flattens and a
    /// lens bends.
    fn stripes(width: usize, height: usize, step: usize) -> DisplayList {
        let mut display = DisplayList::default();
        for column in (0..width).step_by(step) {
            display.push(DrawCommand::FillRect {
                rect: box_at(column as f64, 0.0, (step / 2) as f64, height as f64),
                color: Color::BLACK,
                corner_radius: Corners::ZERO,
            });
        }
        display
    }

    fn luma(pixel: u32) -> f64 {
        let (r, g, b) = ((pixel >> 24) & 0xFF, (pixel >> 16) & 0xFF, (pixel >> 8) & 0xFF);
        0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64
    }

    fn pane(rect: Rect, glass: crate::layout::Glass, radius: f64) -> DrawCommand {
        DrawCommand::Backdrop {
            rect,
            glass: glass.resolve(rect),
            corner_radius: Corners::all(radius),
        }
    }

    #[test]
    fn a_pane_flattens_the_stripes_under_it_and_leaves_the_rest_alone() {
        let mut display = stripes(120, 80, 8);
        display.push(pane(box_at(30.0, 20.0, 60.0, 40.0), Glass::frosted(), 0.0));
        let bitmap = rasterize(&display, 120, 80, Color::WHITE);

        // outside the pane the stripes keep their full contrast
        let outside: Vec<f64> = (0..16).map(|x| luma(bitmap.pixel(x, 4).expect("pixel"))).collect();
        let outside_range = outside.iter().cloned().fold(f64::MIN, f64::max)
            - outside.iter().cloned().fold(f64::MAX, f64::min);
        assert!(outside_range > 200.0, "the raw stripes are black on white: {outside_range}");

        // under it they are one wash
        let inside: Vec<f64> = (40..80).map(|x| luma(bitmap.pixel(x, 40).expect("pixel"))).collect();
        let inside_range = inside.iter().cloned().fold(f64::MIN, f64::max)
            - inside.iter().cloned().fold(f64::MAX, f64::min);
        assert!(
            inside_range < outside_range / 4.0,
            "a heavy blur must kill the stripes: {inside_range} vs {outside_range}"
        );
    }

    #[test]
    fn the_lens_magnifies_it_never_pinches() {
        // a scene that is dark up to x=40 and bright after it. The
        // pane's left rim sits in the dark, and the rim samples
        // INWARD: it must show the bright side and come out BRIGHTER.
        // Sampling outward pinches, and a pinch is the loudest tell of
        // a fake lens.
        //
        // The two panes differ ONLY in the amount, so the blur and the
        // rim sharpening are the same on both sides of the assertion.
        let lens = Glass::regular()
            .tint(Color::rgba(0, 0, 0, 0))
            .highlight(Color::WHITE, 0.0, 0.0)
            .saturation(1.0);
        let scene = |glass: Glass| {
            let mut display = DisplayList::default();
            display.push(DrawCommand::FillRect {
                rect: box_at(0.0, 0.0, 40.0, 80.0),
                color: Color::BLACK,
                corner_radius: Corners::ZERO,
            });
            display.push(pane(box_at(30.0, 20.0, 80.0, 40.0), glass, 0.0));
            rasterize(&display, 160, 80, Color::WHITE)
        };
        let still = scene(lens.refraction(24.0, 0.0));
        let bent = scene(lens.refraction(24.0, 20.0));

        let (x, y) = (33, 40);
        assert!(
            luma(bent.pixel(x, y).expect("pixel")) > luma(still.pixel(x, y).expect("pixel")) + 8.0,
            "the rim must pull the bright side in, not push it away"
        );
    }

    #[test]
    fn a_flat_pane_bends_nothing() {
        let scene = |glass: Glass| {
            let mut display = stripes(160, 80, 10);
            display.push(pane(box_at(20.0, 20.0, 120.0, 100.0), glass, 0.0));
            rasterize(&display, 160, 140, Color::WHITE)
        };
        let base = Glass::regular().tint(Color::rgba(0, 0, 0, 0)).highlight(Color::WHITE, 0.0, 0.0);
        // the same BAND on both, so the two only disagree about the
        // amount — the band also drives how the blur backs off, and a
        // pane shorter than its band is all rim
        let flat = scene(base.refraction(24.0, 0.0));
        let bent = scene(base.refraction(24.0, 30.0));

        // the centre is more than one band inward from every rim, so
        // both panes must agree there
        assert_eq!(flat.pixel(80, 70), bent.pixel(80, 70), "the face of the pane never bends");
        // and the rim must NOT agree
        let differs = (22..40).any(|x| flat.pixel(x, 70) != bent.pixel(x, 70));
        assert!(differs, "an amount of 30 has to move the rim");
    }

    #[test]
    fn the_rim_lights_along_both_diagonals() {
        // the dual lobe: the edge facing the light and the edge facing
        // directly away both light up; the perpendicular diagonal goes
        // quiet. One lobe alone reads as a shine painted on a corner.
        let mut display = DisplayList::default();
        display.push(DrawCommand::FillRect {
            rect: box_at(0.0, 0.0, 120.0, 120.0),
            color: Color::hex(0x404060),
            corner_radius: Corners::ZERO,
        });
        display.push(pane(
            box_at(20.0, 20.0, 80.0, 80.0),
            Glass::regular().refraction(0.0, 0.0).highlight(Color::WHITE, 6.0, 1.0),
            24.0,
        ));
        let bitmap = rasterize(&display, 120, 120, Color::WHITE);

        // three points at the same depth (about 3px in) on three
        // corners of the arc — the band is 6, so a deeper sample would
        // read no rim at all
        let corner = |x: usize, y: usize| luma(bitmap.pixel(x, y).expect("pixel"));
        let top_left = corner(29, 29);
        let bottom_right = corner(90, 90);
        let top_right = corner(90, 29);
        assert!(top_left > top_right + 4.0, "the lit diagonal: {top_left} vs {top_right}");
        assert!(
            bottom_right > top_right + 4.0,
            "and its opposite lobe: {bottom_right} vs {top_right}"
        );
    }

    #[test]
    fn a_pane_keeps_its_own_corner() {
        let mut display = DisplayList::default();
        display.push(DrawCommand::FillRect {
            rect: box_at(0.0, 0.0, 80.0, 80.0),
            color: Color::BLACK,
            corner_radius: Corners::ZERO,
        });
        let plain = rasterize(&display, 80, 80, Color::WHITE);
        display.push(pane(box_at(10.0, 10.0, 60.0, 60.0), Glass::regular(), 20.0));
        let rounded = rasterize(&display, 80, 80, Color::WHITE);

        // the box's own corner, outside the arc: the pane never got there
        assert_eq!(rounded.pixel(11, 11), plain.pixel(11, 11), "outside the arc");
        assert_ne!(rounded.pixel(40, 40), plain.pixel(40, 40), "and inside it, glass");
    }

    #[test]
    fn the_touch_lights_add_and_default_to_nothing() {
        let scene = |glass: Glass| {
            let mut display = DisplayList::default();
            display.push(DrawCommand::FillRect {
                rect: box_at(0.0, 0.0, 80.0, 80.0),
                color: Color::hex(0x203040),
                corner_radius: Corners::ZERO,
            });
            display.push(pane(box_at(10.0, 10.0, 60.0, 60.0), glass, 0.0));
            rasterize(&display, 80, 80, Color::WHITE)
        };
        let quiet = scene(Glass::regular());
        let sheened = scene(Glass::regular().sheen(0.25));
        let spotted = scene(Glass::regular().spot(crate::layout::UnitPoint::CENTER, 0.5, 0.5));

        let at = |bitmap: &Bitmap, x: usize, y: usize| luma(bitmap.pixel(x, y).expect("pixel"));
        assert!(at(&sheened, 40, 40) > at(&quiet, 40, 40) + 20.0, "a sheen adds light");
        assert!(at(&spotted, 40, 40) > at(&quiet, 40, 40) + 20.0, "and so does the spot");
        // the spot is a POOL: it reaches zero at its radius
        assert_eq!(at(&spotted, 12, 12), at(&quiet, 12, 12), "the corner is outside the pool");
    }

    #[test]
    fn a_pane_repaints_when_the_scene_under_it_moves() {
        // the pane's own command never changes here — what changes is a
        // box behind it. The damage must still cover the pane, because
        // glass reads what is under it.
        let pane_box = box_at(20.0, 20.0, 60.0, 40.0);
        let frame = |mark: Color| {
            let mut display = DisplayList::default();
            display.push(DrawCommand::FillRect {
                rect: box_at(24.0, 24.0, 10.0, 10.0),
                color: mark,
                corner_radius: Corners::ZERO,
            });
            display.push(pane(pane_box, Glass::regular(), 0.0));
            display
        };
        let mut surface = Surface::new(120, 80, 1, Color::WHITE);
        let images = crate::image_engine::RawImages::default();
        surface.frame(frame(Color::BLACK), &PixelFont, &images);
        let damage = surface.frame(frame(Color::hex(0x3B82F6)), &PixelFont, &images);

        assert!(!damage.is_empty(), "a changed box under glass is not a silent frame");
        let covered = damage.iter().any(|rect| {
            rect.0 <= 20 && rect.1 <= 20 && rect.2 >= 80 && rect.3 >= 60
        });
        assert!(covered, "the whole pane repaints, not just the box that moved: {damage:?}");
    }

    #[test]
    fn the_material_merges_knob_by_knob() {
        // `.liquid_glass().backdrop_blur(16)` is ONE material: the
        // chain's blur wins and every other knob keeps the tuned value
        let inner = Glass::regular();
        let outer = Glass::regular().blur(16.0);
        let merged = inner.or(outer).resolve(box_at(0.0, 0.0, 100.0, 100.0));
        assert_eq!(merged.blur, 16.0);
        assert_eq!(merged.tint, Glass::TUNED_TINT);
        assert_eq!(merged.saturation, Glass::TUNED_SATURATION);

        // and the one CLOSEST to the view wins the knobs it named
        let pinned = Glass::regular().blur(2.0).or(Glass::regular().blur(16.0));
        assert_eq!(pinned.blur, Some(2.0));
    }

    #[test]
    fn the_blur_floor_is_the_first_level_of_the_pyramid() {
        // every blur below the pyramid's own level 0 renders as level 0:
        // the minimum glass is light glass, never clear glass
        assert_eq!(crate::glass::level_for(0.0), 0.0);
        assert_eq!(crate::glass::level_for(crate::glass::SIGMA_L0), 0.0);
        assert_eq!(crate::glass::level_for(crate::glass::SIGMA_L0 * 2.0), 1.0);
        // and it never climbs past the top of the pyramid
        assert_eq!(crate::glass::level_for(10_000.0), crate::glass::MAX_LEVEL as f64);
    }
}
