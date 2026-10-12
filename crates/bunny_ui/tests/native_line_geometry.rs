//! Public editor behavior against an independent nonmonotonic geometry oracle.
extern crate bunny_ui_core as bunny_ui;
use bunny_ui::layout::{DrawCommand, LayoutResult};
use bunny_ui::prelude::*;
use bunny_ui::text_engine::{FontSpec, LineMetrics, TextEngine, TextRaster};
use bunny_ui::text_geometry::{LineGeometry, TextCell};
use bunny_ui::text_input::{CaretShape, CaretState, EditCommand, EditingStrategy};
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
struct Oracle(Cell<usize>);
impl TextEngine for Oracle {
    fn measure_line(&self, text: &str, _: &FontSpec) -> LineMetrics {
        LineMetrics {
            width: f64::from(u32::try_from(text.chars().count()).unwrap()) * 10.0,
            ascent: 12.0,
            descent: 3.0,
        }
    }
    fn raster_line(&self, _: &str, _: &FontSpec, _: Color, _: usize) -> Option<TextRaster> {
        None
    }
    fn line_geometry(&self, text: &str, _: &FontSpec) -> Option<LineGeometry> {
        self.0.set(self.0.get() + 1);
        let positions = match text {
            "abאבcd" => vec![
                (0., 10.),
                (10., 20.),
                (40., 30.),
                (30., 20.),
                (40., 50.),
                (50., 60.),
            ],
            "אבab" => vec![(40., 30.), (30., 20.), (0., 10.), (10., 20.)],
            _ => (0..u32::try_from(text.chars().count()).unwrap())
                .map(|i| (f64::from(i) * 10., f64::from(i + 1) * 10.))
                .collect(),
        };
        Some(
            LineGeometry::new(
                text,
                text.char_indices()
                    .zip(positions)
                    .map(|((at, ch), (a, b))| TextCell::new(at..at + ch.len_utf8(), a, b).unwrap())
                    .collect(),
            )
            .unwrap(),
        )
    }
}
fn runtime() -> Runtime {
    Runtime::new().text_engine(Rc::new(Oracle::default()))
}
fn frame(runtime: &Runtime, view: &impl View) -> LayoutResult {
    runtime.render_stable(view);
    runtime.layout(
        view,
        Proposal::exact(Size {
            width: 200.,
            height: 100.,
        }),
    )
        .unwrap()
}
fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 0.001,
        "expected {expected}, got {actual}"
    );
}
fn bands(layout: &LayoutResult, height: f64) -> Vec<(f64, f64)> {
    let x = layout.fields[0].text_origin.x;
    layout
        .display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { rect, .. }
                if (rect.size.height - height).abs() < 0.001
                    && rect.size.width > 2.
                    && rect.size.width < 100. =>
            {
                Some((rect.origin.x - x, rect.size.width))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn pointer_drag_preserves_both_sides_of_the_same_logical_boundary() {
    let runtime = runtime();
    let content = State::new("abאבcd".to_owned());
    let view = text_field("", content.binding())
        .frame(200., 30.)
        .id("field");
    let layout = frame(&runtime, &view);
    let field = &layout.fields[0];
    let (x, y) = (field.text_origin.x, field.run.origin.y + 5.);
    runtime.pointer_pressed(x + 40., y).unwrap();
    close(runtime.ime_snapshot().unwrap().unwrap().caret_rect.origin.x, x + 40.);
    assert_eq!(runtime.ime_snapshot().unwrap().unwrap().selected, (2, 0));
    assert!(
        runtime.pointer_moved(x + 20., y, false).unwrap(),
        "affinity changes even when the byte does not"
    );
    close(runtime.ime_snapshot().unwrap().unwrap().caret_rect.origin.x, x + 20.);
    assert_eq!(runtime.ime_snapshot().unwrap().unwrap().selected, (2, 0));
    runtime.pointer_moved(x + 30., y, false).unwrap();
    assert_eq!(runtime.ime_snapshot().unwrap().unwrap().selected, (2, 1));
    assert_eq!(runtime.key(EditCommand::Copy).unwrap().output.as_deref(), Some("א")
    );
    runtime.pointer_released(x + 30., y);
    close(runtime.ime_rect_for(3).unwrap().unwrap().origin.x, x + 30.);
}

#[test]
fn partial_selection_and_composition_keep_the_visual_hole() {
    let runtime = runtime();
    let content = State::new("abאבcd".to_owned());
    let view = text_field("", content.binding())
        .frame(200., 30.)
        .id("field");
    let first = frame(&runtime, &view);
    runtime.sync_field(&first.fields[0].path, "abאבcd", 1);
    runtime.key(EditCommand::Right(true)).unwrap();
    runtime.key(EditCommand::Right(true)).unwrap();
    let selected = frame(&runtime, &view);
    assert_eq!(bands(&selected, 15.), vec![(10., 10.), (30., 10.)]);
    assert_eq!(runtime.key(EditCommand::Copy).unwrap().output.as_deref(), Some("bא")
    );
    runtime.key(EditCommand::SetMarked {
        text: "bא".into(),
        caret_utf16: (1, 0),
    })
        .unwrap();
    let marked = frame(&runtime, &view);
    assert_eq!(bands(&marked, 1.), vec![(10., 10.), (30., 10.)]);
    let snapshot = runtime.ime_snapshot().unwrap().unwrap();
    assert_eq!(snapshot.marked, Some((1, 2)));
    assert_eq!(snapshot.selected, (2, 0));
    close(
        snapshot.caret_rect.origin.x,
        marked.fields[0].text_origin.x + 20.,
    );
}

struct Policy(CaretShape);
impl EditingStrategy for Policy {
    fn takes_text(&self) -> bool {
        true
    }
    fn caret_shape(&self) -> CaretShape {
        self.0
    }
    fn key(&self, _: &bunny_ui::action::Stroke, _: &mut String, _: &mut CaretState) -> bool {
        false
    }
}
#[test]
fn every_caret_shape_follows_the_native_cell_and_block_repaints_the_full_line() {
    for shape in [CaretShape::Bar, CaretShape::Block, CaretShape::Underline] {
        let runtime = runtime();
        let content = State::new("abאבcd".to_owned());
        let view = text_field("", content.binding())
            .editing_strategy(Some(Rc::new(Policy(shape))))
            .frame(200., 30.)
            .id("field");
        let first = frame(&runtime, &view);
        runtime.sync_field(&first.fields[0].path, "abאבcd", 2);
        let painted = frame(&runtime, &view);
        let x = painted.fields[0].text_origin.x;
        let expected = if shape == CaretShape::Bar {
            (40., 1.5)
        } else {
            (30., 10.)
        };
        assert!(painted.display.iter().any(|command| matches!(command,
            DrawCommand::FillRect{rect,..} if (rect.origin.x-x-expected.0).abs()<0.001
            && (rect.size.width-expected.1).abs()<0.001)));
        let lines: Vec<_> = painted
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { range, origin, .. } => Some((*range, origin.x)),
                _ => None,
            })
            .collect();
        assert_eq!(lines.len(), if shape == CaretShape::Block { 2 } else { 1 });
        for (range, origin) in lines {
            assert_eq!(range, (0, 8));
            close(origin, x);
        }
    }
}

