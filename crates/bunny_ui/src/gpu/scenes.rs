//! The parity catalog: the scenes every tier is held against.
//!
//! One list, so "this tier passes seven scenes" means the same sentence
//! on every backend. The rasterizer next door draws each of these too,
//! and the difference between the two pictures is the whole
//! certification.

use crate::layout::{Color, Corners, DisplayList, DrawCommand, Point, Rect, Size};

/// The logical box every scene is drawn in.
pub const SIZE: Size = Size { width: 120.0, height: 80.0 };

/// How many scenes the catalog holds.
pub const COUNT: u32 = 7;

fn at(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { origin: Point { x, y }, size: Size { width: w, height: h } }
}

/// Scene `index`, and the name a report should call it by.
pub fn scene(index: u32) -> (DisplayList, &'static str) {
    let mut display = DisplayList::default();
    let name = match index {
        0 => {
            display.push(DrawCommand::FillRect {
                rect: at(10.0, 10.0, 60.0, 30.0),
                color: Color::rgba(20, 90, 160, 255),
                corner_radius: Corners::ZERO,
            });
            "flat opaque rects"
        }
        1 => {
            display.push(DrawCommand::FillRect {
                rect: at(10.0, 10.0, 60.0, 30.0),
                color: Color::rgba(20, 90, 160, 90),
                corner_radius: Corners::ZERO,
            });
            "a translucent veil"
        }
        2 => {
            display.push(DrawCommand::FillRect {
                rect: at(10.0, 10.0, 60.0, 30.0),
                color: Color::rgba(20, 90, 160, 255),
                corner_radius: Corners::all(8.0),
            });
            "a rounded fill"
        }
        3 => {
            display.push(DrawCommand::StrokeRect {
                rect: at(10.0, 10.0, 60.0, 30.0),
                color: Color::rgba(20, 90, 160, 255),
                width: 3.0,
                corner_radius: Corners::all(6.0),
            });
            "a stroke ring"
        }
        4 => {
            display.push(DrawCommand::PushClip {
                rect: at(20.0, 15.0, 40.0, 25.0),
                corner_radius: Corners::all(6.0),
            });
            display.push(DrawCommand::FillRect {
                rect: at(0.0, 0.0, 120.0, 80.0),
                color: Color::rgba(200, 60, 40, 255),
                corner_radius: Corners::ZERO,
            });
            display.push(DrawCommand::PopClip);
            "a rounded clip"
        }
        5 => {
            display.push(DrawCommand::Shadow {
                rect: at(30.0, 25.0, 40.0, 20.0),
                radius: 8.0,
                color: Color::rgba(0, 0, 0, 120),
                corner_radius: Corners::all(5.0),
            });
            "a shadow's falloff"
        }
        _ => {
            display.push(DrawCommand::TextLine {
                origin: Point { x: 12.0, y: 40.0 },
                content: std::sync::Arc::from("parity"),
                range: (0, 6),
                color: Color::rgba(30, 30, 40, 255),
                font: crate::text_engine::FontSpec::DEFAULT,
            });
            "a pixel-font run"
        }
    };
    (display, name)
}
