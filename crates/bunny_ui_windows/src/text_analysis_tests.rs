use super::*;
use crate::ffi::{UnknownVtbl, wide};
use crate::text::{DirectWriteEngine, FontSpec, create_layout, geometry};
use std::sync::atomic::Ordering;

fn engine() -> DirectWriteEngine {
    let engine = DirectWriteEngine::new();
    assert!(
        engine.factories.is_some(),
        "the native DirectWrite factories are required"
    );
    engine
}
fn unit_at(text: &str, character: char) -> usize {
    text[..text.find(character).unwrap()].encode_utf16().count()
}

#[test]
fn paragraph_analysis_matches_independent_whole_layout_levels() {
    let engine = engine();
    let factories = engine.factories.as_ref().unwrap();
    let font = FontSpec {
        size: 15.0,
        ..FontSpec::DEFAULT
    };
    for text in [
        "abc def ghi",
        "\u{202e}abc def ghi\u{202c}",
        "\u{202d}אבג דהו זחט\u{202c}",
        "abc العربية 123 אבג",
        "abc \u{2067}אבג (123)\u{2069} def",
        "אבג (abc) 123 [דהו]",
        "\u{202e}abc 😀 def\u{202c}",
        "\u{202e}abc\u{2029}def",
    ] {
        let prepared = Prepared::new(factories, text).unwrap();
        assert_eq!(prepared.utf16(), text.encode_utf16().collect::<Vec<_>>());
        assert_eq!(prepared.scripts.len(), prepared.utf16().len());
        assert_eq!(prepared.levels.len(), prepared.utf16().len());
        engine.with_slot(&font, |slot| {
            let layout = create_layout(factories, slot, text).unwrap();
            let mut index = 0_u32;
            for character in text.chars() {
                let mut x = 0.0;
                let mut y = 0.0;
                let mut hit = geometry::HitMetrics::default();
                // SAFETY: independent native layout owns the entire paragraph;
                // the initialized out parameters follow the pinned SDK ABI.
                let status = unsafe {
                    ((*(*layout.as_ptr()).vtbl).hit_test_text_position)(
                        layout.as_ptr(), index, 0, &raw mut x, &raw mut y, &raw mut hit,
                    )
                };
                assert!(com_ok(status), "native hit: {status:#x}");
                // Invisible formatting controls can share the neighbouring hit
                // cluster; visible scalar directions are the comparison domain.
                if !matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{2029}') {
                    for offset in 0..character.len_utf16() {
                        let level = prepared.levels[usize::try_from(index).unwrap() + offset];
                        assert_eq!(u32::from(level.resolved), hit.bidi_level,
                            "paragraph {text:?}, scalar {character:?}, unit {index}");
                    }
                }
                index += u32::try_from(character.len_utf16()).unwrap();
            }
        }).unwrap();
    }
}

#[test]
fn paragraph_analysis_retains_override_context_that_a_slice_loses() {
    let engine = engine();
    let factories = engine.factories.as_ref().unwrap();
    for (whole, visible, expected) in [
        ("\u{202e}prefix abc\u{202c}", "abc", 1),
        ("\u{202d}prefix אבג\u{202c}", "אבג", 0),
    ] {
        let full = Prepared::new(factories, whole).unwrap();
        let isolated = Prepared::new(factories, visible).unwrap();
        let first = whole.find(visible).unwrap();
        let unit = whole[..first].encode_utf16().count();
        assert_eq!(full.levels[unit].resolved % 2, expected);
        assert_ne!(
            full.levels[unit].resolved % 2,
            isolated.levels[0].resolved % 2
        );
    }
}

#[test]
fn paragraph_analysis_keeps_script_spans_and_surrogate_units() {
    let engine = engine();
    let text = "abc אבג العربية 😀";
    let prepared = Prepared::new(engine.factories.as_ref().unwrap(), text).unwrap();
    let latin = prepared.scripts[0];
    let hebrew = prepared.scripts[unit_at(text, 'א')];
    let arabic = prepared.scripts[unit_at(text, 'ا')];
    assert_ne!(latin.script, hebrew.script);
    assert_ne!(hebrew.script, arabic.script);
    let emoji = unit_at(text, '😀');
    assert_eq!(prepared.scripts[emoji], prepared.scripts[emoji + 1]);
    assert_eq!(prepared.levels[emoji], prepared.levels[emoji + 1]);
}

#[test]
fn paragraph_analysis_source_windows_count_utf16_not_scalars() {
    let text: Arc<[u16]> = "a😀ב".encode_utf16().collect::<Vec<_>>().into();
    let source = Source::new(Arc::clone(&text), wide("en-US").into()).unwrap();
    let mut block = std::ptr::null();
    let mut length = 0;
    // SAFETY: these calls exercise the real COM callbacks with live storage,
    // including a valid code-unit window starting inside a surrogate pair.
    unsafe {
        let vtbl = &*(*source.as_ptr()).vtbl;
        assert_eq!(
            (vtbl.text_at)(source.as_ptr(), 2, &raw mut block, &raw mut length),
            OK
        );
        assert_eq!(length, 2);
        assert_eq!(std::slice::from_raw_parts(block, 2), &text[2..]);
        assert_eq!(
            (vtbl.text_before)(source.as_ptr(), 2, &raw mut block, &raw mut length),
            OK
        );
        assert_eq!(length, 2);
        assert_eq!(std::slice::from_raw_parts(block, 2), &text[..2]);
        for position in [4, 5, u32::MAX] {
            assert_eq!(
                (vtbl.text_at)(source.as_ptr(), position, &raw mut block, &raw mut length),
                OK
            );
            assert!(block.is_null());
            assert_eq!(length, 0);
        }
        for position in [0, 5, u32::MAX] {
            assert_eq!(
                (vtbl.text_before)(source.as_ptr(), position, &raw mut block, &raw mut length),
                OK
            );
            assert!(block.is_null());
            assert_eq!(length, 0);
        }
    }
}