#[test]
fn vertical_navigation_keeps_the_visual_column_after_each_line_alignment() {
    for (alignment, column) in [
        (TextAlignment::Leading, 4),
        (TextAlignment::Center, 3),
        (TextAlignment::Trailing, 2),
    ] {
        let runtime = runtime();
        let content = State::new("abאבcd\nabcd".to_owned());
        let view = text_editor("", content.binding())
            .multiline_text_alignment(alignment)
            .frame(200., 80.)
            .id("field");
        let first = frame(&runtime, &view);
        runtime.sync_field(&first.fields[0].path, "abאבcd\nabcd", 2);
        let x = runtime.ime_snapshot().unwrap().unwrap().caret_rect.origin.x;
        assert!(runtime.key(EditCommand::Down(false)).unwrap().applied);
        assert_eq!(runtime.ime_snapshot().unwrap().unwrap().selected, (7 + column, 0));
        close(runtime.ime_snapshot().unwrap().unwrap().caret_rect.origin.x, x);
        runtime.key(EditCommand::Up(false)).unwrap();
        assert_eq!(runtime.ime_snapshot().unwrap().unwrap().selected, (2, 0));
        close(runtime.ime_snapshot().unwrap().unwrap().caret_rect.origin.x, x);
    }
}

#[test]
fn horizontal_reveal_tracks_visual_positions_in_both_paragraph_directions() {
    for (text, indices) in [
        ("abאבcd", vec![(2, 40.), (3, 30.), (0, 0.), (6, 60.)]),
        ("אבab", vec![(0, 40.), (2, 0.), (3, 10.), (4, 20.)]),
    ] {
        let runtime = runtime();
        let content = State::new(text.to_owned());
        let view = text_field("", content.binding())
            .frame(45., 30.)
            .id("field");
        let first = frame(&runtime, &view);
        let path = first.fields[0].path.clone();
        for (index, local) in indices {
            runtime.sync_field(&path, text, index);
            runtime.key(EditCommand::Read).unwrap();
            // Read triggers the same reveal path without changing affinity.
            let current = frame(&runtime, &view);
            let point = runtime.ime_snapshot().unwrap().unwrap().caret_rect.origin.x;
            close(point, current.fields[0].text_origin.x + local);
            let run = current.fields[0].run;
            assert!(
                point >= run.origin.x - 0.001 && point <= run.origin.x + run.size.width + 0.001,
                "visual caret must remain in the viewport for {text:?} at {index}: {point}, {run:?}"
            );
        }
    }
}

