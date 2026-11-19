use std::rc::Rc;

use bunny_ui::layout::Color;
use bunny_ui::prelude::*;
use bunny_ui::text_engine::{FontSpec, LineMetrics, PixelFont, TextEngine, TextRaster};
use bunny_ui_core as bunny_ui;

const SIZE: Size = Size { width: 300.0, height: 200.0 };

/// A face whose one is narrow, as most proportional figures have it:
/// every glyph eight points wide but the `1`, four. Its raster inks the
/// first column of each glyph, where its pen stands, and keeps no
/// tracking.
struct NarrowOne;

const fn advance(glyph: char) -> u8 {
    if glyph == '1' { 4 } else { 8 }
}

impl TextEngine for NarrowOne {
    fn measure_line(&self, text: &str, font: &FontSpec) -> LineMetrics {
        let width = text.chars().map(|glyph| f64::from(advance(glyph)) + font.tracking).sum();
        LineMetrics { width, ascent: 12.0, descent: 4.0 }
    }

    fn raster_line(&self, text: &str, _: &FontSpec, color: Color, scale: usize) -> Option<TextRaster> {
        let width = text.chars().map(|glyph| usize::from(advance(glyph))).sum::<usize>() * scale;
        let height = 16 * scale;
        let mut rgba = vec![0u8; width * height * 4];
        let mut pen = 0;
        for glyph in text.chars() {
            for row in 0..height {
                let at = (row * width + pen * scale) * 4;
                rgba[at..at + 4].copy_from_slice(&[color.r, color.g, color.b, color.a]);
            }
            pen += usize::from(advance(glyph));
        }
        Some(TextRaster { width, height, baseline: 12 * scale, ascent: 12.0, rgba })
    }
}

fn frames(runtime: &Runtime, view: &impl View) -> Vec<(String, Rect)> {
    runtime.display_frame(view, SIZE);
    runtime.test_frames().into_iter().map(|(id, rect)| (id.to_string(), rect)).collect()
}

fn width(frames: &[(String, Rect)], name: &str) -> f64 {
    frames.iter().find(|(id, _)| id == name).map(|(_, rect)| rect.size.width).expect(name)
}

fn narrow() -> Runtime {
    Runtime::new().text_engine(Rc::new(NarrowOne))
}

/// `.monospaced_digit()` is `SwiftUI`'s `.monospacedDigit()`: each figure
/// takes the widest figure's advance, so a column of numbers or a clock
/// that ticks keeps its width; the words around them keep theirs.
#[test]
fn every_figure_takes_the_widest_advance() {
    let found = frames(
        &narrow(),
        &vstack!(
            text("11").test_id("plain"),
            text("11").monospaced_digit().test_id("tabular"),
            text("a1:").monospaced_digit().test_id("mixed"),
        ),
    );
    assert_eq!(width(&found, "plain"), 8.0);
    assert_eq!(width(&found, "tabular"), 16.0);
    assert_eq!(width(&found, "mixed"), 24.0);
}

/// It is inherited by the text below, as the font is.
#[test]
fn the_text_below_inherits_it() {
    let found = frames(&narrow(), &vstack!(text("1111").test_id("inner")).monospaced_digit());
    assert_eq!(width(&found, "inner"), 32.0);
}

/// Each figure sits in the middle of its cell, and a tracking spreads
/// the cells as it spreads the letters.
#[test]
fn a_figure_sits_in_the_middle_of_its_cell() {
    let font = FontSpec { monospaced_digits: true, ..FontSpec::DEFAULT };
    let raster = bunny_ui::figures::raster_line(&NarrowOne, "1a1", &font, Color::BLACK, 2).expect("ink");
    let inked: Vec<usize> = (0..raster.width).filter(|column| raster.rgba[column * 4 + 3] > 0).collect();
    assert_eq!(inked, [4, 16, 36], "the ones two points in, the a at its pen");
    assert_eq!(raster.width, 48);
    assert_eq!((raster.baseline, raster.ascent), (24, 12.0));
    let spread = FontSpec { tracking: 2.0, ..font };
    let metrics = bunny_ui::figures::measure_line(&NarrowOne, "1a1", &spread);
    assert_eq!(metrics.width, 30.0);
}

/// A face whose figures already share one advance — most interface
/// faces, and the house font — is the engine's line, byte for byte.
#[test]
fn even_figures_are_left_to_the_engine() {
    let font = FontSpec { monospaced_digits: true, ..FontSpec::DEFAULT };
    let plain = FontSpec::DEFAULT;
    let ours = bunny_ui::figures::raster_line(&PixelFont, "12:30", &font, Color::BLACK, 2).expect("ink");
    let engine = PixelFont.raster_line("12:30", &plain, Color::BLACK, 2).expect("ink");
    assert_eq!((ours.width, ours.height, ours.baseline), (engine.width, engine.height, engine.baseline));
    assert_eq!(ours.rgba, engine.rgba);
    let found = frames(&Runtime::new(), &text("12:30").monospaced_digit().test_id("clock"));
    assert_eq!(width(&found, "clock"), PixelFont.measure_line("12:30", &plain).width);
}

/// The page writes it as `font-variant-numeric: tabular-nums`, where the
/// face is declared; a text that inherits the face names none of it.
#[test]
fn the_page_writes_tabular_nums() {
    let page = bunny_ui::ssr::render(&text("12").monospaced_digit(), SIZE);
    assert!(page.css.contains("font-variant-numeric:tabular-nums"), "{}", page.css);
    let plain = bunny_ui::ssr::render(&text("12"), SIZE);
    assert!(!plain.css.contains("font-variant-numeric"), "{}", plain.css);
    let stack = bunny_ui::ssr::render(&vstack!(text("1"), text("2")).monospaced_digit(), SIZE);
    assert_eq!(stack.css.matches("font-variant-numeric:tabular-nums").count(), 1, "{}", stack.css);
}
