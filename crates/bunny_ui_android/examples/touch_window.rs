//! The finger's vocabulary, on one screen: a list that pans and flings,
//! a button that presses under the finger, a field that raises the
//! keyboard and shrinks the scene above it, a canvas that takes the drag
//! and zooms under two fingers, a row that opens a menu on a long press
//! — and the safe area painted honestly: colored bands at the status bar
//! and the navigation bar, and a one-point frame on the window's own four
//! edges, which is the tripwire for the scale contract (all four visible
//! on a 3× display, or something is off).
//!
//! ```sh
//! crates/bunny_ui_android/android/run-emu.sh touch_window_android
//! ```

#![cfg_attr(not(target_os = "android"), allow(dead_code, unused_imports, unused_variables))]

use std::cell::RefCell;
use std::rc::Rc;

use bunny_ui::layout::{Point, Size};
use bunny_ui::prelude::*;

/// A sketch surface: strokes under the finger (it takes the drag, so a
/// pan inside it draws instead of scrolling the list around it), and a
/// zoom under two fingers.
struct Sketch {
    strokes: Rc<RefCell<Vec<Vec<Point>>>>,
    scale: Rc<RefCell<f64>>,
    ink: Color,
}

impl CustomElement for Sketch {
    fn takes_drag(&self) -> bool {
        true
    }

    fn event(&self, event: &ElementEvent, _ctx: &EventCtx) -> Response {
        match event {
            ElementEvent::PointerDown { at, .. } => {
                self.strokes.borrow_mut().push(vec![*at]);
            }
            ElementEvent::PointerMoved { at, pressed: true, .. } => {
                if let Some(stroke) = self.strokes.borrow_mut().last_mut() {
                    stroke.push(*at);
                }
            }
            ElementEvent::Magnify { scale, .. } => {
                let mut held = self.scale.borrow_mut();
                *held = (*held * scale).clamp(0.5, 4.0);
            }
            _ => return Response::default(),
        }
        Response { handled: true, ..Response::default() }
    }

    fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
        let scale = *self.scale.borrow();
        let size = ctx.frame.size;
        let center = Point { x: size.width / 2.0, y: size.height / 2.0 };
        let zoomed = |point: Point| {
            (
                (center.x + (point.x - center.x) * scale) as f32,
                (center.y + (point.y - center.y) * scale) as f32,
            )
        };
        for stroke in self.strokes.borrow().iter() {
            let mut verbs = Vec::with_capacity(stroke.len());
            for (index, point) in stroke.iter().enumerate() {
                let (x, y) = zoomed(*point);
                verbs.push(if index == 0 { Verb::Move(x, y) } else { Verb::Line(x, y) });
            }
            if verbs.len() == 1 {
                // a dot: a tap leaves a mark too
                let (x, y) = zoomed(stroke[0]);
                verbs.push(Verb::Line(x + 0.5, y + 0.5));
            }
            painter.path(&verbs, Paint::Stroke { width: 3.0 }, self.ink);
        }
    }
}

#[derive(Clone)]
struct Playground {
    count: State<i32>,
    note: State<String>,
    opened: State<usize>,
    strokes: Rc<RefCell<Vec<Vec<Point>>>>,
    zoom: Rc<RefCell<f64>>,
}

impl Component for Playground {
    fn body(self, _ctx: &Context) -> impl View {
        let count = self.count;
        let opened = self.opened;
        let rows = (1..=200).collect::<Vec<usize>>();
        let content = vstack!(
            text!("Touched {} times", self.count).font(Font::Title),
            button(text("Tap me"), move || count.add(1)),
            text_field("Type here…", self.note.binding()),
            text!("Menu opened {} times — hold the row below", self.opened)
                .context_menu(vec![
                    menu_item("Open", move || opened.add(1)),
                    menu_item("Nothing", || {}),
                ]),
            custom(Sketch {
                strokes: Rc::clone(&self.strokes),
                scale: Rc::clone(&self.zoom),
                ink: Color::hex(0x2A6DF4),
            })
            .frame_height(180.0)
            .background_color(Color::hex(0xF2F2F7))
            .corner_radius(12.0),
            list(rows, |row| format!("row{row}"), |row| text(format!("Row {row} — pan and fling"))),
        )
        .spacing(12.0)
        .padding();
        // the bands show where the safe area is; the frame shows where
        // the window is — both reclaim the window with the modifier
        let band = |color: u32, height: f64| {
            // a height of its own, then the whole width: `frame_max` caps
            // and grows only toward an infinite edge
            empty()
                .frame_height(height)
                .frame_max(f64::INFINITY, height, Alignment::Center)
                .background_color(Color::hex_a(color))
        };
        zstack!(
            content,
            vstack!(band(0xFF3B3080, 44.0), spacer(), band(0x34C75980, 20.0))
                .ignores_safe_area(),
            empty()
                .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Center)
                .border(Color::hex(0xFF9500), 1.0)
                .ignores_safe_area(),
        )
    }
}

fn main() {
    let playground = Playground {
        count: State::new(0),
        note: State::new(String::new()),
        opened: State::new(0),
        strokes: Rc::new(RefCell::new(Vec::new())),
        zoom: Rc::new(RefCell::new(1.0)),
    };
    #[cfg(target_os = "android")]
    bunny_ui_android::run_window("touch", Size { width: 390.0, height: 844.0 }, playground);
}

// the activity's entry point, in the app's own shared object
#[cfg(target_os = "android")]
bunny_ui_android::activity!(main);
