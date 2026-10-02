//! A Metal frame the GPU already holds, composited with no CPU pixels.
//!
//! Two doors into the same frame. [`MetalFrame::from_texture`] takes a
//! texture the app filled itself or a `CVMetalTextureCache` minted from
//! a camera's or a decoder's pixel buffer — the frame retains it and
//! keeps whatever lease the app hands beside it alive until the last
//! command buffer that sampled it has completed. [`SurfacePool`], behind
//! the `wgpu-surface` feature, copies a completed wgpu texture GPU to GPU
//! into a bounded pool of leased frames; completion is polled by the
//! producer, never the UI.
//!
//! A frame is an image inside the display list: clipped and z-ordered
//! with the scene, scaled into its box by the linear sampler the feeds
//! use. The presenter retains the import until its Metal submissions
//! finish, so a texture is never rewritten while it is being sampled.
//!
//! ## Production gotchas
//!
//! - **Metal reads every ordered format in r, g, b, a.** A `BGRA8Unorm`
//!   texture samples as RGB through the same shader with no view and no
//!   swizzle; `a_bgra_texture_reads_in_rgb_order` holds the line.
//! - **The sRGB twins are refused by name.** A texture that decodes on
//!   read would land in linear light inside a gamma-space compositor.
//! - **The keep-alive is not optional.** A `CVPixelBuffer` released
//!   while its texture is in flight is a torn or black frame; hand the
//!   lease in and let the frame drop it.

use std::any::Any;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ffi::{Id, Sel, sel};

static NEXT_FRAME: AtomicU64 = AtomicU64::new(1);
/// Folded into every frame's key, so a frame never shares an identity
/// with a feed or a keyed image by accident.
const FRAME_TAG: u64 = 0x626e_795f_6770_7500;

// The same trampoline discipline as metal.rs: one alias per concrete
// message signature, re-declared where it is used.
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_u64(obj: Id, sel: Sel) -> u64;
    #[link_name = "objc_msgSend"]
    fn msg_void(obj: Id, sel: Sel);
}

// MTLPixelFormat, MTLTextureType and MTLTextureUsage — constants in
// source, like metal.rs keeps them
const PIXEL_FORMAT_RGBA8: u64 = 70;
const PIXEL_FORMAT_BGRA8: u64 = 80;
const TEXTURE_TYPE_2D: u64 = 2;
const TEXTURE_USAGE_SHADER_READ: u64 = 1;

/// The layouts a frame may carry: the memory order of `RGBA8Unorm` (70)
/// and of `BGRA8Unorm` (80). The shader reads either as r, g, b, a.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeFormat {
    Rgba8Unorm,
    Bgra8Unorm,
}

impl NativeFormat {
    fn of(pixel_format: u64) -> Result<NativeFormat, String> {
        match pixel_format {
            PIXEL_FORMAT_RGBA8 => Ok(NativeFormat::Rgba8Unorm),
            PIXEL_FORMAT_BGRA8 => Ok(NativeFormat::Bgra8Unorm),
            other => Err(format!(
                "a native frame wants RGBA8Unorm (70) or BGRA8Unorm (80), got MTLPixelFormat {other}"
            )),
        }
    }
}

/// A completed Metal texture the compositor samples where it lies.
///
/// Immutable by contract: the producer finished writing it before the
/// frame was made, and nothing writes it after. The frame retains the
/// texture once and releases it on drop; `keep` goes with it.
pub struct MetalFrame {
    /// Retained once by this frame.
    texture: Id,
    key: u64,
    size: (u32, u32),
    format: NativeFormat,
    /// Whatever owns the texture's backing — a pixel buffer, a cache
    /// entry, a wgpu view — alive as long as the frame is.
    #[allow(dead_code)] // held for its drop, never read
    keep: Arc<dyn Any + Send + Sync>,
}

// SAFETY: a Metal texture is reference counted from any thread, and this
// frame only ever hands out retained handles the compositor SAMPLES —
// nothing writes through it. The lease is `Send + Sync` by its bound.
unsafe impl Send for MetalFrame {}
unsafe impl Sync for MetalFrame {}

