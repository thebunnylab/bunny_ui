//! Editable text must use the same alignment for pixels and native input geometry.
extern crate bunny_ui_core as bunny_ui;

use bunny_ui::layout::{DrawCommand, LayoutResult, Point, Rect};
use bunny_ui::prelude::*;
use bunny_ui::text_input::EditCommand;

fn frame(runtime: &Runtime, view: &impl View) -> LayoutResult {
    runtime.render_stable(view);
    runtime.layout(
        view,
        Proposal::exact(Size {
            width: 200.0,
            height: 100.0,
        }),
    )
}

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 0.001,
        "expected {expected}, got {actual}"
    );
}

fn lines(layout: &LayoutResult) -> Vec<(String, Point)> {
    layout
        .display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine {
                content,
                range,
                origin,
                ..
            } => Some((content[range.0..range.1].to_owned(), *origin)),
            _ => None,
        })
        .collect()
}

fn expected_left(run: Rect, width: f64, alignment: TextAlignment, rtl: bool) -> f64 {
    let spare = run.size.width - width;
    let shift = match alignment {
        TextAlignment::Center => spare / 2.0,
        TextAlignment::Leading if rtl => spare,
        TextAlignment::Trailing if !rtl => spare,
        _ => 0.0,
    };
    run.origin.x + shift
}

#[test]
fn fitting_text_honors_each_alignment_and_scene_direction() {
    for rtl in [false, true] {
        for alignment in [
            TextAlignment::Leading,
            TextAlignment::Center,
            TextAlignment::Trailing,
        ] {
            for value in ["123.45", "אבג"] {
                let runtime = Runtime::new();
                runtime.set_layout_direction(Some(if rtl {
                    LayoutDirection::RightToLeft
                } else {
                    LayoutDirection::LeftToRight
                }));
                let content = State::new(value.to_owned());
                let view = text_field("", content.binding())
                    .multiline_text_alignment(alignment)
                    .frame(200.0, 30.0)
                    .id("field");
                let layout = frame(&runtime, &view);
                let field = &layout.fields[0];
                let width = runtime.text().measure_line(value, &field.font).width;
                let x = expected_left(field.run, width, alignment, rtl);
                close(lines(&layout)[0].1.x, x);
                close(field.text_origin.x, x);
                assert!(runtime.focus_named("field"));
                let start = if value == "אבג" { x + width } else { x };
                close(runtime.ime_rect_for(0).unwrap().origin.x, start);
            }
        }
    }
}

#[test]
fn numeric_column_aligns_decimal_separators_for_different_widths() {
    let runtime = Runtime::new();
    let values = ["12.50", "1,234.56", "-12.50"].map(|s| State::new(s.to_owned()));
    let view = vstack(
        values
            .into_iter()
            .map(|value| {
                text_field("", value.binding())
                    .monospaced()
                    .multiline_text_alignment(TextAlignment::Trailing)
                    .frame(120.0, 30.0)
            })
            .collect::<Vec<_>>(),
    );
    let layout = frame(&runtime, &view);
    let points = lines(&layout)
        .into_iter()
        .zip(&layout.fields)
        .map(|((text, origin), field)| {
            origin.x
                + runtime
                    .text()
                    .measure_line(text.split('.').next().unwrap(), &field.font)
                    .width
        })
        .collect::<Vec<_>>();
    assert_eq!(points.len(), 3);
    for point in points {
        close(
            point,
            layout.fields[0].run.origin.x + layout.fields[0].run.size.width - 24.0,
        );
    }
}

