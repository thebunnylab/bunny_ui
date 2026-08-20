//! CoreText through the house FFI — the Mac's text engine.
//!
//! Implements the bunny-ui [`TextEngine`] border: measuring by the FONT's
//! metrics (stable — the line's metrics jump when a glyph fallback kicks
//! in, and line height must not jump per string) and rastering one line
//! into a `CGBitmapContext` over our own buffer.
//!
//! The CG context only draws premultiplied; the bunny-ui compositor
//! blends STRAIGHT alpha (a single path for all engines — on the web
//! `putImageData` is straight too), so the rectangle is unpremultiplied
//! in place before leaving — one pass over a small text rectangle.
//!
//! Fonts: the `Default` design comes from `CTFontCreateUIFontForLanguage`
//! (the system interface font); `Mono` tries Menlo and, if the family
//! does not exist, DEGRADES to the system font — it never fails. Each
//! `CTFont` is created once per `FontKey` and retained in the engine.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use bunny_ui::layout::Color;
use bunny_ui::text_engine::{
    FontDesign, FontKey, FontSpec, Slant, LineMetrics, TextEngine, TextRaster, Weight,
};

use crate::ffi::{CFRelease, CGColorSpaceCreateDeviceRGB, CGColorSpaceRelease};

type CFStringRef = *const c_void;
type CTFontRef = *const c_void;
type CTLineRef = *const c_void;
type CGContextRef = *mut c_void;
type CFMutableAttributedStringRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CFRange {
    location: isize,
    length: isize,
}

const KCF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
/// `kCGImageAlphaPremultipliedLast` — the only RGBA layout a drawing
/// context accepts.
const ALPHA_PREMULTIPLIED_LAST: u32 = 1;
/// `kCTFontUIFontSystem` / `kCTFontUIFontEmphasizedSystem`.
const UI_FONT_SYSTEM: u32 = 2;
const UI_FONT_EMPHASIZED: u32 = 3;

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontCreateWithName(name: CFStringRef, size: f64, matrix: *const c_void) -> CTFontRef;
    /// The SAME font, asked to lean. `mask` says which traits to look
    /// at, `traits` what to want — and it answers NULL when the family
    /// has no such face, which is the whole error handling we need.
    fn CTFontCreateCopyWithSymbolicTraits(
        font: CTFontRef,
        size: f64,
        matrix: *const c_void,
        traits: u32,
        mask: u32,
    ) -> CTFontRef;
    fn CTFontCreateUIFontForLanguage(
        ui_type: u32,
        size: f64,
        language: CFStringRef,
    ) -> CTFontRef;
    fn CTFontGetAscent(font: CTFontRef) -> f64;
    fn CTFontGetDescent(font: CTFontRef) -> f64;
    fn CTFontGetLeading(font: CTFontRef) -> f64;
    fn CTLineCreateWithAttributedString(attributed: *const c_void) -> CTLineRef;
    fn CTLineGetTypographicBounds(
        line: CTLineRef,
        ascent: *mut f64,
        descent: *mut f64,
        leading: *mut f64,
    ) -> f64;
    fn CTLineDraw(line: CTLineRef, context: CGContextRef);
    /// Every family this Mac can shape, as a CFArray of CFStrings —
    /// the caller owns it.
    fn CTFontManagerCopyAvailableFontFamilyNames() -> *const c_void;
    /// The family the font ACTUALLY belongs to — the only way to know
    /// whether a name was honoured, because creating by name never
    /// fails: an unknown one silently answers a default face.
    fn CTFontCopyFamilyName(font: CTFontRef) -> CFStringRef;
    static kCTFontAttributeName: CFStringRef;
    static kCTForegroundColorFromContextAttributeName: CFStringRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(
        allocator: *const c_void,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: u8,
    ) -> CFStringRef;
    /// UTF-16 units — the count the attributes' `CFRange` uses.
    fn CFStringGetLength(string: CFStringRef) -> isize;
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    /// Copies the string out as UTF-8. Answers 0 when the buffer is too
    /// small, which is the only failure worth a second try.
    fn CFStringGetCString(
        string: CFStringRef,
        buffer: *mut u8,
        size: isize,
        encoding: u32,
    ) -> u8;
    fn CFAttributedStringCreateMutable(
        allocator: *const c_void,
        max_length: isize,
    ) -> CFMutableAttributedStringRef;
    fn CFAttributedStringReplaceString(
        attributed: CFMutableAttributedStringRef,
        range: CFRange,
        replacement: CFStringRef,
    );
    fn CFAttributedStringSetAttribute(
        attributed: CFMutableAttributedStringRef,
        range: CFRange,
        name: CFStringRef,
        value: *const c_void,
    );
    static kCFBooleanTrue: *const c_void;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
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
    fn CGContextScaleCTM(context: CGContextRef, sx: f64, sy: f64);
    fn CGContextSetTextPosition(context: CGContextRef, x: f64, y: f64);
    fn CGContextSetRGBFillColor(context: CGContextRef, r: f64, g: f64, b: f64, a: f64);
}