impl MetalFrame {
    /// A frame over a texture the GPU already holds.
    ///
    /// # Safety
    /// `texture` is a live `id<MTLTexture>`: 2D, one mipmap level,
    /// shader-readable, `RGBA8Unorm` or `BGRA8Unorm`, on the device the
    /// window presents with, and completely written. The frame retains
    /// it once more and releases it on drop. `keep` outlives the
    /// texture's backing and is dropped only after the last command
    /// buffer that sampled the frame completed.
    ///
    /// # Errors
    /// A nil texture, another format, another type, more than one level
    /// or a texture the shader cannot read — refused by name.
    pub unsafe fn from_texture(
        texture: *mut c_void,
        keep: Arc<dyn Any + Send + Sync>,
    ) -> Result<Arc<MetalFrame>, String> {
        if texture.is_null() {
            return Err("a native frame wants a texture, got nil".into());
        }
        unsafe {
            let format = NativeFormat::of(msg_u64(texture, sel("pixelFormat")))?;
            if msg_u64(texture, sel("textureType")) != TEXTURE_TYPE_2D {
                return Err("a native frame wants a 2D texture".into());
            }
            if msg_u64(texture, sel("mipmapLevelCount")) != 1 {
                return Err("a native frame wants one mipmap level".into());
            }
            if msg_u64(texture, sel("usage")) & TEXTURE_USAGE_SHADER_READ == 0 {
                return Err("a native frame wants a texture the shader can read".into());
            }
            let size = (
                msg_u64(texture, sel("width")) as u32,
                msg_u64(texture, sel("height")) as u32,
            );
            msg_void(texture, sel("retain"));
            Ok(Arc::new(MetalFrame {
                texture,
                key: NEXT_FRAME.fetch_add(1, Ordering::Relaxed) ^ FRAME_TAG,
                size,
                format,
                keep,
            }))
        }
    }

    /// The UI source; cloning transfers ownership, never pixel bytes.
    pub fn image(self: &Arc<Self>) -> bunny_ui::image_engine::ImageSource {
        bunny_ui::image_engine::ImageSource::Native {
            key: self.key,
            size: self.size,
            payload: self.clone(),
        }
    }

    /// The frame's identity — a counter, not a hash that would need the
    /// pixels on the CPU.
    pub fn key(&self) -> u64 {
        self.key
    }

    /// Pixel dimensions of the texture.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The texture's memory order.
    pub fn format(&self) -> NativeFormat {
        self.format
    }

    /// A retained handle for the compositor on `device` — `None` when the
    /// texture lives on another device, which no shader here could read.
    pub(crate) fn import(&self, device: Id) -> Option<Id> {
        // SAFETY: both are live retained Metal objects; `registryID`
        // names a physical device uniquely for the process
        unsafe {
            let mine = msg_u64(msg_id(self.texture, sel("device")), sel("registryID"));
            if mine != msg_u64(device, sel("registryID")) {
                return None;
            }
            msg_void(self.texture, sel("retain"));
            Some(self.texture)
        }
    }
}

impl Drop for MetalFrame {
    fn drop(&mut self) {
        // SAFETY: the retain `from_texture` took is given back exactly once
        unsafe { msg_void(self.texture, sel("release")) };
    }
}

/// Manual: the texture is a handle and the lease is opaque — the key,
/// the size and the layout are what a scene node has to say.
impl std::fmt::Debug for MetalFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "frame(0x{:016x}, {}×{}, {:?})", self.key, self.size.0, self.size.1, self.format)
    }
}

// MARK: - The wgpu pool (completed wgpu textures, copied GPU to GPU)

#[cfg(feature = "wgpu-surface")]
pub use pool::{PendingFrame, SurfacePool};

