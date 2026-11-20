//! The figures of a `.monospaced_digit()` face, laid by the layout so
//! that every engine agrees.
//!
//! A face whose ten figures already share one advance is left to its
//! engine: most interface faces draw tabular figures by default (Lato,
//! Helvetica, Avenir), and a page's `tabular-nums` changes nothing in
//! them. Where the figures differ, each one takes a cell as wide as the
//! widest, in its middle, and the words between are shaped as the engine
//! shapes them; the line is composed here out of the engine's pieces.
//! The engine border has no way to ask a face for its own `tnum` glyphs,
//! and the browser's canvas has no switch for them, so the widest figure
//! stands for the cell they keep: on the Mac's system face it is half a
//! percent wider.

use crate::layout::{Color, Px};
use crate::text_engine::{FontSpec, LineMetrics, TextEngine, TextRaster};

const FIGURES: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// Figures closer than this in advance already line up.
const EVEN: Px = 1e-3;

/// What the layout measures: the engine's line, or the figures' cells
/// and the words between them.
pub fn measure_line<E: TextEngine + ?Sized>(engine: &E, text: &str, font: &FontSpec) -> LineMetrics {
    laid(engine, text, font).map_or_else(|| engine.measure_line(text, font), |line| line.metrics)
}

/// What the compositor draws: the engine's raster of the line, or the
/// pieces' rasters at their places in one.
pub fn raster_line<E: TextEngine + ?Sized>(
    engine: &E,
    text: &str,
    font: &FontSpec,
    color: Color,
    scale: usize,
) -> Option<TextRaster> {
    let Some(line) = laid(engine, text, font) else {
        return engine.raster_line(text, font, color, scale);
    };
    let plain = FontSpec { monospaced_digits: false, ..*font };
    #[allow(clippy::cast_precision_loss, reason = "a retina factor")]
    let factor = scale as Px;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a pen on the line, never left of it")]
    let pieces: Vec<(usize, TextRaster)> = line
        .pieces
        .iter()
        .filter_map(|&(pen, piece)| Some(((pen * factor).round().max(0.0) as usize, engine.raster_line(piece, &plain, color, scale)?)))
        .collect();
    let baseline = pieces.iter().map(|(_, raster)| raster.baseline).max()?;
    let below = pieces.iter().map(|(_, raster)| raster.height.saturating_sub(raster.baseline)).max()?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a width")]
    let measured = (line.metrics.width * factor).ceil().max(0.0) as usize;
    let width = pieces.iter().map(|(x, raster)| x + raster.width).max()?.max(measured);
    let height = baseline + below;
    let mut rgba = vec![0u8; width * height * 4];
    for (x, piece) in &pieces {
        let top = baseline - piece.baseline;
        for row in 0..piece.height {
            for column in 0..piece.width {
                let from = (row * piece.width + column) * 4;
                let to = ((top + row) * width + x + column) * 4;
                over(&mut rgba[to..to + 4], &piece.rgba[from..from + 4]);
            }
        }
    }
    Some(TextRaster { width, height, baseline, ascent: line.metrics.ascent, rgba })
}

/// Whether the figures of this line are laid by cells: the engine's own
/// geometry of the line, shaped whole, is not where they stand.
pub fn laid_by_cells<E: TextEngine + ?Sized>(engine: &E, text: &str, font: &FontSpec) -> bool {
    laid(engine, text, font).is_some()
}

/// A line whose figures take cells: its pieces at their pens, and what
/// it measures.
struct Laid<'t> {
    pieces: Vec<(Px, &'t str)>,
    metrics: LineMetrics,
}

fn laid<'t, E: TextEngine + ?Sized>(engine: &E, text: &'t str, font: &FontSpec) -> Option<Laid<'t>> {
    if !font.monospaced_digits || !text.bytes().any(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let plain = FontSpec { monospaced_digits: false, ..*font };
    let cell = cell(engine, &plain)?;
    let mut pieces = Vec::new();
    let mut metrics = LineMetrics { width: 0.0, ascent: 0.0, descent: 0.0 };
    let mut rest = text;
    while let Some(first) = rest.bytes().next() {
        let figure = first.is_ascii_digit();
        let end = if figure { 1 } else { rest.find(|glyph: char| glyph.is_ascii_digit()).unwrap_or(rest.len()) };
        let (piece, tail) = rest.split_at(end);
        rest = tail;
        let measured = engine.measure_line(piece, &plain);
        let (pen, advance) = if figure { ((cell - measured.width) / 2.0, cell) } else { (0.0, measured.width) };
        pieces.push((metrics.width + pen, piece));
        metrics.width += advance;
        metrics.ascent = metrics.ascent.max(measured.ascent);
        metrics.descent = metrics.descent.max(measured.descent);
    }
    Some(Laid { pieces, metrics })
}

/// The widest figure's advance, when the ten are not one already.
fn cell<E: TextEngine + ?Sized>(engine: &E, font: &FontSpec) -> Option<Px> {
    let (narrowest, widest) = FIGURES
        .iter()
        .map(|figure| engine.measure_line(figure, font).width)
        .fold((Px::INFINITY, 0.0_f64), |(narrowest, widest), width| (narrowest.min(width), widest.max(width)));
    (widest - narrowest > EVEN).then_some(widest)
}

/// Straight-alpha source-over of one pixel: the pieces touch at their
/// edges, where a glyph's fringe may reach into its neighbour's.
fn over(into: &mut [u8], from: &[u8]) {
    let (alpha, under) = (u32::from(from[3]), u32::from(into[3]));
    if alpha == 0 {
        return;
    }
    if alpha == 255 || under == 0 {
        into.copy_from_slice(from);
        return;
    }
    let left = under * (255 - alpha);
    let total = alpha * 255 + left;
    for channel in 0..3 {
        let mixed = (u32::from(from[channel]) * alpha * 255 + u32::from(into[channel]) * left + total / 2) / total;
        into[channel] = u8::try_from(mixed).unwrap_or(u8::MAX);
    }
    into[3] = u8::try_from((total + 127) / 255).unwrap_or(u8::MAX);
}
