//! WIC through the house FFI — the Windows image engine.
//!
//! Implements the bunny-ui [`ImageEngine`] border: the platform
//! decodes (PNG, JPEG, everything WIC speaks) and resamples with
//! high-quality interpolation at EXACTLY the physical size the caller
//! asks for. The decoded source is retained by identity — decode
//! happens once; a resample per new size, behind a capped cache.
//!
//! One good deviation from the mac road: the platform's format
//! converter hands the pixels over as STRAIGHT RGBA directly, so the
//! bytes road needs no unpremultiply pass. File icons are the other
//! road — the shell hands them back premultiplied, and that road
//! unpremultiplies the way the mac does.
//!
//! Broken bytes stay a cached failure: nothing paints and the decoder
//! is never asked twice. File icons come from the shell: the icon for
//! a path at the asked size — crisp at sixteen, crisp at sixty-four.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;

use bunny_ui::image_engine::{FILE_ICON_SIZE, ImageEngine, ImageRaster, ImageSource};

use crate::ffi::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, Com, Guid, Hresult, UnknownVtbl, com_init, com_ok,
    wide,
};

/// Resampled rectangles retained before the cache drops them all —
/// entries are whole bitmaps, so the ceiling is low and eviction total.
const IMAGE_KEEP: usize = 64;

#[link(name = "shlwapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn SHCreateMemStream(bytes: *const u8, length: u32) -> *mut IStream;
}

#[link(name = "shell32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SHCreateItemFromParsingName(
        path: *const u16,
        bind_context: *mut c_void,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> Hresult;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateCompatibleDC(hdc: isize) -> isize;
    fn DeleteDC(hdc: isize) -> i32;
    fn DeleteObject(object: isize) -> i32;
    fn GetDIBits(
        hdc: isize,
        bitmap: isize,
        start: u32,
        lines: u32,
        bits: *mut c_void,
        info: *mut GdiBitmapInfo,
        usage: u32,
    ) -> i32;
}

#[repr(C)]
struct GdiBitmapInfo {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels_per_meter: i32,
    y_pels_per_meter: i32,
    clr_used: u32,
    clr_important: u32,
    colors: [u32; 1],
}

// CLSID_WICImagingFactory {CACAF262-9370-4615-A13B-9F5539DA4C0A}
const CLSID_WIC_IMAGING_FACTORY: Guid = Guid {
    d1: 0xCACA_F262,
    d2: 0x9370,
    d3: 0x4615,
    d4: [0xA1, 0x3B, 0x9F, 0x55, 0x39, 0xDA, 0x4C, 0x0A],
};
// IID_IWICImagingFactory {EC5EC8A9-C395-4314-9C77-54D7A935FF70}
const IID_IWIC_IMAGING_FACTORY: Guid = Guid {
    d1: 0xEC5E_C8A9,
    d2: 0xC395,
    d3: 0x4314,
    d4: [0x9C, 0x77, 0x54, 0xD7, 0xA9, 0x35, 0xFF, 0x70],
};
// GUID_WICPixelFormat32bppRGBA {F5C7AD2D-6A8D-43DD-A7A8-A29935261AE9} — STRAIGHT alpha
const WIC_PIXEL_32BPP_RGBA: Guid = Guid {
    d1: 0xF5C7_AD2D,
    d2: 0x6A8D,
    d3: 0x43DD,
    d4: [0xA7, 0xA8, 0xA2, 0x99, 0x35, 0x26, 0x1A, 0xE9],
};
// IID_IShellItemImageFactory {BCC18B79-BA16-442F-80C4-8A59C30C463B}
const IID_ISHELL_ITEM_IMAGE_FACTORY: Guid = Guid {
    d1: 0xBCC1_8B79,
    d2: 0xBA16,
    d3: 0x442F,
    d4: [0x80, 0xC4, 0x8A, 0x59, 0xC3, 0x0C, 0x46, 0x3B],
};

/// `WICBitmapInterpolationModeHighQualityCubic`.
const WIC_INTERPOLATION_HIGH_QUALITY_CUBIC: u32 = 4;
/// `SIIGBF_ICONONLY` — the file's icon, never a thumbnail.
const SIIGBF_ICON_ONLY: u32 = 0x4;

