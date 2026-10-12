//! Context changes advances and caret order even when the visible bytes agree.
extern crate bunny_ui_core as bunny_ui;
use bunny_ui::layout::DrawCommand;
use bunny_ui::prelude::*;
use bunny_ui::text_engine::{
    FontSpec, LineMetrics, MeasureCache, TextEngine, TextRaster, break_lines,
};
use bunny_ui::text_geometry::{LineGeometry, TextCell};
use bunny_ui::text_input::byte_to_utf16;
use bunny_ui::text_source::ParagraphLine;
use std::rc::Rc;

struct Context;
const fn visible(c: char) -> bool {
    c != '\u{202e}' && c != '\u{202c}'
}
fn count(text: &str) -> f64 {
    f64::from(u32::try_from(text.chars().filter(|c| visible(*c)).count()).unwrap())
}
fn metrics(text: &str, advance: f64) -> LineMetrics {
    LineMetrics {
        width: count(text) * advance,
        ascent: 12.0,
        descent: 3.0,
    }
}
fn geometry(text: &str, advance: f64, reverse: bool) -> LineGeometry {
    let width = count(text) * advance;
    let mut pen = 0.0;
    let cells = text
        .char_indices()
        .map(|(at, c)| {
            let before = pen;
            pen += if visible(c) { advance } else { 0.0 };
            let (a, b) = if reverse {
                (width - before, width - pen)
            } else {
                (before, pen)
            };
            TextCell::new(at..at + c.len_utf8(), a, b).unwrap()
        })
        .collect();
    LineGeometry::new(text, cells).unwrap()
}
impl TextEngine for Context {
    fn measure_line(&self, text: &str, _: &FontSpec) -> LineMetrics {
        metrics(text, 10.0)
    }
    fn measure_paragraph_line(&self, line: &ParagraphLine, _: &FontSpec) -> Result<LineMetrics, bunny_ui::text_error::TextError> {
        Ok({
        metrics(
            line.text(),
            if line.paragraph().text().starts_with('\u{202e}') {
                20.0
            } else {
                10.0
            },
        )
    })
    }
    fn line_geometry(&self, text: &str, _: &FontSpec) -> Option<LineGeometry> {
        Some(geometry(text, 10.0, false))
    }
    fn paragraph_line_geometry(&self, line: &ParagraphLine, _: &FontSpec) -> Result<Option<LineGeometry>, bunny_ui::text_error::TextError> {
        Ok({
        let reverse = line.paragraph().text().starts_with('\u{202e}');
        Some(geometry(
            line.text(),
            if reverse { 20.0 } else { 10.0 },
            reverse,
        ))
    })
    }
    fn raster_line(&self, _: &str, _: &FontSpec, _: Color, _: usize) -> Option<TextRaster> {
        None
    }
}
#[test]
fn paragraph_editing_wrap_measures_candidates_in_the_original_context() {
    let text = "\u{202e}abc def ghi";
    let cache = MeasureCache::default();
    assert_eq!(
        break_lines(text, &FontSpec::DEFAULT, 80.0, &Context, &cache).unwrap(),
        vec![(0, 7), (7, 11), (11, 14)]
    );
    assert_eq!(
        *cache.field_lines("editor", text, None, &FontSpec::DEFAULT, 80.0, &Context)
            .unwrap(),
        vec![(0, 7), (7, 11), (11, 14)]
    );
    let edited = "\u{202e}abc def xyz";
    assert_eq!(
        *cache.field_lines("editor", edited, None, &FontSpec::DEFAULT, 80.0, &Context)
            .unwrap(),
        break_lines(
            edited,
            &FontSpec::DEFAULT,
            80.0,
            &Context,
            &MeasureCache::default()
        )
        .unwrap()
    );
}
#[test]
fn paragraph_editing_wrapped_ime_and_pointer_share_contextual_geometry() {
    for alignment in [
        TextAlignment::Leading,
        TextAlignment::Center,
        TextAlignment::Trailing,
    ] {
        let runtime = Runtime::new().text_engine(Rc::new(Context));
        let text = "\u{202e}abc def ghi jkl";
        let state = State::new(text.to_owned());
        let view = text_editor("", state.binding())
            .multiline_text_alignment(alignment)
            .frame(100.0, 250.0)
            .id("editor");
        let size = Size {
            width: 100.0,
            height: 250.0,
        };
        runtime.display_frame(&view, size).unwrap();
        let placed = runtime.layout(&view, Proposal::exact(size)).unwrap();
        assert!(runtime.focus_named("editor"));
        let mut checked = 0;
        for command in placed.display.iter() {
            let DrawCommand::TextLine {
                content,
                range,
                origin,
                ..
            } = command
            else {
                continue;
            };
            if range.0 == 0 {
                continue;
            }
            let text_line = &content[range.0..range.1];
            let Some((local, _)) = text_line
                .char_indices()
                .filter(|(_, c)| c.is_alphabetic())
                .nth(1)
            else {
                continue;
            };
            let expected =
                20.0_f64.mul_add(count(text_line) - count(&text_line[..local]), origin.x);
            let unit = byte_to_utf16(text, range.0 + local);
            let observed = runtime.ime_rect_for(unit).unwrap().unwrap();
            assert!(
                (observed.origin.x - expected).abs() < 0.001,
                "{alignment:?}, row {range:?}: {} != {expected}",
                observed.origin.x
            );
            runtime.pointer_pressed(expected, origin.y + 5.0).unwrap();
            let selected = runtime.ime_snapshot().unwrap().unwrap().selected;
            assert_eq!(
                selected,
                (unit, 0),
                "pointer must name the same native cell"
            );
            runtime.pointer_released(expected, origin.y + 5.0);
            checked += 1;
        }
        assert!(checked >= 2);
    }
}
#[test]
fn paragraph_editing_static_hard_breaks_never_form_a_cross_paragraph_command() {
    let runtime = Runtime::new();
    let view = text("first\nsecond\n").frame(300.0, 100.0);
    let result = runtime.layout(
        &view,
        Proposal::exact(Size {
            width: 300.0,
            height: 100.0,
        }),
    )
        .unwrap();
    let mut lines = Vec::new();
    for command in result.display.iter() {
        if let DrawCommand::TextLine { content, range, .. } = command {
            lines.push(
                content
                    .line(range.0..range.1)
                    .expect("draw ranges cannot cross hard paragraphs")
                    .text()
                    .to_owned(),
            );
        }
    }
    assert_eq!(lines, vec!["first", "second", ""]);
}

