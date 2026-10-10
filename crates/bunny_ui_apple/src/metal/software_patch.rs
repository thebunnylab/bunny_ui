//! Small opaque damage can cost less to raster than to submit to Metal.
//! The surface pool never writes the layer's current contents or a surface
//! still used by the compositor. A busy pool simply keeps the Metal road.

use super::*;
use bunny_ui::layout::{DrawCommand, Point, Rect};
use bunny_ui::raster::{Bitmap, carve_covering, rasterize_with};

const MAX_PIXELS: usize = 128 * 1024;
const MAX_COMMANDS: usize = 64;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_SURFACES: usize = 3;

struct RasterBudget {
    surface: usize,
    text: usize,
    paint: Option<usize>,
}

/// The bounded, translated scene that paints an entire opaque patch.
pub(super) struct Scene {
    display: DisplayList,
    size: (usize, usize),
    rect: DamageRect,
    scale: usize,
    canvas: Color,
}

pub(super) struct BasePiece {
    pub(super) surface: Surface,
    pub(super) bounds: DamageRect,
}

pub(super) struct Mosaic {
    pub(super) background: Surface,
    pub(super) pieces: Vec<BasePiece>,
}

impl Scene {
    pub(super) fn new(
        display: &DisplayList,
        rect: DamageRect,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        cache: &MeasureCache,
    ) -> Option<Self> {
        Self::with_budget(
            display,
            rect,
            scale,
            canvas,
            text,
            cache,
            RasterBudget {
                surface: MAX_PIXELS,
                text: MAX_PIXELS,
                paint: None,
            },
        )
    }

    /// A bounded first frame can seed a native base without a Metal queue.
    /// Later frames still obey the much smaller patch budget.
    pub(super) fn base(
        display: &DisplayList,
        physical: (usize, usize),
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        cache: &MeasureCache,
    ) -> Option<Self> {
        Self::with_budget(
            display,
            (
                0,
                0,
                i64::try_from(physical.0).ok()?,
                i64::try_from(physical.1).ok()?,
            ),
            scale,
            canvas,
            text,
            cache,
            RasterBudget {
                surface: 8 * 1024 * 1024,
                text: 2 * 1024 * 1024,
                paint: Some(16 * 1024 * 1024),
            },
        )
    }

    fn with_budget(
        display: &DisplayList,
        rect: DamageRect,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        cache: &MeasureCache,
        budget: RasterBudget,
    ) -> Option<Self> {
        let width = usize::try_from(rect.2.checked_sub(rect.0)?).ok()?;
        let height = usize::try_from(rect.3.checked_sub(rect.1)?).ok()?;
        if scale == 0
            || canvas.a != 255
            || width == 0
            || height == 0
            || width.checked_mul(height)? > budget.surface
        {
            return None;
        }
        let factor = scale as f64;
        let logical = Rect {
            origin: Point {
                x: rect.0 as f64 / factor,
                y: rect.1 as f64 / factor,
            },
            size: Size {
                width: width as f64 / factor,
                height: height as f64 / factor,
            },
        };
        let candidates = measured_candidates(display, rect, factor, text, cache)?;
        let (_, lifted) = carve_covering(&candidates, (0, candidates.len()), logical)?;
        if lifted.len() > MAX_COMMANDS {
            return None;
        }
        let mut bytes = 0usize;
        let mut text_pixels = 0usize;
        let mut paint_pixels = 0usize;
        for command in lifted.iter() {
            match command {
                DrawCommand::FillRect { rect, .. } | DrawCommand::StrokeRect { rect, .. } => {
                    if let Some(limit) = budget.paint {
                        if ![
                            rect.origin.x,
                            rect.origin.y,
                            rect.size.width,
                            rect.size.height,
                        ]
                        .iter()
                        .all(|value| value.is_finite())
                        {
                            return None;
                        }
                        if let Some(visible) = rect.intersection(logical) {
                            let width = (visible.size.width * factor).ceil().max(0.0) as usize;
                            let height = (visible.size.height * factor).ceil().max(0.0) as usize;
                            paint_pixels = paint_pixels.checked_add(width.checked_mul(height)?)?;
                            if paint_pixels > limit {
                                return None;
                            }
                        }
                    }
                }
                DrawCommand::PushClip { .. } | DrawCommand::PopClip => {}
                DrawCommand::TextLine {
                    range,
                    content,
                    font,
                    ..
                } => {
                    bytes = bytes.checked_add(range.1.checked_sub(range.0)?)?;
                    if bytes > MAX_TEXT_BYTES {
                        return None;
                    }
                    let metrics = cache.get_or_measure(&content[range.0..range.1], font, text);
                    if !metrics.width.is_finite() || !metrics.height().is_finite() {
                        return None;
                    }
                    let width = (metrics.width * factor).ceil().max(0.0) as usize;
                    let height = (metrics.height() * factor).ceil().max(0.0) as usize;
                    text_pixels = text_pixels.checked_add(width.checked_mul(height)?)?;
                    if text_pixels > budget.text {
                        return None;
                    }
                }
                DrawCommand::Gradient { .. }
                | DrawCommand::Backdrop { .. }
                | DrawCommand::Shadow { .. }
                | DrawCommand::Image { .. } => return None,
            }
        }
        let display = patch_coordinates(&lifted, rect, factor)?;
        Some(Self {
            display,
            size: (width, height),
            rect,
            scale,
            canvas,
        })
    }

    /// Equal normalized commands prove equal pixels at the same destination.
    /// The presenter also requires the patch to remain visible over its base.
    pub(super) fn matches(&self, other: &Self) -> bool {
        self.rect == other.rect
            && self.scale == other.scale
            && self.canvas == other.canvas
            && self.display.as_slice() == other.display.as_slice()
    }

    pub(super) const fn bounds(&self) -> DamageRect {
        self.rect
    }

    pub(super) const fn scale(&self) -> usize {
        self.scale
    }

    pub(super) const fn canvas(&self) -> Color {
        self.canvas
    }

