//! A window may start with native opaque bands or a sparse foreground over
//! a solid base, or a bounded frame with small native patches, without a
//! Metal queue. Unsupported updates promote once,
//! in one transaction, keeping the native cover until a GPU frame succeeds.

use super::*;
use bunny_ui::raster::{Bitmap, rasterize_with};
use std::sync::Arc;

const MAX_BASE_PIXELS: usize = 8 * 1024 * 1024;

/// The window base is prepared once. An eight-bit image can declare its
/// opaque pixels without imposing image preparation on entering scroll rows.
struct OpaqueBase(Id);

impl OpaqueBase {
    fn new(bitmap: &Arc<Bitmap>) -> Option<Self> {
        if bitmap.width() == 0
            || bitmap.height() == 0
            || bitmap.pixels().iter().any(|pixel| pixel & 255 != 255)
        {
            return None;
        }
        let stride = bitmap.width().checked_mul(4)?;
        let bytes = bitmap
            .pixels()
            .len()
            .checked_mul(std::mem::size_of::<u32>())?;
        // The provider owns a strong reference, including after a layer retains
        // the image. Its release callback may run on a compositor thread.
        let owner = Arc::into_raw(Arc::clone(bitmap));
        unsafe {
            let provider = crate::ffi::CGDataProviderCreateWithData(
                owner.cast_mut().cast(),
                bitmap.pixels().as_ptr().cast(),
                bytes,
                Some(release_bitmap),
            );
            if provider.is_null() {
                drop(Arc::from_raw(owner));
                return None;
            }
            let space = crate::ffi::CGColorSpaceCreateDeviceRGB();
            if space.is_null() {
                crate::ffi::CGDataProviderRelease(provider);
                return None;
            }
            // Bitmap stores 0xRRGGBBAA words. Describe their native byte order
            // instead of allocating an RGBA byte copy and then a CFData copy.
            // kCGImageAlphaNoneSkipLast keeps the same opaque RGBX semantics.
            let byte_order = if cfg!(target_endian = "little") {
                2 << 12
            } else {
                4 << 12
            };
            let image = crate::ffi::CGImageCreate(
                bitmap.width(),
                bitmap.height(),
                8,
                32,
                stride,
                space,
                5 | byte_order,
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

// `info` is exactly the Arc reference transferred to a successful provider.
// The callback only releases that immutable allocation; it touches no UI state.
unsafe extern "C" fn release_bitmap(info: *mut c_void, _: *const c_void, _: usize) {
    unsafe { drop(Arc::from_raw(info.cast::<Bitmap>())) };
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
    fn new(bitmap: Bitmap) -> Option<Self> {
        // The same intrinsically opaque surface used by scroll bands can
        // also carry the static base without a second image upload backing.
        #[cfg(target_arch = "aarch64")]
        if bitmap.pixels().iter().all(|pixel| pixel & 255 == 255)
            && let Some(surface) =
                software_patch::Surface::new_opaque((bitmap.width(), bitmap.height()))
            && surface.write(&bitmap)
        {
            return Some(Self::Surface(surface));
        }
        let bitmap = Arc::new(bitmap);
        if let Some(image) = OpaqueBase::new(&bitmap) {
            return Some(Self::Image(image));
        }
        let surface = software_patch::Surface::new((bitmap.width(), bitmap.height()))?;
        surface.write(&bitmap).then_some(Self::Surface(surface))
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
            let backing = BaseBacking::new(Bitmap::new(1, 1, bunny_ui::theme::canvas()))?;
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

    unsafe fn paint_scene(
        &mut self,
        scene: &software_patch::Scene,
        size: Size,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
        cache: &MeasureCache,
    ) -> bool {
        if let Some(surface) = software_patch::Surface::from_scene(scene, text, images, cache) {
            unsafe {
                self.size(size, scene.scale());
                msg_void_id(self.raw, sel("setContents:"), surface.raw);
            }
            self.backing = BaseBacking::Surface(surface);
            true
        } else {
            unsafe { self.paint(scene.raster(text, images), size, scene.scale()) }
        }
    }

    unsafe fn paint(&mut self, bitmap: Bitmap, size: Size, scale: usize) -> bool {
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

/// A solid background plus a bounded opaque patch can stay native even
/// without a scroll partition. The remaining picture is retained unchanged;
/// any update outside this contract promotes through the usual handoff.
struct SparseScene {
    color: Color,
    ink: Option<software_patch::Scene>,
    physical: (usize, usize),
}

impl SparseScene {
    fn new(
        display: &DisplayList,
        physical: (usize, usize),
        scale: usize,
        text: &dyn TextEngine,
        boxes: &MeasureCache,
    ) -> Option<Self> {
        use bunny_ui::layout::DrawCommand;
        let background = display.as_slice().first()?;
        let DrawCommand::FillRect {
            rect,
            color,
            corner_radius,
        } = background
        else {
            return None;
        };
        if scale == 0
            || color.a != 255
            || !corner_radius.is_zero()
            || rect.origin.x != 0.0
            || rect.origin.y != 0.0
            || rect.size.width * scale as f64 != physical.0 as f64
            || rect.size.height * scale as f64 != physical.1 as f64
        {
            return None;
        }
        let ink = if display.as_slice()[1..]
            .iter()
            .all(|command| matches!(command, DrawCommand::PushClip { .. } | DrawCommand::PopClip))
        {
            // Clip stack changes without drawing do not alter the background.
            None
        } else {
            match list_damage(
                std::slice::from_ref(background),
                display.as_slice(),
                scale,
                physical,
                PATCH_COMMANDS,
                boxes,
                text,
            ) {
                ListDamage::Same => None,
                ListDamage::Rect(rect) => Some(software_patch::Scene::new(
                    display, rect, scale, *color, text, boxes,
                )?),
                ListDamage::Whole => return None,
            }
        };
        Some(Self {
            color: *color,
            ink,
            physical,
        })
    }
}

/// One native patch is measured against the immutable base, never the last
/// patch, so moving or shrinking ink cannot expose stale pixels underneath.
struct NativePatch {
    layer: Id,
    backing: software_patch::Backing,
    scene: Option<software_patch::Scene>,
}

impl NativePatch {
    unsafe fn new(root: Id, scale: usize) -> Option<Self> {
        unsafe {
            let layer = msg_id(msg_id(class("CALayer"), sel("alloc")), sel("init"));
            if layer.is_null() {
                return None;
            }
            kill_layer_actions(layer);
            msg_void_bool(layer, sel("setOpaque:"), 1);
            msg_void_bool(layer, sel("setHidden:"), 1);
            msg_void_f64(layer, sel("setContentsScale:"), scale as f64);
            msg_void_id_u64(root, sel("insertSublayer:atIndex:"), layer, 0);
            Some(Self {
                layer,
                backing: software_patch::Backing::default(),
                scene: None,
            })
        }
    }

    unsafe fn paint(
        &mut self,
        scene: software_patch::Scene,
        physical: (usize, usize),
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) -> bool {
        unsafe {
            let surface = if self
                .scene
                .as_ref()
                .is_some_and(|prior| prior.matches(&scene))
            {
                None
            } else {
                let bitmap = scene.raster(text, images);
                let Some(surface) = self.backing.prepare(&bitmap) else {
                    return false;
                };
                Some(surface)
            };
            let transaction = class("CATransaction");
            msg_void(transaction, sel("begin"));
            msg_void_bool(transaction, sel("setDisableActions:"), 1);
            if let Some(surface) = surface {
                msg_void_rect(
                    self.layer,
                    sel("setFrame:"),
                    Patch::frame(scene.bounds(), physical, scene.scale()),
                );
                msg_void_id(self.layer, sel("setContents:"), surface);
            }
            msg_void_bool(self.layer, sel("setHidden:"), 0);
            msg_void(transaction, sel("commit"));
            self.scene = Some(scene);
            true
        }
    }
}

impl Drop for NativePatch {
    fn drop(&mut self) {
        unsafe {
            msg_void(self.layer, sel("removeFromSuperlayer"));
            msg_void_id(self.layer, sel("setContents:"), null_mut());
            msg_void(self.layer, sel("release"));
        }
    }
}

struct NativeUpdates {
    base: KeptFrame,
    current: KeptFrame,
    patch: Option<NativePatch>,
}

impl NativeUpdates {
    /// The immutable base needs only the spans it painted, not every hidden
    /// byte of an old document. Current frames retain their cheap shared list.
    fn new(display: &DisplayList, physical: (usize, usize), scale: usize, canvas: Color) -> Self {
        let commands = display
            .iter()
            .cloned()
            .map(|mut command| {
                if let bunny_ui::layout::DrawCommand::TextLine { content, range, .. } = &mut command
                    && content.len() > 4096
                    && range.1 - range.0 < content.len() / 4
                {
                    *content = Arc::from(&content[range.0..range.1]);
                    *range = (0, content.len());
                }
                command
            })
            .collect::<Vec<_>>();
        let base = (
            Rc::new(DisplayList::from(commands)),
            physical,
            scale,
            canvas,
        );
        Self {
            current: base.clone(),
            base,
            patch: None,
        }
    }

    fn present(&mut self, frame: scroll_bands::Frame<'_>) -> bool {
        let scroll_bands::Frame {
            root,
            display,
            physical,
            scale,
            canvas,
            text,
            images,
            boxes,
        } = frame;
        if self.base.1 != physical || self.base.2 != scale || self.base.3 != canvas {
            return false;
        }
        let damage = list_damage(
            self.base.0.as_slice(),
            display.as_slice(),
            scale,
            physical,
            PATCH_COMMANDS,
            boxes,
            text,
        );
        match damage {
            ListDamage::Same => {
                if let Some(patch) = &self.patch {
                    unsafe { msg_void_bool(patch.layer, sel("setHidden:"), 1) };
                }
                self.current = self.base.clone();
                true
            }
            ListDamage::Rect(rect) => {
                let Some(rect) = patch_box(rect, physical) else {
                    return false;
                };
                let Some(scene) =
                    software_patch::Scene::new(display, rect, scale, canvas, text, boxes)
                else {
                    return false;
                };
                if self.patch.is_none() {
                    self.patch = unsafe { NativePatch::new(root, scale) };
                }
                let Some(patch) = &mut self.patch else {
                    return false;
                };
                if !unsafe { patch.paint(scene, physical, text, images) } {
                    return false;
                }
                self.current = (Rc::new(display.clone()), physical, scale, canvas);
                true
            }
            ListDamage::Whole => false,
        }
    }
}

enum NativeState {
    Choosing,
    Bands(KeptFrame),
    Sparse(KeptFrame),
    Patched(Box<NativeUpdates>),
    Software(Option<KeptFrame>),
}

struct Native {
    base: BaseLayer,
    ink: Option<BaseLayer>,
    bands: scroll_bands::Presenter,
    outside_checked: bool,
    boxes: MeasureCache,
    state: NativeState,
}

impl Native {
    fn paint_sparse(
        &mut self,
        scene: SparseScene,
        size: Size,
        scale: usize,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) -> bool {
        let ink = if let Some(paint) = scene.ink {
            let bitmap = paint.raster(text, images);
            let Some(mut layer) = (unsafe { BaseLayer::new(self.base.raw) }) else {
                return false;
            };
            let frame = Patch::frame(paint.bounds(), scene.physical, scale);
            let patch_size = Size {
                width: frame.size.width,
                height: frame.size.height,
            };
            if !unsafe { layer.paint(bitmap, patch_size, scale) } {
                return false;
            }
            unsafe { msg_void_rect(layer.raw, sel("setFrame:"), frame) };
            Some(layer)
        } else {
            None
        };
        if !unsafe { self.base.paint(Bitmap::new(1, 1, scene.color), size, scale) } {
            return false;
        }
        self.ink = ink;
        true
    }
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

/// A macOS window's native Strategy, with one-way promotion to Metal.
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
                ink: None,
                bands: scroll_bands::Presenter::default(),
                outside_checked: false,
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
            NativeState::Bands((prior, p, s, c)) | NativeState::Sparse((prior, p, s, c)) => {
                *p == physical
                    && *s == scale
                    && *c == canvas
                    && prior.as_slice() == display.as_slice()
            }
            NativeState::Patched(held) => {
                held.current.1 == physical
                    && held.current.2 == scale
                    && held.current.3 == canvas
                    && held.current.0.as_slice() == display.as_slice()
            }
            NativeState::Software(prior) => frame_repeats(prior, display, physical, scale, canvas),
        };
        if repeats {
            return;
        }
        native.boxes.begin_frame();
        let bands = matches!(native.state, NativeState::Choosing)
            && !live
            && physical
                .0
                .checked_mul(physical.1)
                .is_some_and(|n| n <= MAX_BASE_PIXELS)
            && native
                .bands
                .seed(display, physical, scale, canvas, text, &native.boxes);
        // A proved empty viewport has no ink layer to cover later bands.
        // Keep the seeded partition while avoiding a full-window allocation
        // that an allocator could retain even after the first rows arrive.
        if bands
            && let Some(scene) = SparseScene::new(display, physical, scale, text, &native.boxes)
            && scene.ink.is_none()
            && native.paint_sparse(scene, size, scale, text, images)
        {
            native.state = NativeState::Bands((Rc::new(display.clone()), physical, scale, canvas));
            return;
        }
        if matches!(native.state, NativeState::Choosing)
            && !live
            && !bands
            && let Some(scene) = SparseScene::new(display, physical, scale, text, &native.boxes)
            && native.paint_sparse(scene, size, scale, text, images)
        {
            native.state = NativeState::Sparse((Rc::new(display.clone()), physical, scale, canvas));
            return;
        }
        if matches!(native.state, NativeState::Choosing)
            && !live
            && !bands
            && let Some(scene) =
                software_patch::Scene::base(display, physical, scale, canvas, text, &native.boxes)
            && unsafe {
                native
                    .base
                    .paint_scene(&scene, size, text, images, &native.boxes)
            }
        {
            native.state = NativeState::Patched(Box::new(NativeUpdates::new(
                display, physical, scale, canvas,
            )));
            return;
        }
        if let NativeState::Patched(updates) = &mut native.state
            && !live
            && updates.present(scroll_bands::Frame {
                root: native.base.raw,
                display,
                physical,
                scale,
                canvas,
                text,
                images,
                boxes: &native.boxes,
            })
        {
            return;
        }
        let admitted = match &native.state {
            NativeState::Choosing => bands,
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
                    if !native.outside_checked {
                        // Compatible bands keep the outside picture unchanged.
                        // Retire the original full-window backing only after
                        // the opaque viewport is successfully visible. Refusal
                        // keeps it; no allocation retry runs on every scroll.
                        native.outside_checked = true;
                        if let Some(scene) = native.bands.outside().and_then(|outside| {
                            SparseScene::new(outside, physical, scale, text, &native.boxes)
                        }) {
                            native.paint_sparse(scene, size, scale, text, images);
                        }
                    }
                    native.state =
                        NativeState::Bands((Rc::new(display.clone()), physical, scale, canvas));
                    return;
                }
                false
            }
            NativeState::Sparse(_) | NativeState::Patched(_) => false,
            NativeState::Software(_) => true,
        };
        if admitted {
            let bitmap =
                rasterize_with(display, physical.0, physical.1, scale, canvas, text, images);
            if unsafe { native.base.paint(bitmap, size, scale) } {
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
            if unsafe { native.base.paint(bitmap, size, scale) } {
                native.ink = None;
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
    fn a_bounded_native_editor_keeps_small_edits_off_metal() {
        use bunny_ui::prelude::*;
        #[derive(Clone)]
        struct Editor {
            text: State<String>,
        }
        impl Component for Editor {
            fn body(self) -> impl View {
                text_editor("", self.text.binding())
                    .font_family("Menlo")
                    .font_size(13.0)
                    .auto_focus()
            }
        }
        unsafe {
            let pool = objc_autoreleasePoolPush();
            for (rows, scale) in [(400, 1), (30_000, 1), (400, 2), (30_000, 2)] {
                let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
                let text = Rc::new(crate::text::CoreTextEngine::new());
                let runtime = Runtime::new().text_engine(text.clone());
                runtime.drop_unseen();
                let editor = Editor {
                    text: State::new(
                        (0..rows)
                            .map(|row| format!("Line {row}: the editor keeps a long document.\n"))
                            .collect(),
                    ),
                };
                let size = Size {
                    width: 1280.0,
                    height: 800.0,
                };
                let mut presenter = WindowPresenter::attach(layer, scale as f64).unwrap();
                for step in 0..6 {
                    if step > 0 {
                        let edit = if step % 2 == 1 {
                            EditCommand::Insert("x".into())
                        } else {
                            EditCommand::Backspace
                        };
                        assert!(runtime.key(edit).applied);
                    }
                    let display = runtime.display_frame(&editor, size);
                    presenter.present(
                        &display,
                        size,
                        scale,
                        bunny_ui::theme::canvas(),
                        &*text,
                        &RawImages::default(),
                        false,
                    );
                    assert!(
                        matches!(presenter.strategy, Strategy::Native(_)),
                        "rows={rows}, scale={scale}, edit={step} must not allocate a Metal queue"
                    );
                    assert!(msg_id(layer, sel("device")).is_null());
                }
                assert!(presenter.rest());
                drop(presenter);
                msg_void(layer, sel("release"));
            }
            objc_autoreleasePoolPop(pool);
        }
    }

    #[test]
    fn a_native_base_witness_releases_hidden_document_bytes() {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
            let size = Size {
                width: 320.0,
                height: 240.0,
            };
            let mut presenter = WindowPresenter::attach(layer, 1.0).unwrap();
            let document: Arc<str> =
                format!("first visible line{}", "hidden text".repeat(100_000)).into();
            let lifetime = Arc::downgrade(&document);
            for content in [document, Arc::from("other visible line")] {
                let display = DisplayList::from(vec![
                    DrawCommand::FillRect {
                        rect: rect(0.0, 0.0, 320.0, 240.0),
                        color: Color::WHITE,
                        corner_radius: Corners::all(5.0),
                    },
                    DrawCommand::TextLine {
                        origin: Point { x: 20.0, y: 30.0 },
                        content,
                        range: (0, 18),
                        color: Color::BLACK,
                        font: FontSpec::DEFAULT,
                    },
                ]);
                presenter.present(
                    &display,
                    size,
                    1,
                    Color::BLACK,
                    &PixelFont,
                    &RawImages::default(),
                    false,
                );
            }
            assert!(matches!(presenter.strategy, Strategy::Native(_)));
            assert!(
                lifetime.upgrade().is_none(),
                "the immutable paint witness must not pin the original document"
            );
            drop(presenter);
            msg_void(layer, sel("release"));
            objc_autoreleasePoolPop(pool);
        }
    }

    #[test]
    fn native_patches_reconstruct_moving_and_shrinking_ink_against_the_base() {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
            let size = Size {
                width: 320.0,
                height: 240.0,
            };
            let display = |x, word: &str| {
                DisplayList::from(vec![
                    DrawCommand::FillRect {
                        rect: rect(0.0, 0.0, 320.0, 240.0),
                        color: Color::WHITE,
                        corner_radius: Corners::all(5.0),
                    },
                    DrawCommand::PushClip {
                        rect: rect(0.0, 0.0, 320.0, 240.0),
                        corner_radius: Corners::all(5.0),
                    },
                    DrawCommand::TextLine {
                        origin: Point { x, y: 30.5 },
                        content: word.into(),
                        range: (0, word.len()),
                        color: Color::BLACK,
                        font: FontSpec::DEFAULT,
                    },
                    DrawCommand::PopClip,
                ])
            };
            for scale in [1, 2] {
                let mut presenter = WindowPresenter::attach(layer, scale as f64).unwrap();
                let physical = (320 * scale, 240 * scale);
                for (x, word) in [
                    (20.5, "old"),
                    (20.5, "new longer"),
                    (80.5, "new"),
                    (20.5, "old"),
                    (0.5, "clipped"),
                ] {
                    let commands = display(x, word);
                    presenter.present(
                        &commands,
                        size,
                        scale,
                        Color::BLACK,
                        &PixelFont,
                        &RawImages::default(),
                        false,
                    );
                    let Strategy::Native(native) = &presenter.strategy else {
                        panic!("small changes stay native")
                    };
                    let NativeState::Patched(held) = &native.state else {
                        panic!("rounded base uses native patches")
                    };
                    let mut composed = rasterize_with(
                        &held.base.0,
                        physical.0,
                        physical.1,
                        scale,
                        Color::BLACK,
                        &PixelFont,
                        &RawImages::default(),
                    )
                    .to_rgba_bytes();
                    if held.current.0.as_slice() != held.base.0.as_slice() {
                        let patch = held.patch.as_ref().unwrap().scene.as_ref().unwrap();
                        let pixels = patch.raster(&PixelFont, &RawImages::default());
                        let bytes = pixels.to_rgba_bytes();
                        let bounds = patch.bounds();
                        for row in 0..pixels.height() {
                            let to =
                                ((bounds.1 as usize + row) * physical.0 + bounds.0 as usize) * 4;
                            let from = row * pixels.width() * 4;
                            composed[to..to + pixels.width() * 4]
                                .copy_from_slice(&bytes[from..from + pixels.width() * 4]);
                        }
                    }
                    let expected = rasterize_with(
                        &commands,
                        physical.0,
                        physical.1,
                        scale,
                        Color::BLACK,
                        &PixelFont,
                        &RawImages::default(),
                    )
                    .to_rgba_bytes();
                    assert_eq!(composed, expected, "scale={scale}, x={x}, word={word}");
                }
                // A resize cannot stretch the old base/patch composition.
                presenter.present(
                    &display(0.5, "clipped"),
                    Size {
                        width: 400.0,
                        height: 300.0,
                    },
                    scale,
                    Color::BLACK,
                    &PixelFont,
                    &RawImages::default(),
                    false,
                );
                assert!(matches!(presenter.strategy, Strategy::Metal(_)));
                drop(presenter);
                // Detach the device before exercising the next native seed.
                msg_void_id(layer, sel("setDevice:"), null_mut());
            }
            msg_void(layer, sel("release"));
            objc_autoreleasePoolPop(pool);
        }
    }

    #[test]
    fn the_static_image_keeps_the_original_allocation_until_its_last_owner() {
        let bitmap = Arc::new(Bitmap::new(2, 2, Color::hex(0x123456)));
        let lifetime = Arc::downgrade(&bitmap);
        let image = OpaqueBase::new(&bitmap).unwrap();
        drop(bitmap);
        assert!(
            lifetime.upgrade().is_some(),
            "the image must own the original pixels, not a copy"
        );
        unsafe { crate::ffi::CFRetain(image.0) };
        let retained = OpaqueBase(image.0);
        drop(image);
        assert!(
            lifetime.upgrade().is_some(),
            "CoreAnimation may retain the image after presentation"
        );
        drop(retained);
        assert!(
            lifetime.upgrade().is_none(),
            "the final image release frees the pixels"
        );
    }

    #[test]
    fn the_static_image_owns_its_bytes_and_refuses_transparency() {
        unsafe extern "C" {
            fn CGBitmapContextCreate(
                data: *mut c_void,
                width: usize,
                height: usize,
                bits: usize,
                stride: usize,
                space: Id,
                info: u32,
            ) -> Id;
            fn CGContextRelease(context: Id);
            fn CGImageGetAlphaInfo(image: Id) -> u32;
            fn CGImageGetDataProvider(image: Id) -> Id;
            fn CGDataProviderCopyData(provider: Id) -> Id;
            fn CFDataGetLength(data: *const std::ffi::c_void) -> isize;
            fn CFDataGetBytePtr(data: *const std::ffi::c_void) -> *const u8;
        }
        let image = {
            let bitmap = Arc::new(Bitmap::new(2, 2, Color::hex(0x123456)));
            OpaqueBase::new(&bitmap).unwrap()
        };
        unsafe {
            assert_eq!(CGImageGetAlphaInfo(image.0), 5);
            let data = CGDataProviderCopyData(CGImageGetDataProvider(image.0));
            assert!(!data.is_null());
            assert_eq!(CFDataGetLength(data), 16);
            assert_eq!(
                std::slice::from_raw_parts(CFDataGetBytePtr(data), 16),
                &0x123456ff_u32.to_ne_bytes().repeat(4)
            );
            CFRelease(data);
            // Reading provider bytes alone cannot catch a wrong byte-order tag.
            // Ask Quartz to interpret the image into an ordinary RGBA context.
            let mut rgba = [0_u8; 16];
            let space = crate::ffi::CGColorSpaceCreateDeviceRGB();
            let context = CGBitmapContextCreate(rgba.as_mut_ptr().cast(), 2, 2, 8, 8, space, 1);
            assert!(!context.is_null());
            crate::ffi::CGContextDrawImage(
                context,
                CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 2.0,
                        height: 2.0,
                    },
                },
                image.0,
            );
            CGContextRelease(context);
            crate::ffi::CGColorSpaceRelease(space);
            assert_eq!(rgba.as_slice(), [0x12, 0x34, 0x56, 0xff].repeat(4));
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
        assert!(
            OpaqueBase::new(&Arc::new(Bitmap::new(
                2,
                2,
                Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 0
                }
            )))
            .is_none()
        );
        assert!(matches!(
            BaseBacking::new(transparent),
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
                    #[cfg(target_arch = "aarch64")]
                    if offset != 0.0 {
                        let Strategy::Native(native) = &presenter.strategy else {
                            unreachable!()
                        };
                        assert!(
                            matches!(&native.base.backing, BaseBacking::Surface(surface) if surface.size == (1, 1)),
                            "the opaque bands replace the viewport pixels; their base retains only the outside scene"
                        );
                    }
                }
                drop(presenter);
            }
            msg_void(layer, sel("release"));
            objc_autoreleasePoolPop(pool);
        }
    }

    #[test]
    fn a_sparse_opaque_base_stays_native_until_its_pixels_change() {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
            let mut presenter = WindowPresenter::attach(layer, 1.0).unwrap();
            let size = Size {
                width: 1280.0,
                height: 800.0,
            };
            let display = DisplayList::from(vec![
                DrawCommand::FillRect {
                    rect: rect(0.0, 0.0, 1280.0, 800.0),
                    color: Color::BLACK,
                    corner_radius: Corners::ZERO,
                },
                DrawCommand::TextLine {
                    origin: Point { x: 12.0, y: 12.0 },
                    content: "small heading".into(),
                    range: (0, 13),
                    color: Color::WHITE,
                    font: FontSpec::DEFAULT,
                },
            ]);
            for _ in 0..2 {
                presenter.present(
                    &display,
                    size,
                    1,
                    Color::BLACK,
                    &PixelFont,
                    &RawImages::default(),
                    false,
                );
                assert!(
                    msg_id(layer, sel("device")).is_null(),
                    "a bounded heading over an opaque background needs no GPU queue"
                );
                assert!(presenter.rest());
            }
            presenter.present(
                &DisplayList::default(),
                size,
                1,
                Color::BLACK,
                &PixelFont,
                &RawImages::default(),
                false,
            );
            assert!(
                matches!(presenter.strategy, Strategy::Metal(_)),
                "an unsupported update promotes once"
            );
            drop(presenter);
            msg_void(layer, sel("release"));
            objc_autoreleasePoolPop(pool);
        }
    }

    #[test]
    fn an_empty_native_viewport_never_allocates_a_full_window_backing() {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
            for scale in [1, 2] {
                let mut presenter = WindowPresenter::attach(layer, scale as f64).unwrap();
                let size = Size {
                    width: 1280.0,
                    height: 800.0,
                };
                let display = DisplayList::from(vec![
                    DrawCommand::FillRect {
                        rect: rect(0.0, 0.0, 1280.0, 800.0),
                        color: Color::WHITE,
                        corner_radius: Corners::ZERO,
                    },
                    DrawCommand::PushClip {
                        rect: rect(0.0, 0.0, 1280.0, 800.0),
                        corner_radius: Corners::ZERO,
                    },
                    DrawCommand::PopClip,
                ]);
                presenter.present(
                    &display,
                    size,
                    scale,
                    Color::WHITE,
                    &PixelFont,
                    &RawImages::default(),
                    false,
                );
                let Strategy::Native(native) = &presenter.strategy else {
                    panic!("the empty viewport must stay native")
                };
                assert!(
                    matches!(native.state, NativeState::Bands(_)),
                    "preserve the seeded viewport for subsequent rows"
                );
                #[cfg(target_arch = "aarch64")]
                assert!(
                    matches!(&native.base.backing, BaseBacking::Surface(surface) if surface.size == (1, 1)),
                    "a uniform empty viewport needs just one background pixel"
                );
                assert!(msg_id(layer, sel("device")).is_null());
                assert!(presenter.rest());
                drop(presenter);
            }
            msg_void(layer, sel("release"));
            objc_autoreleasePoolPop(pool);
        }
    }

