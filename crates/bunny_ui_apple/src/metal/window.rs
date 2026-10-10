//! A window may start on the existing native band Strategy without creating
//! a Metal queue. Unsupported paint promotes it once, in one transaction.

use super::*;
use bunny_ui::raster::{Bitmap, rasterize_with};

const MAX_BASE_PIXELS: usize = 8 * 1024 * 1024;

/// The window base is prepared once. An eight-bit image can declare its
/// opaque pixels without imposing image preparation on entering scroll rows.
struct OpaqueBase(Id);

impl OpaqueBase {
    fn new(bitmap: &Bitmap) -> Option<Self> {
        if bitmap.width() == 0
            || bitmap.height() == 0
            || bitmap.pixels().iter().any(|pixel| pixel & 255 != 255)
        {
            return None;
        }
        let stride = bitmap.width().checked_mul(4)?;
        let pixels = bitmap.to_rgba_bytes();
        unsafe {
            let provider = crate::ffi::owned_provider(pixels.as_ptr(), pixels.len());
            if provider.is_null() {
                return None;
            }
            let space = crate::ffi::CGColorSpaceCreateDeviceRGB();
            if space.is_null() {
                crate::ffi::CGDataProviderRelease(provider);
                return None;
            }
            // kCGImageAlphaNoneSkipLast: RGBX bytes, no transparency.
            let image = crate::ffi::CGImageCreate(
                bitmap.width(),
                bitmap.height(),
                8,
                32,
                stride,
                space,
                5,
                provider,
                std::ptr::null(),
                false,
                0,
            );
            crate::ffi::CGColorSpaceRelease(space);
            crate::ffi::CGDataProviderRelease(provider);
            (!image.is_null()).then(|| Self(image))
        }
    }
}

impl Drop for OpaqueBase {
    fn drop(&mut self) {
        unsafe { crate::ffi::CGImageRelease(self.0) };
    }
}

enum BaseBacking {
    Image(OpaqueBase),
    Surface(software_patch::Surface),
}

impl BaseBacking {
    fn new(bitmap: &Bitmap) -> Option<Self> {
        if let Some(image) = OpaqueBase::new(bitmap) {
            return Some(Self::Image(image));
        }
        let surface = software_patch::Surface::new((bitmap.width(), bitmap.height()))?;
        surface.write(bitmap).then_some(Self::Surface(surface))
    }

    fn raw(&self) -> Id {
        match self {
            Self::Image(image) => image.0,
            Self::Surface(surface) => surface.raw,
        }
    }
}

struct BaseLayer {
    raw: Id,
    backing: BaseBacking,
}

impl BaseLayer {
    unsafe fn new(root: Id) -> Option<Self> {
        unsafe {
            let backing = BaseBacking::new(&Bitmap::new(1, 1, bunny_ui::theme::canvas()))?;
            let raw = msg_id(msg_id(class("CALayer"), sel("alloc")), sel("init"));
            if raw.is_null() {
                return None;
            }
            kill_layer_actions(raw);
            msg_void_bool(raw, sel("setOpaque:"), 1);
            msg_void_id(raw, sel("setContents:"), backing.raw());
            msg_void_id_u64(root, sel("insertSublayer:atIndex:"), raw, 0);
            Some(Self { raw, backing })
        }
    }

    unsafe fn size(&self, size: Size, scale: usize) {
        unsafe {
            msg_void_f64(self.raw, sel("setContentsScale:"), scale as f64);
            msg_void_rect(
                self.raw,
                sel("setFrame:"),
                CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: size.width,
                        height: size.height,
                    },
                },
            );
        }
    }

    unsafe fn paint(&mut self, bitmap: &Bitmap, size: Size, scale: usize) -> bool {
        let Some(backing) = BaseBacking::new(bitmap) else {
            return false;
        };
        unsafe {
            self.size(size, scale);
            msg_void_id(self.raw, sel("setContents:"), backing.raw());
        }
        self.backing = backing;
        true
    }
}

