//! DirectWrite through the house FFI — the Windows text engine.
//!
//! Implements the bunny-ui [`TextEngine`] border: measuring by the
//! FONT's metrics (stable — the line's metrics jump when a glyph
//! fallback kicks in, and line height must not jump per string) and
//! rastering one line through a Direct2D target over a WIC bitmap —
//! the platform's twin of the mac's `CGBitmapContext` road, and the
//! road that renders color emoji without extra work.
//!
//! Direct2D only draws premultiplied; the bunny-ui compositor blends
//! STRAIGHT alpha (a single path for all engines), so the rectangle
//! is unpremultiplied in place before leaving — fused with the
//! BGRA→RGBA swizzle, one pass over a small text rectangle. The text
//! antialias mode is GRAYSCALE by requirement: ClearType's subpixel
//! ink bakes color fringes against an unknown background and cannot
//! become straight alpha.
//!
//! Fonts: the `Default` design is Segoe UI; `Mono` is Consolas. An
//! unknown family DEGRADES to the system font — it never fails. Each
//! text format is created once per `FontKey` and retained in the
//! engine, together with the font-box metrics resolved once. If the
//! platform refuses the factories entirely, the engine degrades to
//! the house pixel font with one line on stderr — it never fails to
//! open.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use bunny_ui::layout::Color;
use bunny_ui::text_engine::{
    FontDesign, FontKey, FontSpec, LineMetrics, PixelFont, Slant, TextEngine, TextRaster, Weight,
};

use crate::ffi::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, Com, Guid, Hresult, com_init, com_ok, wide,
};

// MARK: - The platform border

#[link(name = "dwrite", kind = "raw-dylib")]
unsafe extern "system" {
    fn DWriteCreateFactory(kind: u32, iid: *const Guid, out: *mut *mut c_void) -> Hresult;
}

#[link(name = "d2d1", kind = "raw-dylib")]
unsafe extern "system" {
    fn D2D1CreateFactory(
        kind: u32,
        iid: *const Guid,
        options: *const c_void,
        out: *mut *mut c_void,
    ) -> Hresult;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetUserDefaultLocaleName(name: *mut u16, length: i32) -> i32;
}

// IID_IDWriteFactory {B859EE5A-D838-4B5B-A2E8-1ADC7D93DB48}
const IID_IDWRITE_FACTORY: Guid = Guid {
    d1: 0xB859_EE5A,
    d2: 0xD838,
    d3: 0x4B5B,
    d4: [0xA2, 0xE8, 0x1A, 0xDC, 0x7D, 0x93, 0xDB, 0x48],
};
// IID_ID2D1Factory {06152247-6F50-465A-9245-118BFD3B6007}
const IID_ID2D1_FACTORY: Guid = Guid {
    d1: 0x0615_2247,
    d2: 0x6F50,
    d3: 0x465A,
    d4: [0x92, 0x45, 0x11, 0x8B, 0xFD, 0x3B, 0x60, 0x07],
};
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
// GUID_WICPixelFormat32bppPBGRA {6FDDC324-4E03-4BFE-B185-3D77768DC910}
const WIC_PIXEL_32BPP_PBGRA: Guid = Guid {
    d1: 0x6FDD_C324,
    d2: 0x4E03,
    d3: 0x4BFE,
    d4: [0xB1, 0x85, 0x3D, 0x77, 0x76, 0x8D, 0xC9, 0x10],
};

const DWRITE_FACTORY_SHARED: u32 = 0;
const D2D1_FACTORY_SINGLE_THREADED: u32 = 0;
const DWRITE_WORD_WRAPPING_NO_WRAP: u32 = 1;
const DWRITE_LINE_SPACING_UNIFORM: u32 = 1;
const DWRITE_FONT_STYLE_NORMAL: u32 = 0;
const DWRITE_FONT_STRETCH_NORMAL: u32 = 5;
const WIC_CACHE_ON_LOAD: u32 = 1;
/// `DXGI_FORMAT_B8G8R8A8_UNORM` — the only format a WIC target blends.
const DXGI_B8G8R8A8_UNORM: u32 = 87;
const D2D1_ALPHA_PREMULTIPLIED: u32 = 1;
/// `D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE` — the straight-alpha-compatible mode.
const D2D1_TEXT_AA_GRAYSCALE: u32 = 2;
/// `D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT` — emoji keep their color.
const D2D1_DRAW_TEXT_COLOR_FONT: u32 = 0x4;

// MARK: - Vtables (dwrite.h / d2d1.h / wincodec.h, in header order)

#[repr(C)]
struct DwriteFontMetrics {
    design_units_per_em: u16,
    ascent: u16,
    descent: u16,
    line_gap: i16,
    cap_height: u16,
    x_height: u16,
    underline_position: i16,
    underline_thickness: u16,
    strikethrough_position: i16,
    strikethrough_thickness: u16,
}

