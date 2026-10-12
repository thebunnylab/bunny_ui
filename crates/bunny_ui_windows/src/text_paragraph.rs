//! Runtime paragraph operations share immutable native analysis and logical glyphs.
//!
//! ## Wiring
//! Cache only font-independent source/script/bidi data. A font registration
//! clears the existing font slots; subsequent shaping uses that fresh collection.
//! Oversized paragraphs are prepared for the query but never retained.

use super::{Factories, Prepared, TextError, glyphs, preparation};
use crate::text::{Color, DirectWriteEngine, FontSlot, FontSpec, LineMetrics, TextRaster};
use bunny_ui::text_geometry::LineGeometry;
use bunny_ui::text_source::{Paragraph, ParagraphLine};
use std::collections::HashMap;
use std::rc::Rc;

const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(in crate::text) struct Cache {
    entries: HashMap<Paragraph, Rc<Prepared>>,
    retained: usize,
    shaper: Option<glyphs::Shaper>,
}
impl Cache {
    fn get(
        &mut self,
        factories: &Factories,
        paragraph: &Paragraph,
    ) -> Result<Rc<Prepared>, TextError> {
        if let Some(prepared) = self.entries.get(paragraph) {
            return Ok(Rc::clone(prepared));
        }
        let prepared = Rc::new(Prepared::new(factories, paragraph.text())?);
        // Source UTF-8, the largest lazy byte/UTF-16 index table, owned native
        // analysis arrays and fixed headers. Hash slots are bounded separately.
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

    fn line(
        &mut self,
        factories: &Factories,
        slot: &FontSlot,
        source: &ParagraphLine,
        font: &FontSpec,
    ) -> Result<glyphs::VisualLine, TextError> {
        let prepared = self.get(factories, source.paragraph())?;
        if self.shaper.is_none() {
            self.shaper = Some(glyphs::Shaper::new(factories)?);
        }
        let shaper = self
            .shaper
            .as_ref()
            .ok_or_else(|| preparation("paragraph shaper"))?;
        let font_face = glyphs::BaseFont::new(slot, font)?;
        let range = source.utf16();
        let runs = shaper.shape(&prepared, range.clone(), &font_face, font)?;
        glyphs::VisualLine::new(&prepared, range, runs)
    }
}

impl Prepared {
    fn retained_bytes(&self) -> usize {
        // SAFETY: this owner retains the immutable native source and locale.
        let source = unsafe { &*self.source.as_ptr() };
        self.scripts
            .capacity()
            .saturating_mul(std::mem::size_of::<super::Script>())
            .saturating_add(
                self.levels
                    .capacity()
                    .saturating_mul(std::mem::size_of::<super::Levels>()),
            )
            .saturating_add(
                self.classes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<super::bidi::Class>()),
            )
            .saturating_add(source.text.len().saturating_mul(2))
            .saturating_add(source.locale.len().saturating_mul(2))
            .saturating_add(std::mem::size_of::<Self>())
    }
}

impl DirectWriteEngine {
    pub(in crate::text) fn contextual_metrics(
        &self,
        source: &ParagraphLine,
        font: &FontSpec,
    ) -> Result<LineMetrics, TextError> {
        self.with_paragraph(source, font, |_, _, metrics| Ok(metrics))
    }
    pub(in crate::text) fn contextual_geometry(
        &self,
        source: &ParagraphLine,
        font: &FontSpec,
    ) -> Result<LineGeometry, TextError> {
        self.with_paragraph(source, font, |_, line, _| line.geometry(source.text()))
    }
    pub(in crate::text) fn contextual_raster(
        &self,
        source: &ParagraphLine,
        font: &FontSpec,
        color: Color,
        scale: usize,
    ) -> Result<Option<TextRaster>, TextError> {
        self.with_paragraph(source, font, |factories, line, metrics| {
            line.raster(factories, metrics, color, scale)
        })
    }

    fn with_paragraph<T>(
        &self,
        source: &ParagraphLine,
        font: &FontSpec,
        read: impl FnOnce(&Factories, &glyphs::VisualLine, LineMetrics) -> Result<T, TextError>,
    ) -> Result<T, TextError> {
        let size = glyphs::native_float(font.size)?;
        glyphs::native_float(font.tracking)?;
        if size <= 0.0 {
            return Err(TextError::InvalidMetrics {
                engine: super::ENGINE,
            });
        }
        let factories = self
            .factories
            .as_ref()
            .ok_or_else(|| preparation("native factories"))?;
        self.with_slot(font, |slot| {
            if !slot.ascent.is_finite()
                || !slot.descent.is_finite()
                || slot.ascent < 0.0
                || slot.descent < 0.0
            {
                return Err(TextError::InvalidMetrics {
                    engine: super::ENGINE,
                });
            }
            let line = self
                .paragraphs
                .borrow_mut()
                .line(factories, slot, source, font)?;
            let metrics = LineMetrics {
                width: line.width,
                ascent: slot.ascent,
                descent: slot.descent,
            };
            read(factories, &line, metrics)
        })
        .ok_or_else(|| preparation("paragraph font slot"))?
    }
}

#[cfg(test)]
#[path = "text_paragraph_context_tests.rs"]
mod tests;
