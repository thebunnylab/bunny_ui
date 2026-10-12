use super::*;
use crate::text::FontSpec;

#[link(name = "harfbuzz")]
unsafe extern "C" {
    fn hb_buffer_get_empty() -> *mut c_void;
    fn hb_buffer_reference(buffer: *mut c_void) -> *mut c_void;
}

#[test]
fn native_empty_buffer_reports_allocation_failure() {
    // SAFETY: HarfBuzz's immutable singleton is valid but unsuccessful; taking
    // a native reference gives the Buffer owner exactly one reference to drop.
    let pointer = unsafe { hb_buffer_reference(hb_buffer_get_empty()) };
    let buffer = Buffer(NonNull::new(pointer).unwrap());
    assert!(matches!(
        buffer.check(),
        Err(TextError::Preparation {
            operation: "hb_buffer_allocation_successful",
            ..
        })
    ));
    assert!(
        Buffer::new().is_ok(),
        "a failure does not poison later buffers"
    );
}

#[test]
fn tracking_overflow_is_not_a_truncated_success() {
    let engine = FreeTypeEngine::new();
    let key = FreeTypeEngine::key(&FontSpec::DEFAULT, 1);
    let runs = bidi::runs("ab").unwrap();
    assert!(matches!(
        engine.shape_runs("ab", 0..2, &key, i64::MAX, &runs),
        Err(TextError::IndexOverflow { .. })
    ));
    assert!(engine.shape_runs("ab", 0..2, &key, 0, &runs).is_ok());
}
