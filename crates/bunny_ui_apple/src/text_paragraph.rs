//! Typesetter ownership keeps paragraph resolution separate from line extraction.
//!
//! A failed preparation is an error, never an isolated `CTLine` fallback. Empty
//! ranges bypass `CTTypesetterCreateLine`: its zero length means the remaining
//! paragraph, not an empty insertion point. Cache weight includes retained text
//! and native preparation estimates; oversized paragraphs bypass retention.
use super::{
    ALPHA_PREMULTIPLIED_LAST, CF_NUMBER_DOUBLE, CF_NUMBER_SINT32, CFAttributedStringCreateMutable,
    CFAttributedStringReplaceString, CFAttributedStringSetAttribute, CFNumberCreate, CFRange,
    CFRelease, CFStringGetLength, CGBitmapContextCreate, CGColorSpaceCreateDeviceRGB,
    CGColorSpaceRelease, CGContextRelease, CGContextScaleCTM, CGContextSetRGBFillColor,
    CGContextSetTextPosition, CTFontGetAscent, CTFontGetDescent, CTFontGetLeading, CTFontRef,
    CTLineDraw, CTLineGetTypographicBounds, CTLineRef, CoreTextEngine, cf_string, geometry,
    kCFBooleanTrue, kCTFontAttributeName, kCTForegroundColorFromContextAttributeName,
    kCTKernAttributeName, kCTLigatureAttributeName,
};
use bunny_ui::layout::Color;
use bunny_ui::text_engine::{FontDesign, FontKey, FontSpec, LineMetrics, TextRaster};
use bunny_ui::text_error::TextError;
use bunny_ui::text_geometry::LineGeometry;
use bunny_ui::text_source::{Paragraph, ParagraphLine};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

const ENGINE: &str = "CoreText";
const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTTypesetterCreateWithAttributedString(string: *const c_void) -> *const c_void;
    fn CTTypesetterCreateLine(typesetter: *const c_void, range: CFRange) -> CTLineRef;
}

struct Retained(NonNull<c_void>);
impl Retained {
    fn checked(value: *const c_void, operation: &'static str) -> Result<Self, TextError> {
        NonNull::new(value.cast_mut())
            .map(Self)
            .ok_or(TextError::Preparation {
                engine: ENGINE,
                operation,
            })
    }
    const fn raw(&self) -> *const c_void {
        self.0.as_ptr()
    }
}
impl Drop for Retained {
    fn drop(&mut self) {
        // SAFETY: checked construction owns one non-null CF Create reference.
        unsafe { CFRelease(self.raw()) };
    }
}

struct Entry {
    value: Rc<Retained>,
    used: u64,
    weight: usize,
}
#[derive(Default)]
pub(super) struct ParagraphCache {
    entries: HashMap<(FontKey, Paragraph), Entry>,
    clock: u64,
    weight: usize,
}
impl ParagraphCache {
    fn get(&mut self, key: &(FontKey, Paragraph)) -> Option<Rc<Retained>> {
        self.clock = self.clock.wrapping_add(1);
        self.entries.get_mut(key).map(|entry| {
            entry.used = self.clock;
            Rc::clone(&entry.value)
        })
    }
    fn insert(&mut self, key: (FontKey, Paragraph), value: Rc<Retained>) {
        let weight = key.1.text().len().saturating_mul(32).saturating_add(1024);
        if weight > MAX_BYTES {
            return;
        }
        while self.entries.len() >= MAX_ENTRIES || self.weight.saturating_add(weight) > MAX_BYTES {
            let Some(cold) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&cold) {
                self.weight -= entry.weight;
            }
        }
        if let Some(old) = self.entries.insert(
            key,
            Entry {
                value,
                used: self.clock,
                weight,
            },
        ) {
            self.weight -= old.weight;
        }
        self.weight += weight;
    }
}