#[test]
fn paragraph_analysis_interfaces_balance_references_and_refuse_unknown_ids() {
    let source = Source::new(vec![97_u16].into(), wide("en-US").into()).unwrap();
    let sink = Sink::new(1).unwrap();
    let unsupported = Guid {
        d1: 123,
        d2: 0,
        d3: 0,
        d4: [0; 8],
    };
    let unknown = Guid {
        d1: 0,
        d2: 0,
        d3: 0,
        d4: [0xc0, 0, 0, 0, 0, 0, 0, 0x46],
    };
    for pointer in [
        source.as_ptr().cast::<c_void>(),
        sink.as_ptr().cast::<c_void>(),
    ] {
        // SAFETY: both owned interfaces have the exact IUnknown prefix. The
        // temporary queried identity is released while its first owner lives.
        unsafe {
            let vtbl = &**pointer.cast::<*const UnknownVtbl>();
            let mut answer = std::ptr::null_mut();
            assert_eq!(
                (vtbl.query_interface)(pointer, &raw const unknown, &raw mut answer),
                OK
            );
            assert_eq!(answer, pointer);
            assert_eq!((vtbl.release)(answer), 1);
            assert_eq!(
                (vtbl.query_interface)(pointer, &raw const unsupported, &raw mut answer),
                NO_INTERFACE
            );
            assert!(answer.is_null());
        }
    }
    // SAFETY: both initial owners are still live.
    unsafe {
        assert_eq!((*source.as_ptr()).references.load(Ordering::Relaxed), 1);
        assert_eq!((*sink.as_ptr()).references.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn paragraph_analysis_sink_rejects_invalid_ranges_without_partial_writes() {
    let sink = Sink::new(2).unwrap();
    let script = Script {
        script: 7,
        shapes: 0,
    };
    // SAFETY: initialized ABI objects and valid out-of-range numeric probes.
    unsafe {
        let vtbl = &*(*sink.as_ptr()).vtbl;
        assert_eq!(
            (vtbl.script)(sink.as_ptr(), 1, 2, &raw const script),
            INVALID_ARGUMENT
        );
        assert_eq!(
            (vtbl.bidi)(sink.as_ptr(), u32::MAX, 2, 1, 1),
            INVALID_ARGUMENT
        );
        {
            let output = (*sink.as_ptr()).output.lock().unwrap();
            assert!(output.scripts.iter().all(Option::is_none));
            assert!(output.levels.iter().all(Option::is_none));
            drop(output);
        }
        assert_eq!((vtbl.script)(sink.as_ptr(), 0, 2, &raw const script), OK);
        assert_eq!((vtbl.bidi)(sink.as_ptr(), 0, 2, 1, 2), OK);
        let output = (*sink.as_ptr()).output.lock().unwrap();
        assert_eq!(output.scripts, vec![Some(script); 2]);
        assert_eq!(
            output.levels,
            vec![
                Some(Levels {
                    explicit: 1,
                    resolved: 2
                });
                2
            ]
        );
    }
}

unsafe extern "system" fn refuse_text(
    _this: *mut Source,
    _position: u32,
    _text: *mut *const u16,
    _length: *mut u32,
) -> Hresult {
    FAILED
}

#[test]
fn paragraph_analysis_callback_failure_propagates_and_a_later_call_recovers() {
    let engine = engine();
    let factories = engine.factories.as_ref().unwrap();
    let table = SourceVtbl {
        text_at: refuse_text,
        ..callbacks::source_vtbl()
    };
    let source = Source::new(vec![97_u16].into(), wide("en-US").into()).unwrap();
    // SAFETY: the replacement table has the same ABI and outlives the source;
    // only one callback refuses its native operation with a real HRESULT.
    unsafe {
        (*source.as_ptr()).vtbl = &raw const table;
    }
    assert!(matches!(
        analyze(factories, &source),
        Err(TextError::Preparation {
            operation: "AnalyzeScript",
            ..
        })
    ));
    assert!(Prepared::new(factories, "abc").is_ok());
}

#[test]
fn paragraph_analysis_empty_source_has_no_native_cells() {
    let engine = engine();
    let empty = Prepared::new(engine.factories.as_ref().unwrap(), "").unwrap();
    assert!(empty.utf16().is_empty());
    assert!(empty.scripts.is_empty());
    assert!(empty.levels.is_empty());
    assert_eq!(std::mem::size_of::<Script>(), 8);
    assert_eq!(std::mem::align_of::<Script>(), 4);
}

#[test]
fn paragraph_analysis_missing_native_output_is_a_failure() {
    let script = Script {
        script: 1,
        shapes: 0,
    };
    let level = Levels {
        explicit: 0,
        resolved: 0,
    };
    for (output, expected) in [
        (
            Output {
                scripts: vec![Some(script), None],
                levels: vec![Some(level); 2],
            },
            "script coverage",
        ),
        (
            Output {
                scripts: vec![Some(script); 2],
                levels: vec![None, Some(level)],
            },
            "bidi coverage",
        ),
    ] {
        assert!(
            matches!(output.finish(), Err(TextError::Preparation { operation, .. }) if operation == expected)
        );
    }
}