#[test]
fn mask_geometry_maps_surrogate_units_back_to_the_original_text() {
    let runtime = runtime();
    let content = State::new("a😀ב".to_owned());
    let view = text_field("", content.binding())
        .secret(true)
        .frame(200., 30.)
        .id("field");
    let first = frame(&runtime, &view);
    runtime.sync_field(&first.fields[0].path, "a😀ב", 3);
    let painted = frame(&runtime, &view);
    let field = &painted.fields[0];
    let x = field.text_origin.x;
    close(runtime.ime_rect_for(3).unwrap().unwrap().origin.x, x + 20.);
    assert_eq!(
        runtime.ime_index_at(x + 10., field.run.origin.y + 5.)
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        runtime.ime_index_at(x + 20., field.run.origin.y + 5.)
            .unwrap(),
        Some(3)
    );
}

#[test]
fn replacing_the_engine_drops_its_cached_native_geometry() {
    let first = Rc::new(Oracle::default());
    let second = Rc::new(Oracle::default());
    let runtime = Runtime::new().text_engine(first.clone());
    let content = State::new("abאבcd".to_owned());
    let view = text_field("", content.binding())
        .frame(200., 30.)
        .id("field");
    frame(&runtime, &view);
    runtime.focus_named("field");
    frame(&runtime, &view);
    assert!(first.0.get() > 0);
    let runtime = runtime.text_engine(second.clone());
    frame(&runtime, &view);
    assert!(second.0.get() > 0);
}

#[test]
fn colored_field_highlights_keep_complete_line_shaping_and_disjoint_clips() {
    let runtime = runtime();
    let content = State::new("abאבcd".to_owned());
    let view = text_field("", content.binding())
        .highlight(vec![(1, 4)], Color::WHITE)
        .frame(200.0, 30.0)
        .id("field");
    let layout = frame(&runtime, &view);
    let x = layout.fields[0].text_origin.x;
    let mut clip = None;
    let mut hot = Vec::new();
    for command in layout.display.iter() {
        match command {
            DrawCommand::PushClip { rect, .. } => clip = Some(*rect),
            DrawCommand::TextLine {
                range,
                origin,
                color,
                ..
            } => {
                assert_eq!(*range, (0, 8));
                close(origin.x, x);
                if *color == Color::WHITE {
                    let rect = clip.unwrap();
                    hot.push((rect.origin.x - x, rect.size.width));
                }
            }
            _ => (),
        }
    }
    assert_eq!(hot, vec![(10.0, 10.0), (30.0, 10.0)]);
}
