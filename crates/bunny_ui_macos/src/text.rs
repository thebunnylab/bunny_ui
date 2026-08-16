//! CoreText pela FFI da casa — o engine de texto do Mac.
//!
//! Implementa a borda [`TextEngine`] do bunny-ui: medição pelas métricas
//! da FONTE (estáveis — as da linha pulam quando um fallback de glifo
//! entra, e a altura de linha não pode pular por string) e raster de uma
//! linha num `CGBitmapContext` sobre buffer nosso.
//!
//! O contexto de CG só desenha pré-multiplicado; o compositor do bunny-ui
//! blenda alfa RETO (um caminho único para todos os engines — na web o
//! `putImageData` também é reto), então o retângulo é des-premultiplicado
//! in place antes de sair — uma passada num retângulo pequeno de texto.
//!
//! Fontes: o design `Default` sai de `CTFontCreateUIFontForLanguage` (a
//! fonte de interface do sistema); `Mono` tenta Menlo e, se a família não
//! existir, DEGRADA para a fonte do sistema — nunca falha. Cada `CTFont`
//! é criado uma vez por `FontKey` e retido no engine.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use bunny_ui::layout::Color;
use bunny_ui::text_engine::{
    FontDesign, FontKey, FontSpec, LineMetrics, TextEngine, TextRaster, Weight,
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
/// `kCGImageAlphaPremultipliedLast` — o único layout RGBA que um contexto
/// de desenho aceita.
const ALPHA_PREMULTIPLIED_LAST: u32 = 1;
/// `kCTFontUIFontSystem` / `kCTFontUIFontEmphasizedSystem`.
const UI_FONT_SYSTEM: u32 = 2;
const UI_FONT_EMPHASIZED: u32 = 3;

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontCreateWithName(name: CFStringRef, size: f64, matrix: *const c_void) -> CTFontRef;
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
    /// Unidades UTF-16 — é a contagem que o `CFRange` dos atributos usa.
    fn CFStringGetLength(string: CFStringRef) -> isize;
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

/// Um CTFont retido — solta no Drop (o engine é o dono).
struct OwnedFont(*const c_void);

impl Drop for OwnedFont {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

unsafe fn create_font(spec: &FontSpec) -> CTFontRef {
    unsafe {
        match spec.design {
            FontDesign::Default => {
                // pesos finos via CTFontDescriptor ficam para depois — o
                // emphasized cobre Semibold/Bold com dignidade
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
                    // família desconhecida degrada, nunca falha
                    CTFontCreateUIFontForLanguage(UI_FONT_SYSTEM, spec.size, std::ptr::null())
                } else {
                    font
                }
            }
        }
    }
}

/// A CTLine do texto com a fonte + cor-do-contexto (a cor entra pelo fill
/// color na hora do draw — sem criar CGColor). Quem chama solta a line.
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

/// O engine de texto do Mac. Single-thread, como o resto do shell.
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

impl TextEngine for CoreTextEngine {
    fn measure_line(&self, text: &str, font: &FontSpec) -> LineMetrics {
        let ct_font = self.font(font);
        unsafe {
            let ascent = CTFontGetAscent(ct_font);
            // o leading dobra no descent — o contrato do LineMetrics
            let descent = CTFontGetDescent(ct_font) + CTFontGetLeading(ct_font);
            if text.is_empty() {
                // altura de linha preservada sem criar CTLine
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
            // desenho em pontos lógicos, contexto em px físicos (retina)
            CGContextScaleCTM(context, scale as f64, scale as f64);
            // CG conta y para cima: o baseline fica a `descent` do fundo
            // da caixa de linha (a folga do ceil sobra no topo, sub-pixel)
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

        // des-premultiplica in place — o compositor blenda alfa reto
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
        assert!(metrics.width > 10.0, "largura real: {}", metrics.width);
        assert!(metrics.ascent > 5.0);
        assert!(metrics.height() > metrics.ascent);

        let empty = engine.measure_line("", &FontSpec::DEFAULT);
        assert_eq!(empty.width, 0.0);
        assert_eq!(empty.height(), metrics.height(), "linha vazia preserva a altura");

        let raster = engine
            .raster_line("Hello", &FontSpec::DEFAULT, Color::rgb(10, 20, 30), 2)
            .expect("tem tinta");
        assert!(raster.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0));
        assert!(raster.baseline > 0 && raster.baseline <= raster.height);
    }

    #[test]
    fn mono_design_resolves_to_a_wider_grid_or_degrades() {
        let engine = CoreTextEngine::new();
        let mono = FontSpec { design: FontDesign::Mono, ..FontSpec::DEFAULT };

        // "iiii" em mono tem a MESMA largura de "mmmm" — a prova da grade
        let narrow = engine.measure_line("iiii", &mono);
        let wide = engine.measure_line("mmmm", &mono);
        assert!(
            (narrow.width - wide.width).abs() < 0.01,
            "grade mono: {} vs {}",
            narrow.width,
            wide.width
        );
    }
}
