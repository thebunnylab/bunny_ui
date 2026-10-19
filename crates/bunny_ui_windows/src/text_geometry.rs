//! DirectWrite logical edges from the same full layout used for rasterization.

use super::{DirectWriteEngine, FontSpec, IDWriteTextLayout, com_ok, create_layout};
use bunny_ui::text_geometry::{LineGeometry, TextCell};

/// Exact `DWRITE_HIT_TEST_METRICS` ABI from dwrite.h (`BOOL` is a 32-bit integer).
#[repr(C)]
#[derive(Default)]
pub(super) struct HitMetrics {
    pub(super) position: u32,
    pub(super) length: u32,
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) bidi_level: u32,
    pub(super) is_text: i32,
    pub(super) is_trimmed: i32,
}

fn edge(layout: *mut IDWriteTextLayout, index: u32, trailing: bool) -> Option<f64> {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut metrics = HitMetrics::default();
    // SAFETY: the caller owns the live layout, and every out parameter has the
    // SDK's exact layout. Indices name scalar-aligned positions in that layout.
    let hr = unsafe {
        ((*(*layout).vtbl).hit_test_text_position)(
            layout,
            index,
            i32::from(trailing),
            &raw mut x,
            &raw mut y,
            &raw mut metrics,
        )
    };
    if !com_ok(hr) {
        eprintln!("bunny_ui_windows: DirectWrite caret lookup failed: {hr:#x}");
        return None;
    }
    Some(f64::from(x))
}

pub(super) fn line_geometry(
    engine: &DirectWriteEngine,
    text: &str,
    font: &FontSpec,
) -> Option<LineGeometry> {
    if text.is_empty() {
        return LineGeometry::new(text, Vec::new()).ok();
    }
    let factories = engine.factories.as_ref()?;
    engine.with_slot(font, |slot| {
        let layout = create_layout(factories, slot, text)?;
        let mut index = 0u32;
        let mut cells = Vec::new();
        for (byte, character) in text.char_indices() {
            let end = index.checked_add(u32::try_from(character.len_utf16()).ok()?)?;
            let before = edge(layout.as_ptr(), index, false)?;
            let after = edge(layout.as_ptr(), end - 1, true)?;
            match TextCell::new(byte..byte + character.len_utf8(), before, after) {
                Ok(cell) => cells.push(cell),
                Err(error) => {
                    eprintln!("bunny_ui_windows: {error}");
                    return None;
                }
            }
            index = end;
        }
        match LineGeometry::new(text, cells) {
            Ok(geometry) => Some(geometry),
            Err(error) => {
                eprintln!("bunny_ui_windows: {error}");
                None
            }
        }
    })?
}
