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

/// The bounded, translated scene that paints an entire opaque patch.
pub(super) struct Scene {
    display: DisplayList,
    size: (usize, usize),
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
        let width = usize::try_from(rect.2.checked_sub(rect.0)?).ok()?;
        let height = usize::try_from(rect.3.checked_sub(rect.1)?).ok()?;
        if scale == 0
            || canvas.a != 255
            || width == 0
            || height == 0
            || width.checked_mul(height)? > MAX_PIXELS
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
        for command in lifted.iter() {
            match command {
                DrawCommand::FillRect { .. }
                | DrawCommand::StrokeRect { .. }
                | DrawCommand::PushClip { .. }
                | DrawCommand::PopClip => {}
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
                    if text_pixels > MAX_PIXELS {
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
        })
    }

    pub(super) fn raster(
        &self,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) -> Bitmap {
        rasterize_with(
            &self.display,
            self.size.0,
            self.size.1,
            scale,
            canvas,
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
pub(super) fn patch_coordinates(display: &DisplayList, patch: DamageRect, factor: f64) -> Option<DisplayList> {
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

pub(super) struct Surface {
    pub(super) raw: Id,
    pub(super) size: (usize, usize),
}

impl Surface {
    pub(super) fn new(size: (usize, usize)) -> Option<Self> {
        unsafe {
            let properties = msg_id(class("NSMutableDictionary"), sel("dictionary"));
            if properties.is_null() {
                return None;
            }
            for (key, value) in [
                ("IOSurfaceWidth", size.0 as u64),
                ("IOSurfaceHeight", size.1 as u64),
                ("IOSurfaceBytesPerElement", 4),
                ("IOSurfacePixelFormat", 0x4247_5241), // BGRA
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
            Some(Self { raw, size })
        }
    }

    pub(super) fn busy(&self) -> bool {
        unsafe { IOSurfaceIsInUse(self.raw) != 0 }
    }

    pub(super) fn write(&self, bitmap: &Bitmap) -> bool {
        if self.size != (bitmap.width(), bitmap.height()) {
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
                for (row, pixels) in bitmap.pixels().chunks_exact(self.size.0).enumerate() {
                    let output =
                        std::slice::from_raw_parts_mut(base.add(row * stride), self.size.0 * 4);
                    for (rgba, bgra) in pixels.iter().zip(output.as_chunks_mut::<4>().0) {
                        // An opaque canvas keeps every result opaque: the CPU
                        // packed RGBA becomes little-endian BGRA without alpha conversion.
                        bgra.copy_from_slice(&rgba.rotate_right(8).to_le_bytes());
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
        let patch = scene.raster(scale, Color::WHITE, text, &RawImages::default());
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
