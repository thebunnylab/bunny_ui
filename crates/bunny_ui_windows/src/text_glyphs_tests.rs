use super::*;
use crate::text::{DirectWriteEngine, DwriteTextMetrics, create_layout};
use bunny_ui::text_engine::{FontDesign, Slant, Weight};

fn engine() -> DirectWriteEngine {
    let engine = DirectWriteEngine::new();
    assert!(engine.factories.is_some(), "native factories are required");
    engine
}

// Independent high-level layout: it performs its own analysis, fallback and
// glyph production. Apply style/spacing explicitly because legacy layout ignores
// the FontSpec tracking and uses an upright format even for italic metrics.
fn reference_width(engine: &DirectWriteEngine, text: &str, font: &FontSpec) -> f64 {
    #[repr(C)]
    struct Range {
        start: u32,
        length: u32,
    }
    #[repr(C)]
    struct LayoutVtbl {
        _pad_0_32: [usize; 33],
        set_style: unsafe extern "system" fn(*mut c_void, u32, Range) -> Hresult,
        _pad_34_68: [usize; 35],
        set_spacing: unsafe extern "system" fn(*mut c_void, f32, f32, f32, Range) -> Hresult,
    }
    #[repr(C)]
    struct Layout {
        vtbl: *const LayoutVtbl,
    }
    const ID: Guid = Guid {
        d1: 0x9064_d822,
        d2: 0x80a7,
        d3: 0x465c,
        d4: [0xa9, 0x86, 0xdf, 0x65, 0xf7, 0x8f, 0x8f, 0xeb],
    };
    engine
        .with_slot(font, |slot| {
            let layout = create_layout(engine.factories.as_ref().unwrap(), slot, text).unwrap();
            let one: Com<Layout> = ffi::query(&layout, &ID, "reference layout").unwrap();
            let range = || Range {
                start: 0,
                length: u32::try_from(text.encode_utf16().count()).unwrap(),
            };
            let mut metrics = DwriteTextMetrics::default();
            // SAFETY: exact IDWriteTextLayout1 ABI and a live owning COM reference.
            unsafe {
                let table = &*(*one.as_ptr()).vtbl;
                assert!(com_ok((table.set_style)(
                    one.as_ptr().cast(),
                    style_of(font.slant),
                    range()
                )));
                assert!(com_ok((table.set_spacing)(
                    one.as_ptr().cast(),
                    0.0,
                    native_float(font.tracking).unwrap(),
                    0.0,
                    range()
                )));
                assert!(com_ok(((*(*layout.as_ptr()).vtbl).get_metrics)(
                    layout.as_ptr(),
                    &raw mut metrics
                )));
            }
            f64::from(metrics.width_including_trailing_whitespace)
        })
        .unwrap()
}

fn shape(engine: &DirectWriteEngine, text: &str, font: &FontSpec) -> Vec<Run> {
    let factories = engine.factories.as_ref().unwrap();
    let prepared = Prepared::new(factories, text).unwrap();
    let shaper = Shaper::new(factories).unwrap();
    engine
        .with_slot(font, |slot| {
            let base = BaseFont::new(slot, font).unwrap();
            shaper
                .shape(&prepared, 0..prepared.utf16().len(), &base, font)
                .unwrap()
        })
        .unwrap()
}

#[test]
fn paragraph_glyphs_match_independent_native_layout_widths() {
    let engine = engine();
    for text in [
        "office affine",
        "אבג דהו",
        "العربية 123",
        "नमस्ते",
        "a\u{301} e\u{301}",
        "abc 😀 العربية אבג",
        "\u{202e}abc def\u{202c}",
        "\u{202d}אבג דהו\u{202c}",
    ] {
        let font = FontSpec {
            size: 21.0,
            ..FontSpec::DEFAULT
        };
        let runs = shape(&engine, text, &font);
        let actual: f64 = runs.iter().map(Run::width).sum();
        let expected = reference_width(&engine, text, &font);
        assert!(
            (actual - expected).abs() < 0.03,
            "{text:?}: {actual} vs {expected}"
        );
        assert_eq!(runs.first().unwrap().source.start, 0);
        assert_eq!(runs.last().unwrap().source.end, text.encode_utf16().count());
        assert!(
            runs.windows(2)
                .all(|pair| pair[0].source.end == pair[1].source.start)
        );
    }
}