// MARK: - Vtables (wincodec.h / shobjidl_core.h, in header order)

/// An opaque COM stream — only ever released.
#[repr(C)]
struct IStream {
    vtbl: *const UnknownVtbl,
}

// slots 3 CreateDecoderFromFilename; 4 CreateDecoderFromStream;
// 5..=9 file handles, component info, decoders, encoders, palette;
// 10 CreateFormatConverter; 11 CreateBitmapScaler; the rest unused.
#[repr(C)]
struct IWICImagingFactoryVtbl {
    unknown: UnknownVtbl,
    _pad_3: [usize; 1],
    create_decoder_from_stream: unsafe extern "system" fn(
        *mut IWICImagingFactory,
        *mut IStream,
        *const Guid,
        u32,
        *mut *mut IWICBitmapDecoder,
    ) -> Hresult,
    _pad_5_9: [usize; 5],
    create_format_converter: unsafe extern "system" fn(
        *mut IWICImagingFactory,
        *mut *mut IWICFormatConverter,
    ) -> Hresult,
    create_bitmap_scaler: unsafe extern "system" fn(
        *mut IWICImagingFactory,
        *mut *mut IWICBitmapScaler,
    ) -> Hresult,
}
#[repr(C)]
struct IWICImagingFactory {
    vtbl: *const IWICImagingFactoryVtbl,
}

// slots 3..=12 capability/init/container/info/palette/metadata/
// preview/color contexts/thumbnail/frame count; 13 GetFrame.
#[repr(C)]
struct IWICBitmapDecoderVtbl {
    unknown: UnknownVtbl,
    _pad_3_12: [usize; 10],
    get_frame: unsafe extern "system" fn(
        *mut IWICBitmapDecoder,
        u32,
        *mut *mut IWICBitmapFrameDecode,
    ) -> Hresult,
}
#[repr(C)]
struct IWICBitmapDecoder {
    vtbl: *const IWICBitmapDecoderVtbl,
}

/// The `IWICBitmapSource` prefix every WIC source shares: 3 GetSize,
/// 4 GetPixelFormat, 5 GetResolution, 6 CopyPalette, 7 CopyPixels.
#[repr(C)]
struct IWICBitmapSourceVtbl {
    unknown: UnknownVtbl,
    get_size:
        unsafe extern "system" fn(*mut c_void, *mut u32, *mut u32) -> Hresult,
    _pad_4_6: [usize; 3],
    copy_pixels: unsafe extern "system" fn(
        *mut c_void,
        *const c_void,
        u32,
        u32,
        *mut u8,
    ) -> Hresult,
}

#[repr(C)]
struct IWICBitmapFrameDecode {
    vtbl: *const IWICBitmapSourceVtbl,
}

// IWICBitmapSource prefix (3..=7), then 8 Initialize.
#[repr(C)]
struct IWICFormatConverterVtbl {
    source: IWICBitmapSourceVtbl,
    initialize: unsafe extern "system" fn(
        *mut IWICFormatConverter,
        *mut c_void,
        *const Guid,
        u32,
        *mut c_void,
        f64,
        u32,
    ) -> Hresult,
}
#[repr(C)]
struct IWICFormatConverter {
    vtbl: *const IWICFormatConverterVtbl,
}

// IWICBitmapSource prefix (3..=7), then 8 Initialize.
#[repr(C)]
struct IWICBitmapScalerVtbl {
    source: IWICBitmapSourceVtbl,
    initialize: unsafe extern "system" fn(
        *mut IWICBitmapScaler,
        *mut c_void,
        u32,
        u32,
        u32,
    ) -> Hresult,
}
#[repr(C)]
struct IWICBitmapScaler {
    vtbl: *const IWICBitmapScalerVtbl,
}

#[repr(C)]
struct SizeI {
    cx: i32,
    cy: i32,
}