#[test]
fn paragraph_editing_single_line_projection_preserves_binding_and_native_indices() {
    let runtime = Runtime::new().text_engine(Rc::new(Context));
    let original = "a\n😀\rb";
    let state = State::new(original.to_owned());
    let view = text_field("", state.binding())
        .frame(220.0, 40.0)
        .id("editor");
    let size = Size {
        width: 220.0,
        height: 40.0,
    };
    runtime.display_frame(&view, size).unwrap();
    let placed = runtime.layout(&view, Proposal::exact(size)).unwrap();
    assert!(runtime.focus_named("editor"));
    let lines: Vec<_> = placed
        .display
        .iter()
        .filter_map(|command| {
            if let DrawCommand::TextLine { content, range, .. } = command {
                Some(content.line(range.0..range.1).unwrap())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text(), "a 😀 b");
    let x = placed.fields[0].text_origin.x;
    for (unit, column) in [(0, 0.0), (1, 1.0), (2, 2.0), (4, 3.0), (5, 4.0), (6, 5.0)] {
        assert_eq!(
            runtime.ime_rect_for(unit).unwrap().unwrap().origin.x,
            10.0_f64.mul_add(column, x)
        );
        assert_eq!(
            runtime.ime_index_at(
                10.0_f64.mul_add(column, x),
                placed.fields[0].run.origin.y + 5.0
            )
                .unwrap(),
            Some(unit)
        );
    }
    assert_eq!(state.get(), original);
}

#[test]
fn paragraph_editing_static_highlights_clip_complete_contextual_lines() {
    let runtime = Runtime::new().text_engine(Rc::new(Context));
    let value = "\u{202e}abc def ghi";
    let view = text(value)
        .highlight(vec![(8, 9)], Color::WHITE)
        .frame(80.0, 150.0);
    let placed = runtime.layout(
        &view,
        Proposal::exact(Size {
            width: 80.0,
            height: 150.0,
        }),
    )
        .unwrap();
    let mut clip = None;
    let mut highlighted = 0;
    for command in placed.display.iter() {
        match command {
            DrawCommand::PushClip { rect, .. } => clip = Some(*rect),
            DrawCommand::TextLine {
                content,
                range,
                origin,
                color,
                ..
            } => {
                let line = content.line(range.0..range.1).unwrap();
                assert_eq!(line.paragraph().text(), value);
                if *color == Color::WHITE {
                    assert_eq!(*range, (7, 11));
                    let band = clip.unwrap();
                    assert_eq!(band.origin.x, origin.x + 40.0);
                    assert_eq!(band.size.width, 20.0);
                    highlighted += 1;
                }
            }
            _ => (),
        }
    }
    assert_eq!(highlighted, 1);
}

#[test]
fn paragraph_editing_masked_vertical_arrows_follow_displayed_wraps() {
    use bunny_ui::text_input::EditCommand;
    let runtime = Runtime::new().text_engine(Rc::new(Context));
    let value = "a😀\nbcdefgh";
    let state = State::new(value.to_owned());
    let view = text_editor("", state.binding())
        .secret(true)
        .frame(56.0, 120.0)
        .id("editor");
    let size = Size {
        width: 56.0,
        height: 120.0,
    };
    runtime.display_frame(&view, size).unwrap();
    let placed = runtime.layout(&view, Proposal::exact(size)).unwrap();
    runtime.sync_field(&placed.fields[0].path, value, 1);
    let initial = runtime.ime_snapshot().unwrap().unwrap().caret_rect.origin.x;
    assert!(runtime.key(EditCommand::Down(true)).unwrap().applied);
    assert_eq!(runtime.ime_snapshot().unwrap().unwrap().selected, (1, 5));
    assert_eq!(runtime.ime_rect_for(6).unwrap().unwrap().origin.x, initial);
    assert!(runtime.key(EditCommand::Up(false)).unwrap().applied);
    assert_eq!(runtime.ime_snapshot().unwrap().unwrap().selected, (1, 0));
    assert_eq!(state.get(), value);
}

#[test]
fn paragraph_editing_truncated_hard_breaks_remain_valid_lines() {
    for mode in [Truncation::Start, Truncation::Middle, Truncation::End] {
        let runtime = Runtime::new().text_engine(Rc::new(Context));
        let view = text("first\nsecond\nthird")
            .truncation_mode(mode)
            .frame(140.0, 15.0);
        let placed = runtime.layout(
            &view,
            Proposal::exact(Size {
                width: 140.0,
                height: 15.0,
            }),
        )
            .unwrap();
        let mut count = 0;
        for command in placed.display.iter() {
            if let DrawCommand::TextLine { content, range, .. } = command {
                let line = content.line(range.0..range.1).unwrap();
                assert!(line.text().contains('…'));
                count += 1;
            }
        }
        assert_eq!(count, 1);
    }
}

#[test]
fn paragraph_editing_wrapped_selection_composition_and_vertical_columns_agree() {
    use bunny_ui::text_input::EditCommand;
    let runtime = Runtime::new().text_engine(Rc::new(Context));
    let value = "\u{202e}abc def ghi jkl";
    let state = State::new(value.to_owned());
    let view = text_editor("", state.binding())
        .frame(100.0, 160.0)
        .id("editor");
    let size = Size {
        width: 100.0,
        height: 160.0,
    };
    runtime.display_frame(&view, size).unwrap();
    let placed = runtime.layout(&view, Proposal::exact(size)).unwrap();
    let unit = byte_to_utf16(value, 8);
    runtime.sync_field(&placed.fields[0].path, value, unit);
    let x = runtime.ime_snapshot().unwrap().unwrap().caret_rect.origin.x;
    assert!(runtime.key(EditCommand::Down(false)).unwrap().applied);
    assert_eq!(runtime.ime_snapshot().unwrap().unwrap().selected, (unit + 4, 0));
    assert_eq!(runtime.ime_snapshot().unwrap().unwrap().caret_rect.origin.x, x);
    assert!(runtime.key(EditCommand::Up(false)).unwrap().applied);
    assert_eq!(runtime.ime_snapshot().unwrap().unwrap().selected, (unit, 0));
    assert!(runtime.key(EditCommand::Right(true)).unwrap().applied);
    assert_eq!(runtime.key(EditCommand::Copy).unwrap().output.as_deref(), Some("e"));
    let check_band = |height: f64| {
        let placed = runtime.layout(&view, Proposal::exact(size)).unwrap();
        let bands: Vec<_> = placed
            .display
            .iter()
            .filter_map(|command| {
                if let DrawCommand::FillRect { rect, .. } = command
                    && (rect.size.height - height).abs() < 0.001
                    && (rect.size.width - 20.0).abs() < 0.001
                {
                    return Some(rect);
                }
                None
            })
            .collect();
        assert_eq!(bands.len(), 1);
        assert_eq!(bands[0].origin.x, x - 20.0);
    };
    check_band(15.0);
    runtime.key(EditCommand::SetMarked {
        text: "e".into(),
        caret_utf16: (1, 0),
    })
        .unwrap();
    check_band(1.0);
    let snapshot = runtime.ime_snapshot().unwrap().unwrap();
    assert_eq!(snapshot.marked, Some((unit, 1)));
    assert_eq!(snapshot.selected, (unit + 1, 0));
    assert_eq!(snapshot.caret_rect.origin.x, x - 20.0);
    assert_eq!(state.get(), value);
}

#[test]
fn paragraph_editing_last_row_ellipsis_retains_hidden_embedding_context() {
    let runtime = Runtime::new().text_engine(Rc::new(Context));
    let value = "\u{202e}abc def ghi jkl";
    let view = text(value).frame(80.0, 30.0);
    let placed = runtime.layout(
        &view,
        Proposal::exact(Size {
            width: 80.0,
            height: 30.0,
        }),
    )
        .unwrap();
    let lines: Vec<_> = placed
        .display
        .iter()
        .filter_map(|command| {
            if let DrawCommand::TextLine { content, range, .. } = command {
                Some(content.line(range.0..range.1).unwrap())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(lines.len(), 2);
    let last = &lines[1];
    assert_eq!(last.text(), "def…");
    assert!(last.paragraph().text().starts_with("\u{202e}abc "));
    assert_eq!(
        Context
            .measure_paragraph_line(last, &FontSpec::DEFAULT)
            .unwrap()
            .width,
        80.0
    );
}
