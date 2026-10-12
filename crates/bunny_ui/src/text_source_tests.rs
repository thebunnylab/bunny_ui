use super::*;
use crate::text_input::byte_to_utf16;

#[test]
fn paragraph_source_maps_document_bytes_to_native_units() {
    let content = TextContent::from("first\nאב😀xyz\n");
    let start = content.find('😀').unwrap();
    let line = content.line(start..start + "😀xy".len()).unwrap();
    assert_eq!(line.text(), "😀xy");
    assert_eq!(line.paragraph().text(), "אב😀xyz");
    assert_eq!(line.bytes(), 4..10);
    assert_eq!(line.utf16(), 2..6);
    assert!(
        content
            .line(content.len()..content.len())
            .unwrap()
            .text()
            .is_empty()
    );
}

#[test]
fn paragraph_source_rejects_invalid_and_cross_paragraph_ranges() {
    let content = TextContent::from("אב\ncd");
    assert_eq!(
        content.line(Range { start: 4, end: 2 }).unwrap_err(),
        TextRangeError::InvalidRange
    );
    assert_eq!(
        content.line(0..100).unwrap_err(),
        TextRangeError::InvalidRange
    );
    assert_eq!(content.line(1..4).unwrap_err(), TextRangeError::NotBoundary);
    assert_eq!(
        content.line(0..5).unwrap_err(),
        TextRangeError::CrossesParagraph
    );
    assert!(content.line(4..4).unwrap().text().is_empty());
    assert_eq!(content.line(5..7).unwrap().text(), "cd");
}

#[test]
fn paragraph_source_context_changes_line_identity() {
    let left = TextContent::from("\u{202e}abc def\u{202c}");
    let right = TextContent::from("\u{202d}abc def\u{202c}");
    let start = left.find("def").unwrap();
    let a = left.line(start..start + 3).unwrap();
    let b = right.line(start..start + 3).unwrap();
    assert_eq!(a.text(), b.text());
    assert_ne!(a, b);
    let hash = |line: &ParagraphLine| {
        let mut state = FxHasher::default();
        line.hash(&mut state);
        state.finish()
    };
    assert_ne!(hash(&a), hash(&b));
}

#[test]
fn paragraph_source_unrelated_hard_paragraph_keeps_identity() {
    let a = TextContent::from("unrelated\nאבג abc");
    let b = TextContent::from("changed prefix length\nאבג abc");
    let x = a.find("abc").unwrap();
    let y = b.find("abc").unwrap();
    assert_eq!(a.line(x..x + 3).unwrap(), b.line(y..y + 3).unwrap());
}

#[test]
fn paragraph_source_cache_reuses_source_and_unchanged_paragraphs() {
    let mut cache = ContentCache::default();
    let old: Arc<str> = Arc::from("abc\nunchanged אבג\ntail");
    let first = cache.get(&old);
    let again = cache.get(&old);
    assert!(Arc::ptr_eq(&first.0, &again.0));
    let new = cache.get(&Arc::from("updated first paragraph\nunchanged אבג\ntail"));
    let old_start = first.find("unchanged").unwrap();
    let new_start = new.find("unchanged").unwrap();
    let a = first.line(old_start..old_start + 9).unwrap();
    let b = new.line(new_start..new_start + 9).unwrap();
    assert!(Arc::ptr_eq(&a.paragraph.0, &b.paragraph.0));
}

#[test]
fn paragraph_source_retaining_line_does_not_retain_entire_document() {
    let source: Arc<str> = Arc::from("large irrelevant prefix\nאבג abc\nlarge irrelevant suffix");
    let weak = Arc::downgrade(&source);
    let content = TextContent::from(source.clone());
    let start = content.find("abc").unwrap();
    let line = content.line(start..start + 3).unwrap();
    let compact = TextContent::from(line.paragraph().clone());
    assert_eq!(compact.line(line.bytes()).unwrap(), line);
    assert_eq!(&*compact, "אבג abc");
    drop(source);
    drop(content);
    assert!(weak.upgrade().is_none());
}

#[test]
fn paragraph_source_utf16_mapping_is_scalar_exact_and_shared() {
    let content = TextContent::from("a😀e\u{301}👨‍👩‍👧‍👦אב");
    for (byte, character) in content.char_indices() {
        let range = byte..byte + character.len_utf8();
        let line = content.line(range.clone()).unwrap();
        assert_eq!(
            line.utf16(),
            byte_to_utf16(&content, range.start)..byte_to_utf16(&content, range.end)
        );
    }
    let a = content.line(1..5).unwrap();
    let b = content.line(5..6).unwrap();
    assert!(std::ptr::eq(a.paragraph.indices(), b.paragraph.indices()));
}

