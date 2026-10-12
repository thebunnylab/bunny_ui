//! Runtime composition placement retains the source outside each visible row.

use super::super::super::super::native_float;
use super::{indices, native_area, wrapped_layout_with_font};
use crate::text::{Com, DirectWriteEngine, IDWriteTextLayout, com_ok, geometry};
use bunny_ui::layout::DrawCommand;
use bunny_ui::prelude::{Color, EditCommand, Proposal, Runtime, Size, State, ViewExt, text_editor};
use std::rc::Rc;

fn native_x(layout: &Com<IDWriteTextLayout>, unit: usize) -> f64 {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut metrics = geometry::HitMetrics::default();
    // SAFETY: independent full native paragraph and a scalar boundary.
    unsafe {
        assert!(com_ok(((*(*layout.as_ptr()).vtbl).hit_test_text_position)(
            layout.as_ptr(),
            u32::try_from(unit).unwrap(),
            0,
            &raw mut x,
            &raw mut y,
            &raw mut metrics,
        )));
    }
    f64::from(x)
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 0.03, "{actual} vs {expected}");
}

#[test]
fn paragraph_runtime_composition_and_candidate_rect_keep_wrapped_context() {
    for width in [95.0, 160.0] {
        let engine = Rc::new(DirectWriteEngine::new());
        let runtime = Runtime::new().text_engine(engine.clone());
        let draft = State::new(String::new());
        let view = text_editor("", draft.binding())
            .font_size(21.0)
            .line_height(28.0)
            .frame(width, 400.0)
            .id("paragraph-composition");
        let size = Size {
            width,
            height: 400.0,
        };
        runtime.display_frame(&view, size).unwrap();
        assert!(runtime.focus_named("paragraph-composition"));
        // Change only the outside directional control while marked text is
        // active. The same visible words must acquire the new paragraph context.
        for text in [
            "\u{202e}abc 😀 def ghi jkl mno\u{202c}",
            "\u{202d}abc 😀 def ghi jkl mno\u{202c}",
        ] {
            let byte = text.find("def").unwrap();
            let unit = text[..byte].encode_utf16().count();
            assert!(
                runtime
                    .key(EditCommand::SetMarked {
                        text: text.into(),
                        caret_utf16: (unit, 7)
                    })
                    .unwrap()
                    .applied
            );
            runtime.display_frame(&view, size).unwrap();
            let placed = runtime.layout(&view, Proposal::exact(size)).unwrap();
            let snapshot = runtime.ime_snapshot().unwrap().unwrap();
            assert_eq!(snapshot.text.as_ref(), text);
            assert_eq!(snapshot.marked, Some((0, text.encode_utf16().count())));
            assert_eq!(snapshot.selected, (unit, 7));
            assert_native_composition(&engine, &runtime, &placed, text, unit);
        }
        assert!(runtime.key(EditCommand::Unmark).unwrap().applied);
        runtime.display_frame(&view, size).unwrap();
        assert_eq!(runtime.ime_snapshot().unwrap().unwrap().marked, None);
    }
}

fn assert_native_composition(
    engine: &DirectWriteEngine,
    runtime: &Runtime,
    placed: &bunny_ui::layout::LayoutResult,
    text: &str,
    unit: usize,
) {
    let field = &placed.fields[0];
    let field_width = field.run.size.width;
    let (native, rows) = wrapped_layout_with_font(
        engine,
        text,
        native_float(field.run.size.width).unwrap(),
        &field.font,
    );
    assert!(rows.len() >= 2);
    let map = indices(text);
    let byte_at = |unit| map.iter().find(|(_, value)| *value == unit).unwrap().0;
    let native_ranges: Vec<_> = rows
        .iter()
        .map(|row| (byte_at(row.start), byte_at(row.end)))
        .collect();
    let actual_ranges: Vec<_> = placed
        .display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, range, .. } if content.as_ref() == text => {
                Some(*range)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        actual_ranges, native_ranges,
        "fixture wraps must match native rows: width={field_width}, text={text:?}"
    );
    let mut underlines: Vec<_> = placed
        .display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { rect, color, .. }
                if *color == Color::BLACK
                    && (rect.size.height - 1.0).abs() < 1e-6
                    && rect.size.width > 1e-6 =>
            {
                Some(*rect)
            }
            _ => None,
        })
        .collect();
    underlines.sort_by(|a, b| {
        a.origin
            .y
            .total_cmp(&b.origin.y)
            .then(a.origin.x.total_cmp(&b.origin.x))
    });
    let mut expected = Vec::new();
    for (row, units) in rows.iter().enumerate() {
        for span in native_area(&native, units.clone()) {
            expected.push((
                field.run.origin.x + span.start,
                f64::from(u32::try_from(row + 1).unwrap())
                    .mul_add(field.line_height, field.text_origin.y)
                    - 1.0,
                span.end - span.start,
            ));
        }
    }
    assert_eq!(
        underlines.len(),
        expected.len(),
        "composition rectangle count: {text:?}"
    );
    for (actual, (x, y, width)) in underlines.iter().zip(expected) {
        close(actual.origin.x, x);
        close(actual.origin.y, y);
        close(actual.size.width, width);
    }
    // Interior 'e' is neither a soft-wrap boundary nor a split bidi
    // caret; query the composition's position, not the active caret.
    let query = unit + 1;
    let row = rows.iter().position(|row| row.contains(&query)).unwrap();
    let actual = runtime.ime_rect_for(query).unwrap().unwrap();
    let x = field.run.origin.x + native_x(&native, query);
    let y = f64::from(u32::try_from(row).unwrap()).mul_add(field.line_height, field.text_origin.y);
    close(actual.origin.x, x);
    close(actual.origin.y, y);
    close(actual.size.height, field.line_height);
    assert_eq!(
        runtime
            .ime_index_at(x, y + field.line_height / 2.0)
            .unwrap(),
        Some(query)
    );
    eprintln!(
        "public paragraph composition parity: width={field_width}, text={text:?}, rows={}, underlines={}",
        rows.len(),
        underlines.len()
    );
}
