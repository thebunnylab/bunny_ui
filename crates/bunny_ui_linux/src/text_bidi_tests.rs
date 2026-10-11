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
    let measured = engine.measure_line(logical, &font).width.ceil();
    let raster_width = f64::from(u32::try_from(actual.width).expect("small raster fixture"));
    assert!((measured - raster_width).abs() < f64::EPSILON);
    assert_eq!(
        (actual.width, actual.height, actual.baseline),
        (expected.width, expected.height, expected.baseline)
    );
    assert!(
        actual.rgba == expected.rgba,
        "direction override changes the pixels"
    );
}

#[test]
fn mixed_direction_glyph_clusters_keep_logical_utf8_indices() {
    let engine = FreeTypeEngine::new();
    let key = FreeTypeEngine::key(&fixture_font(), 1);
    let (glyphs, _) = engine.shape("abc אבג def", &key, 0).expect("native shaper");
    let clusters: Vec<_> = glyphs.iter().map(|glyph| glyph.cluster).collect();
    assert_eq!(clusters, [0, 1, 2, 3, 8, 6, 4, 10, 11, 12, 13]);
}

#[test]
fn mixed_direction_arabic_keeps_latin_and_numbers_ltr() {
    assert_visual_runs("مرحبا abc 123", &["abc 123", " ", "مرحبا"]);
}

#[test]
fn mixed_direction_hebrew_marks_stay_with_their_base() {
    assert_visual_runs("abc שָׁלוֹם def", &["abc ", "שָׁלוֹם", " def"]);
}

#[test]
fn mixed_direction_brackets_are_mirrored_by_the_shaper() {
    assert_visual_runs("אבג (abc) דהו", &["דהו", " (", "abc", ") ", "אבג"]);
}

#[test]
fn mixed_direction_rtl_fallback_fragments_follow_visual_order() {
    let engine = FreeTypeEngine::new();
    assert!(
        engine
            .families()
            .iter()
            .any(|name| &**name == "Liberation Sans"),
        "install the declared Liberation Sans test fixture"
    );
    let font = FontSpec {
        family: bunny_ui::text_engine::Family::named("Liberation Sans"),
        ..fixture_font()
    };
    let key = FreeTypeEngine::key(&font, 1);
    let primary = engine.face(&key).expect("primary face");
    // The fixture deliberately splits one RTL directional run across fonts.
    assert_ne!(unsafe { FT_Get_Char_Index(primary.ft, u64::from('א')) }, 0);
    assert_eq!(unsafe { FT_Get_Char_Index(primary.ft, u64::from('س')) }, 0);
    let text = "אב سلام גד";
    let fragments = engine.face_runs(text, 0..text.len(), &key, primary);
    assert_eq!(fragments.len(), 3);
    assert_ne!(fragments[1].0.ft, primary.ft);
    let mut expected = Vec::new();
    let mut pen = 0;
    for run in [" גד", "سلام", "אב "] {
        let (glyphs, width) = visual_glyphs(&engine, run, &key);
        expected.extend(glyphs.into_iter().map(|(glyph, x, y)| (glyph, pen + x, y)));
        pen += width;
    }
    let (actual, width) = visual_glyphs(&engine, text, &key);
    assert_eq!(actual, expected);
    assert_eq!(width, pen);
}

#[test]
fn mixed_direction_isolates_do_not_reverse_the_outer_latin() {
    let text = "abc \u{2067}אבג\u{2069} def";
    let mut visual = String::new();
    for run in bidi::runs(text).expect("native resolver") {
        let visible = |character: &char| !matches!(*character, '\u{2067}' | '\u{2069}');
        match run.direction {
            bidi::Direction::LeftToRight => visual.extend(text[run.bytes].chars().filter(visible)),
            bidi::Direction::RightToLeft => {
                visual.extend(text[run.bytes].chars().rev().filter(visible));
            }
        }
    }
    assert_eq!(visual, "abc גבא def");
}

#[test]
fn mixed_direction_empty_and_ltr_lines_preserve_their_contract() {
    assert!(bidi::runs("").expect("empty input").is_empty());
    assert_visual_runs("abc 123", &["abc ", "123"]);
    let engine = FreeTypeEngine::new();
    let key = FreeTypeEngine::key(&fixture_font(), 1);
    let (glyphs, width) = engine.shape("", &key, 0).expect("empty native shape");
    assert!(glyphs.is_empty());
    assert_eq!(width, 0);
}

#[test]
fn mixed_direction_explicit_paragraphs_reset_the_base_direction() {
    let text = "אבג\nabc";
    let runs = bidi::runs(text).expect("native paragraphs");
    assert_eq!(
        runs,
        [
            bidi::Run {
                bytes: 0..7,
                direction: bidi::Direction::RightToLeft
            },
            bidi::Run {
                bytes: 7..10,
                direction: bidi::Direction::LeftToRight
            },
        ]
    );
}
