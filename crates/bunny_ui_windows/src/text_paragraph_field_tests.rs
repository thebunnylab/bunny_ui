//! Public field geometry must follow native advances after masking and alignment.
use super::super::super::super::native_float;
use super::composition::{close, native_x};
use super::{area, indices, native_area, same_area, wrapped_layout_with_font};
use crate::text::{DirectWriteEngine, FontSpec};
use bunny_ui::layout::{DrawCommand, FieldPlacement, LayoutResult};
use bunny_ui::prelude::{
    EditCommand, LayoutDirection, Proposal, Runtime, Size, State, TextAlignment, ViewExt,
    text_editor,
};
use std::ops::Range;
use std::rc::Rc;

struct NativeRow {
    bytes: Range<usize>,
    spans: Vec<Range<f64>>,
    carets: Vec<(usize, f64)>,
    newline: bool,
    rtl: bool,
}

fn native_rows(
    engine: &DirectWriteEngine,
    shown: &str,
    font: &FontSpec,
    width: f64,
) -> Vec<NativeRow> {
    let mut result = Vec::new();
    let mut offset = 0;
    for paragraph in shown.split('\n') {
        // The fixture's Hebrew paragraph is RTL; controls around Latin do not
        // change its base direction. No candidate direction helper is used.
        let rtl = paragraph.starts_with('א');
        let newline = offset + paragraph.len() < shown.len();
        if paragraph.is_empty() {
            result.push(NativeRow {
                bytes: offset..offset,
                spans: Vec::new(),
                carets: Vec::new(),
                newline,
                rtl,
            });
        } else {
            let (native, rows) =
                wrapped_layout_with_font(engine, paragraph, native_float(width).unwrap(), font);
            let map = indices(paragraph);
            let byte_at = |unit| map.iter().find(|(_, value)| *value == unit).unwrap().0;
            for units in rows {
                let bytes = byte_at(units.start)..byte_at(units.end);
                let scalars: Vec<_> = paragraph[bytes.clone()].char_indices().collect();
                let carets = scalars
                    .windows(3)
                    .filter_map(|triple| {
                        let [(_, previous), (byte, current), (_, next)] = triple else {
                            unreachable!()
                        };
                        let eligible = |c: char| c.is_alphabetic() || c == '•';
                        if !eligible(*previous) || !eligible(*current) || !eligible(*next) {
                            return None;
                        }
                        let byte = bytes.start + byte;
                        let unit = map.iter().find(|(at, _)| *at == byte).unwrap().1;
                        Some((offset + byte, native_x(&native, unit)))
                    })
                    .collect();
                result.push(NativeRow {
                    newline: newline && bytes.end == paragraph.len(),
                    bytes: offset + bytes.start..offset + bytes.end,
                    spans: native_area(&native, units),
                    carets,
                    rtl,
                });
            }
        }
        offset += paragraph.len() + 1;
    }
    result
}

fn row_origin(field: &FieldPlacement, row: &NativeRow, alignment: TextAlignment, rtl: bool) -> f64 {
    let width = row.spans.last().map_or(0.0, |span| span.end);
    let room = field.run.size.width - width;
    let shift = if room < 0.0 {
        if row.rtl { room } else { 0.0 }
    } else {
        match (alignment, rtl) {
            (TextAlignment::Center, _) => room / 2.0,
            (TextAlignment::Leading, true) | (TextAlignment::Trailing, false) => room,
            _ => 0.0,
        }
    };
    field.run.origin.x + shift
}

fn painted_spans(placed: &LayoutResult, row_y: f64, underline: bool) -> Vec<Range<f64>> {
    let field = &placed.fields[0];
    let color = if underline {
        bunny_ui::theme::caret()
    } else {
        bunny_ui::theme::selection()
    };
    let height = if underline { 1.0 } else { field.line_height };
    let y = if underline {
        row_y + field.line_height - 1.0
    } else {
        row_y
    };
    area(
        placed
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::FillRect {
                    rect, color: ink, ..
                } if *ink == color
                    && (rect.origin.y - y).abs() < 0.03
                    && (rect.size.height - height).abs() < 0.03 =>
                {
                    Some(rect.origin.x..rect.origin.x + rect.size.width)
                }
                _ => None,
            })
            .collect(),
    )
}

fn display_map(text: &str, secret: bool) -> (String, Vec<(usize, usize)>) {
    let mut shown = String::new();
    let mut units = 0;
    let mut map = Vec::new();
    for character in text.chars() {
        map.push((shown.len(), units));
        shown.push(if secret { '•' } else { character });
        units += character.len_utf16();
    }
    map.push((shown.len(), units));
    (shown, map)
}

