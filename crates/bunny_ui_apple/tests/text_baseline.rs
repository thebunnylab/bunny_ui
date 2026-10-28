//! A line's glyphs stand on the baseline its layout reports: the line box's
//! top plus the engine's own ascent, on the nearest pixel row.
#![cfg(target_os = "macos")]

use bunny_ui::image_engine::RawImages;
use bunny_ui::layout::{Color, DisplayList, DrawCommand, Point};
use bunny_ui::raster::{Bitmap, rasterize_with};
use bunny_ui::text_engine::{FontSpec, TextEngine};
use bunny_ui_apple::CoreTextEngine;

/// Flat-footed stems: nothing overshoots the baseline, so where their ink
/// ends IS where the glyphs stand.
const STEMS: &str = "IIII";

/// Where the stems' ink ends, in physical rows: the last inked row, plus how
/// much of it they cover.
fn ink_floor(bitmap: &Bitmap) -> f64 {
    let rows: Vec<f64> = (0..bitmap.height())
        .map(|y| {
            (0..bitmap.width())
                .map(|x| {
                    let red = bitmap.pixel(x, y).map_or(255, |pixel| pixel >> 24 & 0xff);
                    f64::from(255 - red) / 255.0
                })
                .sum()
        })
        .collect();
    let stem = rows.iter().copied().fold(0.0, f64::max);
    let last = rows
        .iter()
        .rposition(|coverage| *coverage > 0.02 * stem)
        .expect("the stems leave ink");
    last as f64 + rows[last] / stem
}

#[test]
fn glyphs_stand_on_the_baseline_the_layout_reports() {
    let engine = CoreTextEngine::new();
    // the system face, and a named one whose leading is not zero
    for face in [
        FontSpec::DEFAULT,
        FontSpec::DEFAULT.family("Helvetica Neue"),
    ] {
        for size in [12.0, 14.0, 26.0] {
            let font = FontSpec { size, ..face };
            let ascent = engine.measure_line(STEMS, &font).ascent;
            // whole, quarter and the 19.6 pt line height's own fraction
            for y in [20.0, 20.25, 209.76] {
                for scale in [1, 2] {
                    let display = DisplayList::from(vec![DrawCommand::TextLine {
                        origin: Point { x: 10.0, y },
                        content: STEMS.into(),
                        range: (0, STEMS.len()),
                        color: Color::BLACK,
                        font,
                    }]);
                    let bitmap = rasterize_with(
                        &display,
                        120 * scale,
                        260 * scale,
                        scale,
                        Color::WHITE,
                        &engine,
                        &RawImages::default(),
                    );
                    let floor = ink_floor(&bitmap);
                    let reported = y + ascent;
                    let case = format!("{font:?} at y {y}, scale {scale}");
                    // a baseline on the pixel grid ends the stems on a row
                    // boundary, crisp, the way a browser snaps it
                    assert!(
                        (floor - floor.round()).abs() < 0.1,
                        "{case}: the stems end mid-row, at {floor:.3}"
                    );
                    let painted = floor.round() / scale as f64;
                    assert!(
                        (painted - reported).abs() <= 0.5,
                        "{case}: the glyphs stand on {painted:.3}, the layout reports {reported:.3}"
                    );
                }
            }
        }
    }
}
