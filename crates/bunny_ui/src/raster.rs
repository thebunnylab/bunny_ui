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

use crate::image_engine::{ImageEngine, RawImages};
use crate::layout::{Color, DisplayList, DrawCommand, Rect};
use crate::text_engine::{PixelFont, TextEngine, TextRaster};

/// An RGBA buffer (one `0xRRGGBBAA` `u32` per pixel, rows top to
/// bottom) — what the platform backend blits into the window.
pub struct Bitmap {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
    /// Clip stack in physical px (intersections already resolved) — `set`
    /// checks the top; fill, stroke and text respect it for free.
    clip: Vec<(i64, i64, i64, i64)>,
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

impl Bitmap {
    pub fn new(width: usize, height: usize, background: Color) -> Self {
        Bitmap { width, height, pixels: vec![pack(background); width * height], clip: Vec::new() }
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

    fn set(&mut self, x: i64, y: i64, color: u32) {
        if x < 0 || y < 0 || (x as usize) >= self.width || (y as usize) >= self.height {
            return;
        }
        if let Some((cx0, cy0, cx1, cy1)) = self.clip.last().copied()
            && (x < cx0 || y < cy0 || x >= cx1 || y >= cy1)
        {
            return;
        }
        let index = y as usize * self.width + x as usize;
        self.pixels[index] = blend_px(color, self.pixels[index]);
    }

    /// Pushes the snapped clip, already intersected with the current top.
    fn push_clip(&mut self, rect: Rect) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        let clipped = match self.clip.last().copied() {
            Some((cx0, cy0, cx1, cy1)) => (x0.max(cx0), y0.max(cy0), x1.min(cx1), y1.min(cy1)),
            None => (x0, y0, x1, y1),
        };
        self.clip.push(clipped);
    }

    fn pop_clip(&mut self) {
        self.clip.pop();
    }

    /// Pushes an ALREADY-physical clip (the damage replay) — same stack,
    /// no snapping, intersected with the current top like any clip.
    fn push_clip_physical(&mut self, rect: DamageRect) {
        let clipped = match self.clip.last().copied() {
            Some((cx0, cy0, cx1, cy1)) => {
                (rect.0.max(cx0), rect.1.max(cy0), rect.2.min(cx1), rect.3.min(cy1))
            }
            None => rect,
        };
        self.clip.push(clipped);
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
    fn shadow_rect(&mut self, rect: Rect, color: Color, radius: f64, corner_radius: f64) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        let reach = radius.max(1.0);
        let reach_px = reach.round() as i64;
        let corner = corner_radius
            .max(0.0)
            .min((x1 - x0) as f64 / 2.0)
            .min((y1 - y0) as f64 / 2.0);
        let core_x0 = x0 as f64 + corner;
        let core_x1 = x1 as f64 - corner;
        let core_y0 = y0 as f64 + corner;
        let core_y1 = y1 as f64 - corner;
        let corner_px = corner.ceil() as i64;
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
                    let dx = px - px.clamp(core_x0, core_x1);
                    let dy = py - py.clamp(core_y0, core_y1);
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
            Some((x0, y0, x1, y1)) => (
                x0.max(surface.0),
                y0.max(surface.1),
                x1.min(surface.2),
                y1.min(surface.3),
            ),
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
            let row = y as usize * self.width;
            self.pixels[row + from as usize..row + to as usize].fill(packed);
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
    /// smooth). `corner_radius: 0.0` reproduces the straight rectangle
    /// byte for byte.
    fn fill_rect(&mut self, rect: Rect, color: Color, corner_radius: f64) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.clip_box();
        if x0 >= cx1 || x1 <= cx0 || y0 >= cy1 || y1 <= cy0 {
            return;
        }
        let packed = pack(color);
        let radius = corner_radius
            .max(0.0)
            .min((x1 - x0) as f64 / 2.0)
            .min((y1 - y0) as f64 / 2.0);
        if radius < 0.5 {
            for y in y0.max(cy0)..y1.min(cy1) {
                self.span(y, x0, x1, color, packed);
            }
            return;
        }
        let reach = radius.ceil() as i64;
        let left_center = x0 as f64 + radius;
        let right_center = x1 as f64 - radius;
        for y in y0.max(cy0)..y1.min(cy1) {
            let in_top = y < y0 + reach;
            let in_bottom = y >= y1 - reach;
            if !in_top && !in_bottom {
                self.span(y, x0, x1, color, packed);
                continue;
            }
            let center_y = if in_top { y0 as f64 + radius } else { y1 as f64 - radius };
            self.span(y, x0 + reach, x1 - reach, color, packed);
            let py = y as f64 + 0.5;
            for x in x0.max(cx0)..(x0 + reach).min(cx1) {
                let distance = (x as f64 + 0.5 - left_center).hypot(py - center_y);
                self.set_covered(x, y, color, (radius - distance + 0.5).clamp(0.0, 1.0));
            }
            for x in (x1 - reach).max(cx0)..x1.min(cx1) {
                let distance = (x as f64 + 0.5 - right_center).hypot(py - center_y);
                self.set_covered(x, y, color, (radius - distance + 0.5).clamp(0.0, 1.0));
            }
        }
    }