#[test]
fn paragraph_glyphs_preserve_style_tracking_and_diacritic_clusters() {
    let engine = engine();
    for tracking in [-0.75, 0.0, 2.0] {
        for slant in [Slant::Upright, Slant::Italic] {
            let font = FontSpec {
                size: 19.0,
                weight: Weight::Bold,
                slant,
                tracking,
                ..FontSpec::DEFAULT
            };
            for text in ["office", "a\u{301}b", "العربية", "abc 😀 def"] {
                let runs = shape(&engine, text, &font);
                let actual: f64 = runs.iter().map(Run::width).sum();
                let expected = reference_width(&engine, text, &font);
                assert!(
                    (actual - expected).abs() < 0.03,
                    "{text:?}, {font:?}: {actual} vs {expected}"
                );
            }
        }
    }
}

#[test]
fn paragraph_glyphs_keep_original_source_offsets_and_override_levels() {
    let engine = engine();
    let factories = engine.factories.as_ref().unwrap();
    let shaper = Shaper::new(factories).unwrap();
    let font = FontSpec::DEFAULT;
    for (text, visible, parity) in [
        ("\u{202e}prefix abc\u{202c}", "abc", 1),
        ("\u{202d}prefix אבג\u{202c}", "אבג", 0),
    ] {
        let prepared = Prepared::new(factories, text).unwrap();
        let byte = text.find(visible).unwrap();
        let start = text[..byte].encode_utf16().count();
        let range = start..start + visible.encode_utf16().count();
        engine
            .with_slot(&font, |slot| {
                let base = BaseFont::new(slot, &font).unwrap();
                let runs = shaper
                    .shape(&prepared, range.clone(), &base, &font)
                    .unwrap();
                assert_eq!(runs.len(), 1);
                assert_eq!(runs[0].source, range);
                assert_eq!(runs[0].level % 2, parity);
                assert_eq!(runs[0].clusters.len(), visible.encode_utf16().count());
                assert!(runs[0].glyphs.iter().all(|&glyph| glyph != 0));
            })
            .unwrap();
    }
}

#[test]
fn paragraph_glyphs_native_fallback_keeps_supplementary_clusters() {
    let engine = engine();
    let font = FontSpec {
        design: FontDesign::Mono,
        ..FontSpec::DEFAULT
    };
    let text = "a😀ב";
    let runs = shape(&engine, text, &font);
    assert!(
        runs.iter()
            .all(|run| run.glyphs.iter().all(|&glyph| glyph != 0))
    );
    let emoji = runs.iter().find(|run| run.source.contains(&1)).unwrap();
    assert!(emoji.source.end >= 3);
    assert_eq!(
        emoji.clusters[1 - emoji.source.start],
        emoji.clusters[2 - emoji.source.start]
    );
    let width: f64 = runs.iter().map(Run::width).sum();
    assert!((width - reference_width(&engine, text, &font)).abs() < 0.03);
}

#[test]
fn paragraph_glyphs_reject_split_surrogates_and_invalid_metrics() {
    let engine = engine();
    let factories = engine.factories.as_ref().unwrap();
    let prepared = Prepared::new(factories, "a😀ב").unwrap();
    let shaper = Shaper::new(factories).unwrap();
    engine
        .with_slot(&FontSpec::DEFAULT, |slot| {
            let base = BaseFont::new(slot, &FontSpec::DEFAULT).unwrap();
            for range in [2..3, 0..2, 4..5, Range { start: 3, end: 1 }] {
                assert!(
                    shaper
                        .shape(&prepared, range, &base, &FontSpec::DEFAULT)
                        .is_err()
                );
            }
            for font in [
                FontSpec {
                    size: f64::NAN,
                    ..FontSpec::DEFAULT
                },
                FontSpec {
                    size: 0.0,
                    ..FontSpec::DEFAULT
                },
                FontSpec {
                    tracking: f64::INFINITY,
                    ..FontSpec::DEFAULT
                },
            ] {
                assert!(shaper.shape(&prepared, 0..4, &base, &font).is_err());
            }
            assert!(
                shaper
                    .shape(&prepared, 0..0, &base, &FontSpec::DEFAULT)
                    .unwrap()
                    .is_empty()
            );
            assert!(
                !shaper
                    .shape(&prepared, 0..4, &base, &FontSpec::DEFAULT)
                    .unwrap()
                    .is_empty()
            );
        })
        .unwrap();
}

