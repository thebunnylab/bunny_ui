//! Bounded immutable paragraph analysis shared by all native text operations.
//!
//! ## Wiring
//! The cache owns `FriBidi` results only: font registration can keep them because
//! they depend on source text, not fonts. Every line is shaped with its original
//! paragraph, then the same glyph stream drives measurement, raster and cells.

use super::{
    Color, FACE_SIZE, FaceKey, FontSpec, FreeTypeEngine, HashMap, LineMetrics, ParagraphLine,
    Shaped, TextError, TextRaster, bidi, c_void, fixed_26_6, geometry, raster, read, shaping,
};
use bunny_ui::text_geometry::LineGeometry;
use bunny_ui::text_source::Paragraph;
use std::rc::Rc;

const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Cache {
    entries: HashMap<Paragraph, Rc<bidi::ResolvedText>>,
    retained: usize,
}
impl Cache {
    fn get(&mut self, paragraph: &Paragraph) -> Result<Rc<bidi::ResolvedText>, TextError> {
        if let Some(value) = self.entries.get(paragraph) {
            return Ok(Rc::clone(value));
        }
        let prepared =
            Rc::new(bidi::ResolvedText::new(paragraph.text()).map_err(shaping::bidi_error)?);
        // Include the source, its maximum lazy UTF-16 boundary table, native
        // Vec capacities and per-entry headers. Hash table slots are separately
        // bounded by MAX_ENTRIES. Oversized analyses live only for this query.
        let weight = paragraph
            .text()
            .len()
            .saturating_add(1)
            .saturating_mul(17)
            .saturating_add(prepared.retained_bytes())
            .saturating_add(256);
        if weight <= MAX_BYTES {
            if self.entries.len() >= MAX_ENTRIES || self.retained > MAX_BYTES - weight {
                self.entries.clear();
                self.retained = 0;
            }
            self.entries.insert(paragraph.clone(), Rc::clone(&prepared));
            self.retained += weight;
        }
        Ok(prepared)
    }
}

impl FreeTypeEngine {
    pub(super) fn paragraph_shape(
        &self,
        line: &ParagraphLine,
        key: &FaceKey,
        tracking: i64,
    ) -> Result<(Vec<Shaped>, i64), TextError> {
        let prepared = self.paragraphs.borrow_mut().get(line.paragraph())?;
        let runs = prepared
            .line_runs(line.bytes())
            .map_err(shaping::bidi_error)?;
        self.shape_runs(line.paragraph().text(), line.bytes(), key, tracking, &runs)
    }

    fn paragraph_font(font: &FontSpec, scale: usize) -> Result<(FaceKey, i64), TextError> {
        let scale = scale.max(1);
        let scale_u32 =
            u32::try_from(scale).map_err(|_| TextError::InvalidMetrics { engine: "FreeType" })?;
        let resolution = scale_u32.checked_mul(72);
        let tracking = font.tracking * f64::from(scale_u32);
        let physical_size = font.size * f64::from(scale_u32);
        // Geometry supports signed 32-bit pixels. All fixed-point values in
        // this range are exactly representable in f64 before conversion.
        let limit = f64::from(i32::MAX);
        if !physical_size.is_finite()
            || physical_size <= 0.0
            || physical_size > limit
            || !tracking.is_finite()
            || tracking.abs() > limit
            || resolution.is_none()
        {
            return Err(TextError::InvalidMetrics { engine: "FreeType" });
        }
        Ok((Self::key(font, scale), fixed_26_6(tracking)))
    }

    fn paragraph_box(&self, key: &FaceKey) -> Result<(f64, f64), TextError> {
        let primary = self
            .face(key)
            .ok_or_else(|| shaping::preparation("primary face"))?;
        // SAFETY: a cached face owns a sized native FreeType face. Check its
        // metrics pointer before the legacy helper, which cannot return errors.
        if unsafe { read::<*mut c_void>(primary.ft, FACE_SIZE) }.is_null() {
            return Err(shaping::preparation("FT_Size metrics"));
        }
        let (ascent, descent) = self.line_box(primary);
        if !ascent.is_finite() || !descent.is_finite() || ascent < 0.0 || descent < 0.0 {
            return Err(TextError::InvalidMetrics { engine: "FreeType" });
        }
        Ok((ascent, descent))
    }

    pub(super) fn paragraph_metrics(
        &self,
        line: &ParagraphLine,
        font: &FontSpec,
    ) -> Result<LineMetrics, TextError> {
        let (key, tracking) = Self::paragraph_font(font, 1)?;
        let (ascent, descent) = self.paragraph_box(&key)?;
        let (_, advance) = self.paragraph_shape(line, &key, tracking)?;
        Ok(LineMetrics {
            width: geometry::pixels(advance)
                .ok_or(TextError::InvalidMetrics { engine: "HarfBuzz" })?
                .max(0.0),
            ascent,
            descent,
        })
    }

    pub(super) fn paragraph_geometry(
        &self,
        line: &ParagraphLine,
        font: &FontSpec,
    ) -> Result<LineGeometry, TextError> {
        if line.text().is_empty() {
            return LineGeometry::new(line.text(), Vec::new()).map_err(TextError::from);
        }
        let (key, tracking) = Self::paragraph_font(font, 1)?;
        let (glyphs, _) = self.paragraph_shape(line, &key, tracking)?;
        geometry::from_shaped(line.text(), &glyphs)
    }

    pub(super) fn paragraph_raster(
        &self,
        line: &ParagraphLine,
        font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Result<Option<TextRaster>, TextError> {
        if line.text().is_empty() {
            return Ok(None);
        }
        let (key, tracking) = Self::paragraph_font(font, scale)?;
        let (ascent, descent) = self.paragraph_box(&key)?;
        let (glyphs, advance) = self.paragraph_shape(line, &key, tracking)?;
        raster::render(&glyphs, advance, ascent, descent, color)
    }
}

#[cfg(test)]
#[path = "text_paragraph_context_tests.rs"]
mod tests;