    /// A frame inward from the edge. With `corner_radius` the border
    /// FOLLOWS the curve — an anti-aliased ring between the outer circle
    /// and the inner one (outer minus `width`); a straight bar cutting
    /// across the arc is exactly the bug this kills. `0.0` keeps the
    /// four straight non-overlapping bars byte for byte (a translucent
    /// border cannot blend a corner twice).
    fn stroke_rect(&mut self, rect: Rect, color: Color, width: f64, corner_radius: f64) {
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
        let radius = corner_radius
            .max(0.0)
            .min((x1 - x0) as f64 / 2.0)
            .min((y1 - y0) as f64 / 2.0);
        if radius < 0.5 {
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
        let reach = radius.ceil() as i64;
        let inner = (radius - thickness as f64).max(0.0);
        let left_center = x0 as f64 + radius;
        let right_center = x1 as f64 - radius;
        for y in y0.max(cy0)..y1.min(cy1) {
            let in_top = y < y0 + reach;
            let in_bottom = y >= y1 - reach;
            if !in_top && !in_bottom {
                // straight sides between the corners
                self.span(y, x0, x0 + thickness, color, packed);
                self.span(y, x1 - thickness, x1, color, packed);
                continue;
            }
            // the straight middle of the horizontal bars
            if y < y0 + thickness || y >= y1 - thickness {
                self.span(y, x0 + reach, x1 - reach, color, packed);
            }
            let center_y = if in_top { y0 as f64 + radius } else { y1 as f64 - radius };
            let py = y as f64 + 0.5;
            let ring = |distance: f64| {
                ((radius - distance + 0.5).clamp(0.0, 1.0)
                    - (inner - distance + 0.5).clamp(0.0, 1.0))
                    .clamp(0.0, 1.0)
            };
            for x in x0.max(cx0)..(x0 + reach).min(cx1) {
                let distance = (x as f64 + 0.5 - left_center).hypot(py - center_y);
                self.set_covered(x, y, color, ring(distance));
            }
            for x in (x1 - reach).max(cx0)..x1.min(cx1) {
                let distance = (x as f64 + 0.5 - right_center).hypot(py - center_y);
                self.set_covered(x, y, color, ring(distance));
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
                if let Some(raster) = images.raster(source, width, height) {
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
            DrawCommand::PushClip { rect } => bitmap.push_clip(scale_rect(*rect, factor)),
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
            DrawCommand::FillRect { rect, .. } | DrawCommand::StrokeRect { rect, .. } => {
                Bitmap::snap(scale_rect(*rect, factor))
            }
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
            DrawCommand::PushClip { .. } | DrawCommand::PopClip => return None,
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
                DrawCommand::PushClip { rect } => {
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
                    DrawCommand::PushClip { rect } => {
                        self.bitmap.push_clip(scale_rect(*rect, factor));
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
                        if let Some(raster) = images.raster(source, width, height) {
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
    use super::*;
    use crate::layout::{DrawCommand, Point};

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
            corner_radius: 0.0,
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
            corner_radius: 0.0,
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
            corner_radius: 0.0,
        });
        display.push(DrawCommand::FillRect {
            rect,
            color: Color { r: 1, g: 2, b: 3, a: 255 },
            corner_radius: 0.0,
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
            corner_radius: 3.0,
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
            corner_radius: 0.0,
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
            corner_radius: 0.0,
        }
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
            corner_radius: 12.0,
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
            corner_radius: 10.0,
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
            corner_radius: 0.0,
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
                    corner_radius: 0.0,
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
                corner_radius: 0.0,
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
}