unsafe fn cf_string(text: &str) -> CFStringRef {
    unsafe {
        CFStringCreateWithBytes(
            std::ptr::null(),
            text.as_ptr(),
            text.len() as isize,
            KCF_STRING_ENCODING_UTF8,
            0,
        )
    }
}

/// A retained CTFont — released on Drop (the engine is the owner).
struct OwnedFont(*const c_void);

impl Drop for OwnedFont {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

/// `kCTFontItalicTrait` — bit 0 of the symbolic traits.
const FONT_TRAIT_ITALIC: u32 = 1;

unsafe fn create_font(spec: &FontSpec) -> CTFontRef {
    let upright = unsafe { create_upright(spec) };
    if spec.slant == Slant::Upright {
        return upright;
    }
    // ask the family to lean; a family with no italic face answers
    // NULL and keeps its upright font — the lean degrades, never fails
    unsafe {
        let leaning = CTFontCreateCopyWithSymbolicTraits(
            upright,
            spec.size,
            std::ptr::null(),
            FONT_TRAIT_ITALIC,
            FONT_TRAIT_ITALIC,
        );
        if leaning.is_null() {
            upright
        } else {
            CFRelease(upright as *const c_void);
            leaning
        }
    }
}

/// `kCTFontBoldTrait` — bit 1 of the symbolic traits.
const FONT_TRAIT_BOLD: u32 = 2;

/// A named family carries its own weights, so the bold face is asked
/// for through the traits. A family without one keeps the regular —
/// the same way the lean degrades rather than fails.
unsafe fn emphasize(font: CTFontRef, spec: &FontSpec) -> CTFontRef {
    if matches!(spec.weight, Weight::Regular | Weight::Medium) {
        return font;
    }
    unsafe {
        let bold = CTFontCreateCopyWithSymbolicTraits(
            font,
            spec.size,
            std::ptr::null(),
            FONT_TRAIT_BOLD,
            FONT_TRAIT_BOLD,
        );
        if bold.is_null() {
            font
        } else {
            CFRelease(font as *const c_void);
            bold
        }
    }
}

unsafe fn create_upright(spec: &FontSpec) -> CTFontRef {
    unsafe {
        // a family the app NAMED is the most specific thing anyone
        // said about this text, so it comes before the design. A name
        // this Mac does not carry answers NULL and the scene keeps the
        // face it would have had anyway
        if let Some(name) = spec.family.name() {
            let cf_name = cf_string(&name);
            let font = CTFontCreateWithName(cf_name, spec.size, std::ptr::null());
            CFRelease(cf_name);
            // creating by name NEVER fails — a name this Mac does not
            // carry comes back as some default face instead. So the
            // font is asked what family it landed in, and only a font
            // that landed where it was sent is kept
            if !font.is_null() {
                let landed = CTFontCopyFamilyName(font);
                let honoured = cf_string_out(landed)
                    .is_some_and(|landed| landed.eq_ignore_ascii_case(&name));
                CFRelease(landed as *const c_void);
                if honoured {
                    return emphasize(font, spec);
                }
                CFRelease(font as *const c_void);
            }
        }
        match spec.design {
            FontDesign::Default => {
                // fine-grained weights via CTFontDescriptor come later —
                // emphasized covers Semibold/Bold with dignity
                let kind = match spec.weight {
                    Weight::Regular | Weight::Medium => UI_FONT_SYSTEM,
                    Weight::Semibold | Weight::Bold => UI_FONT_EMPHASIZED,
                };
                CTFontCreateUIFontForLanguage(kind, spec.size, std::ptr::null())
            }
            FontDesign::Mono => {
                let name = match spec.weight {
                    Weight::Regular | Weight::Medium => "Menlo-Regular",
                    Weight::Semibold | Weight::Bold => "Menlo-Bold",
                };
                let cf_name = cf_string(name);
                let font = CTFontCreateWithName(cf_name, spec.size, std::ptr::null());
                CFRelease(cf_name);
                if font.is_null() {
                    // an unknown family degrades, never fails
                    CTFontCreateUIFontForLanguage(UI_FONT_SYSTEM, spec.size, std::ptr::null())
                } else {
                    font
                }
            }
        }
    }
}

/// The text's CTLine with the font + context color (the color enters as
/// fill color at draw time — no CGColor created). The caller releases the
/// line.
unsafe fn make_line(text: &str, font: CTFontRef) -> CTLineRef {
    unsafe {
        let string = cf_string(text);
        let attributed = CFAttributedStringCreateMutable(std::ptr::null(), 0);
        CFAttributedStringReplaceString(
            attributed,
            CFRange { location: 0, length: 0 },
            string,
        );
        let range = CFRange { location: 0, length: CFStringGetLength(string) };
        CFAttributedStringSetAttribute(attributed, range, kCTFontAttributeName, font);
        CFAttributedStringSetAttribute(
            attributed,
            range,
            kCTForegroundColorFromContextAttributeName,
            kCFBooleanTrue,
        );
        let line = CTLineCreateWithAttributedString(attributed as *const c_void);
        CFRelease(attributed as *const c_void);
        CFRelease(string);
        line
    }
}

/// The Mac text engine. Single-thread, like the rest of the shell.
pub struct CoreTextEngine {
    fonts: RefCell<HashMap<FontKey, OwnedFont>>,
}

impl CoreTextEngine {
    pub fn new() -> Self {
        CoreTextEngine { fonts: RefCell::new(HashMap::new()) }
    }

