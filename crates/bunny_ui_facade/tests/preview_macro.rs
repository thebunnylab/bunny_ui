//! `#[preview]` in an app that forbids `unsafe`: the symbol it exports is
//! the facade's own `unsafe(no_mangle)`, never the app's, and the entry
//! resolves by name through the hot crate's door — in the program itself
//! here, in a generation under `bunny preview`.

#![forbid(unsafe_code)]
#![cfg(not(target_arch = "wasm32"))]

use bunny_ui::__private::hot::preview::{PreviewError, SYMBOL_PREFIX, preview_entry, symbol};
use bunny_ui::prelude::*;
use bunny_ui::preview::Device;

/// The plainest preview: a bare view, under the default traits.
#[bunny_ui::preview]
fn home() -> impl View {
    text("Hello")
}

/// Named, scoped and labelled, with traits of its own — and `pub`.
#[bunny_ui::preview(name = "Home", scope = "Screens", example = "Dark phone")]
pub fn home_dark() -> Preview<impl View> {
    Preview::new(text("Hi")).dark().device(Device::IPhone16)
}

/// Under another attribute, with the crate named.
#[cfg(not(target_arch = "wasm32"))]
#[bunny_ui::preview(crate = ::bunny_ui)]
pub(crate) fn attributed() -> impl View {
    text("x")
}

fn fallback() -> impl View {
    text("fallback")
}

bunny_ui::declare_preview!(fallback, bunny_preview__fallback, "Fallback", "", "fallback");

#[test]
fn the_symbol_is_the_prefix_and_the_name() {
    assert_eq!(SYMBOL_PREFIX, "bunny_preview__");
    assert_eq!(symbol("home"), "bunny_preview__home");
}

#[test]
fn every_marked_function_resolves_by_name_with_its_meta_and_traits() {
    let home = preview_entry("home").expect("home exports its entry");
    assert_eq!(home.meta.ident, "home");
    assert_eq!(home.meta.name, "home", "the name defaults to the ident");
    assert_eq!(home.meta.example, "home", "so does the example");
    assert_eq!(home.meta.scope, "");
    assert!(home.meta.file.ends_with("preview_macro.rs"), "{}", home.meta.file);
    assert_eq!(home.meta.line, 14, "the attribute's own line");
    assert_eq!(home.meta.module, "preview_macro");
    assert_eq!((home.make)().traits, Traits::default());

    let dark = preview_entry("home_dark").expect("home_dark exports its entry");
    assert_eq!((dark.meta.name, dark.meta.scope, dark.meta.example), ("Home", "Screens", "Dark phone"));
    let output = (dark.make)();
    assert_eq!(output.traits.scheme, Scheme::Dark);
    assert_eq!(output.traits.size, Sizing::Device(Device::IPhone16));

    assert!(preview_entry("attributed").is_ok());
    let fallback = preview_entry("fallback").expect("the declarative entry");
    assert_eq!(fallback.meta.name, "Fallback");

    assert_eq!(
        preview_entry("nothing_of_the_kind").unwrap_err(),
        PreviewError::NoSuchPreview {
            ident: String::from("nothing_of_the_kind"),
            symbol: String::from("bunny_preview__nothing_of_the_kind"),
        }
    );
}

/// The view a preview builds is a real one: it lays out.
#[test]
fn the_output_renders() {
    let runtime = Runtime::new();
    let output = (preview_entry("home").unwrap().make)();
    let display = runtime.display_frame(&output.root, Size { width: 100.0, height: 40.0 });
    assert!(!display.is_empty());
}