fn check_field(text: &str, width: f64, alignment: TextAlignment, rtl: bool, secret: bool) {
    let engine = Rc::new(DirectWriteEngine::new());
    let runtime = Runtime::new().text_engine(engine.clone());
    runtime.set_layout_direction(Some(if rtl {
        LayoutDirection::RightToLeft
    } else {
        LayoutDirection::LeftToRight
    }));
    let draft = State::new(String::new());
    let view = text_editor("", draft.binding())
        .secret(secret)
        .multiline_text_alignment(alignment)
        .font_size(21.0)
        .line_height(28.0)
        .frame(width, 600.0)
        .id("native-field");
    let size = Size {
        width,
        height: 600.0,
    };
    runtime.display_frame(&view, size).unwrap();
    assert!(runtime.focus_named("native-field"));
    let length = text.encode_utf16().count();
    assert!(
        runtime
            .key(EditCommand::SetMarked {
                text: text.into(),
                caret_utf16: (0, length)
            })
            .unwrap()
            .applied
    );
    runtime.display_frame(&view, size).unwrap();
    let placed = runtime.layout(&view, Proposal::exact(size)).unwrap();
    let field = &placed.fields[0];
    let snapshot = runtime.ime_snapshot().unwrap().unwrap();
    assert_eq!(snapshot.text.as_ref(), text);
    assert_eq!(snapshot.marked, Some((0, length)));
    assert_eq!(snapshot.selected, (0, length));
    let (shown, map) = display_map(text, secret);
    let rows = native_rows(&engine, &shown, &field.font, field.run.size.width);
    let drawn = drawn_rows(&placed, &shown, &rows);
    let mut points = 0;
    for (index, row) in rows.iter().enumerate() {
        let x = row_origin(field, row, alignment, rtl);
        let y = f64::from(u32::try_from(index).unwrap())
            .mul_add(field.line_height, field.text_origin.y);
        if let Some((_, drawn_x)) = drawn.iter().find(|(bytes, _)| *bytes == row.bytes) {
            close(*drawn_x, x);
        }
        let underlined: Vec<_> = row
            .spans
            .iter()
            .map(|span| x + span.start..x + span.end)
            .collect();
        assert!(
            same_area(&painted_spans(&placed, y, true), &underlined),
            "native composition row {index}"
        );
        let mut selected = underlined;
        if row.newline {
            if row.rtl {
                selected.push(x - field.line_height / 2.0..x);
            } else {
                let edge = selected.last().map_or(x, |span| span.end);
                selected.push(edge..edge + field.line_height / 2.0);
            }
        }
        let selected = area(selected);
        let actual = painted_spans(&placed, y, false);
        assert!(
            same_area(&actual, &selected),
            "native selection {text:?}, secret={secret}, alignment={alignment:?}, rtl={rtl}, width={width}, row={index}: {actual:?} vs {selected:?}"
        );
        for &(byte, offset) in &row.carets {
            let unit = map.iter().find(|(at, _)| *at == byte).unwrap().1;
            let caret = runtime.ime_rect_for(unit).unwrap().unwrap();
            close(caret.origin.x, x + offset);
            close(caret.origin.y, y);
            assert_eq!(
                runtime
                    .ime_index_at(x + offset, y + field.line_height / 2.0)
                    .unwrap(),
                Some(unit)
            );
            points += 1;
        }
    }
    assert!(points >= 3);
    eprintln!(
        "native field projection parity: width={width}, secret={secret}, alignment={alignment:?}, rtl={rtl}, rows={}, points={points}",
        rows.len()
    );
}

#[test]
fn paragraph_runtime_mask_alignment_and_hard_breaks_match_native_geometry() {
    for text in [
        "\u{202e}abc def ghi jkl\u{202c}\n\nmno😀pqr",
        "אבג abc def אבג ghi\nxyz a\u{301}😀 mno",
    ] {
        for width in [95.0, 160.0] {
            for secret in [false, true] {
                for rtl in [false, true] {
                    for alignment in [
                        TextAlignment::Leading,
                        TextAlignment::Center,
                        TextAlignment::Trailing,
                    ] {
                        check_field(text, width, alignment, rtl, secret);
                    }
                }
            }
        }
    }
}

fn drawn_rows(placed: &LayoutResult, shown: &str, rows: &[NativeRow]) -> Vec<(Range<usize>, f64)> {
    let drawn: Vec<_> = placed
        .display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine {
                content,
                range,
                origin,
                ..
            } => {
                assert_eq!(content.as_ref(), shown);
                Some((range.0..range.1, origin.x))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        drawn
            .iter()
            .map(|(range, _)| range.clone())
            .collect::<Vec<_>>(),
        rows.iter()
            .filter(|row| !row.bytes.is_empty())
            .map(|row| row.bytes.clone())
            .collect::<Vec<_>>(),
        "native wraps: {shown:?}"
    );
    drawn
}