    /// A static opaque picture can share one background pixel and retain
    /// only the exact non-background ink. Raster bytes, not command bounds,
    /// decide what can be omitted; antialiasing and borders stay intact.
    pub(super) fn mosaic(
        &self,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
        cache: &MeasureCache,
    ) -> Option<Mosaic> {
        let color = self
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::FillRect { rect, color, .. } if color.a == 255 => {
                    Some((rect.size.width * rect.size.height, *color))
                }
                _ => None,
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map_or(self.canvas, |(_, color)| color);
        let background = Surface::new_opaque((1, 1))?;
        if !background.write(&Bitmap::new(1, 1, color)) {
            return None;
        }
        let mut bytes = background.allocated_bytes();
        // Native layers have a cost too: retain pieces only when their
        // real surface allocations save at least one quarter of the picture.
        let limit = self.size.0.checked_mul(self.size.1)?.checked_mul(3)?;
        let column = 256usize.checked_mul(self.scale)?;
        let mut pieces = Vec::new();
        let complete = self.raster_bands(text, images, cache, |y, bitmap| {
            for x in (0..bitmap.width()).step_by(column) {
                let Some(region) = PixelRegion::new(
                    bitmap,
                    (x, 0, (x + column).min(bitmap.width()), bitmap.height()),
                ) else {
                    return false;
                };
                let Some(ink) = region.foreground(color) else {
                    continue;
                };
                if pieces.len() >= 128 {
                    return false;
                }
                let Some(surface) = Surface::new_opaque(ink.size()) else {
                    return false;
                };
                let Some(total) = bytes.checked_add(surface.allocated_bytes()) else {
                    return false;
                };
                if total > limit || !surface.write_pixels(ink) {
                    return false;
                }
                bytes = total;
                pieces.push(BasePiece {
                    surface,
                    bounds: (
                        ink.rect.0 as i64,
                        (y + ink.rect.1) as i64,
                        ink.rect.2 as i64,
                        (y + ink.rect.3) as i64,
                    ),
                });
            }
            true
        });
        complete.then_some(Mosaic { background, pieces })
    }

    /// Emits opaque bands without allocating a full-window scratch bitmap.
    pub(super) fn raster_bands(
        &self,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
        cache: &MeasureCache,
        mut paint: impl FnMut(usize, &Bitmap) -> bool,
    ) -> bool {
        let rows = MAX_PIXELS / self.size.0;
        if rows == 0 {
            return false;
        }
        if self.size.1 <= rows {
            return paint(0, &self.raster(text, images));
        }
        let factor = self.scale as f64;
        for y in (0..self.size.1).step_by(rows) {
            let height = rows.min(self.size.1 - y);
            let rect = (0, y as i64, self.size.0 as i64, (y + height) as i64);
            let Some(candidates) = measured_candidates(&self.display, rect, factor, text, cache)
            else {
                return false;
            };
            let logical = Rect {
                origin: Point {
                    x: 0.0,
                    y: y as f64 / factor,
                },
                size: Size {
                    width: self.size.0 as f64 / factor,
                    height: height as f64 / factor,
                },
            };
            let lifted = carve_covering(&candidates, (0, candidates.len()), logical)
                .map_or_else(DisplayList::default, |(_, display)| display);
            let Some(display) = patch_coordinates(&lifted, rect, factor) else {
                return false;
            };
            let bitmap = rasterize_with(
                &display,
                self.size.0,
                height,
                self.scale,
                self.canvas,
                text,
                images,
            );
            if !paint(y, &bitmap) {
                return false;
            }
        }
        true
    }

    pub(super) fn raster(&self, text: &dyn TextEngine, images: &dyn ImageEngine) -> Bitmap {
        rasterize_with(
            &self.display,
            self.size.0,
            self.size.1,
            self.scale,
            self.canvas,
            text,
            images,
        )
    }
}

// Text coverage in carve_covering deliberately has no right edge. A
// narrow patch must not spend its raster budget on lines that stop before
// it. Filter before the carve so irrelevant text cannot multiply clip
// wrappers or consume its command budget. The presenter's aged metrics
// serve both this bound and the admitted raster-work estimate.
fn measured_candidates(
    display: &DisplayList,
    patch: DamageRect,
    factor: f64,
    text: &dyn TextEngine,
    cache: &MeasureCache,
) -> Option<DisplayList> {
    let mut commands = Vec::with_capacity(display.len());
    for command in display.iter() {
        if let DrawCommand::TextLine {
            origin,
            content,
            range,
            font,
            ..
        } = command
        {
            let line = content.get(range.0..range.1)?;
            // Do not shape an unbounded line just to decide to reject it.
            if line.len() > MAX_TEXT_BYTES {
                return None;
            }
            let metrics = cache.get_or_measure(line, font, text);
            let x = (origin.x * factor).round();
            let y = (origin.y * factor).round();
            let width = (metrics.width * factor).ceil();
            let height = (metrics.height() * factor).ceil();
            if [x, y, width, height].iter().any(|value| !value.is_finite())
                || metrics.width < 0.0
                || metrics.height() < 0.0
            {
                return None;
            }
            // The same two physical pixels of rounding slack as the
            // damage oracle. Clips can only remove ink from this bound.
            if x - 2.0 >= patch.2 as f64
                || x + width + 2.0 <= patch.0 as f64
                || y - 2.0 >= patch.3 as f64
                || y + height + 2.0 <= patch.1 as f64
            {
                continue;
            }
        }
        commands.push(command.clone());
    }
    Some(DisplayList::from(commands))
}

// The whole raster snaps in window coordinates. Rounding a translated
// half-pixel instead can cross zero and move the result one pixel: round
// ties away from zero do not commute with an integer translation.
pub(super) fn patch_coordinates(
    display: &DisplayList,
    patch: DamageRect,
    factor: f64,
) -> Option<DisplayList> {
    let point = |origin: Point| Point {
        x: ((origin.x * factor).round() - patch.0 as f64) / factor,
        y: ((origin.y * factor).round() - patch.1 as f64) / factor,
    };
    let rect = |rect: Rect| {
        let x = rect.origin.x * factor;
        let y = rect.origin.y * factor;
        Rect {
            origin: point(rect.origin),
            // Match scale_rect followed by Bitmap::snap, including its
            // endpoint addition order. Radii and stroke widths stay logical.
            size: Size {
                width: ((x + rect.size.width * factor).round() - x.round()) / factor,
                height: ((y + rect.size.height * factor).round() - y.round()) / factor,
            },
        }
    };
    display
        .iter()
        .cloned()
        .map(|mut command| {
            match &mut command {
                DrawCommand::FillRect { rect: bounds, .. }
                | DrawCommand::StrokeRect { rect: bounds, .. }
                | DrawCommand::PushClip { rect: bounds, .. } => *bounds = rect(*bounds),
                DrawCommand::TextLine { origin, .. } => *origin = point(*origin),
                DrawCommand::PopClip => {}
                DrawCommand::Gradient { .. }
                | DrawCommand::Backdrop { .. }
                | DrawCommand::Shadow { .. }
                | DrawCommand::Image { .. } => return None,
            }
            Some(command)
        })
        .collect::<Option<Vec<_>>>()
        .map(DisplayList::from)
}

#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    fn IOSurfaceCreate(properties: Id) -> Id;
    fn IOSurfaceSetValue(surface: Id, key: Id, value: Id);
    static kIOSurfaceColorSpace: Id;
    fn IOSurfaceIsInUse(surface: Id) -> u8;
    fn IOSurfaceLock(surface: Id, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceUnlock(surface: Id, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceGetBaseAddress(surface: Id) -> *mut c_void;
    fn IOSurfaceGetBytesPerRow(surface: Id) -> usize;
    fn IOSurfaceGetAllocSize(surface: Id) -> usize;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> Id;
    fn CGColorSpaceCopyPropertyList(space: Id) -> Id;
}

#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_void_id_id(obj: Id, sel: Sel, value: Id, key: Id);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SurfaceFormat {
    Bgra8,
    #[cfg(target_arch = "aarch64")]
    OpaqueRgb10,
}

