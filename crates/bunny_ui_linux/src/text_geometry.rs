//! Editing coordinates from the exact glyph clusters used by the raster path.
//!
//! GDEF caret coordinates are translated by the shaped glyph origin. When a
//! cluster has no complete caret metadata, its advance is subdivided by scalar;
//! the core filters pointer stops to Unicode grapheme boundaries. Such internal
//! positions are a documented approximation, never a claim of font metadata.

use super::{FreeTypeEngine, Shaped, fixed_26_6};
use bunny_ui::text_engine::FontSpec;
use bunny_ui::text_geometry::{LineGeometry, TextCell};
use std::ffi::{c_uint, c_void};

#[link(name = "harfbuzz")]
unsafe extern "C" {
    fn hb_ot_layout_get_ligature_carets(
        font: *mut c_void,
        direction: c_uint,
        glyph: c_uint,
        start_offset: c_uint,
        caret_count: *mut c_uint,
        caret_array: *mut i32,
    ) -> c_uint;
}

/// Keep all 26.6 fractional bits inside a signed 32-bit pixel coordinate.
/// A line exceeding two billion logical pixels has no supported geometry.
fn pixels(value: i64) -> Option<f64> {
    let whole = i32::try_from(value / 64).ok()?;
    let fraction = i32::try_from(value % 64).ok()?;
    Some(f64::from(whole) + f64::from(fraction) / 64.0)
}

/// Read a complete, ordered set of interior font carets for a single glyph.
fn ligature_edges(glyphs: &[Shaped], count: usize, left: f64, right: f64) -> Option<Vec<f64>> {
    let [glyph] = glyphs else { return None };
    if count < 2 {
        return None;
    }
    let mut length = c_uint::try_from(count - 1).ok()?;
    let mut positions = vec![0; count - 1];
    // SAFETY: the face owns the live hb_font; capacity is exactly the in/out
    // count. The native API fills at most that count, synchronously.
    let total = unsafe {
        hb_ot_layout_get_ligature_carets(
            glyph.face.hb,
            if glyph.rtl { 5 } else { 4 },
            glyph.glyph,
            0,
            &raw mut length,
            positions.as_mut_ptr(),
        )
    };
    if usize::try_from(total).ok()? != count - 1 || total != length {
        return None;
    }
    let mut edges: Vec<_> = positions
        .into_iter()
        .map(|position| pixels(glyph.x + i64::from(position)))
        .collect::<Option<_>>()?;
    edges.sort_by(f64::total_cmp);
    if edges
        .iter()
        .any(|x| !x.is_finite() || *x <= left || *x >= right)
        || edges.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return None;
    }
    Some(edges)
}

pub(super) fn line_geometry(
    engine: &FreeTypeEngine,
    text: &str,
    font: &FontSpec,
) -> Option<LineGeometry> {
    if text.is_empty() {
        return LineGeometry::new(text, Vec::new()).ok();
    }
    let key = FreeTypeEngine::key(font, 1);
    let (glyphs, _) = engine.shape(text, &key, fixed_26_6(font.tracking))?;
    let mut clusters: Vec<_> = glyphs
        .chunk_by(|a, b| a.cluster == b.cluster && a.fragment_end == b.fragment_end)
        .collect();
    clusters.sort_by_key(|group| group[0].cluster);
    let mut cells = Vec::new();
    for (index, group) in clusters.iter().enumerate() {
        let first = group.first()?;
        let last = group.last()?;
        let start = usize::try_from(first.cluster).ok()?;
        let end = clusters.get(index + 1).map_or(first.fragment_end, |next| {
            (next[0].cluster as usize).min(first.fragment_end)
        });
        let source = text.get(start..end)?;
        let scalars: Vec<_> = source.char_indices().collect();
        if scalars.is_empty() {
            return None;
        }
        let a = pixels(first.pen)?;
        let b = pixels(last.pen + last.advance)?;
        let (left, right) = (a.min(b), a.max(b));
        let count = u32::try_from(scalars.len()).ok()?;
        let mut edges = vec![left];
        match ligature_edges(group, scalars.len(), left, right) {
            Some(interior) => edges.extend(interior),
            None => edges.extend(
                (1..count).map(|i| left + (right - left) * f64::from(i) / f64::from(count)),
            ),
        }
        edges.push(right);
        if first.rtl {
            edges.reverse();
        }
        for (i, (byte, character)) in scalars.into_iter().enumerate() {
            match TextCell::new(
                start + byte..start + byte + character.len_utf8(),
                edges[i],
                edges[i + 1],
            ) {
                Ok(cell) => cells.push(cell),
                Err(error) => {
                    eprintln!("bunny_ui_linux: {error}");
                    return None;
                }
            }
        }
    }
    match LineGeometry::new(text, cells) {
        Ok(geometry) => Some(geometry),
        Err(error) => {
            eprintln!("bunny_ui_linux: {error}");
            None
        }
    }
}