// IShellItemImageFactory : IUnknown — one method: 3 GetImage.
#[repr(C)]
struct IShellItemImageFactoryVtbl {
    unknown: UnknownVtbl,
    get_image: unsafe extern "system" fn(
        *mut IShellItemImageFactory,
        SizeI,
        u32,
        *mut isize,
    ) -> Hresult,
}
#[repr(C)]
struct IShellItemImageFactory {
    vtbl: *const IShellItemImageFactoryVtbl,
}

// MARK: - The engine

/// One decoded identity: the converter that answers straight RGBA,
/// and the intrinsic size read once.
struct DecodedSource {
    converter: Com<IWICFormatConverter>,
    width: u32,
    height: u32,
}

/// The Windows image engine. Single-thread, like the rest of the shell.
pub struct WicImageEngine {
    factory: Option<Com<IWICImagingFactory>>,
    /// Decoded sources by identity — `None` is a remembered failure
    /// (broken bytes never reach the decoder twice).
    decoded: RefCell<HashMap<u64, Option<DecodedSource>>>,
    rasters: RefCell<HashMap<(u64, usize, usize), Rc<ImageRaster>>>,
}

impl WicImageEngine {
    pub fn new() -> Self {
        com_init();
        let mut raw: *mut c_void = std::ptr::null_mut();
        let hr = unsafe {
            CoCreateInstance(
                &CLSID_WIC_IMAGING_FACTORY,
                std::ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_IWIC_IMAGING_FACTORY,
                &mut raw,
            )
        };
        let factory = if com_ok(hr) {
            Com::from_raw(raw as *mut IWICImagingFactory)
        } else {
            eprintln!("bunny_ui wic: no imaging factory (0x{:08X})", hr as u32);
            None
        };
        WicImageEngine {
            factory,
            decoded: RefCell::new(HashMap::new()),
            rasters: RefCell::new(HashMap::new()),
        }
    }

    /// Decodes once per identity: stream → decoder → first frame →
    /// converter pinned to straight RGBA. `None` is remembered.
    fn decode(&self, bytes: &[u8]) -> Option<DecodedSource> {
        let factory = self.factory.as_ref()?;
        unsafe {
            let stream = SHCreateMemStream(bytes.as_ptr(), bytes.len() as u32);
            let stream = Com::from_raw(stream)?;
            let factory_ptr = factory.as_ptr();
            let mut decoder: *mut IWICBitmapDecoder = std::ptr::null_mut();
            // WICDecodeMetadataCacheOnDemand = 0; no preferred vendor
            let hr = ((*(*factory_ptr).vtbl).create_decoder_from_stream)(
                factory_ptr,
                stream.as_ptr(),
                std::ptr::null(),
                0,
                &mut decoder,
            );
            if !com_ok(hr) {
                return None;
            }
            let decoder = Com::from_raw(decoder)?;
            let mut frame: *mut IWICBitmapFrameDecode = std::ptr::null_mut();
            let hr = ((*(*decoder.as_ptr()).vtbl).get_frame)(decoder.as_ptr(), 0, &mut frame);
            if !com_ok(hr) {
                return None;
            }
            let frame = Com::from_raw(frame)?;
            let mut converter: *mut IWICFormatConverter = std::ptr::null_mut();
            let hr = ((*(*factory_ptr).vtbl).create_format_converter)(
                factory_ptr,
                &mut converter,
            );
            if !com_ok(hr) {
                return None;
            }
            let converter = Com::from_raw(converter)?;
            // dither none, no palette, threshold 0, palette type custom
            let hr = ((*(*converter.as_ptr()).vtbl).initialize)(
                converter.as_ptr(),
                frame.as_ptr() as *mut c_void,
                &WIC_PIXEL_32BPP_RGBA,
                0,
                std::ptr::null_mut(),
                0.0,
                0,
            );
            if !com_ok(hr) {
                return None;
            }
            let mut width = 0u32;
            let mut height = 0u32;
            let hr = ((*(*converter.as_ptr()).vtbl).source.get_size)(
                converter.as_ptr() as *mut c_void,
                &mut width,
                &mut height,
            );
            if !com_ok(hr) || width == 0 || height == 0 {
                return None;
            }
            Some(DecodedSource { converter, width, height })
        }
    }

