use super::*;
use crate::text::DirectWriteEngine;

#[test]
fn paragraph_raster_native_surface_preserves_empty_alpha_and_records_capabilities() {
    let engine = DirectWriteEngine::new();
    let surface = Surface::new(engine.factories.as_ref().unwrap(), 12, 14, 2.0).unwrap();
    assert_eq!(surface.paint(|_| Ok(())).unwrap(), vec![0; 12 * 14 * 4]);
    let four: Option<Com<ID2D1RenderTarget>> =
        ffi::query_optional(&surface.target, &CONTEXT4, "native context4").unwrap();
    let seven: Option<Com<ID2D1RenderTarget>> =
        ffi::query_optional(&surface.target, &CONTEXT7, "native context7").unwrap();
    eprintln!(
        "native WIC target: ID2D1DeviceContext4={}, ID2D1DeviceContext7={}",
        four.is_some(),
        seven.is_some()
    );
}

#[test]
fn paragraph_raster_surface_refusal_and_draw_failure_do_not_publish_pixels() {
    let engine = DirectWriteEngine::new();
    let factories = engine.factories.as_ref().unwrap();
    for (width, height, scale) in [
        (u32::MAX, 2, 1.0),
        (0, 3, 1.0),
        (3, 3, 0.0),
        (3, 3, f32::NAN),
    ] {
        assert!(Surface::new(factories, width, height, scale).is_err());
    }
    let surface = Surface::new(factories, 8, 8, 1.0).unwrap();
    assert!(
        surface
            .paint(|_| Err(failure("injected draw refusal")))
            .is_err()
    );
    assert_eq!(surface.paint(|_| Ok(())).unwrap(), vec![0; 8 * 8 * 4]);
}

#[test]
fn paragraph_raster_native_end_draw_failure_is_reported_and_later_draw_recovers() {
    let engine = DirectWriteEngine::new();
    let surface = Surface::new(engine.factories.as_ref().unwrap(), 8, 8, 1.0).unwrap();
    let result = surface.paint(|target| {
        // SAFETY: the SDK explicitly defines a repeated BeginDraw as a native
        // error reported by EndDraw. This is API fault injection, not invalid memory.
        unsafe {
            ((*(*target.as_ptr()).vtbl).begin_draw)(target.as_ptr());
        }
        Ok(())
    });
    assert!(matches!(
        result,
        Err(TextError::Rasterization {
            operation: "EndDraw",
            ..
        })
    ));
    assert_eq!(surface.paint(|_| Ok(())).unwrap(), vec![0; 8 * 8 * 4]);
}

#[test]
fn paragraph_raster_unwind_balances_the_pending_native_batch() {
    let engine = DirectWriteEngine::new();
    let surface = Surface::new(engine.factories.as_ref().unwrap(), 8, 8, 1.0).unwrap();
    let panic = std::panic::catch_unwind(|| surface.paint(|_| panic!("draw callback unwound")));
    assert!(panic.is_err());
    assert_eq!(surface.paint(|_| Ok(())).unwrap(), vec![0; 8 * 8 * 4]);
}
