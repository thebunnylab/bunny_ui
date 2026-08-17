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

    /// Edges rounded in device px — the single point of snapping.
    fn snap(rect: Rect) -> (i64, i64, i64, i64) {
        let x0 = rect.origin.x.round() as i64;
        let y0 = rect.origin.y.round() as i64;
        let x1 = (rect.origin.x + rect.size.width).round() as i64;
        let y1 = (rect.origin.y + rect.size.height).round() as i64;
        (x0, y0, x1, y1)
    }

    /// Fill with optional corners: inset per scanline, a circle per
    /// corner, ONE square root per row — never per pixel.
    /// `corner_radius: 0.0` reproduces the straight rectangle byte for byte.
    fn fill_rect(&mut self, rect: Rect, color: Color, corner_radius: f64) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let packed = pack(color);
        let height = (y1 - y0) as f64;
        let radius = corner_radius
            .max(0.0)
            .min((x1 - x0) as f64 / 2.0)
            .min(height / 2.0);
        for y in y0..y1 {
            // distance from the row center to the straight band between corners
            let center = (y - y0) as f64 + 0.5;
            let dy = if center < radius {
                radius - center
            } else if center > height - radius {
                center - (height - radius)
            } else {
                0.0
            };
            let inset = if dy > 0.0 {
                (radius - (radius * radius - dy * dy).sqrt()).round() as i64
            } else {
                0
            };
            for x in (x0 + inset)..(x1 - inset) {
                self.set(x, y, packed);
            }
        }
    }

    /// A frame inward from the edge, in 4 bars with NO overlap — a
    /// translucent border cannot blend the corner twice.
    fn stroke_rect(&mut self, rect: Rect, color: Color, width: f64) {
        let (x0, y0, x1, y1) = Self::snap(rect);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let packed = pack(color);
        let thickness = width.max(1.0).round() as i64;
        let top_end = (y0 + thickness).min(y1);
        let bottom_start = (y1 - thickness).max(top_end);
        let left_end = (x0 + thickness).min(x1);
        let right_start = (x1 - thickness).max(left_end);
        for y in y0..top_end {
            for x in x0..x1 {
                self.set(x, y, packed);
            }
        }
        for y in bottom_start..y1 {
            for x in x0..x1 {
                self.set(x, y, packed);
            }
        }
        for y in top_end..bottom_start {
            for x in x0..left_end {
                self.set(x, y, packed);
            }
            for x in right_start..x1 {
                self.set(x, y, packed);
            }
        }
    }

    /// Composites a line rasterized by the engine at the logical origin,
    /// snapped ONCE (the raster already comes in physical pixels).
    fn composite_text(&mut self, origin_x: f64, origin_y: f64, scale: usize, raster: &TextRaster) {
        let base_x = (origin_x * scale as f64).round() as i64;
        let base_y = (origin_y * scale as f64).round() as i64;
        for row in 0..raster.height {
            for col in 0..raster.width {
                let index = (row * raster.width + col) * 4;
                let alpha = raster.rgba[index + 3];
                if alpha == 0 {
                    continue;
                }
                let packed = ((raster.rgba[index] as u32) << 24)
                    | ((raster.rgba[index + 1] as u32) << 16)
                    | ((raster.rgba[index + 2] as u32) << 8)
                    | alpha as u32;
                self.set(base_x + col as i64, base_y + row as i64, packed);
            }
        }
    }
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
    rasterize_with(display, width, height, scale, background, &PixelFont)
}

/// The full path: paints the list with the frame's [`TextEngine`] — it
/// is what the `Runtime` calls (PixelFont in headless, CoreText on the
/// Mac).
pub fn rasterize_with(
    display: &DisplayList,
    width: usize,
    height: usize,
    scale: usize,
    background: Color,
    text: &dyn TextEngine,
) -> Bitmap {
    let mut bitmap = Bitmap::new(width, height, background);
    let factor = scale as f64;
    for command in display.iter() {
        match command {
            DrawCommand::FillRect { rect, color, corner_radius } => {
                bitmap.fill_rect(scale_rect(*rect, factor), *color, corner_radius * factor)
            }
            DrawCommand::StrokeRect { rect, color, width } => {
                bitmap.stroke_rect(scale_rect(*rect, factor), *color, width * factor)
            }
            DrawCommand::TextLine { origin, content, range, color, font } => {
                let slice = &content[range.0..range.1];
                if let Some(raster) = text.raster_line(slice, font, *color, scale) {
                    bitmap.composite_text(origin.x, origin.y, scale, &raster);
                }
            }
            DrawCommand::PushClip { rect } => bitmap.push_clip(scale_rect(*rect, factor)),
            DrawCommand::PopClip => bitmap.pop_clip(),
        }
    }
    bitmap
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
            content: std::rc::Rc::from("1"),
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
        });
        let bitmap = rasterize(&display, 6, 6, Color::WHITE);

        assert_eq!(bitmap.pixel(0, 0), bitmap.pixel(3, 0), "corner == top");
        assert_eq!(bitmap.pixel(0, 3), bitmap.pixel(3, 0), "side == top");
        assert_eq!(bitmap.pixel(3, 3), Some(super::pack(Color::WHITE)), "interior untouched");
    }
}