// slots 0-2 IUnknown; 3 GetSystemFontCollection; 4..=14 loaders and
// custom collections; 15 CreateTextFormat; 16 CreateTypography,
// 17 GetGdiInterop; 18 CreateTextLayout; the rest unused.
#[repr(C)]
struct IDWriteFactoryVtbl {
    unknown: crate::ffi::UnknownVtbl,
    get_system_font_collection: unsafe extern "system" fn(
        *mut IDWriteFactory,
        *mut *mut IDWriteFontCollection,
        i32,
    ) -> Hresult,
    _pad_4_14: [usize; 11],
    create_text_format: unsafe extern "system" fn(
        *mut IDWriteFactory,
        *const u16,
        *mut c_void,
        u32,
        u32,
        u32,
        f32,
        *const u16,
        *mut *mut IDWriteTextFormat,
    ) -> Hresult,
    _pad_16_17: [usize; 2],
    create_text_layout: unsafe extern "system" fn(
        *mut IDWriteFactory,
        *const u16,
        u32,
        *mut IDWriteTextFormat,
        f32,
        f32,
        *mut *mut IDWriteTextLayout,
    ) -> Hresult,
}
#[repr(C)]
struct IDWriteFactory {
    vtbl: *const IDWriteFactoryVtbl,
}

// slots 0-2 IUnknown; 3 GetFontFamilyCount; 4 GetFontFamily;
// 5 FindFamilyName; 6 GetFontFromFontFace.
#[repr(C)]
struct IDWriteFontCollectionVtbl {
    unknown: crate::ffi::UnknownVtbl,
    /// Returns the count itself, not an HRESULT.
    get_font_family_count: unsafe extern "system" fn(*mut IDWriteFontCollection) -> u32,
    get_font_family: unsafe extern "system" fn(
        *mut IDWriteFontCollection,
        u32,
        *mut *mut IDWriteFontFamily,
    ) -> Hresult,
    find_family_name: unsafe extern "system" fn(
        *mut IDWriteFontCollection,
        *const u16,
        *mut u32,
        *mut i32,
    ) -> Hresult,
}
#[repr(C)]
struct IDWriteFontCollection {
    vtbl: *const IDWriteFontCollectionVtbl,
}

// IDWriteFontList: 3 GetFontCollection, 4 GetFontCount, 5 GetFont;
// IDWriteFontFamily adds 6 GetFamilyNames, 7 GetFirstMatchingFont.
#[repr(C)]
struct IDWriteFontFamilyVtbl {
    unknown: crate::ffi::UnknownVtbl,
    _pad_3_5: [usize; 3],
    get_family_names: unsafe extern "system" fn(
        *mut IDWriteFontFamily,
        *mut *mut IDWriteLocalizedStrings,
    ) -> Hresult,
    get_first_matching_font: unsafe extern "system" fn(
        *mut IDWriteFontFamily,
        u32,
        u32,
        u32,
        *mut *mut IDWriteFont,
    ) -> Hresult,
}
#[repr(C)]
struct IDWriteFontFamily {
    vtbl: *const IDWriteFontFamilyVtbl,
}

// slots 0-2 IUnknown; 3 GetCount; 4 FindLocaleName; 5 GetLocaleNameLength;
// 6 GetLocaleName; 7 GetStringLength; 8 GetString.
#[repr(C)]
struct IDWriteLocalizedStringsVtbl {
    unknown: crate::ffi::UnknownVtbl,
    /// Returns the count itself, not an HRESULT.
    get_count: unsafe extern "system" fn(*mut IDWriteLocalizedStrings) -> u32,
    _pad_4_6: [usize; 3],
    /// The length WITHOUT the terminator; the buffer must hold one more.
    get_string_length: unsafe extern "system" fn(
        *mut IDWriteLocalizedStrings,
        u32,
        *mut u32,
    ) -> Hresult,
    get_string: unsafe extern "system" fn(
        *mut IDWriteLocalizedStrings,
        u32,
        *mut u16,
        u32,
    ) -> Hresult,
}
#[repr(C)]
struct IDWriteLocalizedStrings {
    vtbl: *const IDWriteLocalizedStringsVtbl,
}

// slots 3..=10 family/weight/stretch/style/symbol/names/strings/
// simulations; 11 GetMetrics (returns void).
#[repr(C)]
struct IDWriteFontVtbl {
    unknown: crate::ffi::UnknownVtbl,
    _pad_3_10: [usize; 8],
    get_metrics: unsafe extern "system" fn(*mut IDWriteFont, *mut DwriteFontMetrics),
}
#[repr(C)]
struct IDWriteFont {
    vtbl: *const IDWriteFontVtbl,
}