fn create_paragraph(
    text: &str,
    font: CTFontRef,
    spec: &FontSpec,
    create: unsafe extern "C" fn(*const c_void) -> *const c_void,
) -> Result<Retained, TextError> {
    let _ = isize::try_from(text.len()).map_err(|_| TextError::IndexOverflow { engine: ENGINE })?;
    // SAFETY: all CF Create results are checked before use and retained through
    // the call. Attribute ranges use CFString's original UTF-16 coordinates.
    unsafe {
        let string = Retained::checked(cf_string(text), "CFStringCreateWithBytes")?;
        let attributed = Retained::checked(
            CFAttributedStringCreateMutable(std::ptr::null(), 0),
            "CFAttributedStringCreateMutable",
        )?;
        CFAttributedStringReplaceString(
            attributed.raw().cast_mut(),
            CFRange {
                location: 0,
                length: 0,
            },
            string.raw(),
        );
        let range = CFRange {
            location: 0,
            length: CFStringGetLength(string.raw()),
        };
        CFAttributedStringSetAttribute(
            attributed.raw().cast_mut(),
            range,
            kCTFontAttributeName,
            font,
        );
        CFAttributedStringSetAttribute(
            attributed.raw().cast_mut(),
            range,
            kCTForegroundColorFromContextAttributeName,
            kCFBooleanTrue,
        );
        if spec.tracking != 0.0 {
            let tracking = Retained::checked(
                CFNumberCreate(
                    std::ptr::null(),
                    CF_NUMBER_DOUBLE,
                    std::ptr::from_ref(&spec.tracking).cast(),
                ),
                "CFNumberCreate(tracking)",
            )?;
            CFAttributedStringSetAttribute(
                attributed.raw().cast_mut(),
                range,
                kCTKernAttributeName,
                tracking.raw(),
            );
        }
        if matches!(spec.design, FontDesign::Mono) {
            let required: i32 = 0;
            let ligatures = Retained::checked(
                CFNumberCreate(
                    std::ptr::null(),
                    CF_NUMBER_SINT32,
                    std::ptr::from_ref(&required).cast(),
                ),
                "CFNumberCreate(ligatures)",
            )?;
            CFAttributedStringSetAttribute(
                attributed.raw().cast_mut(),
                range,
                kCTLigatureAttributeName,
                ligatures.raw(),
            );
        }
        Retained::checked(
            create(attributed.raw()),
            "CTTypesetterCreateWithAttributedString",
        )
    }
}