#[cfg(feature = "wgpu-surface")]
mod pool {
    use super::MetalFrame;
    use foreign_types::ForeignType;
    use std::sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    };

    const POOL_LIMIT: usize = 12;

    /// A pending GPU copy. Its frame cannot escape until completion is observed.
    pub struct PendingFrame {
        frame: Arc<MetalFrame>,
        ready: Arc<AtomicBool>,
    }
    impl PendingFrame {
        /// Nonblocking readiness check. The producer must poll its wgpu device.
        pub fn completed(&self) -> Option<Arc<MetalFrame>> {
            self.ready.load(Ordering::Acquire).then(|| self.frame.clone())
        }
    }
    struct Slot {
        texture: wgpu::Texture,
        lease: Weak<MetalFrame>,
    }
    /// At most twelve textures, shared by pending copies, in-flight presents and UI.
    #[derive(Default)]
    pub struct SurfacePool {
        slots: Vec<Slot>,
    }
    impl SurfacePool {
        /// Enqueue a GPU-only copy. Backpressure returns `Ok(None)` without waiting.
        /// The caller retains and polls PendingFrame until completion; changing size
        /// never invalidates leases held by an older UI frame.
        pub fn copy(
            &mut self,
            device: &wgpu::Device,
            queue: &wgpu::Queue,
            source: &wgpu::Texture,
        ) -> Result<Option<PendingFrame>, String> {
            if source.format() != wgpu::TextureFormat::Rgba8UnormSrgb {
                return Err("Native surface requires RGBA8 sRGB output".into());
            }
            if source.sample_count() != 1
                || source.dimension() != wgpu::TextureDimension::D2
                || source.depth_or_array_layers() != 1
                || !source.usage().contains(wgpu::TextureUsages::COPY_SRC)
            {
                return Err(
                    "Native surface requires a single-layer resolved COPY_SRC texture".into()
                );
            }
            // SAFETY: borrowed only for querying its owning device; no handle is destroyed.
            unsafe {
                let raw = source
                    .as_hal::<wgpu::hal::api::Metal>()
                    .ok_or("Native surface requires the Metal backend")?;
                let compositor =
                    metal::Device::system_default().ok_or("Metal compositor unavailable")?;
                if raw.raw_handle().device().registry_id() != compositor.registry_id() {
                    return Err(
                        "The renderer and compositor must use the same Metal device".into()
                    );
                }
            }
            let free = self.slots.iter().position(|slot| slot.lease.strong_count() == 0);
            let index = match free {
                Some(index) => index,
                None if self.slots.len() < POOL_LIMIT => {
                    self.slots.push(Slot {
                        texture: make_texture(device, source),
                        lease: Weak::new(),
                    });
                    self.slots.len() - 1
                }
                None => return Ok(None),
            };
            let slot = &mut self.slots[index];
            if slot.texture.size() != source.size() {
                slot.texture = make_texture(device, source);
            }
            // SAFETY: the texture is alive, never manually destroyed, and all access
            // to its bytes stays on the GPU. The retained view owns the allocation,
            // and the frame keeps the view alive beside its own retain.
            let frame = unsafe {
                let raw = slot
                    .texture
                    .as_hal::<wgpu::hal::api::Metal>()
                    .ok_or("Native surface requires Metal")?;
                // an sRGB texture seen as RGBA8Unorm: the compositor works in
                // gamma space and must read the bytes as they are
                let view = raw.raw_handle().new_texture_view(metal::MTLPixelFormat::RGBA8Unorm);
                let texture = view.as_ptr().cast::<std::ffi::c_void>();
                MetalFrame::from_texture(texture, Arc::new(view))?
            };
            slot.lease = Arc::downgrade(&frame);
            let ready = Arc::new(AtomicBool::new(false));
            let completed = ready.clone();
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("bunny-native-surface-copy"),
            });
            encoder.copy_texture_to_texture(
                source.as_image_copy(),
                slot.texture.as_image_copy(),
                source.size(),
            );
            queue.submit([encoder.finish()]);
            // Retain the lease even if a producer is cancelled before the copy ends.
            let held = frame.clone();
            queue.on_submitted_work_done(move || {
                completed.store(true, Ordering::Release);
                drop(held);
            });
            Ok(Some(PendingFrame { frame, ready }))
        }

        #[cfg(test)]
        pub(super) fn slot_count(&self) -> usize {
            self.slots.len()
        }
    }
    fn make_texture(device: &wgpu::Device, source: &wgpu::Texture) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bunny-native-surface"),
            size: source.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: source.format(),
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
        })
    }

    #[cfg(test)]
    pub(super) const POOL_LIMIT_FOR_TESTS: usize = POOL_LIMIT;
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metal::{self, OffscreenGpu};
    use bunny_ui::{
        image_engine::{RawImages, raster_source},
        prelude::*,
        text_engine::PixelFont,
    };

    /// A `w`×`h` gradient, `phase` in the third channel.
    fn gradient((w, h): (u32, u32), phase: u8) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[
                    ((x * 255) / w.max(1)) as u8,
                    ((y * 255) / h.max(1)) as u8,
                    phase,
                    255,
                ]);
            }
        }
        rgba
    }

    /// A frame over a texture of `pixel_format` filled with `bytes`, on
    /// the system device — `None` when there is no device.
    fn frame_of(pixel_format: u64, size: (u32, u32), bytes: &[u8]) -> Option<Result<Arc<MetalFrame>, String>> {
        unsafe {
            let device = metal::default_device()?;
            let texture = metal::shared_texture(device, pixel_format, size.0, size.1);
            assert!(!texture.is_null(), "the device makes a texture");
            metal::upload_texture(texture, 0, 0, size.0, size.1, bytes, size.0);
            let frame = MetalFrame::from_texture(texture, Arc::new(()));
            // the frame holds its own retain; the test's goes back
            msg_void(texture, sel("release"));
            Some(frame)
        }
    }

    fn present(root: &impl View, size: (usize, usize)) -> Vec<u8> {
        let display = Runtime::new().display_frame(
            root,
            Size { width: size.0 as f64, height: size.1 as f64 },
        );
        let mut gpu = OffscreenGpu::new(size.0, size.1).expect("offscreen gpu");
        gpu.present_wait(&display, 1, Color::CANVAS, &PixelFont, &RawImages::default());
        gpu.read_rgba()
    }

    #[test]
    fn a_bgra_texture_reads_in_rgb_order() {
        // the pixels, in memory as BGRA
        let rgba: [[u8; 4]; 4] =
            [[200, 30, 60, 255], [10, 180, 250, 255], [255, 128, 0, 255], [17, 33, 65, 255]];
        let bgra: Vec<u8> = rgba.iter().flat_map(|[r, g, b, a]| [*b, *g, *r, *a]).collect();
        let Some(frame) = frame_of(PIXEL_FORMAT_BGRA8, (2, 2), &bgra) else {
            eprintln!("no metal device — skipping");
            return;
        };
        let frame = frame.expect("a BGRA texture is a frame");
        assert_eq!(frame.format(), NativeFormat::Bgra8Unorm);
        assert_eq!(frame.size(), (2, 2));
        assert!(
            raster_source(&RawImages::default(), &frame.image(), 2, 2).is_none(),
            "a native frame never enters the CPU rasterizer"
        );
        let shown = present(&image(frame.image()).resizable().frame(2.0, 2.0), (2, 2));
        let expected: Vec<u8> = rgba.concat();
        assert_eq!(shown, expected, "the shader reads BGRA as r, g, b, a with no swizzle");
    }

    #[test]
    fn a_native_frame_samples_like_a_feed() {
        let bytes = gradient((64, 48), 7);
        let Some(frame) = frame_of(PIXEL_FORMAT_RGBA8, (64, 48), &bytes) else {
            eprintln!("no metal device — skipping");
            return;
        };
        let frame = frame.expect("an RGBA texture is a frame");
        let feed = ImageFeed::new();
        feed.push((64, 48), bytes);
        // scaled 1.5× and cut by a smaller box: both ride the linear
        // sampler, so the two roads agree byte for byte
        let root = |source: ImageSource| {
            image(source).resizable().frame(96.0, 72.0).clipped().frame(80.0, 60.0)
        };
        let native = present(&root(frame.image()), (80, 60));
        let fed = present(&root(feed.source()), (80, 60));
        assert_eq!(native, fed, "a native frame and a feed of the same bytes paint the same");
        assert!(
            native.chunks_exact(4).any(|pixel| pixel[..3] != [0xF2, 0xF3, 0xF7]),
            "and the picture is there"
        );
    }

    #[test]
    fn a_frame_of_another_format_is_refused_by_name() {
        // MTLPixelFormatRGBA8Unorm_sRGB: decodes on read, which a
        // gamma-space compositor cannot take
        let Some(frame) = frame_of(71, (2, 2), &gradient((2, 2), 0)) else {
            eprintln!("no metal device — skipping");
            return;
        };
        let refusal = frame.expect_err("an sRGB texture is refused");
        assert!(refusal.contains("71"), "the refusal names the format: {refusal}");
        let nil = unsafe { MetalFrame::from_texture(std::ptr::null_mut(), Arc::new(())) };
        assert!(nil.is_err(), "nil is refused too");
    }

    #[cfg(feature = "wgpu-surface")]
    mod pool {
        use super::super::pool::POOL_LIMIT_FOR_TESTS;
        use super::super::*;
        use crate::metal::OffscreenGpu;
        use bunny_ui::{image_engine::RawImages, prelude::*, text_engine::PixelFont};
        use std::time::Duration;

        fn producer() -> (wgpu::Device, wgpu::Queue, wgpu::Texture) {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: wgpu::Backends::METAL,
                ..Default::default()
            });
            let adapter =
                pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
            let (device, queue) =
                pollster::block_on(adapter.request_device(&Default::default())).unwrap();
            let source = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 2, height: 2, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                source.as_image_copy(),
                &PIXELS,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(8),
                    rows_per_image: Some(2),
                },
                source.size(),
            );
            (device, queue, source)
        }
        const PIXELS: [u8; 16] =
            [128, 32, 64, 255, 0, 192, 255, 255, 255, 128, 0, 255, 17, 33, 65, 255];
        fn finish(device: &wgpu::Device, pending: PendingFrame) -> Arc<MetalFrame> {
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(10)),
                })
                .unwrap();
            pending.completed().expect("completed GPU frame")
        }

        #[test]
        fn a_pooled_frame_samples_like_a_feed_after_scaling_and_clipping() {
            let (device, queue, source) = producer();
            let mut pool = SurfacePool::default();
            let frame = finish(&device, pool.copy(&device, &queue, &source).unwrap().unwrap());
            assert_eq!(frame.format(), NativeFormat::Rgba8Unorm);
            let feed = ImageFeed::new();
            feed.push((2, 2), PIXELS.to_vec());
            let runtime = Runtime::new();
            let size = Size { width: 40.0, height: 32.0 };
            let root = |source| image(source).resizable().frame(64.0, 48.0).clipped().frame(40.0, 32.0);
            let native = runtime.display_frame(&root(frame.image()), size);
            let reference = runtime.display_frame(&root(feed.source()), size);
            let mut gpu = OffscreenGpu::new(40, 32).unwrap();
            gpu.present_wait(&native, 1, Color::CANVAS, &PixelFont, &RawImages::default());
            let actual = gpu.read_rgba();
            gpu.present_wait(&reference, 1, Color::CANVAS, &PixelFont, &RawImages::default());
            assert_eq!(
                actual,
                gpu.read_rgba(),
                "the pooled frame keeps orientation, sRGB bytes and the clip, like a feed"
            );
        }

        #[test]
        fn compositor_retires_leases_so_the_bounded_stream_keeps_advancing() {
            let (device, queue, source) = producer();
            let mut pool = SurfacePool::default();
            let runtime = Runtime::new();
            let mut gpu = OffscreenGpu::new(32, 32).unwrap();
            for _ in 0..40 {
                let pending = pool
                    .copy(&device, &queue, &source)
                    .unwrap()
                    .expect("retired compositor frames must release pool slots");
                let frame = finish(&device, pending);
                let root = image(frame.image()).resizable().frame(32.0, 32.0);
                let display =
                    runtime.display_frame(&root, Size { width: 32.0, height: 32.0 });
                gpu.present_wait(&display, 1, Color::CANVAS, &PixelFont, &RawImages::default());
            }
            assert!(pool.slot_count() <= POOL_LIMIT_FOR_TESTS);
        }

        #[test]
        fn producer_applies_backpressure_without_overwriting_a_held_frame() {
            let (device, queue, source) = producer();
            let mut pool = SurfacePool::default();
            let mut held = Vec::new();
            for _ in 0..POOL_LIMIT_FOR_TESTS {
                held.push(finish(&device, pool.copy(&device, &queue, &source).unwrap().unwrap()));
            }
            assert!(pool.copy(&device, &queue, &source).unwrap().is_none());
            let previous = held.pop().unwrap().key();
            let replacement =
                finish(&device, pool.copy(&device, &queue, &source).unwrap().unwrap());
            assert_ne!(replacement.key(), previous);
            assert_eq!(pool.slot_count(), POOL_LIMIT_FOR_TESTS);
        }
    }
}
