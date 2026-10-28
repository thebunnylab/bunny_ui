//! Every step of the weight scale paints its own stroke: the light end is
//! lighter than Regular, and the system face keeps the steps CSS names
//! apart (Medium is not Regular, Semibold is not Bold).
#![cfg(target_os = "macos")]

use bunny_ui::image_engine::RawImages;
use bunny_ui::layout::{Color, DisplayList, DrawCommand, Point};
use bunny_ui::raster::rasterize_with;
use bunny_ui::text_engine::{FontSpec, TextEngine, Weight};
use bunny_ui_apple::CoreTextEngine;

/// Straight stems and round bowls: the ink a weight lays down grows with
/// the stroke, whatever the face does to its widths.
const SAMPLE: &str = "Hamburgefonstiv";

const SCALE: [Weight; 9] = [
    Weight::Thin,
    Weight::ExtraLight,
    Weight::Light,
    Weight::Regular,
    Weight::Medium,
    Weight::Semibold,
    Weight::Bold,
    Weight::ExtraBold,
    Weight::Black,
];

/// The ink of one line, black on white, in coverage units.
fn ink(engine: &CoreTextEngine, font: FontSpec) -> f64 {
    let width = engine.measure_line(SAMPLE, &font).width.ceil() as usize + 20;
    let display = DisplayList::from(vec![DrawCommand::TextLine {
        origin: Point { x: 10.0, y: 10.0 },
        content: SAMPLE.into(),
        range: (0, SAMPLE.len()),
        color: Color::BLACK,
        font,
    }]);
    let bitmap =
        rasterize_with(&display, width, 80, 1, Color::WHITE, engine, &RawImages::default());
    (0..bitmap.height())
        .flat_map(|y| (0..bitmap.width()).map(move |x| (x, y)))
        .map(|(x, y)| {
            let red = bitmap.pixel(x, y).map_or(255, |pixel| pixel >> 24 & 0xff);
            f64::from(255 - red) / 255.0
        })
        .sum()
}

#[test]
fn the_system_face_paints_nine_distinct_weights() {
    let engine = CoreTextEngine::new();
    let inks: Vec<(Weight, f64)> = SCALE
        .iter()
        .map(|&weight| (weight, ink(&engine, FontSpec { size: 26.0, weight, ..FontSpec::DEFAULT })))
        .collect();
    for pair in inks.windows(2) {
        let ((lighter, a), (heavier, b)) = (pair[0], pair[1]);
        assert!(
            b > a * 1.02,
            "{heavier:?} must paint heavier than {lighter:?}: {b:.0} against {a:.0} ({inks:?})"
        );
    }
}

#[test]
fn light_paints_lighter_than_regular_in_a_named_family() {
    let engine = CoreTextEngine::new();
    let face = FontSpec { size: 26.0, ..FontSpec::DEFAULT }.family("Helvetica Neue");
    let light = ink(&engine, FontSpec { weight: Weight::Light, ..face });
    let regular = ink(&engine, face);
    let bold = ink(&engine, FontSpec { weight: Weight::Bold, ..face });
    assert!(light < regular * 0.95, "Light {light:.0} against Regular {regular:.0}");
    assert!(bold > regular * 1.05, "Bold {bold:.0} against Regular {regular:.0}");
}

#[test]
fn a_face_without_a_light_cut_keeps_its_regular() {
    // Menlo ships Regular and Bold: a lighter ask lands on the nearest
    // face it has, never on another family
    let engine = CoreTextEngine::new();
    let mono = FontSpec {
        size: 26.0,
        design: bunny_ui::text_engine::FontDesign::Mono,
        ..FontSpec::DEFAULT
    };
    let light = ink(&engine, FontSpec { weight: Weight::Light, ..mono });
    let regular = ink(&engine, mono);
    assert!((light - regular).abs() < regular * 0.01, "Light {light:.0} against Regular {regular:.0}");
}
