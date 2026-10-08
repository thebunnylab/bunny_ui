//! Opaque bands can scroll by moving their retained pixels. Admission is
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
        if (rect.2 - rect.0) * (rect.3 - rect.1) > MAX_PIXELS as i64 {
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

    fn same_pixels(&self, other: &Self) -> bool {
        self.rect.2 - self.rect.0 == other.rect.2 - other.rect.0
            && self.rect.3 - self.rect.1 == other.rect.3 - other.rect.1
            && self.display.as_slice() == other.display.as_slice()
    }
}

/// A proved partition: the viewport is completely covered by opaque,
/// disjoint bands. Everything above them either stays outside or is a
/// bounded overlay painted from the complete scene.
struct Scene {
    viewport: DamageRect,
    bands: Vec<Band>,
    outside: DisplayList,
    overlay: Option<(DamageRect, software_patch::Scene)>,
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
                }
                _ => {}
            }
            if bands.len() >= MAX_BANDS {
                return None;
            }
        }
        let end = end?;
        let first = first?;
        // A scroll decoration can be inside the viewport's final clip.
        // Its geometry, not its role or size, separates it from the last
        // opaque band. The suffix paints above every band in either case.
        let last_rect = ink(commands.get(first)?, scale, text, cache)?;
        let mut tail = end;
        let mut depth = 0usize;
        for (at, command) in commands.iter().enumerate().take(end).skip(first + 1) {
            match command {
                DrawCommand::PushClip { .. } => depth += 1,
                DrawCommand::PopClip => depth = depth.checked_sub(1)?,
                _ if depth == 0 && !contains(last_rect, ink(command, scale, text, cache)?) => {
                    tail = at;
                    break;
                }
                _ => {}
            }
        }
        bands.push(Band::new(&commands[first..tail], scale, text, cache)?);
        if bands.len() < 2 || bands.len() > MAX_BANDS {
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
        let mut outside = commands[..start].to_vec();
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
            Some(rect) => Some((
                rect,
                software_patch::Scene::new(display, rect, scale, canvas, text, cache)?,
            )),
            None => None,
        };
        Some(Self {
            viewport,
            bands,
            outside: DisplayList::from(outside),
            overlay,
        })
    }

    fn compatible(&self, other: &Self) -> bool {
        self.viewport == other.viewport && self.outside.as_slice() == other.outside.as_slice()
    }

    fn translation(&self, old: &Self) -> Option<i64> {
        if !self.compatible(old) {
            return None;
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
        if self.retired.len() < 12 {
            self.retired.push(Retired {
                surface,
                frame: self.frame,
                at: Instant::now(),
                observed_use,
            });
        }
    }
    fn prepare(&mut self, bitmap: &Bitmap) -> Option<software_patch::Surface> {
        let size = (bitmap.width(), bitmap.height());
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
            None => software_patch::Surface::new(size)?,
        };
        surface.write(bitmap).then_some(surface)
    }
}

struct Layer {
    raw: Id,
    surface: Option<software_patch::Surface>,
    observed_use: bool,
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
            })
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
    bitmap: Bitmap,
    layer: Layer,
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
    overlays: Vec<Overlay>,
    cache: RasterCache,
    surfaces: Surfaces,
    offset: i64,
    active: bool,
}