#[test]
fn pointer_caret_selection_and_utf16_rectangles_follow_aligned_text() {
    for alignment in [TextAlignment::Center, TextAlignment::Trailing] {
        let runtime = Runtime::new();
        let value = State::new("a😀bc".to_owned());
        let view = text_field("", value.binding())
            .multiline_text_alignment(alignment)
            .frame(200.0, 30.0)
            .id("field");
        let layout = frame(&runtime, &view);
        let field = &layout.fields[0];
        let x = expected_left(field.run, 32.0, alignment, false);
        let y = field.run.origin.y + field.line_height / 2.0;
        runtime.pointer_pressed(x + 16.0, y);
        runtime.pointer_released(x + 16.0, y);
        let layout = frame(&runtime, &view);
        assert_eq!(runtime.ime_snapshot().unwrap().selected, (3, 0));
        close(
            runtime.ime_snapshot().unwrap().caret_rect.origin.x,
            x + 16.0,
        );
        close(runtime.ime_rect_for(3).unwrap().origin.x, x + 16.0);
        assert_eq!(runtime.ime_index_at(x + 16.0, y), Some(3));
        assert!(layout.display.iter().any(|command| matches!(command,
            DrawCommand::FillRect { rect, color, .. } if *color == Color::BLACK
            && (rect.origin.x - (x + 16.0)).abs() < 0.001 && rect.size.width < 2.0)));
        runtime.key(EditCommand::SelectAll);
        let selected = frame(&runtime, &view);
        assert!(selected.display.iter().any(|command| matches!(command,
            DrawCommand::FillRect { rect, .. } if (rect.origin.x - x).abs() < 0.001
            && (rect.size.width - 32.0).abs() < 0.001 && rect.size.height > 8.0)));
    }
}

#[test]
fn composition_underline_and_native_range_share_the_aligned_origin() {
    let runtime = Runtime::new();
    let content = State::new("12".to_owned());
    let view = text_field("", content.binding())
        .multiline_text_alignment(TextAlignment::Trailing)
        .frame(200.0, 30.0)
        .id("field");
    frame(&runtime, &view);
    assert!(runtime.focus_named("field"));
    runtime.key(EditCommand::SelectAll);
    runtime.key(EditCommand::SetMarked {
        text: "日本".into(),
        caret_utf16: (1, 0),
    });
    let layout = frame(&runtime, &view);
    let field = &layout.fields[0];
    let x = field.run.origin.x + field.run.size.width - 16.0;
    let ime = runtime.ime_snapshot().unwrap();
    assert_eq!(ime.marked, Some((0, 2)));
    close(ime.caret_rect.origin.x, x + 8.0);
    close(runtime.ime_rect_for(0).unwrap().origin.x, x);
    assert!(layout.display.iter().any(|command| matches!(command,
        DrawCommand::FillRect { rect, .. } if (rect.origin.x - x).abs() < 0.001
        && (rect.size.width - 16.0).abs() < 0.001 && (rect.size.height - 1.0).abs() < 0.001)));
}

#[test]
fn multiline_rows_use_their_own_width_for_pointer_and_native_lookup() {
    let runtime = Runtime::new();
    let content = State::new("abcdef\nx😀y".to_owned());
    let view = text_editor("", content.binding())
        .multiline_text_alignment(TextAlignment::Trailing)
        .frame(200.0, 100.0)
        .id("field");
    let layout = frame(&runtime, &view);
    let field = &layout.fields[0];
    let drawn = lines(&layout);
    assert_eq!(drawn.len(), 2);
    let right = field.run.origin.x + field.run.size.width;
    close(drawn[0].1.x, right - 48.0);
    close(drawn[1].1.x, right - 24.0);
    let x = right - 8.0;
    let y = field.run.origin.y + field.line_height * 1.5;
    runtime.pointer_pressed(x, y);
    runtime.pointer_released(x, y);
    frame(&runtime, &view);
    assert_eq!(runtime.ime_snapshot().unwrap().selected, (10, 0));
    close(runtime.ime_rect_for(10).unwrap().origin.x, x);
    assert_eq!(runtime.ime_index_at(x, y), Some(10));
}

