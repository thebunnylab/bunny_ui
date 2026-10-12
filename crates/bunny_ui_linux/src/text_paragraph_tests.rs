//! `FriBidi` resolves full-paragraph levels before the runtime chooses its rows.
use super::*;
use std::rc::Rc;

#[path = "../../bunny_ui/tests/support/paragraph_direction.rs"]
mod support;

#[link(name = "fribidi")]
unsafe extern "C" {
    fn fribidi_get_bidi_type(character: u32) -> u32;
    fn fribidi_get_bidi_types(text: *const u32, length: c_int, types: *mut u32);
    fn fribidi_get_bracket_types(
        text: *const u32,
        length: c_int,
        types: *const u32,
        brackets: *mut u32,
    );
    fn fribidi_get_par_embedding_levels_ex(
        types: *const u32,
        brackets: *const u32,
        length: c_int,
        base: *mut u32,
        levels: *mut i8,
    ) -> i8;
}

fn native_levels(text: &str) -> Vec<(usize, bool)> {
    assert!(!text.contains('\n'), "each fixture is one paragraph");
    let (bytes, scalars): (Vec<_>, Vec<_>) = text
        .char_indices()
        .map(|(byte, ch)| (byte, u32::from(ch)))
        .unzip();
    let length = c_int::try_from(scalars.len()).unwrap();
    let mut types = vec![0; scalars.len()];
    let mut brackets = vec![0; scalars.len()];
    let mut levels = vec![0; scalars.len()];
    // SAFETY: all arrays have exactly `length` elements matching FriBidi's ABI.
    // The complete paragraph is resolved once, before any soft line extraction.
    let status = unsafe {
        fribidi_get_bidi_types(scalars.as_ptr(), length, types.as_mut_ptr());
        fribidi_get_bracket_types(
            scalars.as_ptr(),
            length,
            types.as_ptr(),
            brackets.as_mut_ptr(),
        );
        let mut base = fribidi_get_bidi_type(u32::from('!'));
        fribidi_get_par_embedding_levels_ex(
            types.as_ptr(),
            brackets.as_ptr(),
            length,
            &raw mut base,
            levels.as_mut_ptr(),
        )
    };
    assert_ne!(status, 0, "whole-paragraph native resolution failed");
    bytes
        .into_iter()
        .zip(levels.into_iter().map(|level| level % 2 != 0))
        .collect()
}

fn check(text: &str) {
    let engine = Rc::new(FreeTypeEngine::new());
    assert!(!engine.library.is_null() && !engine.config.is_null());
    support::directions_match_paragraph(engine, text, 70.0, &native_levels(text));
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
