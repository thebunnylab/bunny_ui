//! Fallible bitmap assembly from the same glyphs used by native geometry.
//!
//! ## Wiring
//! Raster storage is private until every glyph succeeds. A failed glyph load,
//! invalid bitmap or allocation rejects the entire line, never partial ink.

use super::{
    BITMAP_BUFFER, BITMAP_PITCH, BITMAP_PIXEL_MODE, BITMAP_ROWS, BITMAP_WIDTH, Color, FACE_GLYPH,
    FT_LOAD_COLOR, FT_LOAD_RENDER, FT_Load_Glyph, PIXEL_MODE_BGRA, PIXEL_MODE_GRAY, SLOT_BITMAP,
    SLOT_BITMAP_LEFT, SLOT_BITMAP_TOP, Shaped, TextError, TextRaster, c_void, geometry, read,
};

const fn failed(operation: &'static str) -> TextError {
    TextError::Rasterization {
        engine: "FreeType",
        operation,
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the preceding finite, nonnegative i32 range check proves the u32 conversion"
)]
fn dimension(value: f64) -> Result<usize, TextError> {
    let rounded = value.ceil();
    if !rounded.is_finite() || rounded < 0.0 || rounded > f64::from(i32::MAX) {
        return Err(failed("bitmap dimensions"));
    }
    usize::try_from(rounded as u32).map_err(|_| failed("bitmap dimensions"))
}

pub(super) fn render(
    glyphs: &[Shaped],
    advance: i64,
    ascent: f64,
    descent: f64,
    color: Color,
) -> Result<Option<TextRaster>, TextError> {
    render_with(glyphs, advance, ascent, descent, color, |entry| {
        // SAFETY: each shaped glyph retains a live face at its original size.
        if unsafe { FT_Load_Glyph(entry.face.ft, entry.glyph, FT_LOAD_RENDER | FT_LOAD_COLOR) } != 0
        {
            return Err(failed("FT_Load_Glyph"));
        }
        Ok(())
    })
}

fn render_with(
    glyphs: &[Shaped],
    advance: i64,
    ascent: f64,
    descent: f64,
    color: Color,
    mut load: impl FnMut(&Shaped) -> Result<(), TextError>,
) -> Result<Option<TextRaster>, TextError> {
    let width = dimension(
        geometry::pixels(advance)
            .ok_or_else(|| failed("bitmap width"))?
            .max(0.0),
    )?;
    let baseline = dimension(ascent)?;
    let height = baseline
        .checked_add(dimension(descent)?)
        .ok_or_else(|| failed("bitmap height"))?;
    if width == 0 || height == 0 {
        return Ok(None);
    }
    let length = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| failed("bitmap capacity"))?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(length)
        .map_err(|_| failed("bitmap allocation"))?;
    rgba.resize(length, 0);
    let mut raster = TextRaster {
        width,
        height,
        baseline,
        rgba,
    };
    for entry in glyphs {
        load(entry)?;
        paint_glyph(&mut raster, entry, color)?;
    }
    Ok(Some(raster))
}

fn paint_glyph(raster: &mut TextRaster, entry: &Shaped, color: Color) -> Result<(), TextError> {
    // SAFETY: called immediately after successful FT_Load_Glyph; the face's
    // slot and bitmap stay live until the next load, after this call returns.
    unsafe {
        let slot: *mut c_void = read(entry.face.ft, FACE_GLYPH);
        if slot.is_null() {
            return Err(failed("glyph slot"));
        }
        let bitmap = slot.cast::<u8>().add(SLOT_BITMAP).cast::<c_void>();
        let rows = read::<u32>(bitmap, BITMAP_ROWS);
        let cols = read::<u32>(bitmap, BITMAP_WIDTH);
        if rows == 0 || cols == 0 {
            return Ok(());
        }
        let buffer: *const u8 = read(bitmap, BITMAP_BUFFER);
        if buffer.is_null() {
            return Err(failed("bitmap buffer"));
        }
        let mode = read::<u8>(bitmap, BITMAP_PIXEL_MODE);
        let channels = match mode {
            PIXEL_MODE_GRAY => 1,
            PIXEL_MODE_BGRA => 4,
            _ => return Err(failed("bitmap pixel mode")),
        };
        let pitch = read::<i32>(bitmap, BITMAP_PITCH);
        let stride = usize::try_from(pitch.unsigned_abs()).map_err(|_| failed("bitmap stride"))?;
        let rows_len = usize::try_from(rows).map_err(|_| failed("bitmap rows"))?;
        let cols_len = usize::try_from(cols).map_err(|_| failed("bitmap columns"))?;
        let row_bytes = cols_len
            .checked_mul(channels)
            .ok_or_else(|| failed("bitmap stride"))?;
        if stride < row_bytes
            || rows_len
                .checked_mul(stride)
                .is_none_or(|n| n > isize::MAX as usize)
        {
            return Err(failed("bitmap stride"));
        }
        let left = i64::from(read::<i32>(slot, SLOT_BITMAP_LEFT));
        let top = i64::from(read::<i32>(slot, SLOT_BITMAP_TOP));
        let pen_x = round_fixed(entry.x) + left;
        let pen_y = i64::try_from(raster.baseline).map_err(|_| failed("bitmap baseline"))?
            - top
            - round_fixed(entry.y);
        let pitch = isize::try_from(pitch).map_err(|_| failed("bitmap stride"))?;
        for row in 0..rows {
            let Ok(target_y) = usize::try_from(pen_y + i64::from(row)) else {
                continue;
            };
            if target_y >= raster.height {
                continue;
            }
            // FreeType's signed pitch is the offset to move down one row.
            let row_offset = isize::try_from(row).map_err(|_| failed("bitmap row"))? * pitch;
            let source = buffer.offset(row_offset);
            for col in 0..cols {
                let Ok(target_x) = usize::try_from(pen_x + i64::from(col)) else {
                    continue;
                };
                if target_x >= raster.width {
                    continue;
                }
                let at = (target_y * raster.width + target_x) * 4;
                let pixel = &mut raster.rgba[at..at + 4];
                let col = usize::try_from(col).map_err(|_| failed("bitmap column"))?;
                if mode == PIXEL_MODE_GRAY {
                    let coverage = u32::from(*source.add(col));
                    let alpha = u8::try_from((coverage * u32::from(color.a) + 127) / 255)
                        .map_err(|_| failed("gray alpha"))?;
                    if alpha > pixel[3] {
                        pixel.copy_from_slice(&[color.r, color.g, color.b, alpha]);
                    }
                } else {
                    // Color emoji keep their native colors. Undo native
                    // premultiplication to honor TextRaster's straight alpha.
                    let b = *source.add(col * 4);
                    let g = *source.add(col * 4 + 1);
                    let r = *source.add(col * 4 + 2);
                    let a = *source.add(col * 4 + 3);
                    if a > pixel[3] {
                        let un = |c: u8| {
                            let alpha = u32::from(a);
                            u8::try_from(((u32::from(c) * 255 + alpha / 2) / alpha.max(1)).min(255))
                                .map_err(|_| failed("color alpha"))
                        };
                        pixel.copy_from_slice(&[un(r)?, un(g)?, un(b)?, a]);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Round 26.6 coordinates away from zero at half a pixel, without float casts.
const fn round_fixed(value: i64) -> i64 {
    value / 64
        + if value % 64 >= 32 {
            1
        } else if value % 64 <= -32 {
            -1
        } else {
            0
        }
}

#[cfg(test)]
#[path = "text_raster_tests.rs"]
mod tests;