// slots 3 SetTextAlignment, 4 SetParagraphAlignment, 5 SetWordWrapping,
// 6..=9 direction/tabs/trimming, 10 SetLineSpacing, 11..=27 getters.
#[repr(C)]
struct IDWriteTextFormatVtbl {
    unknown: crate::ffi::UnknownVtbl,
    _pad_3_4: [usize; 2],
    set_word_wrapping: unsafe extern "system" fn(*mut IDWriteTextFormat, u32) -> Hresult,
    _pad_6_9: [usize; 4],
    set_line_spacing:
        unsafe extern "system" fn(*mut IDWriteTextFormat, u32, f32, f32) -> Hresult,
}
#[repr(C)]
struct IDWriteTextFormat {
    vtbl: *const IDWriteTextFormatVtbl,
}

#[repr(C)]
#[derive(Default)]
struct DwriteTextMetrics {
    left: f32,
    top: f32,
    width: f32,
    width_including_trailing_whitespace: f32,
    height: f32,
    layout_width: f32,
    layout_height: f32,
    max_bidi_reordering_depth: u32,
    line_count: u32,
}

// IDWriteTextLayout inherits IDWriteTextFormat's slots 3..=27; its own
// setters/getters run 28..=57 (the typography getter mirrors its
// setter — 39 own methods), 58 Draw, 59 GetLineMetrics, 60 GetMetrics;
// the rest unused.
#[repr(C)]
struct IDWriteTextLayoutVtbl {
    unknown: crate::ffi::UnknownVtbl,
    _pad_3_59: [usize; 57],
    get_metrics:
        unsafe extern "system" fn(*mut IDWriteTextLayout, *mut DwriteTextMetrics) -> Hresult,
}
#[repr(C)]
struct IDWriteTextLayout {
    vtbl: *const IDWriteTextLayoutVtbl,
}

#[repr(C)]
struct D2dPixelFormat {
    format: u32,
    alpha_mode: u32,
}

#[repr(C)]
struct D2dRenderTargetProperties {
    kind: u32,
    pixel_format: D2dPixelFormat,
    dpi_x: f32,
    dpi_y: f32,
    usage: u32,
    min_level: u32,
}

#[repr(C)]
struct D2dColor {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct D2dPoint {
    x: f32,
    y: f32,
}

// slots 3 ReloadSystemMetrics, 4 GetDesktopDpi, 5..=12 geometry and
// stroke factories; 13 CreateWicBitmapRenderTarget; the rest unused.
#[repr(C)]
struct ID2D1FactoryVtbl {
    unknown: crate::ffi::UnknownVtbl,
    _pad_3_12: [usize; 10],
    create_wic_bitmap_render_target: unsafe extern "system" fn(
        *mut ID2D1Factory,
        *mut IWICBitmap,
        *const D2dRenderTargetProperties,
        *mut *mut ID2D1RenderTarget,
    ) -> Hresult,
}
#[repr(C)]
struct ID2D1Factory {
    vtbl: *const ID2D1FactoryVtbl,
}

// ID2D1Resource: 3 GetFactory. ID2D1RenderTarget: 4..=7 bitmap and
// brush factories, 8 CreateSolidColorBrush, 9..=27 brushes/layers/
// draw calls, 28 DrawTextLayout, 29..=33 glyph run and transforms,
// 34 SetTextAntialiasMode, 35..=46 params/tags/layers/clip, 47 Clear,
// 48 BeginDraw, 49 EndDraw. GetPixelFormat (50) and the other
// struct-return methods are never declared — the ABI prohibition.
#[repr(C)]
struct ID2D1RenderTargetVtbl {
    unknown: crate::ffi::UnknownVtbl,
    _pad_3_7: [usize; 5],
    create_solid_color_brush: unsafe extern "system" fn(
        *mut ID2D1RenderTarget,
        *const D2dColor,
        *const c_void,
        *mut *mut ID2D1Brush,
    ) -> Hresult,
    _pad_9_27: [usize; 19],
    draw_text_layout: unsafe extern "system" fn(
        *mut ID2D1RenderTarget,
        D2dPoint,
        *mut IDWriteTextLayout,
        *mut ID2D1Brush,
        u32,
    ),
    _pad_29_33: [usize; 5],
    set_text_antialias_mode: unsafe extern "system" fn(*mut ID2D1RenderTarget, u32),
    _pad_35_46: [usize; 12],
    clear: unsafe extern "system" fn(*mut ID2D1RenderTarget, *const D2dColor),
    begin_draw: unsafe extern "system" fn(*mut ID2D1RenderTarget),
    end_draw:
        unsafe extern "system" fn(*mut ID2D1RenderTarget, *mut u64, *mut u64) -> Hresult,
}
#[repr(C)]
struct ID2D1RenderTarget {
    vtbl: *const ID2D1RenderTargetVtbl,
}

#[repr(C)]
struct ID2D1Brush {
    vtbl: *const c_void,
}

// slots 3..=16 decoders/encoders/palette/converters/scaler/stream;
// 17 CreateBitmap; the rest unused.
#[repr(C)]
struct IWICImagingFactoryVtbl {
    unknown: crate::ffi::UnknownVtbl,
    _pad_3_16: [usize; 14],
    create_bitmap: unsafe extern "system" fn(
        *mut IWICImagingFactory,
        u32,
        u32,
        *const Guid,
        u32,
        *mut *mut IWICBitmap,
    ) -> Hresult,
}
#[repr(C)]
struct IWICImagingFactory {
    vtbl: *const IWICImagingFactoryVtbl,
}

// IWICBitmapSource: 3 GetSize, 4 GetPixelFormat, 5 GetResolution,
// 6 CopyPalette, 7 CopyPixels. IWICBitmap's own slots are unused.
#[repr(C)]
struct IWICBitmapVtbl {
    unknown: crate::ffi::UnknownVtbl,
    _pad_3_6: [usize; 4],
    copy_pixels: unsafe extern "system" fn(
        *mut IWICBitmap,
        *const c_void,
        u32,
        u32,
        *mut u8,
    ) -> Hresult,
}
#[repr(C)]
struct IWICBitmap {
    vtbl: *const IWICBitmapVtbl,
}

// MARK: - The engine

/// One retained face: the text format (shaping) and the font-box
/// metrics (measure), both resolved once per `FontKey`.
struct FontSlot {
    format: Com<IDWriteTextFormat>,
    ascent: f64,
    descent: f64,
}

/// The three factories the raster road needs. Absent on a platform
/// that refuses them — the engine degrades to the pixel font.
struct Factories {
    dwrite: Com<IDWriteFactory>,
    d2d: Com<ID2D1Factory>,
    wic: Com<IWICImagingFactory>,
    locale: Vec<u16>,
}

/// The Windows text engine. Single-thread, like the rest of the shell.
pub struct DirectWriteEngine {
    factories: Option<Factories>,
    fonts: RefCell<HashMap<FontKey, FontSlot>>,
    fallback: PixelFont,
}

fn family_of(design: FontDesign) -> &'static str {
    match design {
        FontDesign::Default => "Segoe UI",
        FontDesign::Mono => "Consolas",
    }
}

