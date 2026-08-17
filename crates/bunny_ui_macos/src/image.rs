//! ImageIO through the house FFI — the Mac's image engine.
//!
//! Implements the bunny-ui [`ImageEngine`] border: the platform decodes
//! (PNG, JPEG, everything ImageIO speaks) and resamples with high
//! interpolation quality into a `CGBitmapContext` over our own buffer,
//! at EXACTLY the physical size the caller asks for. The decoded
//! `CGImage` is retained by identity — decode happens once; a resample
//! per new size, behind a capped cache.
//!
//! The CG context only draws premultiplied; the compositor blends
//! STRAIGHT alpha, so the rectangle is unpremultiplied in place before
//! leaving — the same pass the text engine does.
//!
//! Broken bytes stay a cached failure: nothing paints and the decoder
//! is never asked twice. File icons come from the workspace: the icon
//! for a path, drawn from the representation that matches the asked
//! size — crisp at sixteen, crisp at sixty-four.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;

use bunny_ui::image_engine::{FILE_ICON_SIZE, ImageEngine, ImageRaster, ImageSource};

use crate::ffi::{
    CFRelease, CGColorSpaceCreateDeviceRGB, CGColorSpaceRelease, CGContextDrawImage,
    CGContextSetInterpolationQuality, CGImageRelease, CGPoint, CGRect, CGSize, Id, Sel, class,
    sel,
};

type CFDataRef = *const c_void;
type CGImageSourceRef = *const c_void;
type CGContextRef = *mut c_void;

/// `kCGImageAlphaPremultipliedLast` — the only RGBA layout a drawing
/// context accepts.
const ALPHA_PREMULTIPLIED_LAST: u32 = 1;
/// `kCGInterpolationHigh` (3 — Medium is 4, a historical accident).
const INTERPOLATION_HIGH: i32 = 3;
/// Resampled rectangles retained before the cache drops them all —
/// entries are whole bitmaps, so the ceiling is low and eviction total.
const IMAGE_KEEP: usize = 64;

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithData(data: CFDataRef, options: *const c_void) -> CGImageSourceRef;
    fn CGImageSourceCreateImageAtIndex(
        source: CGImageSourceRef,
        index: usize,
        options: *const c_void,
    ) -> Id;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> CFDataRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGImageGetWidth(image: Id) -> usize;
    fn CGImageGetHeight(image: Id) -> usize;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *mut c_void,
        bitmap_info: u32,
    ) -> CGContextRef;
    fn CGContextRelease(context: CGContextRef);
}

/// A retained CGImage — released on Drop (the engine is the owner).
struct OwnedImage(Id);

impl Drop for OwnedImage {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CGImageRelease(self.0) };
        }
    }
}

// The workspace bridge (file icons) — msgSend casts in the house
// pattern, local to the messages this module sends.
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_arg(obj: Id, sel: Sel, a: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_id_cstr(obj: Id, sel: Sel, a: *const std::ffi::c_char) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void_size(obj: Id, sel: Sel, size: CGSize);
    #[link_name = "objc_msgSend"]
    fn msg_image_rect(obj: Id, sel: Sel, rect: *mut CGRect, context: Id, hints: Id) -> Id;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

/// Draws a CGImage into a fresh buffer at EXACTLY width×height physical
/// px, high interpolation — the one resample of the pipeline. The
/// pixels come back PREMULTIPLIED (the context's only mode).
unsafe fn draw_image(image: Id, width: usize, height: usize) -> Option<Vec<u8>> {
    let mut rgba = vec![0u8; width * height * 4];
    unsafe {
        let space = CGColorSpaceCreateDeviceRGB();
        let context = CGBitmapContextCreate(
            rgba.as_mut_ptr() as *mut c_void,
            width,
            height,
            8,
            width * 4,
            space,
            ALPHA_PREMULTIPLIED_LAST,
        );
        CGColorSpaceRelease(space);
        if context.is_null() {
            return None;
        }
        CGContextSetInterpolationQuality(context, INTERPOLATION_HIGH);
        CGContextDrawImage(
            context,
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: width as f64, height: height as f64 },
            },
            image,
        );
        CGContextRelease(context);
    }
    Some(rgba)
}