    fn font(&self, spec: &FontSpec) -> CTFontRef {
        let key = spec.key();
        if let Some(font) = self.fonts.borrow().get(&key) {
            return font.0;
        }
        let created = unsafe { create_font(spec) };
        self.fonts.borrow_mut().insert(key, OwnedFont(created));
        created
    }
}

impl Default for CoreTextEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// One CFString out as UTF-8. The buffer grows once when the name is
/// longer than the room offered — family names are short, and the
/// second try is sized from the string itself.
unsafe fn cf_string_out(string: CFStringRef) -> Option<String> {
    unsafe {
        // UTF-16 units × 3 covers every UTF-8 expansion, + the NUL
        let room = (CFStringGetLength(string).max(0) as usize) * 3 + 1;
        let mut buffer = vec![0u8; room];
        if CFStringGetCString(
            string,
            buffer.as_mut_ptr(),
            room as isize,
            KCF_STRING_ENCODING_UTF8,
        ) == 0
        {
            return None;
        }
        let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(room);
        buffer.truncate(end);
        String::from_utf8(buffer).ok()
    }
}

impl TextEngine for CoreTextEngine {
    fn families(&self) -> Vec<std::sync::Arc<str>> {
        unsafe {
            let array = CTFontManagerCopyAvailableFontFamilyNames();
            if array.is_null() {
                return Vec::new();
            }
            let mut names = Vec::new();
            for index in 0..CFArrayGetCount(array) {
                let value = CFArrayGetValueAtIndex(array, index) as CFStringRef;
                // the dot-prefixed ones are the system's own internal
                // faces: a menu that lists them offers what nobody can
                // choose on purpose
                if let Some(name) = cf_string_out(value)
                    && !name.starts_with('.')
                {
                    names.push(std::sync::Arc::from(name.as_str()));
                }
            }
            CFRelease(array);
            names.sort();
            names
        }
    }