impl CoreTextEngine {
    fn contextual_font(&self, spec: &FontSpec) -> Result<CTFontRef, TextError> {
        let font = self.font(spec);
        if font.is_null() {
            Err(TextError::Preparation {
                engine: ENGINE,
                operation: "CTFontCreate",
            })
        } else {
            Ok(font)
        }
    }
    fn prepare_paragraph(
        &self,
        paragraph: &Paragraph,
        spec: &FontSpec,
    ) -> Result<Rc<Retained>, TextError> {
        let key = (spec.key(), paragraph.clone());
        if let Some(value) = self.paragraphs.borrow_mut().get(&key) {
            return Ok(value);
        }
        let font = self.contextual_font(spec)?;
        let value = Rc::new(create_paragraph(
            paragraph.text(),
            font,
            spec,
            CTTypesetterCreateWithAttributedString,
        )?);
        self.paragraphs.borrow_mut().insert(key, Rc::clone(&value));
        Ok(value)
    }
    fn contextual_line(
        &self,
        line: &ParagraphLine,
        font: &FontSpec,
    ) -> Result<Option<Retained>, TextError> {
        if line.text().is_empty() {
            return Ok(None);
        }
        let prepared = self.prepare_paragraph(line.paragraph(), font)?;
        let units = line.utf16();
        let location = isize::try_from(units.start)
            .map_err(|_| TextError::IndexOverflow { engine: ENGINE })?;
        let length = isize::try_from(units.end - units.start)
            .map_err(|_| TextError::IndexOverflow { engine: ENGINE })?;
        // SAFETY: the range is validated, nonempty, and uses original UTF-16
        // indices. No forced embedding level or unbounded-layout option is set.
        let native =
            unsafe { CTTypesetterCreateLine(prepared.raw(), CFRange { location, length }) };
        Retained::checked(native, "CTTypesetterCreateLine").map(Some)
    }
    fn contextual_metrics(
        &self,
        line: Option<&Retained>,
        spec: &FontSpec,
    ) -> Result<LineMetrics, TextError> {
        let font = self.contextual_font(spec)?;
        // SAFETY: font and optional native line are retained and non-null.
        let metrics = unsafe {
            LineMetrics {
                width: line.map_or(0.0, |line| {
                    CTLineGetTypographicBounds(
                        line.raw(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                }),
                ascent: CTFontGetAscent(font),
                descent: CTFontGetDescent(font) + CTFontGetLeading(font),
            }
        };
        if [metrics.width, metrics.ascent, metrics.descent]
            .into_iter()
            .all(f64::is_finite)
        {
            Ok(metrics)
        } else {
            Err(TextError::InvalidMetrics { engine: ENGINE })
        }
    }
    /// Measure a logical line extracted from its complete native paragraph.
    ///
    /// # Errors
    /// Reports native index limits, refused preparation or invalid metrics.
    pub fn try_measure_paragraph_line(
        &self,
        line: &ParagraphLine,
        font: &FontSpec,
    ) -> Result<LineMetrics, TextError> {
        let native = self.contextual_line(line, font)?;
        self.contextual_metrics(native.as_ref(), font)
    }
    /// Return cells local to the visible UTF-8 line, retaining native paragraph context.
    ///
    /// # Errors
    /// Reports preparation or geometry failures instead of prefix approximation.
    pub fn try_paragraph_line_geometry(
        &self,
        line: &ParagraphLine,
        font: &FontSpec,
    ) -> Result<Option<LineGeometry>, TextError> {
        let Some(native) = self.contextual_line(line, font)? else {
            return LineGeometry::new("", Vec::new())
                .map(Some)
                .map_err(Into::into);
        };
        geometry::from_line(native.raw(), line.text(), line.utf16().start).map(Some)
    }
    /// Raster the same paragraph range that supplies measurement and caret cells.
    ///
    /// # Errors
    /// Reports native preparation, bitmap size and allocation failures.
    pub fn try_raster_paragraph_line(
        &self,
        line: &ParagraphLine,
        font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Result<Option<TextRaster>, TextError> {
        let Some(native) = self.contextual_line(line, font)? else {
            return Ok(None);
        };
        let metrics = self.contextual_metrics(Some(&native), font)?;
        raster_line(&native, metrics, color, scale)
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "The finite, nonnegative, integral value is checked against u32::MAX before conversion to native pixel dimensions."
)]
fn pixels(value: f64) -> Result<usize, TextError> {
    if value.is_finite() && value >= 0.0 && value <= f64::from(u32::MAX) {
        usize::try_from(value as u32).map_err(|_| raster_error("pixel dimensions"))
    } else {
        Err(raster_error("pixel dimensions"))
    }
}
const fn raster_error(operation: &'static str) -> TextError {
    TextError::Rasterization {
        engine: ENGINE,
        operation,
    }
}
fn raster_line(
    line: &Retained,
    metrics: LineMetrics,
    color: Color,
    scale: usize,
) -> Result<Option<TextRaster>, TextError> {
    let scale = f64::from(u32::try_from(scale).map_err(|_| raster_error("pixel scale"))?);
    let width = pixels((metrics.width * scale).ceil())?;
    let height = pixels((metrics.height() * scale).ceil())?;
    if width == 0 || height == 0 {
        return Ok(None);
    }
    let row = width
        .checked_mul(4)
        .ok_or_else(|| raster_error("bitmap byte count"))?;
    let bytes = row
        .checked_mul(height)
        .ok_or_else(|| raster_error("bitmap byte count"))?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(bytes)
        .map_err(|_| raster_error("bitmap allocation"))?;
    rgba.resize(bytes, 0);
    // SAFETY: checked dimensions and backing storage cover the complete bitmap.
    // CoreGraphics resources are released on all paths after their creation.
    unsafe {
        let space = CGColorSpaceCreateDeviceRGB();
        if space.is_null() {
            return Err(raster_error("CGColorSpaceCreateDeviceRGB"));
        }
        let context = CGBitmapContextCreate(
            rgba.as_mut_ptr().cast(),
            width,
            height,
            8,
            row,
            space,
            ALPHA_PREMULTIPLIED_LAST,
        );
        CGColorSpaceRelease(space);
        if context.is_null() {
            return Err(raster_error("CGBitmapContextCreate"));
        }
        CGContextScaleCTM(context, scale, scale);
        CGContextSetTextPosition(context, 0.0, metrics.descent);
        CGContextSetRGBFillColor(
            context,
            f64::from(color.r) / 255.0,
            f64::from(color.g) / 255.0,
            f64::from(color.b) / 255.0,
            f64::from(color.a) / 255.0,
        );
        CTLineDraw(line.raw(), context);
        CGContextRelease(context);
    }
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = u8::try_from(((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255))
                    .map_err(|_| raster_error("alpha conversion"))?;
            }
        }
    }
    Ok(Some(TextRaster {
        width,
        height,
        baseline: pixels((metrics.ascent * scale).round())?,
        rgba,
    }))
}

#[cfg(test)]
#[path = "text_paragraph_preparation_tests.rs"]
mod tests;