    /// The retained source for an identity, decoding on the first ask.
    fn with_decoded<T>(
        &self,
        source: &ImageSource,
        read: impl FnOnce(&DecodedSource) -> T,
    ) -> Option<T> {
        let key = source.key();
        if let Some(entry) = self.decoded.borrow().get(&key) {
            return entry.as_ref().map(read);
        }
        let decoded = match source {
            ImageSource::Bytes { bytes, .. } => self.decode(bytes),
            // icons resolve per size through the shell, never here
            ImageSource::FileIcon { .. } => None,
            // symbols never reach an engine — the door intercepts them
            ImageSource::Symbol { .. } => None,
        };
        let answer = decoded.as_ref().map(read);
        self.decoded.borrow_mut().insert(key, decoded);
        answer
    }

    /// The one resample of the pipeline: the platform scaler at
    /// EXACTLY the asked physical size; the same size skips it and
    /// hands the pixels over verbatim.
    fn resample(&self, decoded: &DecodedSource, width: usize, height: usize) -> Option<Vec<u8>> {
        let mut rgba = vec![0u8; width * height * 4];
        unsafe {
            if (decoded.width as usize, decoded.height as usize) == (width, height) {
                let hr = ((*(*decoded.converter.as_ptr()).vtbl).source.copy_pixels)(
                    decoded.converter.as_ptr() as *mut c_void,
                    std::ptr::null(),
                    (width * 4) as u32,
                    rgba.len() as u32,
                    rgba.as_mut_ptr(),
                );
                return com_ok(hr).then_some(rgba);
            }
            let factory = self.factory.as_ref()?;
            let factory_ptr = factory.as_ptr();
            let mut scaler: *mut IWICBitmapScaler = std::ptr::null_mut();
            let hr = ((*(*factory_ptr).vtbl).create_bitmap_scaler)(factory_ptr, &mut scaler);
            if !com_ok(hr) {
                return None;
            }
            let scaler = Com::from_raw(scaler)?;
            let hr = ((*(*scaler.as_ptr()).vtbl).initialize)(
                scaler.as_ptr(),
                decoded.converter.as_ptr() as *mut c_void,
                width as u32,
                height as u32,
                WIC_INTERPOLATION_HIGH_QUALITY_CUBIC,
            );
            if !com_ok(hr) {
                return None;
            }
            let hr = ((*(*scaler.as_ptr()).vtbl).source.copy_pixels)(
                scaler.as_ptr() as *mut c_void,
                std::ptr::null(),
                (width * 4) as u32,
                rgba.len() as u32,
                rgba.as_mut_ptr(),
            );
            com_ok(hr).then_some(rgba)
        }
    }
}

