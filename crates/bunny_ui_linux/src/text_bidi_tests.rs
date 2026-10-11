//! Native directional ordering regressions using the installed text stack.
use super::*;

fn fixture_font() -> FontSpec {
    FontSpec {
        family: bunny_ui::text_engine::Family::named("DejaVu Sans"),
        size: 24.0,
        ..FontSpec::DEFAULT
    }
}

fn visual_glyphs(
    engine: &FreeTypeEngine,
    text: &str,
    key: &FaceKey,
) -> (Vec<(u32, i64, i64)>, i64) {
    let (glyphs, width) = engine.shape(text, key, 0).expect("native font and shaper");
    assert!(
        glyphs.iter().all(|glyph| glyph.glyph != 0),
        "fixture has no missing glyphs"
    );
    (
        glyphs
            .iter()
            .map(|glyph| (glyph.glyph, glyph.x, glyph.y))
            .collect(),
        width,
    )
}

fn assert_visual_runs(text: &str, visual_runs: &[&str]) {
    let engine = FreeTypeEngine::new();
    let key = FreeTypeEngine::key(&fixture_font(), 1);
    let mut expected = Vec::new();
    let mut pen = 0;
    for run in visual_runs {
        let (glyphs, width) = visual_glyphs(&engine, run, &key);
        expected.extend(glyphs.into_iter().map(|(glyph, x, y)| (glyph, pen + x, y)));
        pen += width;
    }
    let (actual, width) = visual_glyphs(&engine, text, &key);
    assert_eq!(
        actual, expected,
        "visual glyph order and positions for {text:?}"
    );
    assert_eq!(width, pen, "one shared advance for measurement and raster");
}

#[test]
fn mixed_direction_ltr_paragraph_reverses_only_the_hebrew_run() {
    assert_visual_runs("abc אבג def", &["abc ", "אבג", " def"]);
}

#[test]
fn mixed_direction_rtl_paragraph_keeps_latin_and_numbers_ltr() {
    assert_visual_runs("אבג abc 123", &["abc 123", " ", "אבג"]);
}

#[test]
fn mixed_direction_override_agrees_with_visible_raster() {
    let engine = FreeTypeEngine::new();
    let font = FontSpec {
        family: bunny_ui::text_engine::Family::named("DejaVu Sans Mono"),
        design: FontDesign::Mono,
        size: 24.0,
        ..FontSpec::DEFAULT
    };
    let color = Color {
        r: 10,
        g: 20,
        b: 30,
        a: 255,
    };
    let logical = "abc\u{202e}def\u{202c}";
    let expected = engine
        .raster_line("abcfed", &font, color, 1)
        .expect("visible fixture");
    let actual = engine
        .raster_line(logical, &font, color, 1)
        .expect("visible mixed line");
    assert_eq!(
        engine.measure_line(logical, &font).width.ceil() as usize,
        actual.width
    );
    assert_eq!(
        (actual.width, actual.height, actual.baseline),
        (expected.width, expected.height, expected.baseline)
    );
    assert!(
        actual.rgba == expected.rgba,
        "direction override changes the pixels"
    );
}