impl SurfaceFormat {
    fn code(self) -> u32 {
        match self {
            Self::Bgra8 => u32::from_be_bytes(*b"BGRA"),
            #[cfg(target_arch = "aarch64")]
            Self::OpaqueRgb10 => u32::from_be_bytes(*b"w30r"),
        }
    }
}

// BGR10_XR decodes each channel as (code - 384) / 510. Every SDR byte
// therefore has an exact code, 384 + 2 * byte. The upper two bits are
// padding, not alpha: the compositor sees an intrinsically opaque image.
#[cfg(any(target_arch = "aarch64", test))]
fn opaque_rgb10(rgba: u32) -> [u8; 4] {
    let channel = |shift: u32| ((rgba >> shift) & 255) * 2 + 384;
    ((channel(24) << 20) | (channel(16) << 10) | channel(8)).to_le_bytes()
}

/// A validated borrowed rectangle, retaining the source stride. Cropping a
/// native backing needs no intermediate bitmap or second rasterization.
#[derive(Clone, Copy)]
pub(super) struct PixelRegion<'a> {
    bitmap: &'a Bitmap,
    rect: (usize, usize, usize, usize),
}

impl<'a> PixelRegion<'a> {
    pub(super) fn new(bitmap: &'a Bitmap, rect: (usize, usize, usize, usize)) -> Option<Self> {
        (rect.0 < rect.2
            && rect.1 < rect.3
            && rect.2 <= bitmap.width()
            && rect.3 <= bitmap.height())
        .then_some(Self { bitmap, rect })
    }

    fn foreground(self, background: Color) -> Option<Self> {
        let background =
            u32::from_be_bytes([background.r, background.g, background.b, background.a]);
        let (mut left, mut top, mut right, mut bottom) = (usize::MAX, usize::MAX, 0, 0);
        for (y, row) in self.rows().enumerate() {
            for (x, pixel) in row.iter().enumerate() {
                if *pixel != background {
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x + 1);
                    bottom = bottom.max(y + 1);
                }
            }
        }
        (left < right).then(|| Self {
            bitmap: self.bitmap,
            rect: (
                self.rect.0 + left,
                self.rect.1 + top,
                self.rect.0 + right,
                self.rect.1 + bottom,
            ),
        })
    }

    pub(super) fn whole(bitmap: &'a Bitmap) -> Option<Self> {
        Self::new(bitmap, (0, 0, bitmap.width(), bitmap.height()))
    }

    pub(super) fn size(self) -> (usize, usize) {
        (self.rect.2 - self.rect.0, self.rect.3 - self.rect.1)
    }

    fn rows(self) -> impl Iterator<Item = &'a [u32]> {
        self.bitmap
            .pixels()
            .chunks_exact(self.bitmap.width())
            .skip(self.rect.1)
            .take(self.rect.3 - self.rect.1)
            .map(move |row| &row[self.rect.0..self.rect.2])
    }
}

pub(super) struct Surface {
    pub(super) raw: Id,
    pub(super) size: (usize, usize),
    format: SurfaceFormat,
}

impl Surface {
    pub(super) fn new(size: (usize, usize)) -> Option<Self> {
        Self::with_format(size, SurfaceFormat::Bgra8)
    }

    /// Apple Silicon can share opaque RGB10 pixels directly with CA. Other
    /// Macs and refused allocations retain the existing BGRA backing.
    pub(super) fn new_opaque(size: (usize, usize)) -> Option<Self> {
        #[cfg(target_arch = "aarch64")]
        if let Some(surface) = Self::with_format(size, SurfaceFormat::OpaqueRgb10) {
            return Some(surface);
        }
        Self::new(size)
    }

    fn with_format(size: (usize, usize), format: SurfaceFormat) -> Option<Self> {
        unsafe {
            let properties = msg_id(class("NSMutableDictionary"), sel("dictionary"));
            if properties.is_null() {
                return None;
            }
            for (key, value) in [
                ("IOSurfaceWidth", size.0 as u64),
                ("IOSurfaceHeight", size.1 as u64),
                ("IOSurfaceBytesPerElement", 4),
                ("IOSurfacePixelFormat", u64::from(format.code())),
            ] {
                let number =
                    msg_id_u64(class("NSNumber"), sel("numberWithUnsignedLongLong:"), value);
                msg_void_id_id(properties, sel("setObject:forKey:"), number, ns_string(key));
            }
            let raw = IOSurfaceCreate(properties);
            if raw.is_null() {
                return None;
            }
            // Match the uncolormatched Metal layer. An untagged IOSurface
            // takes CA's default profile and visibly lightens the patch.
            let space = CGColorSpaceCreateDeviceRGB();
            if space.is_null() {
                CFRelease(raw);
                return None;
            }
            let description = CGColorSpaceCopyPropertyList(space);
            CFRelease(space);
            if description.is_null() {
                CFRelease(raw);
                return None;
            }
            IOSurfaceSetValue(raw, kIOSurfaceColorSpace, description);
            CFRelease(description);
            Some(Self { raw, size, format })
        }
    }

    /// Build a fresh surface in bounded scratch bands before it is visible.
    pub(super) fn from_scene(
        scene: &Scene,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
        cache: &MeasureCache,
    ) -> Option<Self> {
        let surface = Self::new_opaque(scene.size)?;
        scene
            .raster_bands(text, images, cache, |y, bitmap| {
                PixelRegion::whole(bitmap).is_some_and(|pixels| surface.write_rows(y, pixels))
            })
            .then_some(surface)
    }

    pub(super) fn allocated_bytes(&self) -> usize {
        unsafe { IOSurfaceGetAllocSize(self.raw) }
    }

    pub(super) fn busy(&self) -> bool {
        unsafe { IOSurfaceIsInUse(self.raw) != 0 }
    }

    pub(super) fn write(&self, bitmap: &Bitmap) -> bool {
        PixelRegion::whole(bitmap).is_some_and(|pixels| self.write_pixels(pixels))
    }

    pub(super) fn write_pixels(&self, pixels: PixelRegion<'_>) -> bool {
        if self.size != pixels.size() {
            return false;
        }
        self.write_rows(0, pixels)
    }