/// `DWRITE_FONT_STYLE_ITALIC` — the family's own leaning face when it
/// has one; DirectWrite falls back to a simulated oblique when it does
/// not, which is the same dignity CoreText gives.
const DWRITE_FONT_STYLE_ITALIC: u32 = 2;

fn style_of(slant: Slant) -> u32 {
    match slant {
        Slant::Upright => DWRITE_FONT_STYLE_NORMAL,
        Slant::Italic => DWRITE_FONT_STYLE_ITALIC,
    }
}

fn weight_of(weight: Weight) -> u32 {
    match weight {
        Weight::Regular => 400,
        Weight::Medium => 500,
        Weight::Semibold => 600,
        Weight::Bold => 700,
        Weight::ExtraBold => 800,
        Weight::Black => 900,
    }
}

fn create_factories() -> Option<Factories> {
    com_init();
    unsafe {
        let mut dwrite: *mut c_void = std::ptr::null_mut();
        let hr = DWriteCreateFactory(DWRITE_FACTORY_SHARED, &IID_IDWRITE_FACTORY, &mut dwrite);
        if !com_ok(hr) {
            eprintln!("bunny_ui dwrite: no DirectWrite factory (0x{:08X})", hr as u32);
            return None;
        }
        let dwrite = Com::from_raw(dwrite as *mut IDWriteFactory)?;

        let mut d2d: *mut c_void = std::ptr::null_mut();
        let hr = D2D1CreateFactory(
            D2D1_FACTORY_SINGLE_THREADED,
            &IID_ID2D1_FACTORY,
            std::ptr::null(),
            &mut d2d,
        );
        if !com_ok(hr) {
            eprintln!("bunny_ui dwrite: no Direct2D factory (0x{:08X})", hr as u32);
            return None;
        }
        let d2d = Com::from_raw(d2d as *mut ID2D1Factory)?;

        let mut wic: *mut c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_WIC_IMAGING_FACTORY,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IWIC_IMAGING_FACTORY,
            &mut wic,
        );
        if !com_ok(hr) {
            eprintln!("bunny_ui dwrite: no WIC factory (0x{:08X})", hr as u32);
            return None;
        }
        let wic = Com::from_raw(wic as *mut IWICImagingFactory)?;

        // LOCALE_NAME_MAX_LENGTH = 85
        let mut locale = vec![0u16; 85];
        if GetUserDefaultLocaleName(locale.as_mut_ptr(), locale.len() as i32) == 0 {
            locale = wide("en-US");
        }
        Some(Factories { dwrite, d2d, wic, locale })
    }
}

