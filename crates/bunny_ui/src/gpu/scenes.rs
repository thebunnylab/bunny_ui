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
pub const COUNT: u32 = 9;

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
        7 | 8 => {
            // something to read THROUGH: the pane's whole job is what
            // is under it
            display.push(DrawCommand::FillRect {
                rect: at(0.0, 0.0, 120.0, 40.0),
                color: Color::rgba(200, 60, 40, 255),
                corner_radius: Corners::ZERO,
            });
            display.push(DrawCommand::FillRect {
                rect: at(0.0, 40.0, 120.0, 40.0),
                color: Color::rgba(30, 120, 90, 255),
                corner_radius: Corners::ZERO,
            });
            let pane = at(20.0, 20.0, 60.0, 40.0);
            display.push(DrawCommand::Backdrop {
                rect: pane,
                glass: crate::layout::Glass::regular().resolve(pane),
                corner_radius: Corners::all(10.0),
            });
            if index == 8 {
                // a pane over a pane: the second one reads the first,
                // so the batch must break and the capture happen twice
                let over = at(50.0, 35.0, 50.0, 35.0);
                display.push(DrawCommand::Backdrop {
                    rect: over,
                    glass: crate::layout::Glass::regular().resolve(over),
                    corner_radius: Corners::all(8.0),
                });
                return (display, "two panes, each reading the one below");
            }
            "one pane of glass"
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

/// A scene with the SHAPE of a real one, for a bench line: a panel, and
/// a field of striped rows each carrying a label. `nudge` moves one
/// number so consecutive frames are never identical — both roads skip
/// an unchanged frame, and the skip is not what a bench measures.
pub fn bench_scene(logical: Size, nudge: f64) -> DisplayList {
    let mut display = DisplayList::default();
    display.push(DrawCommand::FillRect {
        rect: at(8.0, 8.0, logical.width - 16.0, logical.height - 16.0),
        color: Color::rgba(255, 255, 255, 255),
        corner_radius: Corners::all(10.0),
    });
    let rows = (((logical.height - 24.0) / 18.0).max(0.0)) as usize;
    for row in 0..rows {
        let y = 16.0 + row as f64 * 18.0 + nudge;
        display.push(DrawCommand::FillRect {
            rect: at(12.0, y, logical.width - 24.0, 16.0),
            color: match row % 2 {
                0 => Color::rgba(246, 247, 250, 255),
                _ => Color::rgba(255, 255, 255, 255),
            },
            corner_radius: Corners::all(3.0),
        });
        display.push(DrawCommand::TextLine {
            origin: Point { x: 18.0, y: y + 12.0 },
            content: std::sync::Arc::from("file_0000.rs"),
            range: (0, 12),
            color: Color::rgba(30, 30, 40, 255),
            font: crate::text_engine::FontSpec::DEFAULT,
        });
    }
    display
}
