//! Opaque bands scroll by moving retained pixels, and growing lists paint
//! only bounded new rows. Uniform empty space needs one pixel. Admission is
//! deliberately narrower than the display language; every refusal returns
//! to a whole Metal frame. No layout node or application identity is needed.

use super::*;
use bunny_ui::layout::{DrawCommand, Rect};
use bunny_ui::raster::{Bitmap, rasterize_with};
use bunny_ui::text_engine::{FontKey, FontSpec, LineMetrics, TextRaster};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

const MAX_BANDS: usize = 128;
const MAX_PIXELS: usize = 128 * 1024;
const MAX_TOTAL_PIXELS: usize = 8 * 1024 * 1024;

fn snapped(rect: Rect, scale: usize) -> Option<DamageRect> {
    let factor = scale as f64;
    let x = rect.origin.x * factor;
    let y = rect.origin.y * factor;
    let values = [
        x,
        y,
        x + rect.size.width * factor,
        y + rect.size.height * factor,
    ];
    if values
        .iter()
        .any(|v| !v.is_finite() || v.abs() > i32::MAX as f64)
    {
        return None;
    }
    let [x0, y0, x1, y1] = values.map(|v| v.round() as i64);
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

fn contains(outer: DamageRect, inner: DamageRect) -> bool {
    outer.0 <= inner.0 && outer.1 <= inner.1 && outer.2 >= inner.2 && outer.3 >= inner.3
}

fn intersects(a: DamageRect, b: DamageRect) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

fn ink(
    command: &DrawCommand,
    scale: usize,
    text: &dyn TextEngine,
    cache: &MeasureCache,
) -> Option<DamageRect> {
    match command {
        DrawCommand::FillRect { rect, .. } | DrawCommand::StrokeRect { rect, .. } => {
            snapped(*rect, scale)
        }
        DrawCommand::TextLine {
            origin,
            content,
            range,
            font,
            ..
        } => {
            let line = content.get(range.0..range.1)?;
            if line.len() > 4096 {
                return None;
            }
            let metrics = cache.get_or_measure(line, font, text);
            let factor = scale as f64;
            let x = (origin.x * factor).round();
            let y = (origin.y * factor).round();
            let w = (metrics.width * factor).ceil();
            let h = (metrics.height() * factor).ceil();
            if [x, y, w, h]
                .iter()
                .any(|v| !v.is_finite() || v.abs() > i32::MAX as f64)
                || w < 0.0
                || h < 0.0
                || w * h > MAX_PIXELS as f64
            {
                return None;
            }
            Some((
                x as i64 - 2,
                y as i64 - 2,
                (x + w) as i64 + 2,
                (y + h) as i64 + 2,
            ))
        }
        _ => None,
    }
}

/// Keep every non-background pixel, including antialiased glyph edges.
/// An all-background patch reduces to one pixel of the same solid color.
fn foreground_ink(bitmap: &Bitmap, background: Color) -> (usize, usize, usize, usize) {
    let packed = u32::from_be_bytes([background.r, background.g, background.b, background.a]);
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for (y, row) in bitmap.pixels().chunks_exact(bitmap.width()).enumerate() {
        if let Some(first) = row.iter().position(|pixel| *pixel != packed) {
            let last = row
                .iter()
                .rposition(|pixel| *pixel != packed)
                .unwrap_or(first);
            bounds = Some(match bounds {
                None => (first, y, last + 1, y + 1),
                Some((x0, y0, x1, _)) => (x0.min(first), y0, x1.max(last + 1), y + 1),
            });
        }
    }
    bounds.unwrap_or((0, 0, 1, 1))
}

struct Band {
    rect: DamageRect,
    display: DisplayList,
}

impl Band {
    fn new(
        commands: &[DrawCommand],
        scale: usize,
        text: &dyn TextEngine,
        cache: &MeasureCache,
    ) -> Option<Self> {
        let DrawCommand::FillRect {
            rect,
            color,
            corner_radius,
        } = commands.first()?
        else {
            return None;
        };
        if color.a != 255 || !corner_radius.is_zero() || commands.len() > 64 {
            return None;
        }
        let rect = snapped(*rect, scale)?;
        let budget = if commands.len() == 1 {
            MAX_TOTAL_PIXELS
        } else {
            MAX_PIXELS
        };
        if (rect.2 - rect.0) * (rect.3 - rect.1) > budget as i64 {
            return None;
        }
        let mut clips = Vec::new();
        let mut text_bytes = 0usize;
        for command in commands {
            match command {
                DrawCommand::PushClip { rect, .. } => clips.push(snapped(*rect, scale)?),
                DrawCommand::PopClip => {
                    clips.pop()?;
                }
                _ => {
                    let mut bounds = ink(command, scale, text, cache)?;
                    for clip in &clips {
                        bounds = (
                            bounds.0.max(clip.0),
                            bounds.1.max(clip.1),
                            bounds.2.min(clip.2),
                            bounds.3.min(clip.3),
                        );
                    }
                    // Every admitted row spans the viewport width. The
                    // viewport clips horizontal glyph overhang, including
                    // text placed flush against its left edge.
                    bounds.0 = bounds.0.max(rect.0);
                    bounds.2 = bounds.2.min(rect.2);
                    if bounds.0 < bounds.2 && bounds.1 < bounds.3 && !contains(rect, bounds) {
                        return None;
                    }
                    if let DrawCommand::TextLine { range, .. } = command {
                        text_bytes = text_bytes.checked_add(range.1.checked_sub(range.0)?)?;
                        if text_bytes > 4096 {
                            return None;
                        }
                    }
                }
            }
        }
        if !clips.is_empty() {
            return None;
        }
        let display = software_patch::patch_coordinates(
            &DisplayList::from(commands.to_vec()),
            rect,
            scale as f64,
        )?;
        Some(Self { rect, display })
    }

    fn solid(&self) -> Option<Color> {
        match self.display.as_slice() {
            [DrawCommand::FillRect { color, .. }] => Some(*color),
            _ => None,
        }
    }

    fn raster_pixels(&self) -> usize {
        if self.solid().is_some() {
            1
        } else {
            ((self.rect.2 - self.rect.0) * (self.rect.3 - self.rect.1)) as usize
        }
    }

    /// A second layer earns its cost only when it removes at least half
    /// the row's backing. Its pixels include the opaque background, so
    /// compositor blending cannot change the text raster's appearance.
    fn foreground(
        &self,
        scale: usize,
        text: &dyn TextEngine,
        cache: &MeasureCache,
    ) -> Option<(Color, software_patch::Scene)> {
        let commands = self.display.as_slice();
        let DrawCommand::FillRect { color, .. } = commands.first()? else {
            return None;
        };
        let size = (
            (self.rect.2 - self.rect.0) as usize,
            (self.rect.3 - self.rect.1) as usize,
        );
        let ListDamage::Rect(bounds) = list_damage(
            &commands[..1],
            commands,
            scale,
            size,
            PATCH_COMMANDS,
            cache,
            text,
        ) else {
            return None;
        };
        let pixels = (bounds.2 - bounds.0).checked_mul(bounds.3 - bounds.1)?;
        if pixels <= 0 || pixels.checked_mul(2)? > (size.0 * size.1) as i64 {
            return None;
        }
        Some((
            *color,
            software_patch::Scene::new(&self.display, bounds, scale, *color, text, cache)?,
        ))
    }

    fn same_pixels(&self, other: &Self) -> bool {
        self.rect.2 - self.rect.0 == other.rect.2 - other.rect.0
            && self.rect.3 - self.rect.1 == other.rect.3 - other.rect.1
            && self.display.as_slice() == other.display.as_slice()
    }
}

type OverlayPaint = (DamageRect, software_patch::Scene);

/// A proved partition: the viewport is completely covered by opaque,
/// disjoint bands. Everything above them either stays outside or is a
/// bounded overlay painted from the complete scene.
struct Scene {
    physical: (usize, usize),
    scale: usize,
    canvas: Color,
    source: DisplayList,
    decoration: (usize, usize, usize),
    viewport: DamageRect,
    bands: Vec<Band>,
    outside: DisplayList,
    overlay: Option<OverlayPaint>,
}

impl Scene {
    fn new(
        display: &DisplayList,
        physical: (usize, usize),
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        cache: &MeasureCache,
    ) -> Option<Self> {
        if scale == 0 || canvas.a != 255 || display.len() > 4096 {
            return None;
        }
        // A backdrop reads other paint, and an image may update its pixels
        // without changing its identity. Neither can be proved stationary.
        if display.iter().any(|c| {
            matches!(
                c,
                DrawCommand::Backdrop { .. }
                    | DrawCommand::Image { .. }
                    | DrawCommand::Gradient { .. }
                    | DrawCommand::Shadow { .. }
            )
        }) {
            return None;
        }
        let commands = display.as_slice();
        let start = commands
            .iter()
            .position(|c| matches!(c, DrawCommand::PushClip { .. }))?;
        let DrawCommand::PushClip {
            rect,
            corner_radius,
        } = commands[start]
        else {
            return None;
        };
        if !corner_radius.is_zero() {
            return None;
        }
        let viewport = snapped(rect, scale)?;
        if !contains((0, 0, physical.0 as i64, physical.1 as i64), viewport) {
            return None;
        }
        let mut depth = 0usize;
        let mut first = None;
        let mut first_row = None;
        let mut bands = Vec::new();
        let mut end = None;
        for (at, command) in commands.iter().enumerate().skip(start + 1) {
            match command {
                DrawCommand::PushClip { .. } => depth += 1,
                DrawCommand::PopClip if depth > 0 => depth -= 1,
                DrawCommand::PopClip => {
                    end = Some(at);
                    break;
                }
                DrawCommand::FillRect {
                    rect,
                    color,
                    corner_radius,
                } if depth == 0
                    && color.a == 255
                    && corner_radius.is_zero()
                    && snapped(*rect, scale)
                        .is_some_and(|r| r.0 == viewport.0 && r.2 == viewport.2) =>
                {
                    if let Some(first) = first {
                        bands.push(Band::new(&commands[first..at], scale, text, cache)?);
                    }
                    // Earlier ink is behind the complete opaque partition.
                    // Layout may keep text whose conservative bounds cross
                    // the clip even after its background has left it.
                    first = Some(at);
                    first_row.get_or_insert(at);
                }
                _ => {}
            }
            if bands.len() >= MAX_BANDS {
                return None;
            }
        }
        let end = end?;
        let mut tail = end;
        if let Some(first) = first {
            // Decorations after the last row remain above the partition.
            let last_rect = ink(commands.get(first)?, scale, text, cache)?;
            let mut depth = 0usize;
            for (at, command) in commands.iter().enumerate().take(end).skip(first + 1) {
                match command {
                    DrawCommand::PushClip { .. } => depth += 1,
                    DrawCommand::PopClip => depth = depth.checked_sub(1)?,
                    _ if depth == 0 => {
                        let bounds = ink(command, scale, text, cache)?;
                        let clipped = (
                            bounds.0.max(viewport.0),
                            bounds.1.max(viewport.1),
                            bounds.2.min(viewport.2),
                            bounds.3.min(viewport.3),
                        );
                        if clipped.0 < clipped.2
                            && clipped.1 < clipped.3
                            && !contains(last_rect, clipped)
                        {
                            tail = at;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            bands.push(Band::new(&commands[first..tail], scale, text, cache)?);
        } else if start + 1 != end {
            // No opaque row may silently discard text in an empty viewport.
            return None;
        }
        let top = bands.first().map_or(viewport.3, |band| band.rect.1);
        let bottom = bands.last().map_or(viewport.3, |band| band.rect.3);
        if top > viewport.1 || bottom < viewport.3 {
            if first_row.is_some_and(|first| first != start + 1) {
                return None;
            }
            // Prove the paint beneath every uncovered pixel is one opaque
            // color. Any overlapping ink invalidates it until a later opaque
            // fill covers the complete viewport again.
            let mut background = Some(canvas);
            for command in &commands[..start] {
                let bounds = ink(command, scale, text, cache)?;
                if intersects(bounds, viewport) {
                    background = match command {
                        DrawCommand::FillRect {
                            color,
                            corner_radius,
                            ..
                        } if color.a == 255
                            && corner_radius.is_zero()
                            && contains(bounds, viewport) =>
                        {
                            Some(*color)
                        }
                        _ => None,
                    };
                }
            }
            let color = background?;
            let blank = |y0, y1| {
                Band::new(
                    &[DrawCommand::FillRect {
                        rect: Rect {
                            origin: bunny_ui::layout::Point {
                                x: viewport.0 as f64 / scale as f64,
                                y: y0 as f64 / scale as f64,
                            },
                            size: Size {
                                width: (viewport.2 - viewport.0) as f64 / scale as f64,
                                height: (y1 - y0) as f64 / scale as f64,
                            },
                        },
                        color,
                        corner_radius: bunny_ui::layout::Corners::ZERO,
                    }],
                    scale,
                    text,
                    cache,
                )
            };
            if top > viewport.1 {
                bands.insert(0, blank(viewport.1, top.min(viewport.3))?);
            }
            if bottom < viewport.3 {
                bands.push(blank(bottom.max(viewport.1), viewport.3)?);
            }
        }
        if bands.is_empty() || bands.len() > MAX_BANDS {
            return None;
        }
        let mut total = 0i64;
        for (i, band) in bands.iter().enumerate() {
            if band.rect.0 != viewport.0
                || band.rect.2 != viewport.2
                || (i > 0 && bands[i - 1].rect.3 != band.rect.1)
            {
                return None;
            }
            total += (band.rect.2 - band.rect.0) * (band.rect.3 - band.rect.1);
        }
        if total > MAX_TOTAL_PIXELS as i64
            || bands.first()?.rect.1 > viewport.1
            || bands.last()?.rect.3 < viewport.3
        {
            return None;
        }
        let (extra, overlay) =
            Self::decoration(display, (tail, end), viewport, scale, canvas, text, cache)?;
        let mut outside = commands[..start].to_vec();
        outside.extend(extra);
        Some(Self {
            physical,
            scale,
            canvas,
            source: display.clone(),
            decoration: (start, tail, end),
            viewport,
            bands,
            outside: DisplayList::from(outside),
            overlay,
        })
    }

    fn decoration(
        display: &DisplayList,
        (tail, end): (usize, usize),
        viewport: DamageRect,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        cache: &MeasureCache,
    ) -> Option<(Vec<DrawCommand>, Option<OverlayPaint>)> {
        let commands = display.as_slice();
        let mut outside = Vec::new();
        let mut overlay: Option<DamageRect> = None;
        // Tail clips are conservatively refused. A later opaque pane may
        // cover the viewport, but a sampled or unbounded effect never may.
        let suffix = commands[tail..end]
            .iter()
            .map(|c| (c, true))
            .chain(commands[end + 1..].iter().map(|c| (c, false)));
        for (command, clipped) in suffix {
            let raw = ink(command, scale, text, cache)?;
            let bounds = if clipped {
                (
                    raw.0.max(viewport.0),
                    raw.1.max(viewport.1),
                    raw.2.min(viewport.2),
                    raw.3.min(viewport.3),
                )
            } else {
                raw
            };
            if clipped && (bounds.0 >= bounds.2 || bounds.1 >= bounds.3) {
                continue;
            }
            if intersects(bounds, viewport) {
                if !contains(viewport, bounds) {
                    return None;
                }
                overlay = Some(match overlay {
                    None => bounds,
                    Some(old) => (
                        old.0.min(bounds.0),
                        old.1.min(bounds.1),
                        old.2.max(bounds.2),
                        old.3.max(bounds.3),
                    ),
                });
            } else {
                outside.push(command.clone());
            }
        }
        let overlay = match overlay {
            Some(rect) => {
                // A narrow decoration fits a stable viewport-height strip.
                // Its raster work stays bounded, and changing thumb heights
                // can reuse storage immediately after the retirement grace.
                // Wider overlays keep the smaller grid-aligned backing.
                let strip_pixels = (rect.2 - rect.0) * (viewport.3 - viewport.1);
                let (top, bottom) = if strip_pixels <= 16 * 1024 {
                    (viewport.1, viewport.3)
                } else {
                    (
                        (rect.1.div_euclid(PATCH_GRID) * PATCH_GRID).max(viewport.1),
                        ((rect.3 + PATCH_GRID - 1).div_euclid(PATCH_GRID) * PATCH_GRID)
                            .min(viewport.3),
                    )
                };
                let rect = (rect.0, top, rect.2, bottom);
                Some((
                    rect,
                    software_patch::Scene::new(display, rect, scale, canvas, text, cache)?,
                ))
            }
            None => None,
        };
        Some((outside, overlay))
    }

    /// The row commands and their clips are unchanged. Only bounded paint
    /// above that partition may change; the outside picture stays exact.
    fn updated_decoration(
        &self,
        display: &DisplayList,
        physical: (usize, usize),
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        cache: &MeasureCache,
    ) -> Option<Option<OverlayPaint>> {
        let (start, tail, end) = self.decoration;
        let commands = display.as_slice();
        if physical != self.physical
            || scale != self.scale
            || canvas != self.canvas
            || commands.len() != self.source.len()
            || commands[..tail] != self.source.as_slice()[..tail]
            || !matches!(commands[end], DrawCommand::PopClip)
        {
            return None;
        }
        let (outside, overlay) = Self::decoration(
            display,
            (tail, end),
            self.viewport,
            scale,
            canvas,
            text,
            cache,
        )?;
        (outside.as_slice() == &self.outside.as_slice()[start..]).then_some(overlay)
    }

    fn compatible(&self, other: &Self) -> bool {
        self.viewport == other.viewport && self.outside.as_slice() == other.outside.as_slice()
    }

    fn translation(&self, old: &Self) -> Option<i64> {
        if !self.compatible(old) {
            return None;
        }
        // A changing scroll decoration need not search every pair of rows.
        // Equal pixels at equal positions prove the stationary partition.
        if self.bands.len() == old.bands.len()
            && self
                .bands
                .iter()
                .zip(&old.bands)
                .all(|(band, prior)| band.rect == prior.rect && band.same_pixels(prior))
        {
            return Some(0);
        }
        // Full pixel equality is the identity. Repeated labels and duplicate
        // rows are valid; they cannot select a wrong backing by key collision.
        let mut translations = HashMap::<i64, usize>::new();
        for band in &self.bands {
            for prior in &old.bands {
                if band.same_pixels(prior) {
                    *translations.entry(band.rect.1 - prior.rect.1).or_default() += 1;
                }
            }
        }
        translations
            .into_iter()
            .filter(|(_, count)| *count >= 2 && *count * 2 >= self.bands.len())
            .max_by_key(|(dy, count)| (*count, std::cmp::Reverse(dy.abs()), *dy))
            .map(|(dy, _)| dy)
    }
}

type RasterKey = (FontKey, [u8; 4], usize, String);
struct CachedRaster {
    pixels: TextRaster,
    used: u64,
}

#[derive(Default)]
struct RasterCache {
    entries: RefCell<HashMap<RasterKey, CachedRaster>>,
    bytes: Cell<usize>,
    tick: Cell<u64>,
}

struct CachedText<'a> {
    engine: &'a dyn TextEngine,
    cache: &'a RasterCache,
}
fn copy_raster(r: &TextRaster) -> TextRaster {
    TextRaster {
        width: r.width,
        height: r.height,
        baseline: r.baseline,
        rgba: r.rgba.clone(),
    }
}
impl TextEngine for CachedText<'_> {
    fn families(&self) -> Vec<std::sync::Arc<str>> {
        self.engine.families()
    }
    fn measure_line(&self, line: &str, font: &FontSpec) -> LineMetrics {
        self.engine.measure_line(line, font)
    }
    fn raster_line(
        &self,
        line: &str,
        font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Option<TextRaster> {
        let cache = self.cache;
        let tick = cache.tick.get().wrapping_add(1);
        cache.tick.set(tick);
        let key = (
            font.key(),
            [color.r, color.g, color.b, color.a],
            scale,
            line.to_owned(),
        );
        let mut entries = cache.entries.borrow_mut();
        if let Some(raster) = entries.get_mut(&key) {
            raster.used = tick;
            return Some(copy_raster(&raster.pixels));
        }
        let raster = self.engine.raster_line(line, font, color, scale)?;
        const LIMIT: usize = 2 * 1024 * 1024;
        if raster.rgba.len() <= LIMIT {
            while cache.bytes.get() + raster.rgba.len() > LIMIT || entries.len() >= 256 {
                let key = entries
                    .iter()
                    .min_by_key(|(_, raster)| raster.used)
                    .map(|(key, _)| key.clone())?;
                let old = entries.remove(&key)?;
                cache.bytes.set(cache.bytes.get() - old.pixels.rgba.len());
            }
            cache.bytes.set(cache.bytes.get() + raster.rgba.len());
            entries.insert(
                key,
                CachedRaster {
                    pixels: copy_raster(&raster),
                    used: tick,
                },
            );
        }
        Some(raster)
    }
}

struct Retired {
    surface: software_patch::Surface,
    frame: u64,
    at: Instant,
    observed_use: bool,
}
#[derive(Default)]
struct Surfaces {
    retired: Vec<Retired>,
    frame: u64,
}
impl Surfaces {
    fn retire(&mut self, surface: software_patch::Surface, observed_use: bool) {
        // Retain recent dimensions when the scene evolves. Releasing an
        // obsolete reference is safe even while CA still owns that surface;
        // reuse below still requires observed use and the full grace period.
        if self.retired.len() == 12 {
            self.retired.remove(0);
        }
        self.retired.push(Retired {
            surface,
            frame: self.frame,
            at: Instant::now(),
            observed_use,
        });
    }
    fn prepare(&mut self, bitmap: &Bitmap) -> Option<software_patch::Surface> {
        self.prepare_pixels(software_patch::PixelRegion::whole(bitmap)?)
    }

    fn prepare_pixels(
        &mut self,
        pixels: software_patch::PixelRegion<'_>,
    ) -> Option<software_patch::Surface> {
        let size = pixels.size();
        // A frequently reused size must not pin unrelated obsolete sizes.
        // Expiration needs only advancing frames, not an idle timer. Dropping
        // our reference never overwrites pixels still retained by CA.
        self.retired
            .retain(|retired| self.frame.saturating_sub(retired.frame) <= 12);
        // As with drawable retirement, let a presentation land before
        // trusting its cross-process use count. An unobserved surface
        // might still be pending: it is released, never overwritten. The
        // visible layer keeps ownership until removal's CA commit.
        for retired in &mut self.retired {
            retired.observed_use = retired.observed_use || retired.surface.busy();
        }
        let free = self.retired.iter().position(|r| {
            r.observed_use
                && r.surface.size == size
                && self.frame.saturating_sub(r.frame) >= 3
                && r.at.elapsed() >= Duration::from_millis(100)
                && !r.surface.busy()
        });
        let surface = match free {
            Some(at) => self.retired.swap_remove(at).surface,
            None => software_patch::Surface::new_opaque(size)?,
        };
        surface.write_pixels(pixels).then_some(surface)
    }
}

struct Layer {
    raw: Id,
    surface: Option<software_patch::Surface>,
    observed_use: bool,
    inset: Option<Box<Layer>>,
}
impl Layer {
    unsafe fn new(scale: usize) -> Option<Self> {
        unsafe {
            let raw = msg_id(msg_id(class("CALayer"), sel("alloc")), sel("init"));
            if raw.is_null() {
                return None;
            }
            kill_layer_actions(raw);
            msg_void_f64(raw, sel("setContentsScale:"), scale as f64);
            msg_void_bool(raw, sel("setOpaque:"), 1);
            Some(Self {
                raw,
                surface: None,
                observed_use: false,
                inset: None,
            })
        }
    }
}
impl Layer {
    /// Prepare a detached row: a device-RGB color with an opaque cropped
    /// child. `surface` remains owned by this row's normal retirement path.
    unsafe fn set_inset(
        &mut self,
        color: Color,
        bounds: DamageRect,
        physical: (usize, usize),
        scale: usize,
        surface: Id,
    ) -> bool {
        unsafe {
            let Some(child) = Layer::new(scale) else {
                return false;
            };
            let space = crate::ffi::CGColorSpaceCreateDeviceRGB();
            if space.is_null() {
                return false;
            }
            let components =
                [color.r, color.g, color.b, color.a].map(|channel| channel as f64 / 255.0);
            let native_color = crate::ffi::CGColorCreate(space, components.as_ptr());
            crate::ffi::CGColorSpaceRelease(space);
            if native_color.is_null() {
                return false;
            }
            msg_void_id(self.raw, sel("setBackgroundColor:"), native_color);
            CFRelease(native_color);
            msg_void_id(child.raw, sel("setContents:"), surface);
            msg_void_rect(
                child.raw,
                sel("setFrame:"),
                Patch::frame(bounds, physical, scale),
            );
            msg_void_id(self.raw, sel("addSublayer:"), child.raw);
            self.inset = Some(Box::new(child));
            true
        }
    }
}

impl Drop for Layer {
    fn drop(&mut self) {
        unsafe {
            msg_void(self.raw, sel("removeFromSuperlayer"));
            msg_void_id(self.raw, sel("setContents:"), null_mut());
            msg_void(self.raw, sel("release"));
        }
    }
}

enum Prepared {
    Reuse(usize),
    Fresh(Layer),
}

struct Shown {
    band: Band,
    layer: Layer,
}
struct Overlay {
    layer: Layer,
}

#[derive(Default)]
struct Overlays {
    layers: Vec<Overlay>,
    // The layer painted by prior.overlay, not a failed trial scene.
    shown: Option<Id>,
    raster: Option<software_patch::Raster>,
}

enum PreparedOverlay {
    Hidden,
    Reuse(usize),
    Fresh(Box<Overlay>),
}

pub(super) struct Frame<'a> {
    pub root: Id,
    pub display: &'a DisplayList,
    pub physical: (usize, usize),
    pub scale: usize,
    pub canvas: Color,
    pub text: &'a dyn TextEngine,
    pub images: &'a dyn ImageEngine,
    pub boxes: &'a MeasureCache,
}

/// One strategy per presenter, with no work on a repeating frame.
#[derive(Default)]
pub(super) struct Presenter {
    prior: Option<Scene>,
    viewport: Option<Layer>,
    shown: Vec<Shown>,
    overlays: Overlays,
    cache: RasterCache,
    surfaces: Surfaces,
    offset: i64,
    active: bool,
}

impl Presenter {
    /// Seeds the same generic admission from the initial native frame.
    pub(super) fn seed(
        &mut self,
        display: &DisplayList,
        physical: (usize, usize),
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        boxes: &MeasureCache,
    ) -> bool {
        self.overlays.shown = None;
        self.prior = Scene::new(display, physical, scale, canvas, text, boxes);
        self.prior.is_some()
    }

    pub(super) fn active(&self) -> bool {
        self.active
    }

    /// Once the bands are visible, only this stationary picture is needed
    /// behind them. The partition has proved full opaque viewport coverage.
    pub(super) fn outside(&self) -> Option<&DisplayList> {
        self.prior
            .as_ref()
            .filter(|_| self.active)
            .map(|scene| &scene.outside)
    }

    pub(super) fn discard_trial(&mut self) {
        if !self.active {
            self.prior = None;
        }
    }

    /// Called in the transaction that presents the replacement whole frame.
    pub(super) unsafe fn hide(&mut self) {
        unsafe {
            if let Some(viewport) = &self.viewport {
                msg_void_bool(viewport.raw, sel("setHidden:"), 1);
            }
            for overlay in &self.overlays.layers {
                msg_void_bool(overlay.layer.raw, sel("setHidden:"), 1);
            }
        }
        self.active = false;
        self.prior = None;
        self.shown.clear();
        self.overlays = Overlays::default();
        self.cache = RasterCache::default();
        self.offset = 0;
    }

    fn prepare_overlay(
        overlays: &mut Overlays,
        surfaces: &mut Surfaces,
        previous: Option<&OverlayPaint>,
        paint: Option<&OverlayPaint>,
        scale: usize,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) -> Option<PreparedOverlay> {
        let Some((_, paint)) = paint else {
            overlays.raster = None;
            return Some(PreparedOverlay::Hidden);
        };
        // Patch coordinates already express the exact snapped pixel geometry.
        // Reuse the last presented picture before allocating/rasterizing a
        // bitmap; different commands still use the exact pixel cache below.
        if let Some(((_, prior), shown)) = previous.zip(overlays.shown)
            && paint.matches(prior)
            && let Some(at) = overlays.layers.iter().position(|o| o.layer.raw == shown)
        {
            return Some(PreparedOverlay::Reuse(at));
        }
        let bitmap = paint.raster_in(&mut overlays.raster, text, images);
        let pixels = software_patch::PixelRegion::whole(bitmap)?;
        if let Some(at) = overlays.layers.iter().position(|o| {
            o.layer
                .surface
                .as_ref()
                .is_some_and(|surface| surface.matches_pixels(pixels))
        }) {
            return Some(PreparedOverlay::Reuse(at));
        }
        let surface = surfaces.prepare(bitmap)?;
        let mut layer = unsafe { Layer::new(scale)? };
        unsafe { msg_void_id(layer.raw, sel("setContents:"), surface.raw) };
        layer.surface = Some(surface);
        Some(PreparedOverlay::Fresh(Box::new(Overlay { layer })))
    }

    unsafe fn show_overlay(
        &mut self,
        prepared: PreparedOverlay,
        rect: Option<DamageRect>,
        root: Id,
        viewport: Id,
        physical: (usize, usize),
        scale: usize,
    ) -> Option<(software_patch::Surface, bool)> {
        unsafe {
            let overlay = match prepared {
                PreparedOverlay::Hidden => None,
                PreparedOverlay::Reuse(at) => Some(at),
                PreparedOverlay::Fresh(fresh) => {
                    // Immediately above the viewport, below native islands.
                    crate::ffi::msg_void_id_id(
                        root,
                        sel("insertSublayer:above:"),
                        fresh.layer.raw,
                        viewport,
                    );
                    self.overlays.layers.push(*fresh);
                    Some(self.overlays.layers.len() - 1)
                }
            };
            self.overlays.shown = overlay.map(|at| self.overlays.layers[at].layer.raw);
            for (at, kept) in self.overlays.layers.iter().enumerate() {
                msg_void_bool(
                    kept.layer.raw,
                    sel("setHidden:"),
                    (Some(at) != overlay) as i8,
                );
                if Some(at) == overlay
                    && let Some(rect) = rect
                {
                    msg_void_rect(
                        kept.layer.raw,
                        sel("setFrame:"),
                        Patch::frame(rect, physical, scale),
                    );
                }
            }
            // Three exact pixel variants handle small movements and returns.
            if self.overlays.layers.len() > 3 {
                let at = (0..self.overlays.layers.len()).find(|at| Some(*at) != overlay)?;
                let mut old = self.overlays.layers.remove(at);
                return old
                    .layer
                    .surface
                    .take()
                    .map(|surface| (surface, old.layer.observed_use));
            }
            None
        }
    }

    fn advance_surfaces(&mut self) {
        self.surfaces.frame += 1;
        for shown in &mut self.shown {
            shown.layer.observed_use = shown.layer.observed_use
                || shown
                    .layer
                    .surface
                    .as_ref()
                    .is_some_and(|surface| surface.busy());
        }
        for overlay in &mut self.overlays.layers {
            overlay.layer.observed_use = overlay.layer.observed_use
                || overlay
                    .layer
                    .surface
                    .as_ref()
                    .is_some_and(|surface| surface.busy());
        }
    }

    /// Preparation is fallible and changes no visible layer. Only after all
    /// entering bands and the overlay have backing does one transaction move
    /// the viewport and replace its departing children.
    pub(super) unsafe fn present(&mut self, frame: Frame<'_>) -> bool {
        let Frame {
            root,
            display,
            physical,
            scale,
            canvas,
            text,
            images,
            boxes,
        } = frame;
        if self.active
            && let Some(overlay) = self.prior.as_ref().and_then(|prior| {
                prior.updated_decoration(display, physical, scale, canvas, text, boxes)
            })
        {
            // The complete row/clip prefix and outside picture are identical.
            // Reuse the partition and its layers; only update the decoration.
            let Some(viewport) = self.viewport.as_ref().map(|layer| layer.raw) else {
                return false;
            };
            self.advance_surfaces();
            let cached = CachedText {
                engine: text,
                cache: &self.cache,
            };
            let Some(prepared) = Self::prepare_overlay(
                &mut self.overlays,
                &mut self.surfaces,
                self.prior.as_ref().and_then(|prior| prior.overlay.as_ref()),
                overlay.as_ref(),
                scale,
                &cached,
                images,
            ) else {
                return false;
            };
            let retired = unsafe {
                let transaction = class("CATransaction");
                msg_void(transaction, sel("begin"));
                msg_void_bool(transaction, sel("setDisableActions:"), 1);
                let retired = self.show_overlay(
                    prepared,
                    overlay.as_ref().map(|(rect, _)| *rect),
                    root,
                    viewport,
                    physical,
                    scale,
                );
                msg_void(transaction, sel("commit"));
                retired
            };
            if let Some((surface, observed_use)) = retired {
                self.surfaces.retire(surface, observed_use);
            }
            if let Some(prior) = &mut self.prior {
                prior.source = display.clone();
                prior.overlay = overlay;
            }
            return true;
        }
        let Some(scene) = Scene::new(display, physical, scale, canvas, text, boxes) else {
            self.prior = None;
            return false;
        };
        let prior = self.prior.as_ref();
        if !prior.is_some_and(|prior| scene.compatible(prior)) {
            self.overlays.shown = None;
            self.prior = Some(scene);
            return false;
        }
        let translation = prior.and_then(|prior| scene.translation(prior));
        let dy = translation.unwrap_or(0);
        if translation.is_none() {
            // A stationary list may append or edit a few rows. It must not
            // turn a broad filter/replacement into a CPU repaint of every row.
            let pixels: usize = scene
                .bands
                .iter()
                .filter(|band| {
                    !self
                        .shown
                        .iter()
                        .any(|old| old.band.rect == band.rect && band.same_pixels(&old.band))
                })
                .map(Band::raster_pixels)
                .sum();
            if pixels > MAX_PIXELS {
                self.overlays.shown = None;
                self.prior = Some(scene);
                return false;
            }
        }
        let offset = if self.active { self.offset + dy } else { 0 };
        // Periodically rebase through the normal whole frame instead of
        // letting layer coordinates grow without bound on a long scroll.
        if offset.unsigned_abs() > 1_000_000 {
            self.prior = None;
            return false;
        }
        self.advance_surfaces();
        let cached = CachedText {
            engine: text,
            cache: &self.cache,
        };
        // A stationary row already retains its rendered pixels in its layer.
        // Cache source rasters only while translating rows, where labels may
        // recur across entering bands; do not keep a second stationary copy.
        let row_text: &dyn TextEngine = if dy == 0 { text } else { &cached };
        let mut matched = vec![false; self.shown.len()];
        let mut plan = Vec::with_capacity(scene.bands.len());
        for (index, band) in scene.bands.iter().enumerate() {
            let matches = |i: usize, shown: &Shown| {
                !matched[i]
                    && band.rect.1 == shown.band.rect.1 + dy
                    && band.same_pixels(&shown.band)
            };
            let old = self
                .shown
                .get(index)
                .filter(|shown| matches(index, shown))
                .map(|_| index)
                .or_else(|| {
                    self.shown
                        .iter()
                        .enumerate()
                        .position(|(i, shown)| matches(i, shown))
                });
            if let Some(at) = old {
                matched[at] = true;
                plan.push(Prepared::Reuse(at));
                continue;
            }
            // Cropping saves retained pixels for stationary lists. During
            // motion, whole entering rows avoid repeating that analysis.
            let foreground = (dy == 0)
                .then(|| band.foreground(scale, row_text, boxes))
                .flatten();
            let bitmap = match &foreground {
                Some((_, scene)) => scene.raster(row_text, images),
                None => match band.solid() {
                    Some(color) => Bitmap::new(1, 1, color),
                    None => rasterize_with(
                        &band.display,
                        (band.rect.2 - band.rect.0) as usize,
                        (band.rect.3 - band.rect.1) as usize,
                        scale,
                        canvas,
                        row_text,
                        images,
                    ),
                },
            };
            let (region, inset) = match foreground {
                Some((color, scene)) => {
                    let crop = foreground_ink(&bitmap, color);
                    let bounds = scene.bounds();
                    let rect = (
                        bounds.0 + crop.0 as i64,
                        bounds.1 + crop.1 as i64,
                        bounds.0 + crop.2 as i64,
                        bounds.1 + crop.3 as i64,
                    );
                    (
                        software_patch::PixelRegion::new(&bitmap, crop),
                        Some((color, rect)),
                    )
                }
                None => (software_patch::PixelRegion::whole(&bitmap), None),
            };
            let Some(surface) = region.and_then(|pixels| self.surfaces.prepare_pixels(pixels))
            else {
                return false;
            };
            let Some(mut layer) = (unsafe { Layer::new(scale) }) else {
                return false;
            };
            unsafe {
                if let Some((color, bounds)) = inset {
                    let physical = (
                        (band.rect.2 - band.rect.0) as usize,
                        (band.rect.3 - band.rect.1) as usize,
                    );
                    if !layer.set_inset(color, bounds, physical, scale, surface.raw) {
                        return false;
                    }
                } else {
                    msg_void_id(layer.raw, sel("setContents:"), surface.raw);
                }
            }
            layer.surface = Some(surface);
            plan.push(Prepared::Fresh(layer));
        }
        let Some(overlay) = Self::prepare_overlay(
            &mut self.overlays,
            &mut self.surfaces,
            self.prior.as_ref().and_then(|prior| prior.overlay.as_ref()),
            scene.overlay.as_ref(),
            scale,
            &cached,
            images,
        ) else {
            return false;
        };
        if self.viewport.is_none() {
            let Some(layer) = (unsafe { Layer::new(scale) }) else {
                return false;
            };
            self.viewport = Some(layer);
        }
        let Some(viewport) = &self.viewport else {
            return false;
        };
        let viewport_raw = viewport.raw;
        let mut retired = Vec::new();
        unsafe {
            let transaction = class("CATransaction");
            msg_void(transaction, sel("begin"));
            msg_void_bool(transaction, sel("setDisableActions:"), 1);
            if !self.active {
                msg_void_bool(viewport_raw, sel("setMasksToBounds:"), 1);
                msg_void_f64(viewport_raw, sel("setContentsScale:"), scale as f64);
                msg_void_rect(
                    viewport_raw,
                    sel("setFrame:"),
                    Patch::frame(scene.viewport, physical, scale),
                );
                msg_void_id_u64(root, sel("insertSublayer:atIndex:"), viewport_raw, 0);
            }
            let factor = scale as f64;
            msg_void_rect(
                viewport_raw,
                sel("setBounds:"),
                CGRect {
                    origin: CGPoint {
                        x: 0.0,
                        y: offset as f64 / factor,
                    },
                    size: CGSize {
                        width: (scene.viewport.2 - scene.viewport.0) as f64 / factor,
                        height: (scene.viewport.3 - scene.viewport.1) as f64 / factor,
                    },
                },
            );
            msg_void_bool(viewport_raw, sel("setHidden:"), 0);
            let mut old: Vec<_> = self.shown.drain(..).map(Some).collect();
            let bands = scene.bands.iter().map(|b| Band {
                rect: b.rect,
                display: b.display.clone(),
            });
            for (band, choice) in bands.zip(plan) {
                let layer = match choice {
                    Prepared::Reuse(at) => {
                        let Some(shown) = old[at].take() else {
                            unreachable!("each matched band is consumed once");
                        };
                        shown.layer
                    }
                    Prepared::Fresh(layer) => {
                        let rect = (
                            0,
                            band.rect.1 - scene.viewport.1 - offset,
                            band.rect.2 - band.rect.0,
                            band.rect.3 - scene.viewport.1 - offset,
                        );
                        msg_void_rect(
                            layer.raw,
                            sel("setFrame:"),
                            Patch::frame(
                                rect,
                                (
                                    (scene.viewport.2 - scene.viewport.0) as usize,
                                    (scene.viewport.3 - scene.viewport.1) as usize,
                                ),
                                scale,
                            ),
                        );
                        msg_void_id(viewport_raw, sel("addSublayer:"), layer.raw);
                        layer
                    }
                };
                self.shown.push(Shown { band, layer });
            }
            for mut shown in old.into_iter().flatten() {
                if let Some(surface) = shown.layer.surface.take() {
                    retired.push((surface, shown.layer.observed_use));
                }
                drop(shown);
            }
            if let Some(surface) = self.show_overlay(
                overlay,
                scene.overlay.as_ref().map(|(rect, _)| *rect),
                root,
                viewport_raw,
                physical,
                scale,
            ) {
                retired.push(surface);
            }
            msg_void(transaction, sel("commit"));
        }
        for (surface, observed_use) in retired {
            self.surfaces.retire(surface, observed_use);
        }
        self.offset = offset;
        self.prior = Some(scene);
        self.active = true;
        crate::trace::mark(
            "Q",
            format_args!("scroll-bands rows={} dy={dy}", self.shown.len()),
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bunny_ui::layout::{Corners, Point};
    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            origin: Point { x, y },
            size: Size { width, height },
        }
    }
    use bunny_ui::image_engine::RawImages;
    use bunny_ui::text_engine::PixelFont;

    #[test]
    fn unchanged_decoration_pixels_do_not_raster_again() {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            for scale in [1, 2] {
                let root = msg_id(msg_id(class("CALayer"), sel("alloc")), sel("init"));
                let boxes = MeasureCache::default();
                let images = RawImages::default();
                let mut presenter = Presenter::default();
                let physical = (180 * scale, 130 * scale);
                let mut commands = scene(0.0, false).as_slice().to_vec();
                commands.push(DrawCommand::TextLine {
                    origin: Point { x: 154.5, y: 50.5 },
                    content: "X".into(),
                    range: (0, 1),
                    color: Color::BLACK,
                    font: FontSpec::DEFAULT,
                });
                let display = DisplayList::from(commands.clone());
                assert!(presenter.seed(
                    &display,
                    physical,
                    scale,
                    Color::WHITE,
                    &PixelFont,
                    &boxes
                ));
                macro_rules! frame {
                    ($display:expr) => {
                        Frame {
                            root,
                            display: $display,
                            physical,
                            scale,
                            canvas: Color::WHITE,
                            text: &PixelFont,
                            images: &images,
                            boxes: &boxes,
                        }
                    };
                }
                assert!(presenter.present(frame!(&display)));
                let raster_requests = presenter.cache.tick.get();
                assert!(raster_requests > 0, "the overlay actually rasterizes text");
                for delta in [0.0, 0.01, 0.02] {
                    let DrawCommand::FillRect { rect, .. } = &mut commands[display.len() - 2]
                    else {
                        unreachable!()
                    };
                    rect.size.height = 20.0 + delta;
                    let next = DisplayList::from(commands.clone());
                    assert!(presenter.present(frame!(&next)));
                    assert_eq!(
                        presenter.cache.tick.get(),
                        raster_requests,
                        "a repeated physical picture must not replay its text raster"
                    );
                    assert_eq!(presenter.overlays.layers.len(), 1);
                }
                // A smaller thumb no longer touches the separate glyph below it.
                for height in [10.0, 9.0] {
                    let DrawCommand::FillRect { rect, .. } = &mut commands[display.len() - 2]
                    else {
                        unreachable!()
                    };
                    rect.size.height = height;
                    let next = DisplayList::from(commands.clone());
                    let before = presenter.cache.tick.get();
                    assert!(presenter.present(frame!(&next)));
                    if height == 9.0 {
                        assert_eq!(
                            presenter.cache.tick.get(),
                            before,
                            "small decoration damage must not replay a distant glyph"
                        );
                    }
                }
                let DrawCommand::TextLine { color, .. } = commands.last_mut().unwrap() else {
                    unreachable!()
                };
                *color = Color::hex(0xff0000);
                let changed = DisplayList::from(commands);
                assert!(presenter.present(frame!(&changed)));
                assert!(
                    presenter.cache.tick.get() > raster_requests,
                    "changed paint must still rasterize"
                );
                assert!(presenter.overlays.layers.len() <= 3);
                drop(presenter);
                msg_void(root, sel("release"));
            }
            objc_autoreleasePoolPop(pool);
        }
    }

    #[test]
    fn stationary_row_surfaces_do_not_keep_duplicate_text_rasters() {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let root = msg_id(msg_id(class("CALayer"), sel("alloc")), sel("init"));
            let boxes = MeasureCache::default();
            let images = RawImages::default();
            let mut presenter = Presenter::default();
            assert!(presenter.seed(
                &growing_scene(0),
                (800, 600),
                1,
                Color::WHITE,
                &PixelFont,
                &boxes
            ));
            for rows in [1, 2, 3] {
                assert!(presenter.present(Frame {
                    root,
                    display: &growing_scene(rows),
                    physical: (800, 600),
                    scale: 1,
                    canvas: Color::WHITE,
                    text: &PixelFont,
                    images: &images,
                    boxes: &boxes
                }));
            }
            assert_eq!(
                presenter.cache.bytes.get(),
                0,
                "unchanged stationary rows already own their visible pixels"
            );
            drop(presenter);
            msg_void(root, sel("release"));
            objc_autoreleasePoolPop(pool);
        }
    }

    #[test]
    fn unused_retired_dimensions_expire_as_the_scene_advances() {
        let mut pool = Surfaces::default();
        pool.retire(software_patch::Surface::new_opaque((7, 2)).unwrap(), true);
        pool.frame = 20;
        let _ = pool.prepare(&Bitmap::new(3, 2, Color::WHITE)).unwrap();
        assert!(
            pool.retired.is_empty(),
            "obsolete dimensions should not remain pinned by a busy cache of another size"
        );
    }

    #[test]
    fn retired_surfaces_follow_recent_sizes_instead_of_freezing_the_pool() {
        let mut pool = Surfaces::default();
        for width in 1..=12 {
            pool.retire(
                software_patch::Surface::new_opaque((width, 2)).unwrap(),
                true,
            );
        }
        let recent = software_patch::Surface::new_opaque((20, 2)).unwrap();
        let raw = recent.raw;
        // Prevent address recycling from hiding a discarded surface.
        unsafe { crate::ffi::CFRetain(raw) };
        pool.retire(recent, true);
        // Model compositor use that has ended and the required grace period;
        // no wall-clock wait or concurrent reader is involved in this test.
        pool.frame = 4;
        for retired in &mut pool.retired {
            retired.at = Instant::now() - Duration::from_secs(1);
        }
        let reused = pool.prepare(&Bitmap::new(20, 2, Color::WHITE)).unwrap();
        unsafe { CFRelease(raw) };
        assert_eq!(
            reused.raw, raw,
            "recent compatible storage must remain reusable when the pool fills"
        );
        assert!(pool.retired.len() < 12);
    }

    fn scene(offset: f64, duplicate: bool) -> DisplayList {
        let mut commands = vec![
            DrawCommand::FillRect {
                rect: rect(0.0, 0.0, 180.0, 130.0),
                color: Color::WHITE,
                corner_radius: Corners::ZERO,
            },
            DrawCommand::PushClip {
                rect: rect(10.5, 20.5, 160.0, 96.0),
                corner_radius: Corners::ZERO,
            },
        ];
        for row in 0..7 {
            let y = 20.5 + row as f64 * 24.0 - offset;
            commands.push(DrawCommand::FillRect {
                rect: rect(10.5, y, 160.0, 24.0),
                color: Color::hex(if row % 2 == 0 { 0xeeeeee } else { 0xcccccc }),
                corner_radius: Corners::ZERO,
            });
            commands.push(DrawCommand::TextLine {
                origin: Point {
                    x: 16.5,
                    y: y + 4.0,
                },
                content: if duplicate {
                    "same".into()
                } else {
                    format!("row {row}").into()
                },
                range: (0, if duplicate { 4 } else { 5 }),
                color: Color::BLACK,
                font: FontSpec::DEFAULT,
            });
        }
        commands.push(DrawCommand::PopClip);
        commands.push(DrawCommand::FillRect {
            rect: rect(162.5, 28.5 + offset / 10.0, 4.0, 20.0),
            color: Color {
                r: 40,
                g: 40,
                b: 40,
                a: 120,
            },
            corner_radius: Corners::ZERO,
        });
        DisplayList::from(commands)
    }

    fn growing_scene(rows: usize) -> DisplayList {
        let mut commands = vec![
            DrawCommand::FillRect {
                rect: rect(0.0, 0.0, 800.0, 600.0),
                color: Color::WHITE,
                corner_radius: Corners::ZERO,
            },
            DrawCommand::PushClip {
                rect: rect(16.0, 16.0, 768.0, 560.0),
                corner_radius: Corners::ZERO,
            },
        ];
        for row in 0..rows {
            let y = 16.0 + row as f64 * 28.0;
            commands.push(DrawCommand::FillRect {
                rect: rect(16.0, y, 768.0, 28.0),
                color: Color::hex(0xeeeeee),
                corner_radius: Corners::ZERO,
            });
            commands.push(DrawCommand::TextLine {
                origin: Point {
                    x: 24.0,
                    y: y + 4.0,
                },
                content: "new line".into(),
                range: (0, 8),
                color: Color::BLACK,
                font: FontSpec::DEFAULT,
            });
        }
        commands.push(DrawCommand::PopClip);
        DisplayList::from(commands)
    }

    #[test]
    fn a_growing_list_keeps_its_proved_empty_space_without_missing_pixels() {
        let cache = MeasureCache::default();
        for scale in [1, 2] {
            for rows in [0, 1, 2, 10, 20] {
                let display = growing_scene(rows);
                let size = (800 * scale, 600 * scale);
                let admitted = Scene::new(&display, size, scale, Color::WHITE, &PixelFont, &cache)
                    .expect("the uncovered prefix background is uniform");
                let whole = rasterize_with(
                    &display,
                    size.0,
                    size.1,
                    scale,
                    Color::WHITE,
                    &PixelFont,
                    &RawImages::default(),
                );
                let mut composed = whole.pixels().to_vec();
                // Poison the viewport first: the bands must cover ALL of it,
                // including the empty tail, rather than relying on old pixels.
                let vp = admitted.viewport;
                for y in vp.1..vp.3 {
                    for x in vp.0..vp.2 {
                        composed[y as usize * size.0 + x as usize] = 0;
                    }
                }
                for band in &admitted.bands {
                    let tile = rasterize_with(
                        &band.display,
                        (band.rect.2 - band.rect.0) as usize,
                        (band.rect.3 - band.rect.1) as usize,
                        scale,
                        Color::WHITE,
                        &PixelFont,
                        &RawImages::default(),
                    );
                    for y in band.rect.1.max(vp.1)..band.rect.3.min(vp.3) {
                        for x in vp.0..vp.2 {
                            composed[y as usize * size.0 + x as usize] =
                                tile.pixels()[(y - band.rect.1) as usize * tile.width()
                                    + (x - band.rect.0) as usize];
                        }
                    }
                }
                assert_eq!(composed, whole.pixels(), "scale={scale}, rows={rows}");
            }
        }
    }

    #[test]
    fn a_wide_row_keeps_only_its_foreground_pixels() {
        let cache = MeasureCache::default();
        for scale in [1, 2] {
            let display = growing_scene(1);
            let scene = Scene::new(
                &display,
                (800 * scale, 600 * scale),
                scale,
                Color::WHITE,
                &PixelFont,
                &cache,
            )
            .unwrap();
            let band = &scene.bands[0];
            let (color, foreground) = band
                .foreground(scale, &PixelFont, &cache)
                .expect("the short text leaves most of its row uniform");
            let physical = (
                (band.rect.2 - band.rect.0) as usize,
                (band.rect.3 - band.rect.1) as usize,
            );
            let whole = rasterize_with(
                &band.display,
                physical.0,
                physical.1,
                scale,
                Color::WHITE,
                &PixelFont,
                &RawImages::default(),
            );
            let patch = foreground.raster(&PixelFont, &RawImages::default());
            assert!(patch.width() * patch.height() * 2 < whole.width() * whole.height());
            let mut composed = Bitmap::new(physical.0, physical.1, color).pixels().to_vec();
            let (x0, y0, _, _) = foreground.bounds();
            let crop = foreground_ink(&patch, color);
            assert!(
                (crop.2 - crop.0) * (crop.3 - crop.1) < patch.width() * patch.height(),
                "conservative glyph bounds contain removable padding"
            );
            for y in crop.1..crop.3 {
                for x in crop.0..crop.2 {
                    composed[(y0 as usize + y) * physical.0 + x0 as usize + x] =
                        patch.pixels()[y * patch.width() + x];
                }
            }
            assert_eq!(composed, whole.pixels());
        }
    }

    #[test]
    fn uncovered_text_and_nonuniform_backgrounds_are_not_empty_space() {
        let cache = MeasureCache::default();
        for rows in [0, 1, 2] {
            for inside in [false, true] {
                let mut commands = growing_scene(rows).as_slice().to_vec();
                commands.insert(
                    if inside { 2 } else { 1 },
                    DrawCommand::FillRect {
                        rect: rect(20.0, 400.0, 40.0, 30.0),
                        color: Color::BLACK,
                        corner_radius: Corners::ZERO,
                    },
                );
                assert!(
                    Scene::new(
                        &DisplayList::from(commands),
                        (800, 600),
                        1,
                        Color::WHITE,
                        &PixelFont,
                        &cache
                    )
                    .is_none(),
                    "rows={rows}, inside={inside}"
                );
            }
        }
    }

    #[test]
    fn composed_bands_match_whole_pixels_at_two_scales_and_fractional_origins() {
        let cache = MeasureCache::default();
        for scale in [1, 2] {
            for offset in [0.0, 0.5, 17.0, 24.5, 48.0] {
                let display = scene(offset, false);
                let size = (180 * scale, 130 * scale);
                let admitted = Scene::new(&display, size, scale, Color::WHITE, &PixelFont, &cache)
                    .expect("opaque bands");
                let whole = rasterize_with(
                    &display,
                    size.0,
                    size.1,
                    scale,
                    Color::WHITE,
                    &PixelFont,
                    &RawImages::default(),
                );
                let mut composed = whole.pixels().to_vec();
                let vp = admitted.viewport;
                for band in &admitted.bands {
                    let tile = rasterize_with(
                        &band.display,
                        (band.rect.2 - band.rect.0) as usize,
                        (band.rect.3 - band.rect.1) as usize,
                        scale,
                        Color::WHITE,
                        &PixelFont,
                        &RawImages::default(),
                    );
                    for y in band.rect.1.max(vp.1)..band.rect.3.min(vp.3) {
                        for x in vp.0..vp.2 {
                            composed[y as usize * size.0 + x as usize] =
                                tile.pixels()[(y - band.rect.1) as usize * tile.width()
                                    + (x - band.rect.0) as usize];
                        }
                    }
                }
                if let Some((rect, overlay)) = &admitted.overlay {
                    let tile = overlay.raster(&PixelFont, &RawImages::default());
                    for y in rect.1..rect.3 {
                        for x in rect.0..rect.2 {
                            composed[y as usize * size.0 + x as usize] = tile.pixels()
                                [(y - rect.1) as usize * tile.width() + (x - rect.0) as usize];
                        }
                    }
                }
                assert_eq!(composed, whole.pixels(), "scale={scale} offset={offset}");
            }
        }
    }

    #[test]
    fn decoration_updates_preserve_the_partition_and_refuse_changed_rows() {
        let cache = MeasureCache::default();
        for scale in [1, 2] {
            let first = scene(0.0, false);
            let prior = Scene::new(
                &first,
                (180 * scale, 130 * scale),
                scale,
                Color::WHITE,
                &PixelFont,
                &cache,
            )
            .unwrap();
            let mut commands = first.as_slice().to_vec();
            let Some(DrawCommand::FillRect { rect: thumb, .. }) = commands.last_mut() else {
                unreachable!()
            };
            thumb.size.height -= 1.0;
            let second = DisplayList::from(commands.clone());
            let update = prior
                .updated_decoration(
                    &second,
                    (180 * scale, 130 * scale),
                    scale,
                    Color::WHITE,
                    &PixelFont,
                    &cache,
                )
                .expect("only the decoration changed");
            let (bounds, paint) = update.unwrap();
            let whole = rasterize_with(
                &second,
                180 * scale,
                130 * scale,
                scale,
                Color::WHITE,
                &PixelFont,
                &RawImages::default(),
            );
            let bitmap = paint.raster(&PixelFont, &RawImages::default());
            for y in bounds.1..bounds.3 {
                for x in bounds.0..bounds.2 {
                    assert_eq!(
                        whole.pixels()[y as usize * whole.width() + x as usize],
                        bitmap.pixels()
                            [(y - bounds.1) as usize * bitmap.width() + (x - bounds.0) as usize]
                    );
                }
            }
            let DrawCommand::TextLine { color, .. } = &mut commands[3] else {
                unreachable!()
            };
            *color = Color::WHITE;
            assert!(
                prior
                    .updated_decoration(
                        &DisplayList::from(commands),
                        (180 * scale, 130 * scale),
                        scale,
                        Color::WHITE,
                        &PixelFont,
                        &cache
                    )
                    .is_none(),
                "row changes require a new partition proof"
            );
            let mut commands = first.as_slice().to_vec();
            let Some(DrawCommand::FillRect { rect: r, .. }) = commands.last_mut() else {
                unreachable!()
            };
            *r = rect(0.0, 0.0, 4.0, 4.0);
            assert!(
                prior
                    .updated_decoration(
                        &DisplayList::from(commands),
                        (180 * scale, 130 * scale),
                        scale,
                        Color::WHITE,
                        &PixelFont,
                        &cache
                    )
                    .is_none(),
                "changes outside the viewport require a whole scene proof"
            );
        }
    }

    #[test]
    fn a_one_pixel_thumb_change_keeps_the_overlay_backing_size() {
        let cache = MeasureCache::default();
        let first = scene(0.0, false);
        let mut second = first.as_slice().to_vec();
        let Some(DrawCommand::FillRect { rect, .. }) = second.last_mut() else {
            unreachable!()
        };
        rect.size.height -= 1.0;
        let a = Scene::new(&first, (180, 130), 1, Color::WHITE, &PixelFont, &cache).unwrap();
        let b = Scene::new(
            &DisplayList::from(second),
            (180, 130),
            1,
            Color::WHITE,
            &PixelFont,
            &cache,
        )
        .unwrap();
        let (a_bounds, a_paint) = a.overlay.unwrap();
        let (b_bounds, b_paint) = b.overlay.unwrap();
        assert_eq!(
            a_bounds, b_bounds,
            "the surface can be reused across small geometry changes"
        );
        assert_ne!(
            a_paint.raster(&PixelFont, &RawImages::default()).pixels(),
            b_paint.raster(&PixelFont, &RawImages::default()).pixels(),
            "the exact thumb pixels still change"
        );
    }

    #[test]
    fn row_reuse_tracks_translation_in_both_directions_including_duplicate_rows() {
        let cache = MeasureCache::default();
        for duplicate in [false, true] {
            let a = Scene::new(
                &scene(0.0, duplicate),
                (180, 130),
                1,
                Color::WHITE,
                &PixelFont,
                &cache,
            )
            .expect("first");
            let b = Scene::new(
                &scene(24.0, duplicate),
                (180, 130),
                1,
                Color::WHITE,
                &PixelFont,
                &cache,
            )
            .expect("second");
            assert_eq!(b.translation(&a), Some(-24));
            assert_eq!(a.translation(&b), Some(24));
            let mut changed = scene(24.0, duplicate).as_slice().to_vec();
            if let DrawCommand::TextLine { color, .. } = &mut changed[3] {
                *color = Color::hex(0xff0011);
            }
            let changed = Scene::new(
                &DisplayList::from(changed),
                (180, 130),
                1,
                Color::WHITE,
                &PixelFont,
                &cache,
            )
            .expect("local change");
            assert!(!changed.bands[0].same_pixels(&b.bands[0]));
        }
    }

    #[test]
    fn gaps_unbounded_ink_and_rounded_viewports_stay_on_metal() {
        let cache = MeasureCache::default();
        for case in 0..3 {
            let mut commands = scene(0.0, false).as_slice().to_vec();
            match case {
                0 => {
                    if let DrawCommand::FillRect { rect, .. } = &mut commands[4] {
                        rect.origin.y += 1.0;
                    }
                }
                1 => {
                    if let DrawCommand::TextLine { origin, .. } = &mut commands[3] {
                        origin.y -= 10.0;
                    }
                }
                _ => {
                    if let DrawCommand::PushClip { corner_radius, .. } = &mut commands[1] {
                        *corner_radius = 8.0.into();
                    }
                }
            }
            assert!(
                Scene::new(
                    &DisplayList::from(commands),
                    (180, 130),
                    1,
                    Color::WHITE,
                    &PixelFont,
                    &cache
                )
                .is_none(),
                "case={case}"
            );
        }
    }
    #[test]
    fn the_runtime_virtual_list_uses_the_same_generic_band_partition() {
        use bunny_ui::prelude::*;
        #[derive(Clone)]
        struct List;
        impl Component for List {
            fn body(self) -> impl View {
                vstack!(
                    text!("Records").frame_height(40.0),
                    virtual_list(
                        1000,
                        |row| format!("item-{row}"),
                        |row| {
                            hstack!(text!("Record {row}").frame_width(180.0), text!("Ready"))
                                .spacing(8.0)
                                .padding_edge(Edge::Leading, 12.0)
                                .frame_aligned(400.0, 24.0, Alignment::Leading)
                                .background_color(Color::hex(if row % 2 == 0 {
                                    0xeeeeee
                                } else {
                                    0xcccccc
                                }))
                        }
                    )
                    .row_height(24.0),
                )
                .spacing(0.0)
                .font_size(12.0)
                .foreground_color(Color::BLACK)
                .background_color(Color::WHITE)
            }
        }
        let text = Rc::new(crate::text::CoreTextEngine::new());
        let runtime = Runtime::new().text_engine(text.clone());
        runtime.drop_unseen();
        let size = Size {
            width: 400.0,
            height: 300.0,
        };
        let first = runtime.settled_layout(&List, bunny_ui::layout::Proposal::exact(size));
        let path = first.scrolls[0].path.clone();
        let cache = MeasureCache::default();
        for offset in [0.0, 6.0, 24.0, 180.0, 24.0] {
            runtime.set_scroll_offset(&path, Point { x: 0.0, y: offset });
            let display = runtime.display_frame(&List, size);
            for scale in [1, 2] {
                assert!(
                    Scene::new(
                        &display,
                        (400 * scale, 300 * scale),
                        scale,
                        Color::WHITE,
                        &*text,
                        &cache
                    )
                    .is_some(),
                    "runtime list refused at scale={scale}, offset={offset}: {display:?}"
                );
            }
        }
    }
}
