use super::super::super::{BaseFont, Prepared, Shaper};
use super::*;
use crate::text::{Com, DirectWriteEngine, FontSpec, IDWriteTextLayout, com_ok, geometry};
use bunny_ui::text_engine::Family;

fn edge(layout: &Com<IDWriteTextLayout>, position: usize, trailing: bool) -> f64 {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut metrics = geometry::HitMetrics::default();
    // SAFETY: independent full native layout and initialized SDK out parameters.
    unsafe {
        assert!(com_ok(((*(*layout.as_ptr()).vtbl).hit_test_text_position)(
            layout.as_ptr(),
            u32::try_from(position).unwrap(),
            i32::from(trailing),
            &raw mut x,
            &raw mut y,
            &raw mut metrics
        )));
    }
    f64::from(x)
}

#[test]
fn paragraph_geometry_scalar_edges_match_independent_native_wraps() {
    let engine = DirectWriteEngine::new();
    let factories = engine.factories.as_ref().unwrap();
    let shaper = Shaper::new(factories).unwrap();
    let mut differences = Vec::new();
    for family in [Family::default(), Family::named("Calibri")] {
        let font = FontSpec {
            family,
            size: 21.0,
            ..FontSpec::DEFAULT
        };
        for text in [
            "office affine ffi fi fl",
            "a\u{301} e\u{301}x",
            "العربية لا ل\u{200d}ا",
            "abc אבג 123 def",
            "\u{202e}abc def ghi jkl\u{202c}",
            "a😀b 👨‍👩‍👧‍👦x",
            "नमस्तेक्ष हिन्दी",
        ] {
            let prepared = Prepared::new(factories, text).unwrap();
            for width in [65.0, 130.0, 400.0] {
                let (layout, ranges) =
                    super::super::tests::wrapped_layout_with_font(&engine, text, width, &font);
                engine.with_slot(&font, |slot| {
                    let base = BaseFont::new(slot, &font).unwrap();
                    for range in ranges {
                        let runs = shaper.shape(&prepared, range.clone(), &base, &font).unwrap();
                        let line = VisualLine::new(&prepared, range.clone(), runs).unwrap();
                        let visible = String::from_utf16(&prepared.utf16()[range.clone()]).unwrap();
                        let cells = line.geometry(&visible).unwrap();
                        let mut unit = range.start;
                        for ((_, character), cell) in visible.char_indices().zip(cells.cells()) {
                            let end = unit + character.len_utf16();
                            let expected = (edge(&layout, unit, false), edge(&layout, end - 1, true));
                            if (cell.leading() - expected.0).abs() >= 0.03 || (cell.trailing() - expected.1).abs() >= 0.03 {
                                differences.push(format!("{text:?}, {:?}, width {width}, range {range:?}, unit {unit} {character:?}: {:?} vs {expected:?}", font.family, (cell.leading(),cell.trailing())));
                            }
                            unit = end;
                        }
                    }
                }).unwrap();
            }
        }
    }
    assert!(
        differences.is_empty(),
        "native scalar edge differences: {differences:#?}"
    );
}
