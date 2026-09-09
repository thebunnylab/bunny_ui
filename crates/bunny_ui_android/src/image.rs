//! `AImageDecoder` through the house FFI — the Android image engine.
//!
//! Implements the bunny-ui [`ImageEngine`] border: the platform decodes
//! the bytes it knows (PNG, JPEG, WebP, GIF, HEIF…) straight into an
//! RGBA rectangle of the size the layout asks for, one decode per new
//! size, behind a capped cache.
//!
//! The decoder scales only premultiplied pixels; the compositor blends
//! STRAIGHT alpha, so the rectangle is unpremultiplied in place before
//! leaving. Broken bytes stay a cached failure: nothing paints and the
//! decoder is not asked again. The platform has no icon for a file
//! path here, so `FileIcon` is `None`, honestly.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::rc::Rc;

use bunny_ui::image_engine::{ImageEngine, ImageRaster, ImageSource};

#[repr(C)]
struct AImageDecoder {
    _private: [u8; 0],
}

#[repr(C)]
struct AImageDecoderHeaderInfo {
    _private: [u8; 0],
}

const ANDROID_IMAGE_DECODER_SUCCESS: i32 = 0;
const ANDROID_IMAGE_DECODER_INCOMPLETE: i32 = -1;
const ANDROID_BITMAP_FORMAT_RGBA_8888: i32 = 1;

#[link(name = "jnigraphics")]
unsafe extern "C" {
    fn AImageDecoder_createFromBuffer(
        buffer: *const c_void,
        length: usize,
        decoder: *mut *mut AImageDecoder,
    ) -> i32;
    fn AImageDecoder_delete(decoder: *mut AImageDecoder);
    fn AImageDecoder_getHeaderInfo(decoder: *mut AImageDecoder) -> *const AImageDecoderHeaderInfo;
    fn AImageDecoderHeaderInfo_getWidth(info: *const AImageDecoderHeaderInfo) -> i32;
    fn AImageDecoderHeaderInfo_getHeight(info: *const AImageDecoderHeaderInfo) -> i32;
    fn AImageDecoder_setAndroidBitmapFormat(decoder: *mut AImageDecoder, format: i32) -> i32;
    fn AImageDecoder_setTargetSize(decoder: *mut AImageDecoder, width: i32, height: i32) -> i32;
    fn AImageDecoder_getMinimumStride(decoder: *mut AImageDecoder) -> usize;
    fn AImageDecoder_decodeImage(
        decoder: *mut AImageDecoder,
        pixels: *mut c_void,
        stride: usize,
        size: usize,
    ) -> i32;
}

/// Resampled rectangles retained before the cache drops them all —
/// a picture view redrawn at a dozen sizes is the common case, a
/// thousand is a leak.
const RASTER_CAP: usize = 64;

