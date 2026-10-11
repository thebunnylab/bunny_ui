//! Compare retained editor geometry with the actual native line that draws it.
use super::*;
use bunny_ui::prelude::*;
use std::rc::Rc;

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTLineGetOffsetForStringIndex(line: CTLineRef, index: isize, secondary: *mut f64) -> f64;
}

fn native_offset(engine: &CoreTextEngine, text: &str, font: &FontSpec, index: usize) -> f64 {
    let index = isize::try_from(index).expect("small fixture");
    unsafe {
        let line = make_line(text, engine.font(font), font);
        assert!(!line.is_null());
        let result = CTLineGetOffsetForStringIndex(line, index, std::ptr::null_mut());
        CFRelease(line);
        result
    }
}

fn assert_native_caret(text: &str, utf16: usize, pointer: bool) {
    let engine = Rc::new(CoreTextEngine::new());
    let runtime = Runtime::new().text_engine(engine.clone());
    let content = State::new(text.to_owned());
    let view = text_field("", content.binding())
        .frame(600.0, 30.0)
        .id("field");
    runtime.render_stable(&view);
    let layout = runtime.layout(
        &view,
        Proposal::exact(Size {
            width: 600.0,
            height: 100.0,
        }),
    );
    let field = &layout.fields[0];
    assert!(runtime.focus_named("field"));
    let native = field.text_origin.x + native_offset(&engine, text, &field.font, utf16);
    if pointer {
        let y = field.run.origin.y + field.line_height / 2.0;
        assert_eq!(
            runtime.ime_index_at(native, y),
            Some(utf16),
            "native pointer index for {text:?}"
        );
    } else {
        let actual = runtime.ime_rect_for(utf16).expect("focused field").origin.x;
        assert!(
            (actual - native).abs() < 0.01,
            "native caret for {text:?} at UTF-16 {utf16}: expected {native}, got {actual}"
        );
    }
}

#[test]
fn mixed_geometry_ltr_caret_matches_native_line() {
    assert_native_caret("abc אבג def", 5, false);
}

#[test]
fn mixed_geometry_rtl_caret_matches_native_line() {
    assert_native_caret("אבג abc 123", 5, false);
}

#[test]
fn mixed_geometry_pointer_round_trip_uses_native_line() {
    assert_native_caret("abc אבג def", 5, true);
}
