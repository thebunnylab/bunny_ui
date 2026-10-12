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

#[test]
fn signed_bitmap_pitch_moves_down_from_the_first_row() {
    use crate::text::Face;
    // Aligned ABI fixtures isolate scanline traversal from native font choice.
    let mut face = [0_u64; 24];
    let mut slot = [0_u64; 28];
    let face_pointer = face.as_mut_ptr().cast::<c_void>();
    let slot_pointer = slot.as_mut_ptr().cast::<c_void>();
    let pixels = [64_u8, 128, 255];
    for (pitch, buffer, expected) in [
        (1_i32, pixels.as_ptr(), [64_u8, 128, 255]),
        (-1_i32, pixels.as_ptr().wrapping_add(2), [255_u8, 128, 64]),
    ] {
        // SAFETY: buffers are aligned and exceed every documented slot offset;
        // the gray bitmap has one byte per row in the signed traversal order.
        unsafe {
            face.as_mut_ptr()
                .byte_add(FACE_GLYPH)
                .cast::<*mut c_void>()
                .write(slot_pointer);
            let bitmap = slot.as_mut_ptr().byte_add(SLOT_BITMAP);
            bitmap.byte_add(BITMAP_ROWS).cast::<u32>().write(3);
            bitmap.byte_add(BITMAP_WIDTH).cast::<u32>().write(1);
            bitmap.byte_add(BITMAP_PITCH).cast::<i32>().write(pitch);
            bitmap
                .byte_add(BITMAP_BUFFER)
                .cast::<*const u8>()
                .write(buffer);
            bitmap
                .byte_add(BITMAP_PIXEL_MODE)
                .cast::<u8>()
                .write(PIXEL_MODE_GRAY);
            slot.as_mut_ptr()
                .byte_add(SLOT_BITMAP_TOP)
                .cast::<i32>()
                .write(3);
        }
        let entry = Shaped {
            face: Face {
                ft: face_pointer,
                hb: std::ptr::null_mut(),
            },
            glyph: 0,
            cluster: 0,
            fragment_end: 1,
            rtl: false,
            pen: 0,
            advance: 64,
            x: 0,
            y: 0,
        };
        let mut raster = TextRaster {
            width: 1,
            height: 3,
            baseline: 3,
            rgba: vec![0; 12],
        };
        paint_glyph(&mut raster, &entry, Color::BLACK).unwrap();
        assert_eq!([raster.rgba[3], raster.rgba[7], raster.rgba[11]], expected);
    }
}
