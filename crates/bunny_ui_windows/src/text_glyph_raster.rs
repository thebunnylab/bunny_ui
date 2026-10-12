//! Checked native bitmap surfaces for the paragraph glyph stream.
//!
//! ## Wiring
//! The WIC/Direct2D surface uses the existing engine factories and grayscale
//! text antialiasing. A failed draw never publishes a partially painted bitmap.

use super::{Com, Factories, Guid, Hresult, TextError, buffer, com_ok, ffi};
use crate::text::{
    D2D1_ALPHA_PREMULTIPLIED, D2D1_TEXT_AA_GRAYSCALE, D2dColor, D2dPixelFormat,
    D2dRenderTargetProperties, DXGI_B8G8R8A8_UNORM, ID2D1RenderTarget, IWICBitmap,
    WIC_CACHE_ON_LOAD, WIC_PIXEL_32BPP_PBGRA,
};

const CONTEXT4: Guid = Guid {
    d1: 0x8c42_7831,
    d2: 0x3d90,
    d3: 0x4476,
    d4: [0xb6, 0x47, 0xc4, 0xfa, 0xe3, 0x49, 0xe4, 0xdb],
};
const CONTEXT7: Guid = Guid {
    d1: 0xec89_1cf7,
    d2: 0x9b69,
    d3: 0x4851,
    d4: [0x9d, 0xef, 0x4e, 0x09, 0x15, 0x77, 0x1e, 0x62],
};
const fn failure(operation: &'static str) -> TextError {
    TextError::Rasterization {
        engine: "DirectWrite",
        operation,
    }
}
fn owned<T>(
    status: Hresult,
    pointer: *mut T,
    operation: &'static str,
) -> Result<Com<T>, TextError> {
    let owned = Com::from_raw(pointer);
    if !com_ok(status) {
        return Err(failure(operation));
    }
    owned.ok_or_else(|| failure(operation))
}

struct Surface {
    target: Com<ID2D1RenderTarget>,
    bitmap: Com<IWICBitmap>,
    stride: u32,
    bytes: u32,
}
impl Surface {
    fn new(factories: &Factories, width: u32, height: u32, scale: f32) -> Result<Self, TextError> {
        let stride = width
            .checked_mul(4)
            .ok_or_else(|| failure("bitmap stride"))?;
        let bytes = stride
            .checked_mul(height)
            .ok_or_else(|| failure("bitmap size"))?;
        let dpi = 96.0 * scale;
        if bytes == 0 || !dpi.is_finite() || scale <= 0.0 {
            return Err(failure("bitmap dimensions"));
        }
        // SAFETY: checked dimensions and exact SDK structures; each successful
        // out interface is owned before the next native call can fail.
        unsafe {
            let mut pointer = std::ptr::null_mut();
            let wic = factories.wic.as_ptr();
            let status = ((*(*wic).vtbl).create_bitmap)(
                wic,
                width,
                height,
                &WIC_PIXEL_32BPP_PBGRA,
                WIC_CACHE_ON_LOAD,
                &raw mut pointer,
            );
            let bitmap = owned(status, pointer, "CreateBitmap")?;
            let properties = D2dRenderTargetProperties {
                kind: 0,
                pixel_format: D2dPixelFormat {
                    format: DXGI_B8G8R8A8_UNORM,
                    alpha_mode: D2D1_ALPHA_PREMULTIPLIED,
                },
                dpi_x: dpi,
                dpi_y: dpi,
                usage: 0,
                min_level: 0,
            };
            let mut pointer = std::ptr::null_mut();
            let factory = factories.d2d.as_ptr();
            let status = ((*(*factory).vtbl).create_wic_bitmap_render_target)(
                factory,
                bitmap.as_ptr(),
                &raw const properties,
                &raw mut pointer,
            );
            let target = owned(status, pointer, "CreateWicBitmapRenderTarget")?;
            ((*(*target.as_ptr()).vtbl).set_text_antialias_mode)(
                target.as_ptr(),
                D2D1_TEXT_AA_GRAYSCALE,
            );
            Ok(Self {
                target,
                bitmap,
                stride,
                bytes,
            })
        }
    }

    fn paint(
        &self,
        draw: impl FnOnce(&Com<ID2D1RenderTarget>) -> Result<(), TextError>,
    ) -> Result<Vec<u8>, TextError> {
        let drawing = Drawing::begin(&self.target);
        let result = draw(&self.target);
        let end = drawing.finish();
        result?;
        end?;
        let mut pixels = buffer(self.bytes as usize, "raster backing allocation")
            .map_err(|_| failure("raster backing allocation"))?;
        // SAFETY: WIC writes the checked tightly packed PBGRA buffer only after
        // EndDraw succeeds; no unfinished or failed native batch is published.
        let status = unsafe {
            ((*(*self.bitmap.as_ptr()).vtbl).copy_pixels)(
                self.bitmap.as_ptr(),
                std::ptr::null(),
                self.stride,
                self.bytes,
                pixels.as_mut_ptr(),
            )
        };
        if !com_ok(status) {
            return Err(failure("CopyPixels"));
        }
        Ok(pixels)
    }
}

/// Owns the obligation to end this native batch, including early-return/unwind.
struct Drawing<'a>(Option<&'a Com<ID2D1RenderTarget>>);
impl<'a> Drawing<'a> {
    fn begin(target: &'a Com<ID2D1RenderTarget>) -> Self {
        // SAFETY: the surface owns the target for this entire drawing scope.
        unsafe {
            let pointer = target.as_ptr();
            ((*(*pointer).vtbl).begin_draw)(pointer);
            let clear = D2dColor {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            };
            ((*(*pointer).vtbl).clear)(pointer, &raw const clear);
        }
        Self(Some(target))
    }
    fn finish(mut self) -> Result<(), TextError> {
        let target = self.0.take().ok_or_else(|| failure("draw ownership"))?;
        if !com_ok(end(target)) {
            return Err(failure("EndDraw"));
        }
        Ok(())
    }
}
impl Drop for Drawing<'_> {
    fn drop(&mut self) {
        // Normal results use finish, which propagates EndDraw errors. On unwind
        // the pending image is discarded; still balance the native batch.
        if let Some(target) = self.0.take() {
            let _ = end(target);
        }
    }
}
fn end(target: &Com<ID2D1RenderTarget>) -> Hresult {
    // SAFETY: this is the live target's one matching EndDraw for BeginDraw.
    unsafe {
        ((*(*target.as_ptr()).vtbl).end_draw)(
            target.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    }
}

#[cfg(test)]
#[path = "text_glyph_raster_tests.rs"]
mod tests;