impl DirectWriteEngine {
    pub fn new() -> Self {
        DirectWriteEngine {
            factories: create_factories(),
            fonts: RefCell::new(HashMap::new()),
            fallback: PixelFont,
        }
    }

    /// Resolves (once) and answers the slot for a spec. `None` only
    /// when the factories are absent.
    fn with_slot<T>(&self, spec: &FontSpec, read: impl FnOnce(&FontSlot) -> T) -> Option<T> {
        let factories = self.factories.as_ref()?;
        let key = spec.key();
        if let Some(slot) = self.fonts.borrow().get(&key) {
            return Some(read(slot));
        }
        let slot = create_slot(factories, spec)?;
        let answer = read(&slot);
        self.fonts.borrow_mut().insert(key, slot);
        Some(answer)
    }
}

/// The slot: a no-wrap text format pinned to the font-box line, plus
/// the box itself. An unknown family degrades to the system font.
fn create_slot(factories: &Factories, spec: &FontSpec) -> Option<FontSlot> {
    unsafe {
        let dwrite = factories.dwrite.as_ptr();
        // a family the app NAMED is the most specific thing anyone said
        // about this text; the road below already degrades a name this
        // machine has not got, so it needs nothing of its own
        let named = spec.family.name();
        let mut family: &str = match &named {
            Some(name) => name,
            None => family_of(spec.design),
        };

        // the font box, from the family's face — not from any string
        let mut collection: *mut IDWriteFontCollection = std::ptr::null_mut();
        let hr = ((*(*dwrite).vtbl).get_system_font_collection)(dwrite, &mut collection, 0);
        if !com_ok(hr) {
            eprintln!("bunny_ui dwrite: no system font collection (0x{:08X})", hr as u32);
            return None;
        }
        let collection = Com::from_raw(collection)?;
        let mut index = 0u32;
        let mut exists = 0i32;
        let name = wide(family);
        ((*(*collection.as_ptr()).vtbl).find_family_name)(
            collection.as_ptr(),
            name.as_ptr(),
            &mut index,
            &mut exists,
        );
        if exists == 0 {
            // an unknown family degrades, never fails
            family = "Segoe UI";
            let name = wide(family);
            ((*(*collection.as_ptr()).vtbl).find_family_name)(
                collection.as_ptr(),
                name.as_ptr(),
                &mut index,
                &mut exists,
            );
            if exists == 0 {
                return None;
            }
        }
        let mut family_object: *mut IDWriteFontFamily = std::ptr::null_mut();
        let hr = ((*(*collection.as_ptr()).vtbl).get_font_family)(
            collection.as_ptr(),
            index,
            &mut family_object,
        );
        if !com_ok(hr) {
            return None;
        }
        let family_object = Com::from_raw(family_object)?;
        let mut font: *mut IDWriteFont = std::ptr::null_mut();
        let hr = ((*(*family_object.as_ptr()).vtbl).get_first_matching_font)(
            family_object.as_ptr(),
            weight_of(spec.weight),
            DWRITE_FONT_STRETCH_NORMAL,
            style_of(spec.slant),
            &mut font,
        );
        if !com_ok(hr) {
            return None;
        }
        let font = Com::from_raw(font)?;
        let mut metrics = DwriteFontMetrics {
            design_units_per_em: 0,
            ascent: 0,
            descent: 0,
            line_gap: 0,
            cap_height: 0,
            x_height: 0,
            underline_position: 0,
            underline_thickness: 0,
            strikethrough_position: 0,
            strikethrough_thickness: 0,
        };
        ((*(*font.as_ptr()).vtbl).get_metrics)(font.as_ptr(), &mut metrics);
        let upem = metrics.design_units_per_em.max(1) as f64;
        let ascent = metrics.ascent as f64 * spec.size / upem;
        // the leading folds into the descent — the LineMetrics contract
        let descent =
            (metrics.descent as f64 + metrics.line_gap.max(0) as f64) * spec.size / upem;

        let family_name = wide(family);
        let mut format: *mut IDWriteTextFormat = std::ptr::null_mut();
        let hr = ((*(*dwrite).vtbl).create_text_format)(
            dwrite,
            family_name.as_ptr(),
            std::ptr::null_mut(),
            weight_of(spec.weight),
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            spec.size as f32,
            factories.locale.as_ptr(),
            &mut format,
        );
        if !com_ok(hr) {
            eprintln!("bunny_ui dwrite: CreateTextFormat failed (0x{:08X})", hr as u32);
            return None;
        }
        let format = Com::from_raw(format)?;
        // breaking is ours, and the drawn line box is pinned to OUR
        // metrics — the twin of the mac's explicit text position
        ((*(*format.as_ptr()).vtbl).set_word_wrapping)(
            format.as_ptr(),
            DWRITE_WORD_WRAPPING_NO_WRAP,
        );
        ((*(*format.as_ptr()).vtbl).set_line_spacing)(
            format.as_ptr(),
            DWRITE_LINE_SPACING_UNIFORM,
            (ascent + descent) as f32,
            ascent as f32,
        );
        Some(FontSlot { format, ascent, descent })
    }
}

