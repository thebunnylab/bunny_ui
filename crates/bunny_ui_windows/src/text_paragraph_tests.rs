//! DirectWrite's whole-paragraph levels are independent of isolated row layouts.
use super::*;
use std::rc::Rc;

#[path = "../../bunny_ui/tests/support/paragraph_direction.rs"]
mod support;

fn native_levels(engine: &DirectWriteEngine, text: &str) -> Vec<(usize, bool)> {
    let font = FontSpec {
        size: 15.0,
        ..FontSpec::DEFAULT
    };
    engine
        .with_slot(&font, |slot| {
            let layout = create_layout(engine.factories.as_ref().unwrap(), slot, text).unwrap();
            let mut index = 0u32;
            text.char_indices()
                .map(|(byte, character)| {
                    let mut x = 0.;
                    let mut y = 0.;
                    let mut hit = geometry::HitMetrics::default();
                    // SAFETY: layout owns the complete paragraph; the typed native out
                    // parameters are initialized and index names a UTF-16 scalar start.
                    let status = unsafe {
                        ((*(*layout.as_ptr()).vtbl).hit_test_text_position)(
                            layout.as_ptr(),
                            index,
                            0,
                            &raw mut x,
                            &raw mut y,
                            &raw mut hit,
                        )
                    };
                    assert!(
                        com_ok(status),
                        "whole-paragraph native hit failed: {status:#x}"
                    );
                    index += u32::try_from(character.len_utf16()).unwrap();
                    (byte, hit.bidi_level % 2 == 1)
                })
                .collect()
        })
        .unwrap()
}

fn check(text: &str) {
    let engine = Rc::new(DirectWriteEngine::new());
    let levels = native_levels(&engine, text);
    support::directions_match_paragraph(engine, text, 70.0, &levels);
}

#[test]
fn paragraph_plain_latin_direction_control() {
    check("abc def ghi jkl mno");
}
#[test]
fn paragraph_override_survives_soft_wrapped_latin() {
    check("\u{202e}abc def ghi jkl mno\u{202c}");
}
#[test]
fn paragraph_override_survives_soft_wrapped_hebrew() {
    check("\u{202d}אבג דהו זחט יכל מנס\u{202c}");
}
