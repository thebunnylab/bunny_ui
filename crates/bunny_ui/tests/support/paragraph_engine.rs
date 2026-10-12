//! A contextual test adapter makes loss of context visible in pixels and calls.
use bunny_ui::layout::Color;
use bunny_ui::text_engine::{FontSpec, LineMetrics, PixelFont, TextEngine, TextRaster};
use bunny_ui::text_source::{ParagraphLine, TextContent};
use std::cell::Cell;

#[derive(Default)]
pub struct ContextEngine {
    pub rasters: Cell<usize>,
    pub geometries: Cell<usize>,
}
impl TextEngine for ContextEngine {
    fn measure_line(&self, _: &str, _: &FontSpec) -> LineMetrics {
        panic!("renderer discarded paragraph measurement context");
    }
    fn raster_line(&self, _: &str, _: &FontSpec, _: Color, _: usize) -> Option<TextRaster> {
        panic!("renderer discarded paragraph raster context");
    }
    fn paragraph_line_geometry(
        &self,
        line: &ParagraphLine,
        _: &FontSpec,
    ) -> Option<bunny_ui::text_geometry::LineGeometry> {
        self.geometries.set(self.geometries.get() + 1);
        let cells = line
            .text()
            .char_indices()
            .enumerate()
            .map(|(i, (byte, c))| {
                bunny_ui::text_geometry::TextCell::new(
                    byte..byte + c.len_utf8(),
                    f64::from(u32::try_from(i).unwrap()) * 8.0,
                    f64::from(u32::try_from(i + 1).unwrap()) * 8.0,
                )
                .unwrap()
            })
            .collect();
        Some(bunny_ui::text_geometry::LineGeometry::new(line.text(), cells).unwrap())
    }
    fn measure_paragraph_line(&self, line: &ParagraphLine, font: &FontSpec) -> LineMetrics {
        PixelFont.measure_line(line.text(), font)
    }
    fn raster_paragraph_line(
        &self,
        line: &ParagraphLine,
        font: &FontSpec,
        _: Color,
        scale: usize,
    ) -> Option<TextRaster> {
        self.rasters.set(self.rasters.get() + 1);
        let color = if line.paragraph().text().starts_with('\u{202e}') {
            Color::rgb(200, 10, 20)
        } else {
            Color::rgb(10, 20, 200)
        };
        PixelFont.raster_line(line.text(), font, color, scale)
    }
}
pub fn contextual_line(prefix: &str) -> ParagraphLine {
    let content = TextContent::from(format!("{prefix}abc def\u{202c}"));
    let at = content.find("def").unwrap();
    content.line(at..at + 3).unwrap()
}