/// The text's layout with the slot's format. The caller owns it.
fn create_layout(
    factories: &Factories,
    slot: &FontSlot,
    text: &str,
) -> Option<Com<IDWriteTextLayout>> {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        let dwrite = factories.dwrite.as_ptr();
        let mut layout: *mut IDWriteTextLayout = std::ptr::null_mut();
        let hr = ((*(*dwrite).vtbl).create_text_layout)(
            dwrite,
            utf16.as_ptr(),
            utf16.len() as u32,
            slot.format.as_ptr(),
            1e9,
            1e9,
            &mut layout,
        );
        if !com_ok(hr) {
            return None;
        }
        Com::from_raw(layout)
    }
}

impl Default for DirectWriteEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEngine for DirectWriteEngine {
    fn families(&self) -> Vec<std::sync::Arc<str>> {
        let Some(factories) = self.factories.as_ref() else {
            return Vec::new();
        };
        unsafe {
            let dwrite = factories.dwrite.as_ptr();
            let mut collection: *mut IDWriteFontCollection = std::ptr::null_mut();
            if !com_ok(((*(*dwrite).vtbl).get_system_font_collection)(
                dwrite,
                &mut collection,
                0,
            )) {
                return Vec::new();
            }
            let Some(collection) = Com::from_raw(collection) else {
                return Vec::new();
            };
            let count = ((*(*collection.as_ptr()).vtbl).get_font_family_count)(
                collection.as_ptr(),
            );
            let mut names: Vec<std::sync::Arc<str>> = Vec::new();
            for index in 0..count {
                let mut family: *mut IDWriteFontFamily = std::ptr::null_mut();
                if !com_ok(((*(*collection.as_ptr()).vtbl).get_font_family)(
                    collection.as_ptr(),
                    index,
                    &mut family,
                )) {
                    continue;
                }
                let Some(family) = Com::from_raw(family) else {
                    continue;
                };
                let mut strings: *mut IDWriteLocalizedStrings = std::ptr::null_mut();
                if !com_ok(((*(*family.as_ptr()).vtbl).get_family_names)(
                    family.as_ptr(),
                    &mut strings,
                )) {
                    continue;
                }
                let Some(strings) = Com::from_raw(strings) else {
                    continue;
                };
                // the first locale is the family's own name — a roster
                // is a list of names, not a translation table
                if ((*(*strings.as_ptr()).vtbl).get_count)(strings.as_ptr()) == 0 {
                    continue;
                }
                let mut length = 0u32;
                if !com_ok(((*(*strings.as_ptr()).vtbl).get_string_length)(
                    strings.as_ptr(),
                    0,
                    &mut length,
                )) {
                    continue;
                }
                // the length leaves the terminator out; the buffer holds it
                let mut buffer = vec![0u16; length as usize + 1];
                if !com_ok(((*(*strings.as_ptr()).vtbl).get_string)(
                    strings.as_ptr(),
                    0,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                )) {
                    continue;
                }
                buffer.truncate(length as usize);
                names.push(std::sync::Arc::from(String::from_utf16_lossy(&buffer).as_str()));
            }
            names.sort();
            names.dedup();
            names
        }
    }

    fn measure_line(&self, text: &str, font: &FontSpec) -> LineMetrics {
        let Some(factories) = self.factories.as_ref() else {
            return self.fallback.measure_line(text, font);
        };
        let measured = self.with_slot(font, |slot| {
            if text.is_empty() {
                // line height preserved without creating a layout
                return Some(LineMetrics { width: 0.0, ascent: slot.ascent, descent: slot.descent });
            }
            let layout = create_layout(factories, slot, text)?;
            let mut metrics = DwriteTextMetrics::default();
            let hr = unsafe {
                ((*(*layout.as_ptr()).vtbl).get_metrics)(layout.as_ptr(), &mut metrics)
            };
            com_ok(hr).then(|| LineMetrics {
                // the advance width, trailing whitespace included — the
                // ink-trimmed width would break the caret on a trailing
                // space
                width: metrics.width_including_trailing_whitespace as f64,
                ascent: slot.ascent,
                descent: slot.descent,
            })
        });
        match measured {
            Some(Some(metrics)) => metrics,
            _ => self.fallback.measure_line(text, font),
        }
    }

