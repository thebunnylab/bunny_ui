//! `Painter::draw_list`: a whole display list painted inside an element
//! lands shifted to the element's own origin — the door a tool that shows
//! frames another process rendered paints through.

#![cfg(feature = "canvas")]

use bunny_ui_core as bunny_ui;

use bunny_ui::layout::{Corners, DisplayList, DrawCommand};
use bunny_ui::prelude::*;
use bunny_ui::text_engine::{Family, Slant};

fn list() -> DisplayList {
    DisplayList::from(vec![
        DrawCommand::FillRect {
            rect: Rect { origin: Point { x: 1.0, y: 2.0 }, size: Size { width: 5.0, height: 6.0 } },
            color: Color { r: 9, g: 9, b: 9, a: 255 },
            corner_radius: Corners::default(),
        },
        DrawCommand::PushClip { rect: Rect { origin: Point::ZERO, size: Size { width: 4.0, height: 4.0 } }, corner_radius: Corners::default() },
        DrawCommand::TextLine {
            origin: Point { x: 3.0, y: 4.0 },
            content: "hi".into(),
            range: (0, 2),
            color: Color { r: 1, g: 1, b: 1, a: 255 },
            font: FontSpec { size: 13.0, weight: Weight::Regular, design: FontDesign::Default, slant: Slant::Upright, family: Family::SYSTEM, tracking: 0.0 },
        },
        DrawCommand::PopClip,
    ])
}

#[test]
fn a_list_painted_in_an_element_is_shifted_to_the_elements_origin() {
    let runtime = Runtime::new();
    let view = vstack!(canvas(|_, painter| painter.draw_list(&list())).frame(50.0, 50.0)).padding_length(10.0);
    let display = runtime.display_frame(&view, Size { width: 100.0, height: 100.0 });
    let fills: Vec<Rect> = display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { rect, color, .. } if color.r == 9 => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(fills, [Rect { origin: Point { x: 11.0, y: 12.0 }, size: Size { width: 5.0, height: 6.0 } }]);
    let texts: Vec<Point> = display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { origin, content, .. } if &**content == "hi" => Some(*origin),
            _ => None,
        })
        .collect();
    assert_eq!(texts, [Point { x: 13.0, y: 14.0 }]);
    let clips: Vec<Rect> = display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::PushClip { rect, .. } if rect.size.width == 4.0 => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(clips, [Rect { origin: Point { x: 10.0, y: 10.0 }, size: Size { width: 4.0, height: 4.0 } }]);
}
