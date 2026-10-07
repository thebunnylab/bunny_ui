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
        let (_, lifted) = carve_covering(display, (0, display.len()), logical)?;
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
                    let metrics = text.measure_line(&content[range.0..range.1], font);
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
        let display =
            lifted.translated_slice((0, lifted.len()), -logical.origin.x, -logical.origin.y);
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

struct Surface {
    raw: Id,
    size: (usize, usize),
}

impl Surface {
    fn new(size: (usize, usize)) -> Option<Self> {
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

    fn busy(&self) -> bool {
        unsafe { IOSurfaceIsInUse(self.raw) != 0 }
    }

    fn write(&self, bitmap: &Bitmap) -> bool {
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
        assert!(Scene::new(&display, (0, 0, 128, 128), 1, Color::CANVAS, &PixelFont).is_some());
        assert!(Scene::new(&display, (0, 0, 640, 640), 1, Color::CANVAS, &PixelFont).is_none());
        assert!(Scene::new(&display, (0, 0, 128, 128), 0, Color::CANVAS, &PixelFont).is_none());
        assert!(
            Scene::new(
                &display,
                (0, 0, 128, 128),
                1,
                Color::CANVAS.fade(),
                &PixelFont
            )
            .is_none()
        );
        let commands = DisplayList::from(vec![fill(); MAX_COMMANDS + 1]);
        assert!(Scene::new(&commands, (0, 0, 128, 128), 1, Color::CANVAS, &PixelFont).is_none());
        let long = DisplayList::from(vec![DrawCommand::TextLine {
            origin: Point { x: 0.0, y: 0.0 },
            content: "x".repeat(MAX_TEXT_BYTES + 1).into(),
            range: (0, MAX_TEXT_BYTES + 1),
            color: Color::BLACK,
            font: FontSpec::DEFAULT,
        }]);
        assert!(Scene::new(&long, (0, 0, 128, 128), 1, Color::CANVAS, &PixelFont).is_none());
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
        assert!(Scene::new(&huge_glyph, (0, 0, 128, 128), 1, Color::CANVAS, &crate::CoreTextEngine::new()).is_none());
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
        assert!(Scene::new(&shadow, (0, 0, 128, 128), 1, Color::CANVAS, &PixelFont).is_none());
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
