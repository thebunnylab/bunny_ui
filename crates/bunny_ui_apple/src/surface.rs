//! Completed wgpu/Metal textures in the native compositor, with no CPU pixels.
//!
//! A bounded pool copies GPU-to-GPU into immutable leased frames. Completion is
//! polled by the producer, never the UI. The presenter retains the lease until its
//! Metal submissions finish; a texture cannot be rewritten while being sampled.
use crate::ffi::Id;
use foreign_types::{ForeignType, ForeignTypeRef};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

static NEXT_FRAME: AtomicU64 = AtomicU64::new(1);
const POOL_LIMIT: usize = 12;

/// An immutable, completed Metal image. Constructed only by the producer pool.
pub struct MetalFrame {
    texture: metal::Texture,
    key: u64,
    size: (u32, u32),
}
impl MetalFrame {
    /// Build the UI source; cloning transfers ownership, never pixel bytes.
    pub fn image(self: &Arc<Self>) -> bunny_ui::image_engine::ImageSource {
        bunny_ui::image_engine::ImageSource::Native {
            key: self.key,
            size: self.size,
            payload: self.clone(),
        }
    }
    /// Stream frame identity, not a hash requiring a CPU download.
    pub fn key(&self) -> u64 {
        self.key
    }
    /// Pixel dimensions of the producer texture.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
    pub(crate) fn import(&self, device: Id) -> Option<Id> {
        // SAFETY: both devices are live retained Metal objects. Imported frames
        // are sampled only on the physical device that produced them.
        unsafe {
            if self.texture.device().registry_id()
                != metal::DeviceRef::from_ptr(device.cast()).registry_id()
            {
                return None;
            }
            Some(self.texture.clone().into_ptr().cast())
        }
    }
}

/// A pending GPU copy. Its frame cannot escape until completion is observed.
pub struct PendingFrame {
    frame: Arc<MetalFrame>,
    ready: Arc<AtomicBool>,
}
impl PendingFrame {
    /// Nonblocking readiness check. The producer must poll its wgpu device.
    pub fn completed(&self) -> Option<Arc<MetalFrame>> {
        self.ready
            .load(Ordering::Acquire)
            .then(|| self.frame.clone())
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
            return Err("Native surface requires a single-layer resolved COPY_SRC texture".into());
        }
        // SAFETY: borrowed only for querying its owning device; no handle is destroyed.
        unsafe {
            let raw = source
                .as_hal::<wgpu::hal::api::Metal>()
                .ok_or("Native surface requires the Metal backend")?;
            let compositor =
                metal::Device::system_default().ok_or("Metal compositor unavailable")?;
            if raw.raw_handle().device().registry_id() != compositor.registry_id() {
                return Err("The renderer and compositor must use the same Metal device".into());
            }
        }
        let free = self
            .slots
            .iter()
            .position(|slot| slot.lease.strong_count() == 0);
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
        // to its bytes stays on the GPU. The retained view owns the allocation.
        let texture = unsafe {
            let raw = slot
                .texture
                .as_hal::<wgpu::hal::api::Metal>()
                .ok_or("Native surface requires Metal")?;
            raw.raw_handle()
                .new_texture_view(metal::MTLPixelFormat::RGBA8Unorm)
        };
        let frame = Arc::new(MetalFrame {
            texture,
            key: NEXT_FRAME.fetch_add(1, Ordering::Relaxed) ^ 0x626e_795f_6770_7500,
            size: (source.width(), source.height()),
        });
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
mod tests {
    use super::*;
    use bunny_ui::{
        image_engine::{RawImages, raster_source},
        prelude::*,
        text_engine::PixelFont,
    };
    use std::time::Duration;

    fn producer() -> (wgpu::Device, wgpu::Queue, wgpu::Texture) {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let source = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
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
    const PIXELS: [u8; 16] = [
        128, 32, 64, 255, 0, 192, 255, 255, 255, 128, 0, 255, 17, 33, 65, 255,
    ];
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
    fn native_texture_matches_encoded_pixels_after_scaling_and_clipping() {
        let (device, queue, source) = producer();
        let mut pool = SurfacePool::default();
        let frame = finish(
            &device,
            pool.copy(&device, &queue, &source).unwrap().unwrap(),
        );
        assert!(
            raster_source(&RawImages::default(), &frame.image(), 64, 48).is_none(),
            "native frames must never enter the CPU rasterizer"
        );
        let runtime = Runtime::new();
        let size = Size {
            width: 40.0,
            height: 32.0,
        };
        let root = |source| {
            image(source)
                .resizable()
                .frame(64.0, 48.0)
                .clipped()
                .frame(40.0, 32.0)
        };
        let native = runtime.display_frame(&root(frame.image()), size);
        let reference =
            runtime.display_frame(&root(ImageSource::rgba(998, (2, 2), PIXELS.to_vec())), size);
        let mut gpu = crate::OffscreenGpu::new(40, 32).unwrap();
        gpu.present_wait(&native, 1, Color::CANVAS, &PixelFont, &RawImages::default());
        let actual = gpu.read_rgba();
        gpu.present_wait(
            &reference,
            1,
            Color::CANVAS,
            &PixelFont,
            &RawImages::default(),
        );
        assert_eq!(
            actual,
            gpu.read_rgba(),
            "native import must preserve orientation, sRGB bytes and clipping"
        );
    }
    #[test]
    fn compositor_retires_leases_so_the_bounded_stream_keeps_advancing() {
        let (device, queue, source) = producer();
        let mut pool = SurfacePool::default();
        let runtime = Runtime::new();
        let mut gpu = crate::OffscreenGpu::new(32, 32).unwrap();
        for _ in 0..40 {
            let pending = pool
                .copy(&device, &queue, &source)
                .unwrap()
                .expect("retired compositor frames must release pool slots");
            let frame = finish(&device, pending);
            let root = image(frame.image()).resizable().frame(32.0, 32.0);
            let display = runtime.display_frame(
                &root,
                Size {
                    width: 32.0,
                    height: 32.0,
                },
            );
            gpu.present_wait(
                &display,
                1,
                Color::CANVAS,
                &PixelFont,
                &RawImages::default(),
            );
        }
        assert!(pool.slots.len() <= POOL_LIMIT);
    }

    #[test]
    fn producer_applies_backpressure_without_overwriting_a_held_frame() {
        let (device, queue, source) = producer();
        let mut pool = SurfacePool::default();
        let mut held = Vec::new();
        for _ in 0..POOL_LIMIT {
            held.push(finish(
                &device,
                pool.copy(&device, &queue, &source).unwrap().unwrap(),
            ));
        }
        assert!(pool.copy(&device, &queue, &source).unwrap().is_none());
        let previous = held.pop().unwrap().key();
        let replacement = finish(
            &device,
            pool.copy(&device, &queue, &source).unwrap().unwrap(),
        );
        assert_ne!(replacement.key(), previous);
        assert_eq!(pool.slots.len(), POOL_LIMIT);
    }
}