/// The compositor blends straight alpha — one pass over the rectangle.
fn unpremultiply(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha > 0 && alpha < 255 {
            for channel in 0..3 {
                pixel[channel] =
                    ((pixel[channel] as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}

/// The workspace icon for a path, drawn at the physical size — the
/// workspace picks the sharpest representation for the box (16 stays
/// crisp, 64 stays crisp, no upscaled thumbnail). Everything here is
/// autoreleased, so the drawing happens inside the pool.
unsafe fn icon_rgba(path: &str, width: usize, height: usize) -> Option<Vec<u8>> {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let rgba = (|| {
            let c_path = std::ffi::CString::new(path).ok()?;
            let ns_path =
                msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), c_path.as_ptr());
            if ns_path.is_null() {
                return None;
            }
            let workspace = msg_id(class("NSWorkspace"), sel("sharedWorkspace"));
            let icon = msg_id_arg(workspace, sel("iconForFile:"), ns_path);
            if icon.is_null() {
                return None;
            }
            msg_void_size(
                icon,
                sel("setSize:"),
                CGSize { width: width as f64, height: height as f64 },
            );
            let mut rect = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: width as f64, height: height as f64 },
            };
            let image = msg_image_rect(
                icon,
                sel("CGImageForProposedRect:context:hints:"),
                &mut rect,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            if image.is_null() {
                return None;
            }
            draw_image(image, width, height)
        })();
        objc_autoreleasePoolPop(pool);
        rgba
    }
}

/// Decodes the platform-encoded bytes once. Null = the platform could
/// not read them (cached as a permanent failure by the caller).
unsafe fn decode(bytes: &[u8]) -> Id {
    unsafe {
        let data = CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize);
        if data.is_null() {
            return std::ptr::null_mut();
        }
        let source = CGImageSourceCreateWithData(data, std::ptr::null());
        if source.is_null() {
            CFRelease(data);
            return std::ptr::null_mut();
        }
        let image = CGImageSourceCreateImageAtIndex(source, 0, std::ptr::null());
        CFRelease(source);
        CFRelease(data);
        image
    }
}

/// The Mac image engine. Single-thread, like the rest of the shell.
pub struct CoreGraphicsImageEngine {
    /// Decoded images by identity — `None` is a remembered failure
    /// (broken bytes never reach the decoder twice).
    decoded: RefCell<HashMap<u64, Option<OwnedImage>>>,
    rasters: RefCell<HashMap<(u64, usize, usize), Rc<ImageRaster>>>,
}

impl CoreGraphicsImageEngine {
    pub fn new() -> Self {
        CoreGraphicsImageEngine {
            decoded: RefCell::new(HashMap::new()),
            rasters: RefCell::new(HashMap::new()),
        }
    }

    /// The retained CGImage for the source, decoding on the first ask.
    /// Null = nothing to paint (failure, or a source this phase does
    /// not speak yet — file icons).
    fn image(&self, source: &ImageSource) -> Id {
        let key = source.key();
        if let Some(entry) = self.decoded.borrow().get(&key) {
            return entry.as_ref().map(|image| image.0).unwrap_or(std::ptr::null_mut());
        }
        let image = match source {
            ImageSource::Bytes { bytes, .. } => unsafe { decode(bytes) },
            // icons resolve per size through the workspace, never here
            ImageSource::FileIcon { .. } => std::ptr::null_mut(),
        };
        let entry = (!image.is_null()).then(|| OwnedImage(image));
        self.decoded.borrow_mut().insert(key, entry);
        image
    }
}

