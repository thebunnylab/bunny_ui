use super::*;
use bunny_ui::text_engine::{Family, TextEngine};
use bunny_ui::text_source::TextContent;

fn content(text: &str) -> TextContent {
    std::sync::Arc::<str>::from(text).into()
}
fn font() -> FontSpec {
    FontSpec {
        family: Family::named("DejaVu Sans"),
        ..FontSpec::DEFAULT
    }
}
fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 0.02, "{a} != {b}");
}

#[test]
fn paragraph_cache_reuses_context_without_aliasing_equal_visible_lines() {
    let source = content("\u{202e}prefix abc\u{202c}");
    let first = source.line(0..source.source().len()).unwrap();
    let last = source.line(10..13).unwrap();
    assert_eq!(last.text(), "abc");
    let mut cache = Cache::default();
    let a = cache.get(first.paragraph()).unwrap();
    let b = cache.get(last.paragraph()).unwrap();
    assert!(Rc::ptr_eq(&a, &b));
    let plain = content("prefix abc").line(7..10).unwrap();
    let c = cache.get(plain.paragraph()).unwrap();
    assert!(!Rc::ptr_eq(&a, &c));
    assert_eq!(cache.entries.len(), 2);
    assert_ne!(
        a.line_runs(last.bytes()).unwrap()[0].direction,
        c.line_runs(plain.bytes()).unwrap()[0].direction
    );
}

#[test]
fn paragraph_cache_bounds_entries_and_bypasses_oversized_sources() {
    let mut cache = Cache::default();
    for index in 0..=MAX_ENTRIES {
        let source = content(&format!("context {index}"));
        cache
            .get(source.line(0..source.source().len()).unwrap().paragraph())
            .unwrap();
        assert!(cache.entries.len() <= MAX_ENTRIES);
        assert!(cache.retained <= MAX_BYTES);
    }
    let before = (cache.entries.len(), cache.retained);
    let source = content(&"x".repeat(MAX_BYTES / 17 + 1));
    let paragraph = source.line(0..1).unwrap();
    let a = cache.get(paragraph.paragraph()).unwrap();
    let b = cache.get(paragraph.paragraph()).unwrap();
    assert!(!Rc::ptr_eq(&a, &b));
    assert_eq!((cache.entries.len(), cache.retained), before);
}

#[test]
fn native_face_refusal_propagates_through_all_contextual_operations() {
    let engine = FreeTypeEngine::new();
    let source = content("abc");
    let line = source.line(0..3).unwrap();
    let font = font();
    engine
        .faces
        .borrow_mut()
        .insert(FreeTypeEngine::key(&font, 1), None);
    assert!(matches!(
        engine.measure_paragraph_line(&line, &font),
        Err(TextError::Preparation { .. })
    ));
    assert!(matches!(
        engine.paragraph_line_geometry(&line, &font),
        Err(TextError::Preparation { .. })
    ));
    assert!(matches!(
        engine.raster_paragraph_line(&line, &font, Color::BLACK, 1),
        Err(TextError::Preparation { .. })
    ));
    engine.faces.borrow_mut().clear();
    assert!(engine.measure_paragraph_line(&line, &font).unwrap().width > 0.0);
    assert!(
        engine
            .paragraph_line_geometry(&line, &font)
            .unwrap()
            .is_some()
    );
    assert!(
        engine
            .raster_paragraph_line(&line, &font, Color::BLACK, 1)
            .unwrap()
            .is_some()
    );
}

#[test]
fn contextual_indices_metrics_and_pixels_agree_with_the_override() {
    let engine = FreeTypeEngine::new();
    let source = content("\u{202e}prefix abc\u{202c}");
    let line = source.line(10..13).unwrap();
    let font = font();
    let metrics = engine.measure_paragraph_line(&line, &font).unwrap();
    let geometry = engine
        .paragraph_line_geometry(&line, &font)
        .unwrap()
        .unwrap();
    assert_eq!(geometry.cells().len(), 3);
    assert!(
        geometry
            .cells()
            .iter()
            .all(|cell| cell.leading() > cell.trailing())
    );
    near(geometry.cells()[0].leading(), metrics.width);
    near(geometry.cells()[2].trailing(), 0.0);
    let contextual = engine
        .raster_paragraph_line(&line, &font, Color::BLACK, 1)
        .unwrap()
        .unwrap();
    // A separately shaped, self-contained override supplies a pixel reference.
    // Production never inserts controls into the source or the visible line.
    let reference = engine
        .raster_line("\u{202e}abc\u{202c}", &font, Color::BLACK, 1)
        .unwrap();
    assert_eq!(
        (contextual.width, contextual.height),
        (reference.width, reference.height)
    );
    assert_eq!(contextual.rgba, reference.rgba);
    let plain = engine.raster_line("abc", &font, Color::BLACK, 1).unwrap();
    assert_ne!(contextual.rgba, plain.rgba);
}

#[test]
fn context_queries_preserve_utf8_ranges_across_fonts_scales_and_order() {
    let engine = FreeTypeEngine::new();
    let source = content("\u{202d}prefix אבג 😀 xyz\u{202c}");
    let text = source.source();
    let start = text.find('א').unwrap();
    let end = text.find(" xyz").unwrap();
    let line = source.line(start..end).unwrap();
    for tracking in [0.0, 1.25] {
        let font = FontSpec { tracking, ..font() };
        let before = engine
            .paragraph_line_geometry(&line, &font)
            .unwrap()
            .unwrap();
        let metrics = engine.measure_paragraph_line(&line, &font).unwrap();
        let full = line.paragraph().full_line();
        engine.paragraph_line_geometry(&full, &font).unwrap();
        let after = engine
            .paragraph_line_geometry(&line, &font)
            .unwrap()
            .unwrap();
        assert_eq!(before.cells(), after.cells());
        let cells = before.cells();
        assert_eq!(cells.len(), line.text().chars().count());
        near(cells.first().unwrap().leading(), 0.0);
        near(cells.last().unwrap().trailing(), metrics.width);
        for scale in [1, 2] {
            let raster = engine
                .raster_paragraph_line(&line, &font, Color::BLACK, scale)
                .unwrap()
                .unwrap();
            assert!(raster.width > 0 && raster.height > 0);
        }
    }
}

#[test]
fn empty_continuation_is_empty_instead_of_the_rest_of_the_paragraph() {
    let engine = FreeTypeEngine::new();
    let source = content("prefix suffix");
    let line = source.line(7..7).unwrap();
    assert_eq!(
        engine.measure_paragraph_line(&line, &font()).unwrap().width,
        0.0
    );
    assert!(
        engine
            .paragraph_line_geometry(&line, &font())
            .unwrap()
            .unwrap()
            .cells()
            .is_empty()
    );
    assert!(
        engine
            .raster_paragraph_line(&line, &font(), Color::BLACK, 1)
            .unwrap()
            .is_none()
    );
}
