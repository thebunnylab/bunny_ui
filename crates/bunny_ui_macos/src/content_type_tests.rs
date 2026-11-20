use std::ptr::null_mut;

use bunny_ui::text_input::Autocomplete;
use bunny_ui_apple::autofill::{Kit, content_type};

use super::{CONTENT_TYPE, HOLDS, bunny_content_type, sync_content_type};

/// The text of an AppKit constant, read the way the system reads it.
fn text(content: super::Id) -> String {
    unsafe { bunny_ui_apple::ffi::text_of(content) }.expect("a constant with text")
}

/// Every value a field can say resolves to AppKit's own constant on the
/// system the tests run on — the names are the kit's, not guesses — and
/// a sign-in form's four boxes are four different words.
#[test]
fn appkit_names_every_value_a_field_can_say() {
    for holds in Autocomplete::ALL.into_iter().filter(|holds| *holds != Autocomplete::Off) {
        let content = content_type(Some(holds), Kit::AppKit);
        assert!(!content.is_null(), "{holds:?} resolves on this system");
        assert!(!text(content).is_empty(), "{holds:?}");
    }
    let sign_in = [
        Autocomplete::Username,
        Autocomplete::CurrentPassword,
        Autocomplete::NewPassword,
        Autocomplete::OneTimeCode,
    ]
    .map(|holds| text(content_type(Some(holds), Kit::AppKit)));
    let mut distinct = sign_in.to_vec();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 4, "{sign_in:?}");
    assert!(content_type(Some(Autocomplete::Off), Kit::AppKit).is_null(), "`off` says nothing");
    assert!(content_type(None, Kit::AppKit).is_null());
}

/// The view answers `contentType` with whatever the scene last said the
/// focused field holds, and goes quiet when the focus says nothing.
#[test]
fn the_view_answers_what_the_focused_field_holds() {
    HOLDS.with(|slot| slot.set(None));
    CONTENT_TYPE.with(|slot| slot.set(null_mut()));
    sync_content_type(Some(Autocomplete::OneTimeCode));
    let answered = bunny_content_type(null_mut(), std::ptr::null());
    assert_eq!(answered, content_type(Some(Autocomplete::OneTimeCode), Kit::AppKit));
    assert!(!answered.is_null());
    sync_content_type(None);
    assert!(bunny_content_type(null_mut(), std::ptr::null()).is_null());
}