/// The shell's icon for a path at EXACTLY width×height, as straight
/// RGBA. The shell hands premultiplied BGRA back; this road pays the
/// unpremultiply the converter road does not.
fn icon_rgba(path: &str, width: usize, height: usize) -> Option<Vec<u8>> {
    com_init();
    unsafe {
        let wide_path = wide(path);
        let mut raw: *mut c_void = std::ptr::null_mut();
        let hr = SHCreateItemFromParsingName(
            wide_path.as_ptr(),
            std::ptr::null_mut(),
            &IID_ISHELL_ITEM_IMAGE_FACTORY,
            &mut raw,
        );
        if !com_ok(hr) {
            return None;
        }
        let item = Com::from_raw(raw as *mut IShellItemImageFactory)?;
        let mut bitmap: isize = 0;
        let hr = ((*(*item.as_ptr()).vtbl).get_image)(
            item.as_ptr(),
            SizeI { cx: width as i32, cy: height as i32 },
            SIIGBF_ICON_ONLY,
            &mut bitmap,
        );
        if !com_ok(hr) || bitmap == 0 {
            return None;
        }
        // pull the premultiplied BGRA out of the shell's bitmap,
        // top-down
        let mut info = GdiBitmapInfo {
            size: 40, // the header alone
            width: width as i32,
            height: -(height as i32),
            planes: 1,
            bit_count: 32,
            compression: 0,
            size_image: 0,
            x_pels_per_meter: 0,
            y_pels_per_meter: 0,
            clr_used: 0,
            clr_important: 0,
            colors: [0],
        };
        let mut bgra = vec![0u8; width * height * 4];
        let dc = CreateCompatibleDC(0);
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            bgra.as_mut_ptr() as *mut c_void,
            &mut info,
            0, // DIB_RGB_COLORS
        );
        DeleteDC(dc);
        DeleteObject(bitmap);
        if lines == 0 {
            return None;
        }
        // unpremultiply + BGRA→RGBA, the fused pass
        for pixel in bgra.chunks_exact_mut(4) {
            let alpha = pixel[3] as u32;
            if alpha > 0 && alpha < 255 {
                for channel in 0..3 {
                    pixel[channel] =
                        ((pixel[channel] as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
            pixel.swap(0, 2);
        }
        Some(bgra)
    }
}

impl Default for WicImageEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageEngine for WicImageEngine {
    fn intrinsic(&self, source: &ImageSource) -> Option<(u32, u32)> {
        if let ImageSource::FileIcon { .. } = source {
            // system icons are multi-representation; the fixed contract
            // stands in — the normal use is `.resizable()` plus a frame
            return Some((FILE_ICON_SIZE, FILE_ICON_SIZE));
        }
        self.with_decoded(source, |decoded| (decoded.width, decoded.height))
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
        let rgba = match source {
            ImageSource::Bytes { .. } => {
                self.with_decoded(source, |decoded| self.resample(decoded, width, height))??
            }
            ImageSource::FileIcon { path, .. } => icon_rgba(path, width, height)?,
            ImageSource::Symbol { .. } => {
                debug_assert!(false, "a symbol never reaches an engine");
                return None;
            }
        };

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
        let engine = WicImageEngine::new();
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
        let engine = WicImageEngine::new();
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
    fn a_translucent_pixel_stays_straight() {
        let engine = WicImageEngine::new();
        // half-transparent pure red: premultiplication would darken it
        let source = ImageSource::from_bytes(tiny_png(1, 1, &[255, 0, 0, 128]));
        let raster = engine.raster(&source, 1, 1).expect("decoded");
        assert_eq!(&raster.rgba[..], &[255, 0, 0, 128], "straight alpha, verbatim");
    }

    #[test]
    fn a_file_icon_arrives_from_the_shell() {
        let engine = WicImageEngine::new();
        let icon = bunny_ui::image_engine::file_icon("C:\\Windows\\explorer.exe");
        assert_eq!(engine.intrinsic(&icon), Some((32, 32)), "the fixed contract");

        let small = engine.raster(&icon, 16, 16).expect("an icon at sixteen");
        assert_eq!((small.width, small.height), (16, 16));
        assert!(small.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0), "the icon has ink");
        let large = engine.raster(&icon, 64, 64).expect("an icon at sixty-four");
        assert_eq!((large.width, large.height), (64, 64));

        // the cache answers the scroll — the shell never hears the
        // same (path, size) twice
        let again = engine.raster(&icon, 16, 16).expect("cached");
        assert!(Rc::ptr_eq(&small, &again));
    }

    #[test]
    fn broken_bytes_fail_once_and_stay_failed() {
        let engine = WicImageEngine::new();
        let broken = ImageSource::from_bytes(&b"not an image at all"[..]);
        assert_eq!(engine.intrinsic(&broken), None);
        assert!(engine.raster(&broken, 8, 8).is_none());
        assert_eq!(engine.intrinsic(&broken), None, "the failure is remembered");
    }

    #[test]
    fn the_raster_cache_evicts_whole_past_its_cap() {
        let engine = WicImageEngine::new();
        let source = ImageSource::from_bytes(quadrants_png());
        let first = engine.raster(&source, 3, 3).expect("resampled");
        // fill past the cap: eviction is total, then life goes on
        for size in 4..(4 + IMAGE_KEEP) {
            engine.raster(&source, size, size).expect("resampled");
        }
        let after = engine.raster(&source, 3, 3).expect("fresh after the sweep");
        assert!(!Rc::ptr_eq(&first, &after), "the cap swept the cache");
    }
}
