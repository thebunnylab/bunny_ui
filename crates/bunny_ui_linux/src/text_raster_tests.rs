use super::*;
use crate::text::{FontSpec, FreeTypeEngine};

#[test]
fn a_failed_second_glyph_rejects_the_whole_raster_and_can_retry() {
    let engine = FreeTypeEngine::new();
    let key = FreeTypeEngine::key(&FontSpec::DEFAULT, 1);
    let primary = engine.face(&key).unwrap();
    let (ascent, descent) = engine.line_box(primary);
    let (glyphs, advance) = engine.shape("ab", &key, 0).unwrap();
    let mut loaded = 0;
    let result = render_with(&glyphs, advance, ascent, descent, Color::BLACK, |entry| {
        loaded += 1;
        if loaded == 2 {
            return Err(failed("injected FT_Load_Glyph"));
        }
        // SAFETY: the first glyph exercises real native assembly before refusal.
        assert_eq!(
            unsafe { FT_Load_Glyph(entry.face.ft, entry.glyph, FT_LOAD_RENDER) },
            0
        );
        Ok(())
    });
    assert_eq!(loaded, 2);
    assert!(matches!(result, Err(TextError::Rasterization { .. })));
    let recovered = render(&glyphs, advance, ascent, descent, Color::BLACK)
        .unwrap()
        .unwrap();
    assert!(
        recovered
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] != 0)
    );
}

#[test]
fn invalid_bitmap_dimensions_fail_before_allocation() {
    for value in [f64::NAN, f64::INFINITY, -1.0, f64::from(i32::MAX) + 1.0] {
        assert!(dimension(value).is_err());
    }
    assert_eq!(dimension(1.25).unwrap(), 2);
}