    fn measure_line(&self, text: &str, font: &FontSpec) -> LineMetrics {
        let ct_font = self.font(font);
        unsafe {
            let ascent = CTFontGetAscent(ct_font);
            // the leading folds into the descent — the LineMetrics contract
            let descent = CTFontGetDescent(ct_font) + CTFontGetLeading(ct_font);
            if text.is_empty() {
                // line height preserved without creating a CTLine
                return LineMetrics { width: 0.0, ascent, descent };
            }
            let line = make_line(text, ct_font);
            let width = CTLineGetTypographicBounds(
                line,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            CFRelease(line);
            LineMetrics { width, ascent, descent }
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
        let metrics = self.measure_line(text, font);
        let width = (metrics.width * scale as f64).ceil() as usize;
        let height = (metrics.height() * scale as f64).ceil() as usize;
        if width == 0 || height == 0 {
            return None;
        }

        let ct_font = self.font(font);
        let mut rgba = vec![0u8; width * height * 4];
        unsafe {
            let line = make_line(text, ct_font);
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
                CFRelease(line);
                return None;
            }
            // drawing in logical points, context in physical px (retina)
            CGContextScaleCTM(context, scale as f64, scale as f64);
            // CG counts y upward: the baseline sits `descent` from the
            // bottom of the line box (the ceil slack stays on top, sub-pixel)
            CGContextSetTextPosition(context, 0.0, metrics.descent);
            CGContextSetRGBFillColor(
                context,
                color.r as f64 / 255.0,
                color.g as f64 / 255.0,
                color.b as f64 / 255.0,
                color.a as f64 / 255.0,
            );
            CTLineDraw(line, context);
            CGContextRelease(context);
            CFRelease(line);
        }

        // unpremultiplies in place — the compositor blends straight alpha
        for pixel in rgba.chunks_exact_mut(4) {
            let alpha = pixel[3] as u32;
            if alpha > 0 && alpha < 255 {
                for channel in 0..3 {
                    pixel[channel] =
                        ((pixel[channel] as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
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
    fn core_text_measures_and_rasters() {
        let engine = CoreTextEngine::new();

        let metrics = engine.measure_line("Hello", &FontSpec::DEFAULT);
        assert!(metrics.width > 10.0, "real width: {}", metrics.width);
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
    fn the_roster_is_real_and_a_named_family_shapes_with_it() {
        let engine = CoreTextEngine::new();

        let families = engine.families();
        assert!(families.len() > 10, "a Mac carries more than ten: {}", families.len());
        assert!(families.windows(2).all(|pair| pair[0] <= pair[1]), "sorted");
        assert!(!families.iter().any(|name| name.starts_with('.')), "no internal faces");
        let menlo = families.iter().any(|name| &**name == "Menlo");
        assert!(menlo, "every Mac carries Menlo");

        // a named family really shapes: the same string at the same
        // size comes out a different width than the system's face
        let system = engine.measure_line("iiiii", &FontSpec::DEFAULT);
        let mono = engine.measure_line("iiiii", &FontSpec::DEFAULT.family("Menlo"));
        assert!(
            (system.width - mono.width).abs() > 1.0,
            "a grid font and the UI font do not agree on five i's: {} vs {}",
            system.width,
            mono.width
        );

        // and a name this Mac does not carry keeps the face it had —
        // the scene degrades, it never fails
        let missing = engine.measure_line("iiiii", &FontSpec::DEFAULT.family("Nothing At All"));
        assert_eq!(missing.width, system.width, "an unknown family keeps the system face");
    }

    #[test]
    fn core_text_wraps_words_with_real_measures() {
        use bunny_ui::text_engine::{MeasureCache, break_lines};

        let engine = CoreTextEngine::new();
        let cache = MeasureCache::default();
        let text = "hello world hello world";
        let lines = break_lines(text, &FontSpec::DEFAULT, 60.0, &engine, &cache);

        assert!(lines.len() > 1, "60px does not hold the sentence: {lines:?}");
        // contiguous coverage of the whole text, breaks on clean boundaries
        assert_eq!(lines.first().unwrap().0, 0);
        assert_eq!(lines.last().unwrap().1, text.len());
        for window in lines.windows(2) {
            assert_eq!(window[0].1, window[1].0);
        }
        for (start, end) in lines.iter() {
            assert!(text.is_char_boundary(*start) && text.is_char_boundary(*end));
        }
    }

    #[test]
    fn mono_design_resolves_to_a_wider_grid_or_degrades() {
        let engine = CoreTextEngine::new();
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
}
