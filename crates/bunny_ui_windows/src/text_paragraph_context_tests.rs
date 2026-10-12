use super::*;
use crate::text::Color;
use bunny_ui::text_engine::TextEngine;
use bunny_ui::text_source::TextContent;

#[test]
fn paragraph_runtime_cache_reuses_only_identical_source_and_is_bounded() {
    let engine = DirectWriteEngine::new();
    let factories = engine.factories.as_ref().unwrap();
    let first = TextContent::from("\u{202e}abc def\u{202c}");
    let first = first.line(0..first.len()).unwrap();
    let second = TextContent::from("\u{202d}abc def\u{202c}");
    let second = second.line(0..second.len()).unwrap();
    let mut cache = Cache::default();
    let a = cache.get(factories, first.paragraph()).unwrap();
    let again = cache.get(factories, first.paragraph()).unwrap();
    let b = cache.get(factories, second.paragraph()).unwrap();
    assert!(Rc::ptr_eq(&a, &again));
    assert!(!Rc::ptr_eq(&a, &b));
    assert_ne!(a.levels, b.levels);
    for number in 0..=MAX_ENTRIES {
        let text = TextContent::from(format!("paragraph {number}"));
        let line = text.line(0..text.len()).unwrap();
        cache.get(factories, line.paragraph()).unwrap();
        assert!(cache.entries.len() <= MAX_ENTRIES);
        assert!(cache.retained <= MAX_BYTES);
    }
}

#[test]
fn paragraph_runtime_native_failure_is_not_pixel_font_or_empty_success() {
    let mut engine = DirectWriteEngine::new();
    engine.factories = None;
    let content = TextContent::from("abc");
    let line = content.line(0..3).unwrap();
    assert!(
        engine
            .measure_paragraph_line(&line, &FontSpec::DEFAULT)
            .is_err()
    );
    assert!(
        engine
            .paragraph_line_geometry(&line, &FontSpec::DEFAULT)
            .is_err()
    );
    assert!(
        engine
            .raster_paragraph_line(&line, &FontSpec::DEFAULT, Color::BLACK, 1)
            .is_err()
    );
}

#[test]
fn paragraph_runtime_empty_slice_and_invalid_parameters_preserve_error_semantics() {
    let engine = DirectWriteEngine::new();
    let content = TextContent::from("prefix abc suffix");
    let empty = content.line(7..7).unwrap();
    let source = content.line(7..10).unwrap();
    let font = FontSpec::DEFAULT;
    let metrics = engine.measure_paragraph_line(&empty, &font).unwrap();
    assert_eq!(metrics.width, 0.0);
    assert!(metrics.height() > 0.0);
    assert!(
        engine
            .paragraph_line_geometry(&empty, &font)
            .unwrap()
            .unwrap()
            .cells()
            .is_empty()
    );
    assert!(
        engine
            .raster_paragraph_line(&empty, &font, Color::BLACK, 1)
            .unwrap()
            .is_none()
    );
    for bad in [
        FontSpec {
            size: f64::NAN,
            ..font
        },
        FontSpec { size: -1.0, ..font },
        FontSpec {
            tracking: f64::INFINITY,
            ..font
        },
    ] {
        assert!(engine.measure_paragraph_line(&source, &bad).is_err());
        assert!(engine.paragraph_line_geometry(&source, &bad).is_err());
        assert!(
            engine
                .raster_paragraph_line(&source, &bad, Color::BLACK, 1)
                .is_err()
        );
    }
    for scale in [0, usize::MAX] {
        assert!(
            engine
                .raster_paragraph_line(&source, &font, Color::BLACK, scale)
                .is_err()
        );
    }
    assert!(
        engine
            .raster_paragraph_line(&source, &font, Color::BLACK, 1)
            .unwrap()
            .is_some()
    );
}

#[test]
fn paragraph_runtime_identical_visible_text_keeps_distinct_context_in_all_queries() {
    let engine = DirectWriteEngine::new();
    let rtl = TextContent::from("\u{202e}prefix abc suffix\u{202c}");
    let ltr = TextContent::from("\u{202d}prefix abc suffix\u{202c}");
    let rtl = rtl.line(10..13).unwrap();
    let ltr = ltr.line(10..13).unwrap();
    assert_eq!(rtl.text(), "abc");
    assert_eq!(ltr.text(), "abc");
    let font = FontSpec::DEFAULT;
    for (source, odd) in [(&rtl, true), (&ltr, false), (&rtl, true)] {
        let metrics = engine.measure_paragraph_line(source, &font).unwrap();
        let geometry = engine
            .paragraph_line_geometry(source, &font)
            .unwrap()
            .unwrap();
        let cell = &geometry.cells()[1];
        assert_eq!(cell.trailing() < cell.leading(), odd);
        let image = engine
            .raster_paragraph_line(source, &font, Color::BLACK, 2)
            .unwrap()
            .unwrap();
        assert!(image.width > 0 && image.height > image.baseline);
        assert!(
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] != 0)
        );
        assert!(metrics.width > 0.0);
    }
    assert_eq!(engine.paragraphs.borrow().entries.len(), 2);
}

#[test]
fn paragraph_runtime_oversized_analysis_bypasses_retention() {
    let engine = DirectWriteEngine::new();
    let content = TextContent::from("a".repeat(MAX_BYTES / 17 + 1));
    let line = content.line(0..content.len()).unwrap();
    let mut cache = Cache::default();
    let prepared = cache
        .get(engine.factories.as_ref().unwrap(), line.paragraph())
        .unwrap();
    assert_eq!(prepared.utf16().len(), content.len());
    assert!(cache.entries.is_empty());
    assert_eq!(cache.retained, 0);
}

#[test]
fn paragraph_runtime_font_registration_invalidates_resolved_fonts() {
    use bunny_ui::text_engine::Family;
    let engine = DirectWriteEngine::new();
    let font = FontSpec {
        family: Family::named("Bunny Paragraph Color"),
        size: 21.0,
        ..FontSpec::DEFAULT
    };
    let content = TextContent::from("aaa");
    let line = content.line(0..3).unwrap();
    let before = engine.measure_paragraph_line(&line, &font).unwrap();
    let prepared = Rc::clone(
        engine
            .paragraphs
            .borrow()
            .entries
            .get(line.paragraph())
            .unwrap(),
    );
    assert!(engine.register_font(include_bytes!("../tests/fixtures/paragraph-color.ttf")));
    let after = engine.measure_paragraph_line(&line, &font).unwrap();
    let native = engine.measure_line("aaa", &font);
    assert!((after.width - native.width).abs() < 0.03);
    assert!((after.width - before.width).abs() > 1.0);
    assert!(Rc::ptr_eq(
        &prepared,
        engine
            .paragraphs
            .borrow()
            .entries
            .get(line.paragraph())
            .unwrap()
    ));
}
