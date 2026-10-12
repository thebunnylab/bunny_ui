use super::*;
use bunny_ui::text_source::TextContent;

unsafe extern "C" fn reject(_: *const c_void) -> *const c_void {
    std::ptr::null()
}

#[test]
fn rejected_native_paragraph_preparation_returns_an_explicit_error() {
    let engine = CoreTextEngine::new();
    let font = FontSpec::DEFAULT;
    let result = create_paragraph("אבג abc", engine.font(&font), &font, reject);
    assert!(matches!(
        result,
        Err(TextError::Preparation {
            engine: "CoreText",
            operation: "CTTypesetterCreateWithAttributedString"
        })
    ));
}

#[test]
fn native_paragraph_cache_reuses_context_and_bounds_retention() {
    let engine = CoreTextEngine::new();
    let font = FontSpec::DEFAULT;
    let content = TextContent::from("\u{202e}abc def ghi");
    let line = content.line(7..10).unwrap();
    let first = engine.prepare_paragraph(line.paragraph(), &font).unwrap();
    let again = engine.prepare_paragraph(line.paragraph(), &font).unwrap();
    assert!(Rc::ptr_eq(&first, &again));
    for index in 0..MAX_ENTRIES + 20 {
        let source = TextContent::from(format!("context {index} abc"));
        let line = source.line(0..source.len()).unwrap();
        engine.prepare_paragraph(line.paragraph(), &font).unwrap();
    }
    let cache = engine.paragraphs.borrow();
    assert!(cache.entries.len() <= MAX_ENTRIES);
    assert!(cache.weight <= MAX_BYTES);
    assert_eq!(Rc::strong_count(&first), 2);
}

#[test]
fn tracking_has_separate_context_and_font_registration_releases_cached_paragraphs() {
    let engine = CoreTextEngine::new();
    let content = TextContent::from("office AV");
    let line = content.line(0..content.len()).unwrap();
    let font = FontSpec::DEFAULT;
    let first = engine.prepare_paragraph(line.paragraph(), &font).unwrap();
    let spaced = FontSpec {
        tracking: 2.0,
        ..font
    };
    let second = engine.prepare_paragraph(line.paragraph(), &spaced).unwrap();
    assert!(!Rc::ptr_eq(&first, &second));
    assert!(
        engine
            .try_measure_paragraph_line(&line, &spaced)
            .unwrap()
            .width
            > engine
                .try_measure_paragraph_line(&line, &font)
                .unwrap()
                .width
    );
    let first_weak = Rc::downgrade(&first);
    let second_weak = Rc::downgrade(&second);
    drop(first);
    drop(second);
    // Even a refused registration invalidates font-derived data: a previous
    // fallback must never survive a registration attempt as a stale context.
    assert!(!engine.register_font(b"invalid font"));
    assert!(engine.paragraphs.borrow().entries.is_empty());
    assert_eq!(engine.paragraphs.borrow().weight, 0);
    assert!(first_weak.upgrade().is_none());
    assert!(second_weak.upgrade().is_none());
}

#[test]
fn oversized_paragraph_context_is_transient() {
    let engine = CoreTextEngine::new();
    let content = TextContent::from("a".repeat(MAX_BYTES / 32 + 1));
    let line = content.line(0..1).unwrap();
    let native = engine
        .prepare_paragraph(line.paragraph(), &FontSpec::DEFAULT)
        .unwrap();
    assert_eq!(Rc::strong_count(&native), 1);
    assert!(engine.paragraphs.borrow().entries.is_empty());
    assert_eq!(engine.paragraphs.borrow().weight, 0);
}