    fn write_rows(&self, y: usize, pixels: PixelRegion<'_>) -> bool {
        if pixels.size().0 != self.size.0 || y > self.size.1 || pixels.size().1 > self.size.1 - y {
            return false;
        }
        #[cfg(target_arch = "aarch64")]
        if self.format == SurfaceFormat::OpaqueRgb10
            && pixels.rows().flatten().any(|pixel| pixel & 0xff != 0xff)
        {
            return false;
        }
        unsafe {
            if IOSurfaceLock(self.raw, 0, null_mut()) != 0 {
                return false;
            }
            let base = IOSurfaceGetBaseAddress(self.raw).cast::<u8>();
            let stride = IOSurfaceGetBytesPerRow(self.raw);
            let valid = !base.is_null() && stride >= self.size.0 * 4;
            if valid {
                for (row, pixels) in pixels.rows().enumerate() {
                    let output = std::slice::from_raw_parts_mut(
                        base.add((row + y) * stride),
                        self.size.0 * 4,
                    );
                    match self.format {
                        SurfaceFormat::Bgra8 => {
                            for (rgba, bgra) in pixels.iter().zip(output.as_chunks_mut::<4>().0) {
                                bgra.copy_from_slice(&rgba.rotate_right(8).to_le_bytes());
                            }
                        }
                        #[cfg(target_arch = "aarch64")]
                        SurfaceFormat::OpaqueRgb10 => {
                            for (rgba, rgb10) in pixels.iter().zip(output.as_chunks_mut::<4>().0) {
                                rgb10.copy_from_slice(&opaque_rgb10(*rgba));
                            }
                        }
                    }
                }
            }
            let unlocked = IOSurfaceUnlock(self.raw, 0, null_mut()) == 0;
            valid && unlocked
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.raw);
        }
    }
}

/// Three backings at most. `current` is protected even before CA has
/// acquired its cross-process use count at the transaction's commit.
#[derive(Default)]
pub(super) struct Backing {
    surfaces: Vec<Surface>,
    current: Option<usize>,
}