    #[test]
    fn a_runtime_list_can_grow_natively_but_a_broad_rewrite_promotes() {
        use bunny_ui::prelude::*;
        #[derive(Clone)]
        struct List {
            count: State<usize>,
            ink: State<Color>,
        }
        impl Component for List {
            fn body(self) -> impl View {
                virtual_list(
                    self.count.get(),
                    |row| format!("row-{row}"),
                    |row| {
                        text(format!("A line of text {row}"))
                            .frame_aligned(800.0, 28.0, Alignment::Leading)
                            .background_color(Color::hex(0xeeeeee))
                    },
                )
                .row_height(28.0)
                .font_size(13.0)
                .foreground_color(self.ink.get())
                .background_color(Color::WHITE)
            }
        }
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let layer = msg_id(msg_id(class("CAMetalLayer"), sel("alloc")), sel("init"));
            let mut presenter = WindowPresenter::attach(layer, 1.0).unwrap();
            let text = Rc::new(crate::text::CoreTextEngine::new());
            let runtime = Runtime::new().text_engine(text.clone());
            let list = List {
                count: State::new(0),
                ink: State::new(Color::BLACK),
            };
            let size = Size {
                width: 800.0,
                height: 600.0,
            };
            for count in 0..24 {
                list.count.set(count);
                let display = runtime.display_frame(&list, size);
                presenter.present(
                    &display,
                    size,
                    1,
                    Color::WHITE,
                    &*text,
                    &RawImages::default(),
                    false,
                );
                assert!(
                    msg_id(layer, sel("device")).is_null(),
                    "append {count} should use bounded new rows: {display:?}"
                );
            }
            list.ink.set(Color::hex(0xff0000));
            let display = runtime.display_frame(&list, size);
            presenter.present(
                &display,
                size,
                1,
                Color::WHITE,
                &*text,
                &RawImages::default(),
                false,
            );
            assert!(
                matches!(presenter.strategy, Strategy::Metal(_)),
                "repainting every row exceeds the CPU update budget"
            );
            drop(presenter);
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
                ink: None,
                base: BaseLayer::new(layer).unwrap(),
                bands: scroll_bands::Presenter::default(),
                outside_checked: false,
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