#[test]
fn overflowing_lines_keep_the_caret_visible_in_both_reading_directions() {
    for value in ["abcdefghijklmnopqrstuvwxyz", "אבגדהוזחטיכלמנסעפצקרשתאבגד"]
    {
        for alignment in [
            TextAlignment::Leading,
            TextAlignment::Center,
            TextAlignment::Trailing,
        ] {
            let runtime = Runtime::new();
            let content = State::new(value.to_owned());
            let view = text_field("", content.binding())
                .multiline_text_alignment(alignment)
                .frame(80.0, 30.0)
                .id("field");
            frame(&runtime, &view);
            assert!(runtime.focus_named("field"));
            for command in [EditCommand::End(false), EditCommand::Home(false)] {
                runtime.key(command);
                let layout = frame(&runtime, &view);
                let field = &layout.fields[0];
                let caret = runtime.ime_snapshot().unwrap().caret_rect;
                assert!(caret.origin.x >= field.frame.origin.x);
                assert!(
                    caret.origin.x + caret.size.width
                        <= field.frame.origin.x + field.frame.size.width
                );
                assert_eq!(
                    runtime.ime_index_at(caret.origin.x, caret.origin.y + 4.0),
                    Some(runtime.ime_snapshot().unwrap().selected.0)
                );
            }
        }
    }
}

#[test]
fn empty_aligned_field_keeps_caret_and_ime_at_the_same_insertion_point() {
    let runtime = Runtime::new();
    let content = State::new(String::new());
    let view = text_field("Amount", content.binding())
        .multiline_text_alignment(TextAlignment::Trailing)
        .frame(200.0, 30.0)
        .id("field");
    frame(&runtime, &view);
    assert!(runtime.focus_named("field"));
    let layout = frame(&runtime, &view);
    let run = layout.fields[0].run;
    let x = run.origin.x + run.size.width;
    close(runtime.ime_snapshot().unwrap().caret_rect.origin.x, x);
    assert!(layout.display.iter().any(|command| matches!(command,
        DrawCommand::FillRect { rect, color, .. } if *color == Color::BLACK
        && (rect.origin.x - x).abs() < 0.001 && rect.size.width < 2.0)));
}

#[test]
fn soft_wrapped_last_line_uses_its_own_alignment_for_native_lookup() {
    let runtime = Runtime::new();
    let content = State::new("abcdefghi".to_owned());
    let view = text_editor("", content.binding())
        .multiline_text_alignment(TextAlignment::Trailing)
        .frame(80.0, 100.0)
        .id("field");
    let layout = frame(&runtime, &view);
    let field = &layout.fields[0];
    let drawn = lines(&layout);
    assert_eq!(
        drawn
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>(),
        ["abcdefgh", "i"]
    );
    let right = field.run.origin.x + field.run.size.width;
    close(drawn[1].1.x, right - 8.0);
    assert!(runtime.focus_named("field"));
    let y = field.run.origin.y + field.line_height * 1.5;
    assert_eq!(runtime.ime_index_at(right, y), Some(9));
    close(runtime.ime_rect_for(9).unwrap().origin.x, right);
}

#[test]
fn secret_field_uses_mask_direction_for_aligned_native_geometry() {
    let runtime = Runtime::new();
    let content = State::new("אבג".to_owned());
    let view = text_field("", content.binding())
        .secret(true)
        .multiline_text_alignment(TextAlignment::Trailing)
        .frame(200.0, 30.0)
        .id("field");
    frame(&runtime, &view);
    assert!(runtime.focus_named("field"));
    runtime.key(EditCommand::Home(false));
    let layout = frame(&runtime, &view);
    let drawn = lines(&layout);
    assert_eq!(drawn[0].0, "•••");
    let x = drawn[0].1.x;
    close(runtime.ime_snapshot().unwrap().caret_rect.origin.x, x);
    close(runtime.ime_rect_for(1).unwrap().origin.x, x + 8.0);
    assert_eq!(
        runtime.ime_index_at(x + 8.0, layout.fields[0].run.origin.y + 4.0),
        Some(1)
    );
}