    fn raster_line(
        &self,
        text: &str,
        font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Option<TextRaster> {
        if text.is_empty() {
            return None;
        }
        let Some(factories) = self.factories.as_ref() else {
            return self.fallback.raster_line(text, font, color, scale);
        };
        let metrics = self.measure_line(text, font);
        let width = (metrics.width * scale as f64).ceil() as usize;
        let height = (metrics.height() * scale as f64).ceil() as usize;
        if width == 0 || height == 0 {
            return None;
        }

        let mut rgba = self.with_slot(font, |slot| {
            let layout = create_layout(factories, slot, text)?;
            unsafe {
                let wic = factories.wic.as_ptr();
                let mut bitmap: *mut IWICBitmap = std::ptr::null_mut();
                let hr = ((*(*wic).vtbl).create_bitmap)(
                    wic,
                    width as u32,
                    height as u32,
                    &WIC_PIXEL_32BPP_PBGRA,
                    WIC_CACHE_ON_LOAD,
                    &mut bitmap,
                );
                if !com_ok(hr) {
                    return None;
                }
                let bitmap = Com::from_raw(bitmap)?;

                // drawing in logical points, bitmap in physical px —
                // the target's dpi is the platform's own scale knob
                let properties = D2dRenderTargetProperties {
                    kind: 0, // D2D1_RENDER_TARGET_TYPE_DEFAULT
                    pixel_format: D2dPixelFormat {
                        format: DXGI_B8G8R8A8_UNORM,
                        alpha_mode: D2D1_ALPHA_PREMULTIPLIED,
                    },
                    dpi_x: 96.0 * scale as f32,
                    dpi_y: 96.0 * scale as f32,
                    usage: 0,
                    min_level: 0,
                };
                let d2d = factories.d2d.as_ptr();
                let mut target: *mut ID2D1RenderTarget = std::ptr::null_mut();
                let hr = ((*(*d2d).vtbl).create_wic_bitmap_render_target)(
                    d2d,
                    bitmap.as_ptr(),
                    &properties,
                    &mut target,
                );
                if !com_ok(hr) {
                    return None;
                }
                let target = Com::from_raw(target)?;
                let target_ptr = target.as_ptr();

                ((*(*target_ptr).vtbl).set_text_antialias_mode)(target_ptr, D2D1_TEXT_AA_GRAYSCALE);
                ((*(*target_ptr).vtbl).begin_draw)(target_ptr);
                let clear = D2dColor { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
                ((*(*target_ptr).vtbl).clear)(target_ptr, &clear);
                let ink = D2dColor {
                    r: color.r as f32 / 255.0,
                    g: color.g as f32 / 255.0,
                    b: color.b as f32 / 255.0,
                    a: color.a as f32 / 255.0,
                };
                let mut brush: *mut ID2D1Brush = std::ptr::null_mut();
                let hr = ((*(*target_ptr).vtbl).create_solid_color_brush)(
                    target_ptr,
                    &ink,
                    std::ptr::null(),
                    &mut brush,
                );
                if !com_ok(hr) {
                    return None;
                }
                let brush = Com::from_raw(brush)?;
                // the ceil slack stays on TOP: the line box sits against
                // the bitmap's bottom, baseline `descent` above it
                let slack = height as f32 / scale as f32 - (slot.ascent + slot.descent) as f32;
                ((*(*target_ptr).vtbl).draw_text_layout)(
                    target_ptr,
                    D2dPoint { x: 0.0, y: slack.max(0.0) },
                    layout.as_ptr(),
                    brush.as_ptr(),
                    D2D1_DRAW_TEXT_COLOR_FONT,
                );
                let hr = ((*(*target_ptr).vtbl).end_draw)(
                    target_ptr,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if !com_ok(hr) {
                    return None;
                }

                let mut pixels = vec![0u8; width * height * 4];
                let hr = ((*(*bitmap.as_ptr()).vtbl).copy_pixels)(
                    bitmap.as_ptr(),
                    std::ptr::null(),
                    (width * 4) as u32,
                    pixels.len() as u32,
                    pixels.as_mut_ptr(),
                );
                com_ok(hr).then_some(pixels)
            }
        })??;

        // one fused pass over the small rectangle: unpremultiply (the
        // compositor blends straight) + BGRA→RGBA
        for pixel in rgba.chunks_exact_mut(4) {
            let alpha = pixel[3] as u32;
            if alpha > 0 && alpha < 255 {
                for channel in 0..3 {
                    pixel[channel] =
                        ((pixel[channel] as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
            pixel.swap(0, 2);
        }

        Some(TextRaster {
            width,
            height,
            baseline: (metrics.ascent * scale as f64).round() as usize,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_write_measures_and_rasters() {
        let engine = DirectWriteEngine::new();
        assert!(engine.factories.is_some(), "the platform factories exist on Windows");

        let metrics = engine.measure_line("Hello", &FontSpec::DEFAULT);
        assert!(metrics.width > 10.0 && metrics.width < 200.0, "real width: {}", metrics.width);
        assert!(metrics.ascent > 5.0);
        assert!(metrics.height() > metrics.ascent);

        let empty = engine.measure_line("", &FontSpec::DEFAULT);
        assert_eq!(empty.width, 0.0);
        assert_eq!(empty.height(), metrics.height(), "empty line preserves the height");

        let raster = engine
            .raster_line("Hello", &FontSpec::DEFAULT, Color::rgb(10, 20, 30), 2)
            .expect("has ink");
        assert!(raster.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0));
        assert!(raster.baseline > 0 && raster.baseline <= raster.height);
    }

    #[test]
    fn the_same_line_rasters_byte_identical() {
        let engine = DirectWriteEngine::new();
        let first = engine
            .raster_line("determinism", &FontSpec::DEFAULT, Color::rgb(0, 0, 0), 1)
            .expect("ink");
        let second = engine
            .raster_line("determinism", &FontSpec::DEFAULT, Color::rgb(0, 0, 0), 1)
            .expect("ink");
        assert_eq!(first.rgba, second.rgba, "the raster is deterministic");
        assert_eq!((first.width, first.height), (second.width, second.height));
    }

    #[test]
    fn the_alpha_is_straight_and_grayscale_antialiased() {
        let engine = DirectWriteEngine::new();
        let ink = Color::rgb(200, 40, 40);
        let raster = engine.raster_line("Hello", &FontSpec::DEFAULT, ink, 2).expect("ink");
        // grayscale AA leaves partial coverage at the edges
        let mut saw_edge = false;
        for pixel in raster.rgba.chunks_exact(4) {
            let alpha = pixel[3];
            if alpha > 16 && alpha < 240 {
                saw_edge = true;
                // straight alpha keeps the INK color on the edge; a
                // premultiplied leak would scale it toward black (±2
                // for the round trip through the platform's floats)
                assert!(
                    (pixel[0] as i32 - ink.r as i32).abs() <= 2
                        && (pixel[1] as i32 - ink.g as i32).abs() <= 2
                        && (pixel[2] as i32 - ink.b as i32).abs() <= 2,
                    "edge pixel keeps the ink: {:?}",
                    pixel
                );
            }
        }
        assert!(saw_edge, "antialiasing produced edges");
    }

    #[test]
    fn the_scale_doubles_the_pixels() {
        let engine = DirectWriteEngine::new();
        let one = engine.raster_line("scale", &FontSpec::DEFAULT, Color::rgb(0, 0, 0), 1).unwrap();
        let two = engine.raster_line("scale", &FontSpec::DEFAULT, Color::rgb(0, 0, 0), 2).unwrap();
        let height = (two.height as i64 - 2 * one.height as i64).abs();
        let width = two.width as i64 - 2 * one.width as i64;
        assert!(height <= 2, "height scales: {} vs {}", one.height, two.height);
        assert!(width.abs() <= 2, "width scales: {} vs {}", one.width, two.width);
    }

    #[test]
    fn mono_design_resolves_to_a_wider_grid_or_degrades() {
        let engine = DirectWriteEngine::new();
        let mono = FontSpec { design: FontDesign::Mono, ..FontSpec::DEFAULT };
        // "iiii" in mono has the SAME width as "mmmm" — the grid's proof
        let narrow = engine.measure_line("iiii", &mono);
        let wide = engine.measure_line("mmmm", &mono);
        assert!(
            (narrow.width - wide.width).abs() < 0.01,
            "mono grid: {} vs {}",
            narrow.width,
            wide.width
        );
    }

    #[test]
    fn an_emoji_measures_and_rasters_without_panic() {
        let engine = DirectWriteEngine::new();
        let metrics = engine.measure_line("🙂", &FontSpec::DEFAULT);
        assert!(metrics.width > 0.0, "the emoji advances");
        let raster = engine.raster_line("🙂", &FontSpec::DEFAULT, Color::rgb(0, 0, 0), 1);
        let raster = raster.expect("the color font renders");
        assert!(raster.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0), "the emoji has ink");
    }

    #[test]
    fn direct_write_wraps_words_with_real_measures() {
        use bunny_ui::text_engine::{MeasureCache, break_lines};

        let engine = DirectWriteEngine::new();
        let cache = MeasureCache::default();
        let text = "hello world hello world";
        let lines = break_lines(text, &FontSpec::DEFAULT, 60.0, &engine, &cache);

        assert!(lines.len() > 1, "60px does not hold the sentence: {lines:?}");
        assert_eq!(lines.first().unwrap().0, 0);
        assert_eq!(lines.last().unwrap().1, text.len());
        for window in lines.windows(2) {
            assert_eq!(window[0].1, window[1].0);
        }
    }
}