impl Drop for BaseLayer {
    fn drop(&mut self) {
        unsafe {
            msg_void(self.raw, sel("removeFromSuperlayer"));
            msg_void_id(self.raw, sel("setContents:"), null_mut());
            msg_void(self.raw, sel("release"));
        }
    }
}

enum NativeState {
    Choosing,
    Bands(KeptFrame),
    Software(Option<KeptFrame>),
}

struct Native {
    base: BaseLayer,
    bands: scroll_bands::Presenter,
    boxes: MeasureCache,
    state: NativeState,
}

enum Strategy {
    Native(Box<Native>),
    Metal(Box<Metal>),
}

/// The native cover remains owned until the first GPU frame succeeds.
struct Metal {
    presenter: MetalPresenter,
    cover: Option<Box<Native>>,
}

impl Metal {
    #[allow(
        clippy::too_many_arguments,
        reason = "same presentation boundary as MetalPresenter"
    )]
    fn present(
        &mut self,
        display: &DisplayList,
        size: Size,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
        live: bool,
    ) {
        if self.cover.is_none() {
            self.presenter
                .present(display, size, scale, canvas, text, images, live);
            return;
        }
        unsafe {
            let transaction = class("CATransaction");
            msg_void(transaction, sel("begin"));
            msg_void_bool(transaction, sel("setDisableActions:"), 1);
            self.presenter
                .present(display, size, scale, canvas, text, images, true);
            // A missing drawable is an aborted frame. Keep the old content
            // until a later present succeeds instead of uncovering black.
            if self.presenter.retained.is_some() {
                self.cover.take();
            }
            msg_void(transaction, sel("commit"));
        }
    }
}

/// A macOS window's native-band Strategy, with one-way promotion to Metal.
/// The scene contract decides; no application identity enters admission.
pub struct WindowPresenter {
    layer: Id,
    strategy: Strategy,
}

impl WindowPresenter {
    /// Installs a native base. An explicit CPU override keeps the shell's
    /// existing CPU backend; no Metal device or queue is created here.
    ///
    /// # Safety
    /// `layer` must be a live `CAMetalLayer` owned by the calling main
    /// thread and must outlive this presenter.
    pub unsafe fn attach(layer: Id, scale: f64) -> Option<Self> {
        if layer.is_null() || std::env::var("BUNNY_PRESENT").ok().as_deref() == Some("cpu") {
            return None;
        }
        let base = unsafe { BaseLayer::new(layer)? };
        unsafe {
            kill_layer_actions(layer);
            msg_void_bool(layer, sel("setOpaque:"), 1);
            msg_void_f64(layer, sel("setContentsScale:"), scale.round().max(1.0));
        }
        Some(Self {
            layer,
            strategy: Strategy::Native(Box::new(Native {
                base,
                bands: scroll_bands::Presenter::default(),
                boxes: MeasureCache::default(),
                state: NativeState::Choosing,
            })),
        })
    }

    /// Primes the window with its canvas without starting a GPU queue.
    pub fn prime(&mut self, width: f64, height: f64, scale: usize) {
        match &mut self.strategy {
            Strategy::Native(native) => unsafe { native.base.size(Size { width, height }, scale) },
            Strategy::Metal(metal) => metal.presenter.prime(width, height, scale),
        }
    }