/// Straight alpha out of premultiplied, in place — the compositor's
/// contract, shared with the text engine.
pub(crate) fn unpremultiply(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha > 0 && alpha < 255 {
            for channel in 0..3 {
                pixel[channel] = ((pixel[channel] as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}

/// A decoder over `bytes`, deleted on drop.
struct Decoder(*mut AImageDecoder);

impl Decoder {
    fn over(bytes: &[u8]) -> Option<Decoder> {
        let mut decoder: *mut AImageDecoder = null_mut();
        let made = unsafe { AImageDecoder_createFromBuffer(bytes.as_ptr().cast(), bytes.len(), &mut decoder) };
        (made == ANDROID_IMAGE_DECODER_SUCCESS && !decoder.is_null()).then_some(Decoder(decoder))
    }

    fn size(&self) -> Option<(u32, u32)> {
        unsafe {
            let header = AImageDecoder_getHeaderInfo(self.0);
            if header.is_null() {
                return None;
            }
            let width = AImageDecoderHeaderInfo_getWidth(header);
            let height = AImageDecoderHeaderInfo_getHeight(header);
            (width > 0 && height > 0).then_some((width as u32, height as u32))
        }
    }

    /// Premultiplied RGBA, `width`×`height`, rows packed.
    fn decode(&self, width: usize, height: usize) -> Option<Vec<u8>> {
        unsafe {
            if AImageDecoder_setAndroidBitmapFormat(self.0, ANDROID_BITMAP_FORMAT_RGBA_8888)
                != ANDROID_IMAGE_DECODER_SUCCESS
                || AImageDecoder_setTargetSize(self.0, width as i32, height as i32)
                    != ANDROID_IMAGE_DECODER_SUCCESS
            {
                return None;
            }
            let stride = AImageDecoder_getMinimumStride(self.0).max(width * 4);
            let mut pixels = vec![0u8; stride * height];
            let decoded = AImageDecoder_decodeImage(self.0, pixels.as_mut_ptr().cast(), stride, pixels.len());
            // a truncated file still yields the rows it had
            if decoded != ANDROID_IMAGE_DECODER_SUCCESS && decoded != ANDROID_IMAGE_DECODER_INCOMPLETE {
                return None;
            }
            if stride != width * 4 {
                let mut packed = vec![0u8; width * height * 4];
                for row in 0..height {
                    packed[row * width * 4..(row + 1) * width * 4]
                        .copy_from_slice(&pixels[row * stride..row * stride + width * 4]);
                }
                pixels = packed;
            }
            Some(pixels)
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { AImageDecoder_delete(self.0) };
    }
}

/// The Android image engine. Single-thread, like the rest of the shell.
pub struct AndroidImageEngine {
    /// Intrinsic sizes by source key — `None` is a remembered failure.
    sizes: RefCell<HashMap<u64, Option<(u32, u32)>>>,
    rasters: RefCell<HashMap<(u64, usize, usize), Rc<ImageRaster>>>,
}

impl AndroidImageEngine {
    pub fn new() -> Self {
        AndroidImageEngine { sizes: RefCell::new(HashMap::new()), rasters: RefCell::new(HashMap::new()) }
    }

    /// Lets every cached rectangle and size go — what a low-memory
    /// notice opens: the caches are a convenience, and a phone under
    /// pressure asks for them back.
    pub fn drop_caches(&self) {
        self.sizes.borrow_mut().clear();
        self.rasters.borrow_mut().clear();
    }

    fn bytes(source: &ImageSource) -> Option<&[u8]> {
        match source {
            ImageSource::Bytes { bytes, .. } => Some(bytes),
            // no icon for a path on the phone
            ImageSource::FileIcon { .. } => None,
            // never reaches an engine: the house rasterizes these first
            _ => {
                debug_assert!(false, "a house-drawn source reached the platform engine");
                None
            }
        }
    }
}

impl Default for AndroidImageEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageEngine for AndroidImageEngine {
    fn intrinsic(&self, source: &ImageSource) -> Option<(u32, u32)> {
        let key = source.key();
        if let Some(size) = self.sizes.borrow().get(&key) {
            return *size;
        }
        let size = Self::bytes(source).and_then(Decoder::over).and_then(|decoder| decoder.size());
        self.sizes.borrow_mut().insert(key, size);
        size
    }

    fn raster(&self, source: &ImageSource, width: usize, height: usize) -> Option<Rc<ImageRaster>> {
        if width == 0 || height == 0 {
            return None;
        }
        let cache_key = (source.key(), width, height);
        if let Some(raster) = self.rasters.borrow().get(&cache_key) {
            return Some(Rc::clone(raster));
        }
        self.intrinsic(source)?;
        let mut rgba = Decoder::over(Self::bytes(source)?)?.decode(width, height)?;
        unpremultiply(&mut rgba);
        let raster = Rc::new(ImageRaster { width, height, rgba });
        let mut rasters = self.rasters.borrow_mut();
        if rasters.len() >= RASTER_CAP {
            rasters.clear();
        }
        rasters.insert(cache_key, Rc::clone(&raster));
        Some(raster)
    }
}