impl Default for CoreGraphicsImageEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageEngine for CoreGraphicsImageEngine {
    fn intrinsic(&self, source: &ImageSource) -> Option<(u32, u32)> {
        if let ImageSource::FileIcon { .. } = source {
            // system icons are multi-representation; the fixed contract
            // stands in — the normal use is `.resizable()` plus a frame
            return Some((FILE_ICON_SIZE, FILE_ICON_SIZE));
        }
        let image = self.image(source);
        if image.is_null() {
            return None;
        }
        unsafe { Some((CGImageGetWidth(image) as u32, CGImageGetHeight(image) as u32)) }
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
        let mut rgba = match source {
            ImageSource::Bytes { .. } => {
                let image = self.image(source);
                if image.is_null() {
                    return None;
                }
                unsafe { draw_image(image, width, height) }?
            }
            ImageSource::FileIcon { path, .. } => unsafe { icon_rgba(path, width, height) }?,
        };
        unpremultiply(&mut rgba);

        let raster = Rc::new(ImageRaster { width, height, rgba });
        let mut rasters = self.rasters.borrow_mut();
        if rasters.len() >= IMAGE_KEEP {
            rasters.clear();
        }
        rasters.insert(cache_key, Rc::clone(&raster));
        Some(raster)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use bunny_ui::image_engine::ImageSource;

    /// Bitwise CRC-32 (the PNG polynomial) — slow and tiny, test-only.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc: u32 = 0xffff_ffff;
        for byte in bytes {
            crc ^= *byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    fn adler32(bytes: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for byte in bytes {
            a = (a + *byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut body = kind.to_vec();
        body.extend_from_slice(payload);
        let mut out = (payload.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
        out
    }

    /// A REAL png, written by hand: zlib with one stored (uncompressed)
    /// deflate block is valid, so no compressor is needed — the
    /// platform decoder gets tested with the true container.
    pub(crate) fn tiny_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        assert_eq!(rgba.len(), (width * height * 4) as usize);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA

        // filter byte 0 in front of every row
        let mut raw = Vec::new();
        for row in rgba.chunks((width * 4) as usize) {
            raw.push(0);
            raw.extend_from_slice(row);
        }
        let mut idat = vec![0x78, 0x01, 0x01]; // zlib header + final stored block
        idat.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        idat.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
        idat.extend_from_slice(&raw);
        idat.extend_from_slice(&adler32(&raw).to_be_bytes());

        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&chunk(b"IHDR", &ihdr));
        png.extend_from_slice(&chunk(b"IDAT", &idat));
        png.extend_from_slice(&chunk(b"IEND", &[]));
        png
    }

    /// 2×2 quadrants: red, green / blue, white — all opaque.
    pub(crate) fn quadrants_png() -> Vec<u8> {
        tiny_png(
            2,
            2,
            &[
                255, 0, 0, 255, 0, 255, 0, 255, //
                0, 0, 255, 255, 255, 255, 255, 255,
            ],
        )
    }

    #[test]
    fn the_platform_decodes_a_real_png() {
        let engine = CoreGraphicsImageEngine::new();
        let source = ImageSource::from_bytes(quadrants_png());
        assert_eq!(engine.intrinsic(&source), Some((2, 2)));

        // same size = no resample: the pixels come back verbatim
        let raster = engine.raster(&source, 2, 2).expect("decoded");
        assert_eq!(&raster.rgba[0..4], &[255, 0, 0, 255], "top-left red");
        assert_eq!(&raster.rgba[4..8], &[0, 255, 0, 255], "top-right green");
        assert_eq!(&raster.rgba[8..12], &[0, 0, 255, 255], "bottom-left blue");
        assert_eq!(&raster.rgba[12..16], &[255, 255, 255, 255], "bottom-right white");
    }

    #[test]
    fn a_resample_lands_upright_and_caches() {
        let engine = CoreGraphicsImageEngine::new();
        let source = ImageSource::from_bytes(quadrants_png());
        let raster = engine.raster(&source, 8, 8).expect("resampled");
        // the corners keep their quadrant's hue (interpolation blends
        // the middle, never the extremes)
        let pixel = |x: usize, y: usize| {
            let index = (y * 8 + x) * 4;
            [raster.rgba[index], raster.rgba[index + 1], raster.rgba[index + 2]]
        };
        assert!(pixel(0, 0)[0] > 180 && pixel(0, 0)[2] < 80, "top-left leans red");
        assert!(pixel(0, 7)[2] > 180 && pixel(0, 7)[0] < 80, "bottom-left leans blue — upright");

        let again = engine.raster(&source, 8, 8).expect("cached");
        assert!(Rc::ptr_eq(&raster, &again), "one resample per size");
    }

    #[test]
    fn a_file_icon_arrives_from_the_workspace() {
        let engine = CoreGraphicsImageEngine::new();
        let icon = bunny_ui::image_engine::file_icon("/usr/bin");
        assert_eq!(engine.intrinsic(&icon), Some((32, 32)), "the fixed contract");

        let small = engine.raster(&icon, 16, 16).expect("an icon at sixteen");
        assert_eq!((small.width, small.height), (16, 16));
        assert!(small.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0), "the icon has ink");
        let large = engine.raster(&icon, 64, 64).expect("an icon at sixty-four");
        assert_eq!((large.width, large.height), (64, 64));

        // the cache answers the scroll — the workspace never hears the
        // same (path, size) twice
        let again = engine.raster(&icon, 16, 16).expect("cached");
        assert!(Rc::ptr_eq(&small, &again));
    }

    #[test]
    fn broken_bytes_fail_once_and_stay_failed() {
        let engine = CoreGraphicsImageEngine::new();
        let broken = ImageSource::from_bytes(&b"not an image at all"[..]);
        assert_eq!(engine.intrinsic(&broken), None);
        assert!(engine.raster(&broken, 8, 8).is_none());
        assert_eq!(engine.intrinsic(&broken), None, "the failure is remembered");
    }
}