#[test]
fn paragraph_glyphs_retry_real_native_insufficient_buffer_without_partial_output() {
    let engine = engine();
    let factories = engine.factories.as_ref().unwrap();
    let text = "office affinity";
    let prepared = Prepared::new(factories, text).unwrap();
    let shaper = Shaper::new(factories).unwrap();
    engine
        .with_slot(&FontSpec::DEFAULT, |slot| {
            let base = BaseFont::new(slot, &FontSpec::DEFAULT).unwrap();
            let face = create_face(&base.font).unwrap();
            let input = Input {
                text: prepared.utf16(),
                script: prepared.scripts[0],
                level: 0,
                face: &face,
                em_size: 13.0,
                locale: &factories.locale,
            };
            let retried = shaper.produce(&input, 1.25, 1).unwrap();
            let ample = shaper.produce(&input, 1.25, 128).unwrap();
            assert!(retried.glyphs.len() > 1);
            assert_eq!(retried.glyphs, ample.glyphs);
            assert_eq!(retried.clusters, ample.clusters);
            assert_eq!(retried.advances, ample.advances);
            assert_eq!(retried.offsets, ample.offsets);
        })
        .unwrap();
}

#[test]
fn paragraph_glyphs_fallback_callback_failure_propagates_and_recovers() {
    unsafe extern "system" fn refuse(
        _this: *mut Source,
        _position: u32,
        _text: *mut *const u16,
        _length: *mut u32,
    ) -> Hresult {
        super::super::FAILED
    }
    let engine = engine();
    let factories = engine.factories.as_ref().unwrap();
    let shaper = Shaper::new(factories).unwrap();
    let table = super::super::SourceVtbl {
        text_at: refuse,
        ..super::super::callbacks::source_vtbl()
    };
    let prepared = Prepared::new(factories, "abc").unwrap();
    // SAFETY: table has the same ABI and outlives the prepared source. Only the
    // actual fallback's source callback changes; analysis has already finished.
    unsafe {
        (*prepared.source.as_ptr()).vtbl = &raw const table;
    }
    engine
        .with_slot(&FontSpec::DEFAULT, |slot| {
            let base = BaseFont::new(slot, &FontSpec::DEFAULT).unwrap();
            assert!(matches!(
                shaper.shape(&prepared, 0..3, &base, &FontSpec::DEFAULT),
                Err(TextError::Preparation {
                    operation: "MapCharacters",
                    ..
                })
            ));
            let recovered = Prepared::new(factories, "abc").unwrap();
            assert!(
                !shaper
                    .shape(&recovered, 0..3, &base, &FontSpec::DEFAULT)
                    .unwrap()
                    .is_empty()
            );
        })
        .unwrap();
}

#[test]
fn paragraph_glyphs_keep_custom_unknown_and_missing_font_policies() {
    use bunny_ui::text_engine::Family;
    let engine = engine();
    let bytes = std::fs::read("C:\\Windows\\Fonts\\arial.ttf").expect("native Arial fixture");
    assert!(engine.register_font(&bytes));
    for family in [
        Family::named("Arial"),
        Family::named("Bunny missing paragraph font"),
    ] {
        let font = FontSpec {
            family,
            ..FontSpec::DEFAULT
        };
        for text in ["office 😀", "\u{10fffd}"] {
            let runs = shape(&engine, text, &font);
            let width: f64 = runs.iter().map(Run::width).sum();
            assert!((width - reference_width(&engine, text, &font)).abs() < 0.03);
            if text == "\u{10fffd}" {
                assert!(
                    runs.iter().any(|run| run.glyphs.contains(&0)),
                    "native missing-glyph result is retained"
                );
            }
        }
    }
}

#[test]
fn paragraph_glyphs_owned_faces_and_offsets_survive_the_shaper() {
    const UNKNOWN: Guid = Guid {
        d1: 0,
        d2: 0,
        d3: 0,
        d4: [0xc0, 0, 0, 0, 0, 0, 0, 0x46],
    };
    let runs = {
        let engine = engine();
        shape(&engine, "a\u{327}\u{301} العربية 😀", &FontSpec::DEFAULT)
    };
    for run in &runs {
        let _identity: Com<ffi::Face> = ffi::query(&run.face, &UNKNOWN, "live glyph face").unwrap();
        assert!(run.em_size > 0.0 && run.em_size.is_finite());
        assert_eq!(run.offsets.len(), run.glyphs.len());
        assert!(
            run.offsets
                .iter()
                .all(|offset| offset.advance.is_finite() && offset.ascender.is_finite())
        );
    }
    assert!(
        runs.iter()
            .flat_map(|run| &run.offsets)
            .any(|offset| offset.advance != 0.0 || offset.ascender != 0.0),
        "positioned combining glyphs retain their native offsets"
    );
}
