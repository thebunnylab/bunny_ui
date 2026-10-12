//! Checked shaping of visual runs against the unchanged paragraph source.
//!
//! ## Wiring
//! `FriBidi` supplies line-specific visual runs in paragraph byte coordinates.
//! `HarfBuzz` receives the whole paragraph plus each logical font fragment. Only
//! the exported cluster indices are rebased to the visible line for editing.

use super::{
    Face, FaceKey, FreeTypeEngine, HbGlyphInfo, HbGlyphPos, Shaped, TextError, bidi, c_int, c_uint,
    c_void, hb_buffer_add_utf8, hb_buffer_allocation_successful, hb_buffer_create,
    hb_buffer_destroy, hb_buffer_get_glyph_infos, hb_buffer_get_glyph_positions,
    hb_buffer_guess_segment_properties, hb_buffer_set_direction, hb_font_get_empty, hb_shape_full,
};
use std::ops::Range;
use std::ptr::NonNull;

pub(super) const fn preparation(operation: &'static str) -> TextError {
    TextError::Preparation {
        engine: "HarfBuzz/FreeType",
        operation,
    }
}

pub(super) const fn bidi_error(error: bidi::Error) -> TextError {
    match error {
        bidi::Error::IndexOverflow => TextError::IndexOverflow { engine: "FriBidi" },
        bidi::Error::ResolutionFailed => TextError::Preparation {
            engine: "FriBidi",
            operation: "paragraph or line resolution",
        },
        bidi::Error::InvalidRange => {
            TextError::Range(bunny_ui::text_source::TextRangeError::InvalidRange)
        }
    }
}

pub(super) struct Buffer(NonNull<c_void>);
impl Buffer {
    fn new() -> Result<Self, TextError> {
        // SAFETY: new owned buffer; even allocation failure returns a live
        // empty singleton, which must be checked with the native status API.
        let buffer = Self(
            NonNull::new(unsafe { hb_buffer_create() })
                .ok_or_else(|| preparation("hb_buffer_create"))?,
        );
        buffer.check()?;
        Ok(buffer)
    }

    pub(super) fn check(&self) -> Result<(), TextError> {
        // SAFETY: the RAII owner keeps the buffer live for this call.
        if unsafe { hb_buffer_allocation_successful(self.0.as_ptr()) } == 0 {
            return Err(preparation("hb_buffer_allocation_successful"));
        }
        Ok(())
    }

    fn shape(&self, face: Face) -> Result<(), TextError> {
        // SAFETY: the API returns the immutable, process-lifetime empty font.
        // A failed native font allocation must not become zero-width glyphs.
        if face.hb.is_null() || face.hb == unsafe { hb_font_get_empty() } {
            return Err(preparation("hb_ft_font_create_referenced"));
        }
        self.check()?;
        // SAFETY: live sized font and populated buffer; default features and
        // shapers retain no borrowed Rust storage after this synchronous call.
        let ok = unsafe {
            hb_buffer_guess_segment_properties(self.0.as_ptr());
            hb_shape_full(
                face.hb,
                self.0.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
            )
        };
        self.check()?;
        if ok == 0 {
            return Err(preparation("hb_shape_full"));
        }
        Ok(())
    }

    fn glyphs(&self) -> Result<(&[HbGlyphInfo], &[HbGlyphPos]), TextError> {
        let mut count = 0;
        let mut position_count = 0;
        // SAFETY: live shaped buffer. Native arrays stay valid while this
        // owner is borrowed and no buffer mutation occurs.
        let (infos, positions) = unsafe {
            (
                hb_buffer_get_glyph_infos(self.0.as_ptr(), &raw mut count),
                hb_buffer_get_glyph_positions(self.0.as_ptr(), &raw mut position_count),
            )
        };
        self.check()?;
        if count != position_count || infos.is_null() || positions.is_null() || count == 0 {
            return Err(preparation("glyph arrays"));
        }
        let length = usize::try_from(count).map_err(|_| overflow())?;
        // SAFETY: both native array lengths equal the validated count.
        Ok(unsafe {
            (
                std::slice::from_raw_parts(infos, length),
                std::slice::from_raw_parts(positions, length),
            )
        })
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: releases exactly this owned native reference.
        unsafe { hb_buffer_destroy(self.0.as_ptr()) };
    }
}

const fn overflow() -> TextError {
    TextError::IndexOverflow { engine: "HarfBuzz" }
}

impl FreeTypeEngine {
    pub(super) fn shape_runs(
        &self,
        text: &str,
        visible: Range<usize>,
        key: &FaceKey,
        tracking: i64,
        directions: &[bidi::Run],
    ) -> Result<(Vec<Shaped>, i64), TextError> {
        let text_length = c_int::try_from(text.len()).map_err(|_| overflow())?;
        let primary = self.face(key).ok_or_else(|| preparation("primary face"))?;
        let mut shaped = Vec::new();
        let mut pen = 0_i64;
        for directional in directions {
            let mut runs = self.face_runs(text, directional.bytes.clone(), key, primary);
            let rtl = directional.direction == bidi::Direction::RightToLeft;
            // Reverse fragment order, never logical UTF-8 or native clusters.
            if rtl {
                runs.reverse();
            }
            for (face, range) in runs {
                let start = c_uint::try_from(range.start).map_err(|_| overflow())?;
                let length = c_int::try_from(range.len()).map_err(|_| overflow())?;
                let buffer = Buffer::new()?;
                // SAFETY: range is a validated logical subrange of text. The
                // entire paragraph supplies pre/post context without copies.
                unsafe {
                    hb_buffer_add_utf8(
                        buffer.0.as_ptr(),
                        text.as_ptr().cast(),
                        text_length,
                        start,
                        length,
                    );
                    hb_buffer_set_direction(buffer.0.as_ptr(), if rtl { 5 } else { 4 });
                }
                buffer.shape(face)?;
                let (infos, positions) = buffer.glyphs()?;
                for (info, position) in infos.iter().zip(positions) {
                    let cluster = usize::try_from(info.cluster).map_err(|_| overflow())?;
                    if !range.contains(&cluster)
                        || !visible.contains(&cluster)
                        || !text.is_char_boundary(cluster)
                        || range.end > visible.end
                    {
                        return Err(preparation("glyph cluster range"));
                    }
                    let advance = i64::from(position.x_advance)
                        .checked_add(tracking)
                        .ok_or_else(overflow)?;
                    shaped.push(Shaped {
                        face,
                        glyph: info.codepoint,
                        cluster: u32::try_from(cluster - visible.start).map_err(|_| overflow())?,
                        fragment_end: range.end - visible.start,
                        rtl,
                        pen,
                        advance,
                        x: pen
                            .checked_add(i64::from(position.x_offset))
                            .ok_or_else(overflow)?,
                        y: i64::from(position.y_offset),
                    });
                    pen = pen.checked_add(advance).ok_or_else(overflow)?;
                }
            }
        }
        Ok((shaped, pen))
    }
}

#[cfg(test)]
#[path = "text_shaping_tests.rs"]
mod tests;
