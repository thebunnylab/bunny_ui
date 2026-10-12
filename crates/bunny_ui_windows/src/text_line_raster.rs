//! Publish straight RGBA only after the complete contextual glyph line draws.

use super::{VisualLine, glyphs};
use crate::text::{Color, D2dPoint, LineMetrics, TextRaster};
use glyphs::raster::{Renderer, Surface};
use glyphs::{Factories, TextError, native_float};

const fn failure(operation: &'static str) -> TextError {
    TextError::Rasterization {
        engine: "DirectWrite",
        operation,
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Finite, nonnegative, integral pixel dimensions are checked against u32::MAX before the native conversion."
)]
fn pixels(value: f64) -> Result<u32, TextError> {
    if value.is_finite() && value >= 0.0 && value <= f64::from(u32::MAX) {
        Ok(value as u32)
    } else {
        Err(failure("pixel dimensions"))
    }
}

impl VisualLine {
    pub(in crate::text::analysis) fn raster(
        &self,
        factories: &Factories,
        metrics: LineMetrics,
        color: Color,
        scale: usize,
    ) -> Result<Option<TextRaster>, TextError> {
        let scale = u32::try_from(scale).map_err(|_| failure("pixel scale"))?;
        if scale == 0 {
            return Err(failure("pixel scale"));
        }
        let logical_scale = f64::from(scale);
        let width = pixels((metrics.width * logical_scale).ceil())?;
        let height = pixels((metrics.height() * logical_scale).ceil())?;
        if width == 0 || height == 0 {
            return Ok(None);
        }
        // Keep ceil slack above the font box, as the existing Windows raster
        // does. The reported baseline now names the actual snapped draw origin.
        let slack = (f64::from(height) / logical_scale - metrics.height()).max(0.0);
        let baseline = pixels(((metrics.ascent + slack) * logical_scale).round())?;
        let surface = Surface::new(factories, width, height, native_float(logical_scale)?)?;
        let mut renderer = Renderer::new(&surface, factories, color)?;
        let mut rgba = surface.paint(|_| {
            for span in self.spans() {
                let run = &self.runs[span.run];
                let origin = if run.level.is_multiple_of(2) {
                    span.left
                } else {
                    span.left + span.width
                };
                renderer.draw_range(
                    run,
                    span.glyphs,
                    D2dPoint {
                        x: native_float(origin)?,
                        y: native_float(f64::from(baseline) / logical_scale)?,
                    },
                )?;
            }
            Ok(())
        })?;
        for pixel in rgba.as_chunks_mut::<4>().0 {
            if pixel[3] != 0 && pixel[3] != 255 {
                for channel in 0..3 {
                    let value = (u32::from(pixel[channel]) * 255 + u32::from(pixel[3]) / 2)
                        / u32::from(pixel[3]);
                    pixel[channel] = u8::try_from(value).unwrap_or(u8::MAX);
                }
            }
            pixel.swap(0, 2);
        }
        Ok(Some(TextRaster {
            width: usize::try_from(width).map_err(|_| failure("pixel width"))?,
            height: usize::try_from(height).map_err(|_| failure("pixel height"))?,
            baseline: usize::try_from(baseline).map_err(|_| failure("pixel baseline"))?,
            rgba,
        }))
    }
}
