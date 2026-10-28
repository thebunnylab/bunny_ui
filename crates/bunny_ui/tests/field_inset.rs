//! A field's inset is the caller's: measure, the painted text, the
//! caret and the browser's `<input>` all count from the same edges.
extern crate bunny_ui_core as bunny_ui;

use bunny_ui::layout::{DrawCommand, FIELD_INSET, LayoutResult};
use bunny_ui::prelude::*;

fn laid(runtime: &Runtime, view: &impl View, proposal: Proposal) -> LayoutResult {
    runtime.render_stable(view);
    runtime.layout(view, proposal)
}

fn text_at(layout: &LayoutResult) -> Point {
    layout
        .display
        .iter()
        .find_map(|command| match command {
            DrawCommand::TextLine { origin, .. } => Some(*origin),
            _ => None,
        })
        .expect("the field paints its text")
}

fn open() -> Proposal {
    Proposal { width: None, height: None }
}

#[test]
fn the_house_inset_is_every_fields_default() {
    let runtime = Runtime::new();
    let value = State::new("Search".to_owned());
    let view = text_field("", value.binding()).id("field");
    let layout = laid(&runtime, &view, open());
    let field = &layout.fields[0];
    let line = runtime.text().measure_line("Search", &field.font);
    assert_eq!(
        layout.size,
        Size { width: line.width + 16.0, height: line.height() + 10.0 },
        "8 points each side, 5 above and below"
    );
    assert_eq!(text_at(&layout), Point { x: 8.0, y: 5.0 });
    assert_eq!(field.run.origin, Point { x: 8.0, y: 5.0 });
    assert_eq!(FIELD_INSET, Edges { top: 5.0, trailing: 8.0, bottom: 5.0, leading: 8.0 });
}

#[test]
fn a_field_set_into_a_designs_padding_takes_no_inset_of_its_own() {
    let runtime = Runtime::new();
    let value = State::new("Search".to_owned());
    let view = text_field("", value.binding()).bare().content_inset(Edges::ZERO).id("field");
    let layout = laid(&runtime, &view, open());
    let field = &layout.fields[0];
    let line = runtime.text().measure_line("Search", &field.font);
    assert_eq!(layout.size, Size { width: line.width, height: line.height() });
    assert_eq!(text_at(&layout), Point::ZERO);
    assert_eq!(field.text_origin, Point::ZERO);

    // the caret stands where the text starts
    assert!(runtime.focus_named("field"));
    assert_eq!(runtime.ime_rect_for(0).expect("a caret").origin.x, 0.0);
}

#[test]
fn the_leading_inset_follows_the_reading_direction() {
    let inset = Edges { top: 3.0, trailing: 2.0, bottom: 3.0, leading: 20.0 };
    let place = |direction| {
        let runtime = Runtime::new();
        runtime.set_layout_direction(Some(direction));
        let value = State::new("abc".to_owned());
        let view = text_field("", value.binding()).content_inset(inset).frame(200.0, 30.0).id("field");
        let layout = laid(&runtime, &view, Proposal::exact(Size { width: 200.0, height: 30.0 }));
        let field = &layout.fields[0];
        let width = runtime.text().measure_line("abc", &field.font).width;
        (field.run, text_at(&layout).x, width)
    };

    let (run, x, _) = place(LayoutDirection::LeftToRight);
    assert_eq!((run.origin.x, run.size.width), (20.0, 178.0));
    assert_eq!(x, 20.0, "left to right the text starts at the leading inset");

    let (run, x, width) = place(LayoutDirection::RightToLeft);
    assert_eq!((run.origin.x, run.size.width), (2.0, 178.0));
    assert!((x + width - 180.0).abs() < 0.001, "right to left it ends at the leading inset, x {x}");
}

#[test]
fn a_many_line_field_wraps_inside_its_inset() {
    let runtime = Runtime::new();
    let value = State::new("one two three four five six seven".to_owned());
    let inset = Edges { top: 0.0, trailing: 0.0, bottom: 0.0, leading: 0.0 };
    let view = text_editor("", value.binding()).content_inset(inset).id("note");
    let layout = laid(&runtime, &view, Proposal { width: Some(60.0), height: None });
    let field = &layout.fields[0];
    assert_eq!(field.run.size.width, 60.0, "the whole width is the text's");
    let rows = layout.display.iter().filter(|command| matches!(command, DrawCommand::TextLine { .. })).count();
    assert!(rows > 1, "the note wraps");
    assert_eq!(layout.size.height, rows as f64 * field.line_height, "no inset above or below");
}

/// A field the browser mounts as an `<input>` lives in a component.
#[derive(Clone, Copy)]
struct Search {
    value: State<String>,
    inset: Option<Edges>,
}

impl Component for Search {
    fn body(self) -> impl View {
        let field = text_field("Search", self.value.binding());
        match self.inset {
            Some(inset) => field.content_inset(inset),
            None => field,
        }
    }
}

#[test]
fn a_mounted_field_carries_an_inset_it_names() {
    use bunny_ui::dom::{CreateKind, DomPatch};
    // the field's look as the glue receives it: its own padding when it
    // names one, the glue's house inset when it does not
    let padding = |inset| {
        let runtime = Runtime::new();
        let view = Search { value: State::new(String::new()), inset };
        let patches = runtime.dom_frame(&view, Size { width: 200.0, height: 40.0 });
        patches
            .iter()
            .find_map(|patch| match patch {
                DomPatch::DefineRule { kind: CreateKind::Field, layout, .. } => Some(layout.padding),
                _ => None,
            })
            .expect("the field's rule")
    };
    assert_eq!(padding(Some(Edges::ZERO)), Some((0.0, 0.0, 0.0, 0.0)));
    assert_eq!(padding(None), None);
}
