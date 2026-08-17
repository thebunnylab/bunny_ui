//! The browser's image edge — decode borrowed through the glue.
//!
//! Implements the bunny-ui [`ImageEngine`] border for the canvas mode.
//! The browser decodes asynchronously by nature: the first `intrinsic`
//! miss REGISTERS the bytes (one crossing per source — the glue turns
//! them into a decoded bitmap) and answers `None`; when the platform
//! reports in through `bunny_image_ready`, the shell repaints and the
//! same questions start answering for real. Broken bytes never call
//! back — they stay `None` and nothing paints, without a retry loop.
//!
//! Rastering mirrors the text raster's two-phase out-pointer: the size
//! is decided BEFORE the crossing, wasm allocates exactly that, the
//! glue draws the bitmap at that size and writes the straight-alpha
//! RGBA back. Each crossing is expensive — the raster retains by
//! (source, size) with a hard ceiling, like every other engine.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use bunny_ui::image_engine::{ImageEngine, ImageRaster, ImageSource};

#[link(wasm_import_module = "bunny")]
unsafe extern "C" {
    /// Hands the platform-encoded bytes to the glue (borrowed for the
    /// call — the glue copies into a Blob). The glue decodes and calls
    /// `bunny_image_ready` with the same split key when it lands.
    fn js_image_register(key_hi: u32, key_lo: u32, pointer: *const u8, len: usize);
    /// Writes `[width, height]` into `out` — `[0, 0]` while the
    /// browser has not decoded (or never will).
    fn js_image_size(key_hi: u32, key_lo: u32, out: *mut u32);
    /// Draws the decoded bitmap at EXACTLY width×height physical px
    /// and writes the straight-alpha RGBA into `out`.
    fn js_image_raster(key_hi: u32, key_lo: u32, width: u32, height: u32, out: *mut u8);
}

fn split(key: u64) -> (u32, u32) {
    ((key >> 32) as u32, key as u32)
}

/// Resampled rectangles retained before the cache drops them all.
const IMAGE_KEEP: usize = 64;

/// The web image engine — the browser decodes, we compose.
#[derive(Default)]
pub struct CanvasImageEngine {
    /// Sources already handed to the glue — one crossing per identity.
    registered: RefCell<HashSet<u64>>,
    rasters: RefCell<HashMap<(u64, usize, usize), Rc<ImageRaster>>>,
}

impl CanvasImageEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers on the first ask; answers the browser's decoded size,
    /// `None` until the ready callback has landed.
    fn size(&self, source: &ImageSource) -> Option<(u32, u32)> {
        let ImageSource::Bytes { key, bytes } = source else {
            // no file icons on the web in v1 — nothing paints
            return None;
        };
        let (hi, lo) = split(*key);
        if self.registered.borrow_mut().insert(*key) {
            unsafe { js_image_register(hi, lo, bytes.as_ptr(), bytes.len()) };
        }
        let mut out = [0u32; 2];
        unsafe { js_image_size(hi, lo, out.as_mut_ptr()) };
        (out[0] > 0 && out[1] > 0).then_some((out[0], out[1]))
    }
}

impl ImageEngine for CanvasImageEngine {
    fn intrinsic(&self, source: &ImageSource) -> Option<(u32, u32)> {
        self.size(source)
    }

    fn raster(
        &self,
        source: &ImageSource,
        width: usize,
        height: usize,
    ) -> Option<Rc<ImageRaster>> {
        if width == 0 || height == 0 {
            return None;
        }
        let cache_key = (source.key(), width, height);
        if let Some(raster) = self.rasters.borrow().get(&cache_key) {
            return Some(Rc::clone(raster));
        }
        // not decoded yet = nothing to draw from
        self.size(source)?;
        let (hi, lo) = split(source.key());
        let mut rgba = vec![0u8; width * height * 4];
        unsafe { js_image_raster(hi, lo, width as u32, height as u32, rgba.as_mut_ptr()) };
        let raster = Rc::new(ImageRaster { width, height, rgba });
        let mut rasters = self.rasters.borrow_mut();
        if rasters.len() >= IMAGE_KEEP {
            rasters.clear();
        }
        rasters.insert(cache_key, Rc::clone(&raster));
        Some(raster)
    }
}