    /// Presents the scene or promotes to the existing Metal renderer when
    /// native-band admission no longer proves the requested pixels.
    #[allow(
        clippy::too_many_arguments,
        reason = "same presentation boundary as MetalPresenter"
    )]
    pub fn present(
        &mut self,
        display: &DisplayList,
        size: Size,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
        live: bool,
    ) {
        let pool = unsafe { objc_autoreleasePoolPush() };
        self.present_inner(display, size, scale, canvas, text, images, live);
        unsafe { objc_autoreleasePoolPop(pool) };
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "same presentation boundary as MetalPresenter"
    )]
    fn present_inner(
        &mut self,
        display: &DisplayList,
        size: Size,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
        live: bool,
    ) {
        if let Strategy::Metal(metal) = &mut self.strategy {
            metal.present(display, size, scale, canvas, text, images, live);
            return;
        }
        let physical = (
            (size.width.round().max(0.0) as usize).saturating_mul(scale),
            (size.height.round().max(0.0) as usize).saturating_mul(scale),
        );
        if scale == 0 || physical.0 == 0 || physical.1 == 0 {
            return;
        }
        let Strategy::Native(native) = &mut self.strategy else {
            return;
        };
        let repeats = match &native.state {
            NativeState::Choosing => false,
            NativeState::Bands((prior, p, s, c)) => {
                *p == physical
                    && *s == scale
                    && *c == canvas
                    && prior.as_slice() == display.as_slice()
            }
            NativeState::Software(prior) => frame_repeats(prior, display, physical, scale, canvas),
        };
        if repeats {
            return;
        }
        native.boxes.begin_frame();
        let admitted = match &native.state {
            NativeState::Choosing => {
                !live
                    && physical
                        .0
                        .checked_mul(physical.1)
                        .is_some_and(|n| n <= MAX_BASE_PIXELS)
                    && native
                        .bands
                        .seed(display, physical, scale, canvas, text, &native.boxes)
            }
            NativeState::Bands((_, p, s, c)) => {
                if !live
                    && *p == physical
                    && *s == scale
                    && *c == canvas
                    && unsafe {
                        native.bands.present(scroll_bands::Frame {
                            root: native.base.raw,
                            display,
                            physical,
                            scale,
                            canvas,
                            text,
                            images,
                            boxes: &native.boxes,
                        })
                    }
                {
                    native.state =
                        NativeState::Bands((Rc::new(display.clone()), physical, scale, canvas));
                    return;
                }
                false
            }
            NativeState::Software(_) => true,
        };
        if admitted {
            let bitmap =
                rasterize_with(display, physical.0, physical.1, scale, canvas, text, images);
            if unsafe { native.base.paint(&bitmap, size, scale) } {
                let kept = (Rc::new(display.clone()), physical, scale, canvas);
                native.state = match native.state {
                    NativeState::Software(_) => NativeState::Software(Some(kept)),
                    _ => NativeState::Bands(kept),
                };
                return;
            }
        }
        let Some(metal) = MetalPresenter::attach(self.layer, scale as f64) else {
            // The Metal backend already reports why it refused. Keep the
            // software fallback, including after a previously native frame.
            native.state = NativeState::Software(None);
            unsafe { native.bands.hide() };
            let bitmap =
                rasterize_with(display, physical.0, physical.1, scale, canvas, text, images);
            if unsafe { native.base.paint(&bitmap, size, scale) } {
                native.state = NativeState::Software(Some((
                    Rc::new(display.clone()),
                    physical,
                    scale,
                    canvas,
                )));
            } else {
                eprintln!("bunny_ui: native fallback could not allocate its visible frame");
            }
            return;
        };
        let prior = std::mem::replace(
            &mut self.strategy,
            Strategy::Metal(Box::new(Metal {
                presenter: metal,
                cover: None,
            })),
        );
        if let Strategy::Metal(metal) = &mut self.strategy {
            if let Strategy::Native(native) = prior {
                metal.cover = Some(native);
            }
            metal.present(display, size, scale, canvas, text, images, live);
        }
    }

    /// Offers idle GPU resources only after the Metal Strategy was needed.
    pub fn rest(&mut self) -> bool {
        match &mut self.strategy {
            Strategy::Native(_) => true,
            Strategy::Metal(m) => m.presenter.rest(),
        }
    }
    /// Offers retired drawables when a Metal presentation has landed.
    pub fn offer_drawables(&mut self) -> bool {
        match &mut self.strategy {
            Strategy::Native(_) => true,
            Strategy::Metal(m) => m.presenter.offer_drawables(),
        }
    }
    /// Keeps native frames transactional and forwards any requirement to
    /// Metal. Once required, that layer keeps its transactional contract.
    pub fn set_transactional(&mut self, live: bool) {
        if let Strategy::Metal(m) = &mut self.strategy {
            m.presenter.set_transactional(live);
        }
    }
    /// Reports no drawable wait for a native composition.
    pub fn drawable_wait_ms(&self) -> f64 {
        match &self.strategy {
            Strategy::Native(_) => 0.0,
            Strategy::Metal(m) => m.presenter.drawable_wait_ms(),
        }
    }
    /// Reports no GPU atlas before the scene has required one.
    pub fn atlas_counts(&self) -> AtlasCounts {
        match &self.strategy {
            Strategy::Native(_) => AtlasCounts::default(),
            Strategy::Metal(m) => m.presenter.atlas_counts(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bunny_ui::image_engine::RawImages;
    use bunny_ui::layout::{Corners, DrawCommand, Point, Rect};
    use bunny_ui::text_engine::{FontSpec, PixelFont};
    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            origin: Point { x, y },
            size: Size { width, height },
        }
    }
    fn scene(offset: f64) -> DisplayList {
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
                content: format!("row {row}").into(),
                range: (0, 5),
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
    fn the_static_image_owns_its_bytes_and_refuses_transparency() {
        unsafe extern "C" {
            fn CGImageGetAlphaInfo(image: Id) -> u32;
            fn CGImageGetDataProvider(image: Id) -> Id;
            fn CGDataProviderCopyData(provider: Id) -> Id;
            fn CFDataGetLength(data: *const std::ffi::c_void) -> isize;
            fn CFDataGetBytePtr(data: *const std::ffi::c_void) -> *const u8;
        }
        let image = {
            let bitmap = Bitmap::new(2, 2, Color::hex(0x123456));
            OpaqueBase::new(&bitmap).unwrap()
        };
        unsafe {
            assert_eq!(CGImageGetAlphaInfo(image.0), 5);
            let data = CGDataProviderCopyData(CGImageGetDataProvider(image.0));
            assert!(!data.is_null());
            assert_eq!(CFDataGetLength(data), 16);
            assert_eq!(
                std::slice::from_raw_parts(CFDataGetBytePtr(data), 16),
                &[0x12, 0x34, 0x56, 0xff].repeat(4)
            );
            CFRelease(data);
        }
        let transparent = Bitmap::new(
            2,
            2,
            Color {
                r: 10,
                g: 20,
                b: 30,
                a: 128,
            },
        );
        assert!(OpaqueBase::new(&transparent).is_none());
        assert!(matches!(
            BaseBacking::new(&transparent),
            Some(BaseBacking::Surface(_))
        ));
    }

    #[test]
    fn native_prime_repeats_and_band_motion_never_create_a_metal_device() {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
            for scale in [1, 2] {
                let mut presenter = WindowPresenter::attach(layer, scale as f64).unwrap();
                presenter.prime(180.0, 130.0, scale);
                assert!(
                    msg_id(layer, sel("device")).is_null(),
                    "the prime is native"
                );
                for offset in [0.0, 0.0, 24.5, 48.0, 17.0, 0.0] {
                    presenter.present(
                        &scene(offset),
                        Size {
                            width: 180.0,
                            height: 130.0,
                        },
                        scale,
                        Color::WHITE,
                        &PixelFont,
                        &RawImages::default(),
                        false,
                    );
                    assert!(
                        matches!(&presenter.strategy, Strategy::Native(n) if matches!(n.state, NativeState::Bands(_)))
                    );
                    assert!(
                        msg_id(layer, sel("device")).is_null(),
                        "no queue exists to wake at rest"
                    );
                    assert!(presenter.rest());
                }
                drop(presenter);
            }
            msg_void(layer, sel("release"));
            objc_autoreleasePoolPop(pool);
        }
    }

    #[test]
    fn leaving_band_admission_promotes_once_and_keeps_the_metal_strategy() {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
            let mut presenter = WindowPresenter::attach(layer, 1.0).unwrap();
            let size = Size {
                width: 180.0,
                height: 130.0,
            };
            let images = RawImages::default();
            presenter.present(
                &scene(0.0),
                size,
                1,
                Color::WHITE,
                &PixelFont,
                &images,
                false,
            );
            assert!(matches!(presenter.strategy, Strategy::Native(_)));
            presenter.present(
                &DisplayList::default(),
                size,
                1,
                Color::WHITE,
                &PixelFont,
                &images,
                false,
            );
            assert!(matches!(presenter.strategy, Strategy::Metal(_)));
            assert!(!msg_id(layer, sel("device")).is_null());
            presenter.present(
                &scene(0.0),
                size,
                1,
                Color::WHITE,
                &PixelFont,
                &images,
                false,
            );
            assert!(matches!(presenter.strategy, Strategy::Metal(_)));
            presenter.rest();
            drop(presenter);
            msg_void(layer, sel("release"));
            objc_autoreleasePoolPop(pool);
        }
    }

    #[test]
    fn changed_geometry_canvas_and_live_resize_require_metal() {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let initial = Size {
                width: 180.0,
                height: 130.0,
            };
            for (size, scale, canvas, live) in [
                (
                    Size {
                        width: 200.0,
                        height: 130.0,
                    },
                    1,
                    Color::WHITE,
                    false,
                ),
                (initial, 2, Color::WHITE, false),
                (initial, 1, Color::BLACK, false),
                (initial, 1, Color::WHITE, true),
            ] {
                let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
                let mut presenter = WindowPresenter::attach(layer, 1.0).unwrap();
                presenter.present(
                    &scene(0.0),
                    initial,
                    1,
                    Color::WHITE,
                    &PixelFont,
                    &RawImages::default(),
                    false,
                );
                // Motion plus the changed presentation contract must not
                // reuse an old viewport or old scale as a native band.
                presenter.present(
                    &scene(24.0),
                    size,
                    scale,
                    canvas,
                    &PixelFont,
                    &RawImages::default(),
                    live,
                );
                assert!(matches!(presenter.strategy, Strategy::Metal(_)));
                drop(presenter);
                msg_void(layer, sel("release"));
            }
            objc_autoreleasePoolPop(pool);
        }
    }

    #[test]
    fn an_aborted_first_gpu_frame_keeps_the_native_cover() {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
            let native = Native {
                base: BaseLayer::new(layer).unwrap(),
                bands: scroll_bands::Presenter::default(),
                boxes: MeasureCache::default(),
                state: NativeState::Choosing,
            };
            let mut metal = Metal {
                presenter: MetalPresenter::attach(layer, 1.0).unwrap(),
                cover: Some(Box::new(native)),
            };
            metal.present(
                &DisplayList::default(),
                Size {
                    width: 0.0,
                    height: 0.0,
                },
                1,
                Color::WHITE,
                &PixelFont,
                &RawImages::default(),
                false,
            );
            assert!(
                metal.cover.is_some(),
                "an aborted frame must not uncover black"
            );
            metal.present(
                &scene(0.0),
                Size {
                    width: 180.0,
                    height: 130.0,
                },
                1,
                Color::WHITE,
                &PixelFont,
                &RawImages::default(),
                false,
            );
            assert!(metal.cover.is_none(), "a presented frame removes the cover");
            assert!(metal.presenter.transactional);
            // The handoff can still belong to an outer AppKit transaction.
            // A caller no longer requiring coordination cannot undo it.
            metal.presenter.set_transactional(false);
            assert!(metal.presenter.transactional);
            let prior = metal.presenter.retained.clone().unwrap().0;
            let cursor = metal.presenter.cursor;
            metal.present(
                &scene(0.0),
                Size {
                    width: 180.0,
                    height: 130.0,
                },
                1,
                Color::WHITE,
                &PixelFont,
                &RawImages::default(),
                false,
            );
            assert!(Rc::ptr_eq(
                &prior,
                &metal.presenter.retained.as_ref().unwrap().0
            ));
            assert_eq!(
                cursor, metal.presenter.cursor,
                "a repeated frame still skips encoding"
            );
            drop(metal);
            msg_void(layer, sel("release"));
            objc_autoreleasePoolPop(pool);
        }
    }
}