impl Backing {
    pub(super) fn prepare(&mut self, bitmap: &Bitmap) -> Option<Id> {
        let size = (bitmap.width(), bitmap.height());
        let free = self
            .surfaces
            .iter()
            .enumerate()
            .position(|(index, surface)| Some(index) != self.current && !surface.busy());
        let index = match free {
            Some(index) => {
                if self.surfaces[index].size != size {
                    self.surfaces[index] = Surface::new(size)?;
                }
                index
            }
            None if self.surfaces.len() < MAX_SURFACES => {
                self.surfaces.push(Surface::new(size)?);
                self.surfaces.len() - 1
            }
            None => return None,
        };
        if !self.surfaces[index].write(bitmap) {
            return None;
        }
        // The caller immediately assigns these contents; there is no
        // intervening failure or event-loop turn after preparation.
        self.current = Some(index);
        Some(self.surfaces[index].raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bunny_ui::{
        image_engine::RawImages,
        layout::Corners,
        text_engine::{FontSpec, PixelFont},
    };

    #[test]
    fn a_moderately_dense_base_still_saves_a_quarter_of_surface_storage() {
        let display = DisplayList::from(vec![
            DrawCommand::FillRect {
                rect: Rect {
                    origin: Point::ZERO,
                    size: Size {
                        width: 800.0,
                        height: 600.0,
                    },
                },
                color: Color::WHITE,
                corner_radius: Corners::ZERO,
            },
            DrawCommand::FillRect {
                rect: Rect {
                    origin: Point::ZERO,
                    size: Size {
                        width: 500.0,
                        height: 600.0,
                    },
                },
                color: Color::BLACK,
                corner_radius: Corners::ZERO,
            },
        ]);
        let cache = MeasureCache::default();
        let scene = Scene::base(&display, (800, 600), 1, Color::WHITE, &PixelFont, &cache).unwrap();
        let mosaic = scene
            .mosaic(&PixelFont, &RawImages::default(), &cache)
            .expect("a moderate scene can save surface storage");
        let bytes = mosaic.background.allocated_bytes()
            + mosaic
                .pieces
                .iter()
                .map(|p| p.surface.allocated_bytes())
                .sum::<usize>();
        assert!(bytes > 800 * 600 * 2);
        assert!(bytes <= 800 * 600 * 3);
    }

    #[test]
    fn a_static_mosaic_reconstructs_every_pixel_and_refuses_dense_storage() {
        fn pixels(surface: &Surface) -> Vec<u32> {
            unsafe {
                assert_eq!(IOSurfaceLock(surface.raw, 0, null_mut()), 0);
                let base = IOSurfaceGetBaseAddress(surface.raw).cast::<u8>();
                let stride = IOSurfaceGetBytesPerRow(surface.raw);
                let result = (0..surface.size.1)
                    .flat_map(|y| {
                        std::slice::from_raw_parts(base.add(y * stride), surface.size.0 * 4)
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|bytes| {
                                let word = u32::from_le_bytes(*bytes);
                                match surface.format {
                                    SurfaceFormat::Bgra8 => word.rotate_left(8),
                                    #[cfg(target_arch = "aarch64")]
                                    SurfaceFormat::OpaqueRgb10 => {
                                        let byte =
                                            |shift: u32| (((word >> shift) & 1023u32) - 384) / 2;
                                        (byte(20) << 24) | (byte(10) << 16) | (byte(0) << 8) | 255
                                    }
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                assert_eq!(IOSurfaceUnlock(surface.raw, 0, null_mut()), 0);
                result
            }
        }
        let rect = |x, y, width, height| Rect {
            origin: Point { x, y },
            size: Size { width, height },
        };
        let mut commands = vec![
            DrawCommand::FillRect {
                rect: rect(-0.5, -0.5, 800.0, 600.0),
                color: Color::WHITE,
                corner_radius: Corners::all(10.5),
            },
            DrawCommand::StrokeRect {
                rect: rect(0.5, 0.5, 799.0, 599.0),
                color: Color::hex(0x237bd9),
                corner_radius: Corners::all(8.5),
                width: 1.5,
            },
            DrawCommand::PushClip {
                rect: rect(12.5, 20.5, 350.0, 550.0),
                corner_radius: Corners::all(12.5),
            },
        ];
        for row in 0..20 {
            commands.push(DrawCommand::TextLine {
                origin: Point {
                    x: 8.5,
                    y: 12.5 + row as f64 * 25.0,
                },
                content: "every antialiased pixel stays".into(),
                range: (0, 28),
                color: Color::rgba(50, 20, 150, 170),
                font: FontSpec::DEFAULT,
            });
        }
        commands.push(DrawCommand::PopClip);
        let display = DisplayList::from(commands);
        let cache = MeasureCache::default();
        for scale in [1, 2] {
            let scene = Scene::base(
                &display,
                (800 * scale, 600 * scale),
                scale,
                Color::BLACK,
                &PixelFont,
                &cache,
            )
            .unwrap();
            let mosaic = scene
                .mosaic(&PixelFont, &RawImages::default(), &cache)
                .unwrap();
            let mut actual = vec![pixels(&mosaic.background)[0]; scene.size.0 * scene.size.1];
            let mut touched = vec![false; actual.len()];
            let bytes = mosaic.background.allocated_bytes()
                + mosaic
                    .pieces
                    .iter()
                    .map(|p| p.surface.allocated_bytes())
                    .sum::<usize>();
            assert!(bytes <= actual.len() * 2);
            for piece in &mosaic.pieces {
                let values = pixels(&piece.surface);
                for y in 0..piece.surface.size.1 {
                    for x in 0..piece.surface.size.0 {
                        let at = (piece.bounds.1 as usize + y) * scene.size.0
                            + piece.bounds.0 as usize
                            + x;
                        assert!(!touched[at], "pieces do not overlap");
                        touched[at] = true;
                        actual[at] = values[y * piece.surface.size.0 + x];
                    }
                }
            }
            assert_eq!(
                actual,
                scene.raster(&PixelFont, &RawImages::default()).pixels(),
                "scale {scale}"
            );
        }
        let mut dense = vec![DrawCommand::FillRect {
            rect: rect(0.0, 0.0, 800.0, 600.0),
            color: Color::WHITE,
            corner_radius: Corners::ZERO,
        }];
        for column in 0..16 {
            dense.push(DrawCommand::FillRect {
                rect: rect(column as f64 * 50.0, 0.0, 25.0, 600.0),
                color: Color::BLACK,
                corner_radius: Corners::ZERO,
            });
        }
        let dense = DisplayList::from(dense);
        let scene = Scene::base(&dense, (800, 600), 1, Color::WHITE, &PixelFont, &cache).unwrap();
        assert!(
            scene
                .mosaic(&PixelFont, &RawImages::default(), &cache)
                .is_none(),
            "dense paint keeps the full native surface"
        );
    }

    #[test]
    fn native_base_raster_bands_are_bounded_and_match_the_whole_scene() {
        let display = DisplayList::from(vec![
            DrawCommand::FillRect {
                rect: Rect {
                    origin: Point { x: -0.5, y: -0.5 },
                    size: Size {
                        width: 514.0,
                        height: 378.0,
                    },
                },
                color: Color::WHITE,
                corner_radius: Corners::all(17.5),
            },
            DrawCommand::PushClip {
                rect: Rect {
                    origin: Point { x: 12.5, y: 20.5 },
                    size: Size {
                        width: 475.0,
                        height: 333.0,
                    },
                },
                corner_radius: Corners::all(12.5),
            },
            DrawCommand::FillRect {
                rect: Rect {
                    origin: Point { x: 40.5, y: 40.5 },
                    size: Size {
                        width: 440.0,
                        height: 280.0,
                    },
                },
                color: Color::rgba(200, 40, 80, 123),
                corner_radius: Corners::all(14.5),
            },
            DrawCommand::StrokeRect {
                rect: Rect {
                    origin: Point { x: 4.5, y: 112.5 },
                    size: Size {
                        width: 490.0,
                        height: 170.0,
                    },
                },
                color: Color::rgba(0, 40, 200, 170),
                corner_radius: Corners::all(8.5),
                width: 2.5,
            },
            DrawCommand::TextLine {
                origin: Point { x: -0.5, y: 249.5 },
                content: "crossing the band boundary".into(),
                range: (0, 26),
                color: Color::BLACK,
                font: FontSpec::DEFAULT,
            },
            DrawCommand::PopClip,
        ]);
        for scale in [1, 2, 3] {
            let size = (513 * scale, 377 * scale);
            let cache = MeasureCache::default();
            let scene =
                Scene::base(&display, size, scale, Color::BLACK, &PixelFont, &cache).unwrap();
            let expected = scene.raster(&PixelFont, &RawImages::default());
            let mut actual = vec![0; size.0 * size.1];
            let mut next = 0;
            assert!(scene.raster_bands(
                &PixelFont,
                &RawImages::default(),
                &cache,
                |row, bitmap| {
                    assert_eq!(row, next, "bands cover the canvas exactly once");
                    assert!(
                        bitmap.width() * bitmap.height() <= MAX_PIXELS,
                        "a native base must not allocate a whole-window scratch bitmap"
                    );
                    next += bitmap.height();
                    actual[row * size.0..next * size.0].copy_from_slice(bitmap.pixels());
                    true
                }
            ));
            assert_eq!(next, size.1);
            assert_eq!(actual, expected.pixels(), "scale {scale}");
            let surface =
                Surface::from_scene(&scene, &PixelFont, &RawImages::default(), &cache).unwrap();
            let actual = unsafe {
                assert_eq!(IOSurfaceLock(surface.raw, 0, null_mut()), 0);
                let base = IOSurfaceGetBaseAddress(surface.raw).cast::<u8>();
                let stride = IOSurfaceGetBytesPerRow(surface.raw);
                let bytes = (0..size.1)
                    .flat_map(|row| {
                        std::slice::from_raw_parts(base.add(row * stride), size.0 * 4).to_vec()
                    })
                    .collect::<Vec<_>>();
                assert_eq!(IOSurfaceUnlock(surface.raw, 0, null_mut()), 0);
                bytes
            };
            let expected = expected
                .pixels()
                .iter()
                .flat_map(|rgba| match surface.format {
                    SurfaceFormat::Bgra8 => rgba.rotate_right(8).to_le_bytes(),
                    #[cfg(target_arch = "aarch64")]
                    SurfaceFormat::OpaqueRgb10 => opaque_rgb10(*rgba),
                })
                .collect::<Vec<_>>();
            assert_eq!(
                actual, expected,
                "all rows reach the native surface at scale {scale}"
            );
            let row = Bitmap::new(size.0, 1, Color::WHITE);
            assert!(!surface.write_rows(size.1, PixelRegion::whole(&row).unwrap()));
            assert!(!surface.write_rows(usize::MAX, PixelRegion::whole(&row).unwrap()));
        }
    }

    #[test]
    fn a_cropped_surface_reads_the_source_stride_without_copying_padding() {
        let display = DisplayList::from(vec![DrawCommand::FillRect {
            rect: Rect {
                origin: Point { x: 2.0, y: 1.0 },
                size: Size {
                    width: 1.0,
                    height: 2.0,
                },
            },
            color: Color::BLACK,
            corner_radius: Corners::ZERO,
        }]);
        let bitmap = rasterize_with(
            &display,
            5,
            4,
            1,
            Color::WHITE,
            &PixelFont,
            &RawImages::default(),
        );
        let region = PixelRegion::new(&bitmap, (1, 1, 4, 3)).unwrap();
        assert_eq!(region.size(), (3, 2));
        assert_eq!(
            region.rows().next().unwrap().as_ptr(),
            bitmap.pixels()[6..].as_ptr()
        );
        let surface = Surface::new(region.size()).unwrap();
        assert!(surface.write_pixels(region));
        let actual = unsafe {
            assert_eq!(IOSurfaceLock(surface.raw, 0, null_mut()), 0);
            let base = IOSurfaceGetBaseAddress(surface.raw).cast::<u8>();
            let stride = IOSurfaceGetBytesPerRow(surface.raw);
            let bytes = (0..2)
                .flat_map(|row| std::slice::from_raw_parts(base.add(row * stride), 12).to_vec())
                .collect::<Vec<_>>();
            assert_eq!(IOSurfaceUnlock(surface.raw, 0, null_mut()), 0);
            bytes
        };
        let expected = region
            .rows()
            .flatten()
            .flat_map(|rgba| rgba.rotate_right(8).to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
        assert!(PixelRegion::new(&bitmap, (0, 0, 0, 2)).is_none());
        assert!(PixelRegion::new(&bitmap, (4, 1, 6, 2)).is_none());
        assert!(PixelRegion::new(&bitmap, (1, 3, 4, usize::MAX)).is_none());
        assert!(!surface.write_pixels(PixelRegion::whole(&bitmap).unwrap()));
    }

    #[test]
    fn every_sdr_byte_has_an_exact_opaque_rgb10_code() {
        for value in 0u32..=255 {
            let rgba = (value << 24) | ((255 - value) << 16) | ((value ^ 0xa5) << 8) | 255;
            let packed = u32::from_le_bytes(opaque_rgb10(rgba));
            assert_eq!(packed >> 30, 0, "padding does not encode alpha");
            for (source, destination) in [(24, 20), (16, 10), (8, 0)] {
                let encoded = (packed >> destination) & 1023;
                assert!((384..=894).contains(&encoded));
                assert_eq!((encoded - 384) / 2, (rgba >> source) & 255);
                assert_eq!((encoded - 384) % 2, 0);
            }
        }
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn opaque_surfaces_refuse_alpha_before_touching_shared_pixels() {
        unsafe extern "C" {
            fn IOSurfaceGetPixelFormat(surface: Id) -> u32;
        }
        let surface = Surface::new_opaque((2, 2)).expect("opaque backing");
        assert_eq!(
            unsafe { IOSurfaceGetPixelFormat(surface.raw) },
            u32::from_be_bytes(*b"w30r")
        );
        assert!(surface.write(&Bitmap::new(2, 2, Color::hex(0x123456))));
        let snapshot = || unsafe {
            assert_eq!(IOSurfaceLock(surface.raw, 0, null_mut()), 0);
            let base = IOSurfaceGetBaseAddress(surface.raw).cast::<u8>();
            let stride = IOSurfaceGetBytesPerRow(surface.raw);
            let pixels = (0..2)
                .flat_map(|row| std::slice::from_raw_parts(base.add(row * stride), 8).to_vec())
                .collect::<Vec<_>>();
            assert_eq!(IOSurfaceUnlock(surface.raw, 0, null_mut()), 0);
            pixels
        };
        let expected = opaque_rgb10(0x123456ff).repeat(4);
        assert_eq!(snapshot(), expected);
        let translucent = Color {
            r: 20,
            g: 40,
            b: 60,
            a: 128,
        };
        assert!(!surface.write(&Bitmap::new(2, 2, translucent)));
        assert!(!surface.write(&Bitmap::new(1, 2, Color::WHITE)));
        assert_eq!(snapshot(), expected);
        let ordinary = Surface::new((2, 2)).unwrap();
        assert_eq!(
            unsafe { IOSurfaceGetPixelFormat(ordinary.raw) },
            u32::from_be_bytes(*b"BGRA")
        );
    }

    fn fill() -> DrawCommand {
        DrawCommand::FillRect {
            rect: Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: Size {
                    width: 640.0,
                    height: 640.0,
                },
            },
            color: Color::WHITE,
            corner_radius: Corners::ZERO,
        }
    }

    #[test]
    fn software_work_is_bounded_and_unsupported_ink_stays_on_metal() {
        let display = DisplayList::from(vec![fill()]);
        assert!(
            Scene::new(
                &display,
                (0, 0, 128, 128),
                1,
                Color::CANVAS,
                &PixelFont,
                &MeasureCache::default()
            )
            .is_some()
        );
        assert!(
            Scene::new(
                &display,
                (0, 0, 640, 640),
                1,
                Color::CANVAS,
                &PixelFont,
                &MeasureCache::default()
            )
            .is_none()
        );
        assert!(
            Scene::new(
                &display,
                (0, 0, 128, 128),
                0,
                Color::CANVAS,
                &PixelFont,
                &MeasureCache::default()
            )
            .is_none()
        );
        assert!(
            Scene::new(
                &display,
                (0, 0, 128, 128),
                1,
                Color::CANVAS.fade(),
                &PixelFont,
                &MeasureCache::default()
            )
            .is_none()
        );
        let commands = DisplayList::from(vec![fill(); MAX_COMMANDS + 1]);
        assert!(
            Scene::new(
                &commands,
                (0, 0, 128, 128),
                1,
                Color::CANVAS,
                &PixelFont,
                &MeasureCache::default()
            )
            .is_none()
        );
        let long = DisplayList::from(vec![DrawCommand::TextLine {
            origin: Point { x: 0.0, y: 0.0 },
            content: "x".repeat(MAX_TEXT_BYTES + 1).into(),
            range: (0, MAX_TEXT_BYTES + 1),
            color: Color::BLACK,
            font: FontSpec::DEFAULT,
        }]);
        assert!(
            Scene::new(
                &long,
                (0, 0, 128, 128),
                1,
                Color::CANVAS,
                &PixelFont,
                &MeasureCache::default()
            )
            .is_none()
        );
        let huge_glyph = DisplayList::from(vec![DrawCommand::TextLine {
            origin: Point { x: 0.0, y: 0.0 },
            content: "x".into(),
            range: (0, 1),
            color: Color::BLACK,
            font: FontSpec {
                size: 1024.0,
                ..FontSpec::DEFAULT
            },
        }]);
        assert!(
            Scene::new(
                &huge_glyph,
                (0, 0, 128, 128),
                1,
                Color::CANVAS,
                &crate::CoreTextEngine::new(),
                &MeasureCache::default()
            )
            .is_none()
        );
        let unsupported = DrawCommand::Shadow {
            rect: Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: Size {
                    width: 20.0,
                    height: 20.0,
                },
            },
            radius: 4.0,
            color: Color::BLACK,
            corner_radius: Corners::ZERO,
        };
        let shadow = DisplayList::from(vec![fill(), unsupported]);
        assert!(
            Scene::new(
                &shadow,
                (0, 0, 128, 128),
                1,
                Color::CANVAS,
                &PixelFont,
                &MeasureCache::default()
            )
            .is_none()
        );
    }

    fn assert_crop(display: &DisplayList, rect: DamageRect, scale: usize, text: &dyn TextEngine) {
        let full = rasterize_with(
            display,
            1280 * scale,
            800 * scale,
            scale,
            Color::WHITE,
            text,
            &RawImages::default(),
        );
        let scene = Scene::new(
            display,
            rect,
            scale,
            Color::WHITE,
            text,
            &MeasureCache::default(),
        )
        .expect("a bounded supported scene admits a software patch");
        let patch = scene.raster(text, &RawImages::default());
        for y in rect.1..rect.3 {
            for x in rect.0..rect.2 {
                assert_eq!(
                    patch.pixel((x - rect.0) as usize, (y - rect.1) as usize),
                    full.pixel(x as usize, y as usize),
                    "scale {scale}, pixel {x},{y}"
                );
            }
        }
    }

    #[test]
    fn software_patch_keeps_fractional_ink_and_nested_clips() {
        use bunny_ui::text_engine::Slant;
        let text = crate::CoreTextEngine::new();
        let rect = |x, y, width, height| Rect {
            origin: Point { x, y },
            size: Size { width, height },
        };
        for slant in [Slant::Upright, Slant::Italic] {
            for tracking in [-0.75, 0.0, 1.25] {
                let content = "ffi jA é العربية";
                let display = DisplayList::from(vec![
                    fill(),
                    DrawCommand::PushClip {
                        rect: rect(5.25, 3.75, 210.5, 50.5),
                        corner_radius: Corners::all(7.0),
                    },
                    DrawCommand::TextLine {
                        origin: Point { x: 12.25, y: 8.75 },
                        content: content.into(),
                        range: (0, content.len()),
                        color: Color::BLACK,
                        font: FontSpec {
                            slant,
                            tracking,
                            size: 13.0,
                            ..FontSpec::DEFAULT.family("Menlo")
                        },
                    },
                    DrawCommand::PushClip {
                        rect: rect(24.5, 10.25, 140.25, 25.5),
                        corner_radius: Corners::all(3.0),
                    },
                    DrawCommand::TextLine {
                        origin: Point {
                            x: 143.75,
                            y: 21.25,
                        },
                        content: "edge".into(),
                        range: (0, 4),
                        color: Color::hex(0xDE7932),
                        font: FontSpec::DEFAULT.family("Menlo"),
                    },
                    DrawCommand::PopClip,
                    DrawCommand::PopClip,
                ]);
                for scale in [1, 2] {
                    let s = scale as i64;
                    for (left, right) in [(10, 18), (60, 72), (142, 150), (160, 168), (214, 220)] {
                        assert_crop(&display, (left * s, 0, right * s, 56 * s), scale, &text);
                    }
                }
            }
        }
    }

    #[test]
    fn software_patch_keeps_fractional_fills_strokes_and_clip_edges() {
        let rect = |x, y, width, height| Rect {
            origin: Point { x, y },
            size: Size { width, height },
        };
        let display = DisplayList::from(vec![
            fill(),
            DrawCommand::PushClip {
                rect: rect(5.25, 3.75, 210.5, 50.5),
                corner_radius: Corners::all(12.5),
            },
            DrawCommand::FillRect {
                rect: rect(-0.25, -0.75, 39.5, 33.5),
                color: Color::hex(0x619732),
                corner_radius: Corners::all(8.0),
            },
            DrawCommand::FillRect {
                rect: rect(12.25, 8.75, 172.5, 27.5),
                color: Color::hex(0x325397),
                corner_radius: Corners::all(6.5),
            },
            DrawCommand::StrokeRect {
                rect: rect(24.5, 10.25, 140.25, 25.5),
                color: Color::hex(0xDE7932),
                width: 1.75,
                corner_radius: Corners::all(3.5),
            },
            DrawCommand::PopClip,
        ]);
        for scale in [1, 2] {
            let s = scale as i64;
            for left in [0, 5, 12, 24, 60, 144, 160, 180, 210] {
                for top in [0, 4, 8, 12, 20, 32, 44] {
                    assert_crop(
                        &display,
                        (left * s, top * s, (left + 8) * s, (top + 8) * s),
                        scale,
                        &PixelFont,
                    );
                }
            }
        }
    }

    #[test]
    fn text_left_of_a_striped_thumb_does_not_spend_the_patch_budget() {
        let mut commands = Vec::new();
        for row in 0..28 {
            commands.push(DrawCommand::FillRect {
                rect: Rect {
                    origin: Point {
                        x: 0.0,
                        y: row as f64 * 28.0,
                    },
                    size: Size {
                        width: 1280.0,
                        height: 28.0,
                    },
                },
                color: if row % 2 == 0 {
                    Color::hex(0x17171C)
                } else {
                    Color::hex(0x1C1C21)
                },
                corner_radius: Corners::ZERO,
            });
            let content = format!("message {row}: a token lands, and the list grows by one line");
            commands.push(DrawCommand::TextLine {
                origin: Point {
                    x: 8.0,
                    y: row as f64 * 28.0 + 6.0,
                },
                range: (0, content.len()),
                content: content.into(),
                color: Color::WHITE,
                font: FontSpec::DEFAULT,
            });
        }
        commands.push(DrawCommand::FillRect {
            rect: Rect {
                origin: Point { x: 1274.0, y: 2.0 },
                size: Size {
                    width: 4.0,
                    height: 730.0,
                },
            },
            color: Color::WHITE.fade(),
            corner_radius: Corners::all(2.0),
        });
        let display = DisplayList::from(commands);
        for scale in [1, 2] {
            let s = scale as i64;
            assert_crop(
                &display,
                (1272 * s, 0, 1280 * s, 736 * s),
                scale,
                &PixelFont,
            );
        }
    }

    #[test]
    fn unknown_text_extent_cannot_be_culled_to_admit_a_patch() {
        use bunny_ui::text_engine::{LineMetrics, TextRaster};
        struct UnknownExtent;
        impl TextEngine for UnknownExtent {
            fn measure_line(&self, _: &str, _: &FontSpec) -> LineMetrics {
                LineMetrics {
                    width: f64::NAN,
                    ascent: 12.0,
                    descent: 4.0,
                }
            }
            fn raster_line(&self, _: &str, _: &FontSpec, _: Color, _: usize) -> Option<TextRaster> {
                panic!("unbounded work never reaches the rasterizer")
            }
        }
        let display = DisplayList::from(vec![
            fill(),
            DrawCommand::TextLine {
                origin: Point { x: 8.0, y: 8.0 },
                content: "unknown".into(),
                range: (0, 7),
                color: Color::BLACK,
                font: FontSpec::DEFAULT,
            },
        ]);
        assert!(
            Scene::new(
                &display,
                (600, 0, 608, 32),
                1,
                Color::WHITE,
                &UnknownExtent,
                &MeasureCache::default()
            )
            .is_none()
        );
    }

    #[test]
    fn patch_admission_reuses_the_presenters_text_measurements() {
        use bunny_ui::text_engine::{LineMetrics, TextRaster};
        struct Counting(std::cell::Cell<usize>);
        impl TextEngine for Counting {
            fn measure_line(&self, line: &str, font: &FontSpec) -> LineMetrics {
                self.0.set(self.0.get() + 1);
                PixelFont.measure_line(line, font)
            }
            fn raster_line(&self, _: &str, _: &FontSpec, _: Color, _: usize) -> Option<TextRaster> {
                panic!("admission does not rasterize text")
            }
        }
        let text = Counting(std::cell::Cell::new(0));
        let cache = MeasureCache::default();
        cache.get_or_measure("cached", &FontSpec::DEFAULT, &text);
        let mut commands = vec![fill()];
        for x in [8.0, 600.0] {
            commands.push(DrawCommand::TextLine {
                origin: Point { x, y: 8.0 },
                content: "cached".into(),
                range: (0, 6),
                color: Color::BLACK,
                font: FontSpec::DEFAULT,
            });
        }
        let display = DisplayList::from(commands);
        for _ in 0..3 {
            cache.begin_frame();
            assert!(
                Scene::new(&display, (600, 0, 640, 32), 1, Color::WHITE, &text, &cache).is_some()
            );
        }
        assert_eq!(
            text.0.get(),
            1,
            "patch admission reshaped an already measured line"
        );
    }

    #[test]
    fn physical_scene_reuses_fractional_changes_but_keeps_visible_ink() {
        let commands = |height, clip_height, color| {
            DisplayList::from(vec![
                DrawCommand::PushClip {
                    rect: Rect {
                        origin: Point { x: 0.0, y: 0.0 },
                        size: Size {
                            width: 24.0,
                            height: clip_height,
                        },
                    },
                    corner_radius: Corners::ZERO,
                },
                DrawCommand::FillRect {
                    rect: Rect {
                        origin: Point { x: 2.25, y: 2.25 },
                        size: Size { width: 4.0, height },
                    },
                    color,
                    corner_radius: Corners::ZERO,
                },
                DrawCommand::PopClip,
            ])
        };
        let first = commands(10.1, 20.0, Color::BLACK);
        let same = commands(10.2, 20.0, Color::BLACK);
        assert_ne!(first.as_slice(), same.as_slice());
        for scale in [1, 2] {
            let s = scale as i64;
            let scene = |display: &DisplayList| {
                Scene::new(
                    display,
                    (0, 0, 24 * s, 24 * s),
                    scale,
                    Color::WHITE,
                    &PixelFont,
                    &MeasureCache::default(),
                )
                .expect("bounded fractional scene")
            };
            let first = scene(&first);
            let pixels = first
                .raster(&PixelFont, &RawImages::default())
                .to_rgba_bytes();
            let same = scene(&same);
            assert!(first.matches(&same), "scale {scale}");
            assert_eq!(
                pixels,
                same.raster(&PixelFont, &RawImages::default())
                    .to_rgba_bytes()
            );
            for changed in [
                commands(10.75, 20.0, Color::BLACK),
                commands(10.1, 8.0, Color::BLACK),
                commands(10.1, 20.0, Color::hex(0x123456)),
            ] {
                let changed = scene(&changed);
                assert!(!first.matches(&changed), "scale {scale}");
                assert_ne!(
                    pixels,
                    changed
                        .raster(&PixelFont, &RawImages::default())
                        .to_rgba_bytes()
                );
            }
        }
    }

    #[test]
    fn equal_local_pixels_do_not_reuse_a_different_destination_or_scale() {
        let scene = |rect: DamageRect, scale: usize, canvas| {
            let factor = scale as f64;
            let display = DisplayList::from(vec![DrawCommand::FillRect {
                rect: Rect {
                    origin: Point {
                        x: rect.0 as f64 / factor,
                        y: rect.1 as f64 / factor,
                    },
                    size: Size {
                        width: (rect.2 - rect.0) as f64 / factor,
                        height: (rect.3 - rect.1) as f64 / factor,
                    },
                },
                color: Color::WHITE.fade(),
                corner_radius: Corners::ZERO,
            }]);
            Scene::new(
                &display,
                rect,
                scale,
                canvas,
                &PixelFont,
                &MeasureCache::default(),
            )
            .expect("bounded translucent ink on an opaque canvas")
        };
        let first = scene((0, 0, 8, 8), 1, Color::WHITE);
        let pixels = first
            .raster(&PixelFont, &RawImages::default())
            .to_rgba_bytes();
        for changed in [
            scene((8, 0, 16, 8), 1, Color::WHITE),
            scene((0, 0, 8, 8), 2, Color::WHITE),
        ] {
            assert_eq!(
                pixels,
                changed
                    .raster(&PixelFont, &RawImages::default())
                    .to_rgba_bytes()
            );
            assert!(!first.matches(&changed));
        }
        let changed = scene((0, 0, 8, 8), 1, Color::BLACK);
        assert!(!first.matches(&changed));
        assert_ne!(
            pixels,
            changed
                .raster(&PixelFont, &RawImages::default())
                .to_rgba_bytes()
        );
    }

    #[link(name = "IOSurface", kind = "framework")]
    unsafe extern "C" {
        fn IOSurfaceIncrementUseCount(surface: Id);
        fn IOSurfaceDecrementUseCount(surface: Id);
    }
    struct InUse(Id);
    impl InUse {
        fn new(surface: Id) -> Self {
            unsafe {
                IOSurfaceIncrementUseCount(surface);
            }
            Self(surface)
        }
    }
    impl Drop for InUse {
        fn drop(&mut self) {
            unsafe {
                IOSurfaceDecrementUseCount(self.0);
            }
        }
    }

    #[test]
    fn a_surface_keeps_its_pixels_while_the_layer_or_compositor_reads_it() {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let mut backing = Backing::default();
            let bitmap = rasterize_with(
                &DisplayList::default(),
                3,
                2,
                1,
                Color::hex(0x123456),
                &PixelFont,
                &RawImages::default(),
            );
            let first = backing.prepare(&bitmap).expect("first surface");
            let first_busy = InUse::new(first);
            let second = backing.prepare(&bitmap).expect("second surface");
            assert_ne!(first, second);
            let second_busy = InUse::new(second);
            let third = backing.prepare(&bitmap).expect("third surface");
            assert_ne!(third, first);
            assert_ne!(third, second);
            // The current surface is protected even without a system use
            // count; both prior ones have a real IOSurface use count.
            assert!(backing.prepare(&bitmap).is_none());
            assert_eq!(backing.surfaces.len(), MAX_SURFACES);
            assert_eq!(IOSurfaceLock(first, 1, null_mut()), 0);
            let bytes = IOSurfaceGetBaseAddress(first).cast::<u8>();
            let stride = IOSurfaceGetBytesPerRow(first);
            for row in 0..2 {
                assert_eq!(
                    std::slice::from_raw_parts(bytes.add(row * stride), 12),
                    &[
                        0x56, 0x34, 0x12, 0xff, 0x56, 0x34, 0x12, 0xff, 0x56, 0x34, 0x12, 0xff
                    ]
                );
            }
            assert_eq!(IOSurfaceUnlock(first, 1, null_mut()), 0);
            drop(second_busy);
            assert_eq!(backing.prepare(&bitmap), Some(second));
            drop(first_busy);
            drop(backing);
            objc_autoreleasePoolPop(pool);
        }
    }
}
