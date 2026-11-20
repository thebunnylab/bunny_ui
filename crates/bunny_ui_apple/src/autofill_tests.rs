use bunny_ui::autofill::SavedPasswordError;
use bunny_ui::text_input::Autocomplete;

use super::{Kit, authentication_services, content_type_name, outcome_of_error};

/// Every value a field can say has a constant in both kits, and the two
/// names differ only by the kit's prefix — one table, never two that
/// drift apart.
#[test]
fn every_autofill_value_has_one_name_in_each_kit() {
    for holds in Autocomplete::ALL {
        let app_kit = content_type_name(holds, Kit::AppKit);
        let ui_kit = content_type_name(holds, Kit::UIKit);
        if holds == Autocomplete::Off {
            assert!(app_kit.is_none() && ui_kit.is_none(), "neither kit can say `off`");
            continue;
        }
        let app_kit = app_kit.unwrap_or_else(|| panic!("{holds:?} has no AppKit name")).to_str().expect("ascii");
        let ui_kit = ui_kit.unwrap_or_else(|| panic!("{holds:?} has no UIKit name")).to_str().expect("ascii");
        let app_suffix = app_kit.strip_prefix("NSTextContentType").expect("AppKit prefix");
        let ui_suffix = ui_kit.strip_prefix("UITextContentType").expect("UIKit prefix");
        assert_eq!(app_suffix, ui_suffix, "{holds:?}");
    }
}

/// The boxes a sign-in window is made of name the constants Passwords
/// looks for.
#[test]
fn a_sign_in_form_names_what_passwords_looks_for() {
    let name = |holds| content_type_name(holds, Kit::AppKit).map(|name| name.to_str().expect("ascii"));
    assert_eq!(name(Autocomplete::Username), Some("NSTextContentTypeUsername"));
    assert_eq!(name(Autocomplete::CurrentPassword), Some("NSTextContentTypePassword"));
    assert_eq!(name(Autocomplete::NewPassword), Some("NSTextContentTypeNewPassword"));
    assert_eq!(name(Autocomplete::OneTimeCode), Some("NSTextContentTypeOneTimeCode"));
}

/// A cancel and a request that needed a person it could not ask are the
/// same answer — show the form; every other code is a refusal by name.
#[test]
fn the_system_errors_map_to_their_outcomes() {
    assert_eq!(outcome_of_error(1001, String::from("cancelled")), SavedPasswordError::Declined);
    assert_eq!(outcome_of_error(1005, String::from("not interactive")), SavedPasswordError::Declined);
    for code in [1000, 1002, 1003, 1004] {
        assert_eq!(
            outcome_of_error(code, String::from("why")),
            SavedPasswordError::Failed { code, reason: String::from("why") },
        );
    }
}

/// Whether the runtime has a class by this name registered yet.
fn registered(name: &std::ffi::CStr) -> bool {
    !unsafe { crate::ffi::objc_getClass(name.as_ptr()) }.is_null()
}

/// Linking the Apple half loads neither AuthenticationServices nor the
/// ScreenCaptureKit it drags in; the first request does. The only test
/// that loads it, so the "before" holds whatever runs beside it.
#[test]
fn the_framework_loads_on_the_first_request_never_at_launch() {
    assert!(!registered(c"ASAuthorizationPasswordProvider"), "AuthenticationServices was loaded at launch");
    assert!(!registered(c"SCShareableContent"), "ScreenCaptureKit was loaded at launch");
    assert!(authentication_services(), "this system has AuthenticationServices");
    assert!(registered(c"ASAuthorizationPasswordProvider"), "the provider resolves once loaded");
    assert!(authentication_services(), "a second request finds it loaded");
}