impl Presenter {
    pub(super) fn active(&self) -> bool {
        self.active
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
            for overlay in &self.overlays {
                msg_void_bool(overlay.layer.raw, sel("setHidden:"), 1);
            }
        }
        self.active = false;
        self.prior = None;
        self.shown.clear();
        self.overlays.clear();
        self.cache = RasterCache::default();
        self.offset = 0;
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
        let Some(scene) = Scene::new(display, physical, scale, canvas, text, boxes) else {
            self.prior = None;
            return false;
        };
        let Some(dy) = self
            .prior
            .as_ref()
            .and_then(|prior| scene.translation(prior))
        else {
            self.prior = Some(scene);
            return false;
        };
        if !self.active && dy == 0 {
            self.prior = Some(scene);
            return false;
        }
        let offset = if self.active { self.offset + dy } else { 0 };
        // Periodically rebase through the normal whole frame instead of
        // letting layer coordinates grow without bound on a long scroll.
        if offset.unsigned_abs() > 1_000_000 {
            self.prior = None;
            return false;
        }
        self.surfaces.frame += 1;
        for shown in &mut self.shown {
            shown.layer.observed_use = shown.layer.observed_use
                || shown
                    .layer
                    .surface
                    .as_ref()
                    .is_some_and(|surface| surface.busy());
        }
        for overlay in &mut self.overlays {
            overlay.layer.observed_use = overlay.layer.observed_use
                || overlay
                    .layer
                    .surface
                    .as_ref()
                    .is_some_and(|surface| surface.busy());
        }
        let cached = CachedText {
            engine: text,
            cache: &self.cache,
        };
        let mut matched = vec![false; self.shown.len()];
        let mut plan = Vec::with_capacity(scene.bands.len());
        for band in &scene.bands {
            let old = self.shown.iter().enumerate().position(|(i, shown)| {
                !matched[i]
                    && band.rect.1 == shown.band.rect.1 + dy
                    && band.same_pixels(&shown.band)
            });
            if let Some(at) = old {
                matched[at] = true;
                plan.push(Prepared::Reuse(at));
                continue;
            }
            let bitmap = rasterize_with(
                &band.display,
                (band.rect.2 - band.rect.0) as usize,
                (band.rect.3 - band.rect.1) as usize,
                scale,
                canvas,
                &cached,
                images,
            );
            let Some(surface) = self.surfaces.prepare(&bitmap) else {
                return false;
            };
            let Some(mut layer) = (unsafe { Layer::new(scale) }) else {
                return false;
            };
            unsafe {
                msg_void_id(layer.raw, sel("setContents:"), surface.raw);
            }
            layer.surface = Some(surface);
            plan.push(Prepared::Fresh(layer));
        }
        let mut fresh_overlay = None;
        let overlay = if let Some((_, paint)) = &scene.overlay {
            let bitmap = paint.raster(scale, canvas, &cached, images);
            match self.overlays.iter().position(|o| {
                o.bitmap.width() == bitmap.width()
                    && o.bitmap.height() == bitmap.height()
                    && o.bitmap.pixels() == bitmap.pixels()
            }) {
                Some(at) => Some(at),
                None => {
                    let Some(surface) = self.surfaces.prepare(&bitmap) else {
                        return false;
                    };
                    let Some(mut layer) = (unsafe { Layer::new(scale) }) else {
                        return false;
                    };
                    unsafe {
                        msg_void_id(layer.raw, sel("setContents:"), surface.raw);
                    }
                    layer.surface = Some(surface);
                    fresh_overlay = Some(Overlay { bitmap, layer });
                    Some(self.overlays.len())
                }
            }
        } else {
            None
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
            if let Some(fresh) = fresh_overlay {
                // Immediately above the viewport, still below native islands.
                msg_void_id_u64(root, sel("insertSublayer:atIndex:"), fresh.layer.raw, 1);
                self.overlays.push(fresh);
            }
            for (at, kept) in self.overlays.iter().enumerate() {
                msg_void_bool(
                    kept.layer.raw,
                    sel("setHidden:"),
                    (Some(at) != overlay) as i8,
                );
                if Some(at) == overlay
                    && let Some((rect, _)) = &scene.overlay
                {
                    msg_void_rect(
                        kept.layer.raw,
                        sel("setFrame:"),
                        Patch::frame(*rect, physical, scale),
                    );
                }
            }
            // The selected overlay is last when added. Old ones are hidden;
            // retaining three exact pixel variants handles small movements.
            if self.overlays.len() > 3 {
                let at = (0..self.overlays.len()).find(|at| Some(*at) != overlay);
                if let Some(at) = at {
                    let mut old = self.overlays.remove(at);
                    if let Some(surface) = old.layer.surface.take() {
                        retired.push((surface, old.layer.observed_use));
                    }
                }
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
                    let tile =
                        overlay.raster(scale, Color::WHITE, &PixelFont, &RawImages::default());
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
            fn body(self, _: &Context) -> impl View {
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
