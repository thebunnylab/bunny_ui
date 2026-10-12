//! Only a selected source newline adds the editor's paragraph-separator marker.
extern crate bunny_ui_core as bunny_ui;
use bunny_ui::layout::{DisplayList, DrawCommand};
use bunny_ui::prelude::{EditCommand, Runtime, Size, State, ViewExt, text_editor};

fn select_all(value: &str, secret: bool) -> DisplayList {
    let runtime = Runtime::new();
    let draft = State::new(value.to_owned());
    let view = text_editor("", draft.binding())
        .secret(secret)
        .line_height(20.0)
        .frame(56.0, 400.0)
        .id("selection");
    let size = Size {
        width: 56.0,
        height: 400.0,
    };
    runtime.display_frame(&view, size).unwrap();
    assert!(runtime.focus_named("selection"));
    assert!(runtime.key(EditCommand::SelectAll).unwrap().applied);
    runtime.display_frame(&view, size).unwrap()
}

#[test]
fn paragraph_selection_soft_wrap_does_not_invent_a_separator() {
    let layout = select_all("abcdefghi", false);
    let bands: Vec<_> = layout
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { rect, color, .. } if *color == bunny_ui::theme::selection() => {
                Some(rect)
            }
            _ => None,
        })
        .collect();
    assert_eq!(bands.len(), 2);
    assert!((bands[0].size.width - 40.0).abs() < 0.001);
    assert!((bands[1].size.width - 32.0).abs() < 0.001);
}

#[test]
fn paragraph_selection_real_newlines_keep_visible_markers_including_empty_rows() {
    let layout = select_all("abc\n\nx", false);
    let widths: Vec<_> = layout
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { rect, color, .. } if *color == bunny_ui::theme::selection() => {
                Some(rect.size.width)
            }
            _ => None,
        })
        .collect();
    assert_eq!(widths.len(), 4);
    for (actual, expected) in widths.iter().zip([24.0, 10.0, 10.0, 8.0]) {
        assert!((actual - expected).abs() < 0.001);
    }
}

#[test]
fn paragraph_selection_masked_source_newline_is_one_bullet() {
    let layout = select_all("a😀\nbcdef", true);
    let bands: Vec<_> = layout
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { rect, color, .. } if *color == bunny_ui::theme::selection() => {
                Some(rect)
            }
            _ => None,
        })
        .collect();
    assert_eq!(bands.len(), 2);
    assert!((bands[0].size.width - 40.0).abs() < 0.001);
    assert!((bands[1].size.width - 24.0).abs() < 0.001);
}