#[test]
fn paragraph_source_cache_retention_has_fixed_bounds() {
    let mut cache = ContentCache::default();
    let mut live = Vec::new();
    for index in 0..SOURCE_LIMIT + 10 {
        live.push(cache.get(&Arc::from(format!("revision {index}"))));
    }
    assert!(cache.sources.len() <= SOURCE_LIMIT);
    let metadata = Arc::downgrade(&live.last().unwrap().0);
    drop(live);
    assert!(
        metadata.upgrade().is_none(),
        "the cache owns only weak source handles"
    );
    assert!(cache.sources.iter().all(|entry| entry.upgrade().is_none()));
}

#[test]
fn paragraph_source_measurement_cache_uses_context_and_font() {
    use crate::layout::Color;
    use crate::text_engine::{
        FontSpec, LineMetrics, MeasureCache, PixelFont, TextEngine, TextRaster,
    };
    use std::cell::Cell;
    struct Engine(Cell<usize>, Cell<usize>);
    impl TextEngine for Engine {
        fn measure_line(&self, _: &str, _: &FontSpec) -> LineMetrics {
            panic!("the contextual cache must call the contextual boundary");
        }
        fn measure_paragraph_line(&self, line: &ParagraphLine, font: &FontSpec) -> LineMetrics {
            self.0.set(self.0.get() + 1);
            let mut result = PixelFont.measure_line(line.text(), font);
            if line.paragraph().text().starts_with('\u{202e}') {
                result.width += 10.0;
            }
            result
        }
        fn paragraph_line_geometry(
            &self,
            _: &ParagraphLine,
            _: &FontSpec,
        ) -> Option<crate::text_geometry::LineGeometry> {
            self.1.set(self.1.get() + 1);
            None
        }
        fn raster_line(&self, _: &str, _: &FontSpec, _: Color, _: usize) -> Option<TextRaster> {
            None
        }
    }
    let engine = Engine(Cell::new(0), Cell::new(0));
    let cache = MeasureCache::default();
    let a = cache.text_content(&Arc::from("\u{202e}abc def\u{202c}"));
    let b = cache.text_content(&Arc::from("\u{202d}abc def\u{202c}"));
    let start = a.find("def").unwrap();
    let a = a.line(start..start + 3).unwrap();
    let b = b.line(start..start + 3).unwrap();
    let font = FontSpec::DEFAULT;
    for _ in 0..3 {
        let x = cache.get_or_measure_paragraph(&a, &font, &engine);
        let y = cache.get_or_measure_paragraph(&b, &font, &engine);
        assert!((x.width - y.width - 10.0).abs() < 0.01);
        assert!(
            cache
                .get_or_paragraph_geometry(&a, &font, &engine)
                .is_none()
        );
        assert!(
            cache
                .get_or_paragraph_geometry(&b, &font, &engine)
                .is_none()
        );
    }
    assert_eq!((engine.0.get(), engine.1.get()), (2, 2));
    let changed = FontSpec { size: 24.0, ..font };
    cache.get_or_measure_paragraph(&a, &changed, &engine);
    cache.get_or_paragraph_geometry(&a, &changed, &engine);
    assert_eq!((engine.0.get(), engine.1.get()), (3, 3));
}

#[test]
fn paragraph_source_legacy_engine_defaults_remain_callable() {
    use crate::layout::Color;
    use crate::text_engine::{FontSpec, PixelFont, TextEngine};
    let content = TextContent::from("before abc after");
    let line = content.line(7..10).unwrap();
    let font = FontSpec::DEFAULT;
    assert_eq!(
        PixelFont.measure_paragraph_line(&line, &font),
        PixelFont.measure_line("abc", &font)
    );
    let contextual = PixelFont
        .raster_paragraph_line(&line, &font, Color::BLACK, 1)
        .unwrap();
    let legacy = PixelFont
        .raster_line("abc", &font, Color::BLACK, 1)
        .unwrap();
    assert_eq!(contextual.rgba, legacy.rgba);
    assert!(PixelFont.paragraph_line_geometry(&line, &font).is_none());
}

#[test]
fn paragraph_source_internal_interning_rejects_hard_separators() {
    let cache = ContentCache::default();
    assert_eq!(
        cache.paragraph("one\ntwo").unwrap_err(),
        TextRangeError::CrossesParagraph
    );
    assert_eq!(cache.paragraph("").unwrap().full_line().text(), "");
}
