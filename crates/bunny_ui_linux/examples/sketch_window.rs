//! A box the application owns: it draws, it listens, it types.
//!
//! The framework hands over a rectangle, the paint vocabulary it uses
//! everywhere else, the pointer, the keyboard and the input system.
//! What happens inside is the app's: the ink, the brush, the caption,
//! the caret. Nothing here is a built-in view — and nothing here needs
//! a new one.
//!
//! Drag to draw. The wheel changes the brush. Click the caption strip
//! and type (composition included); Backspace erases, Escape drops the
//! keyboard, and the ink clears with Delete.
//!
//! ```sh
//! cargo run -p bunny-ui-linux --example sketch_window_linux
//! ```

#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use std::rc::Rc;
use std::sync::Arc;

use bunny_ui::layout::{Color, Point, Px, Rect, Size};
use bunny_ui::prelude::*;

const BAR_H: f64 = 44.0;
const LIGHTS_W: f64 = 78.0;
/// The caption strip at the bottom of the box.
const CAPTION_H: Px = 34.0;
/// The dot grid's step.
const GRID: Px = 24.0;

// MARK: - The app's model

/// One stroke: the points the pointer passed through while pressed.
type Stroke = Vec<Point>;

#[derive(Clone, Copy)]
struct Sketch {
    strokes: State<Rc<Vec<Stroke>>>,
    /// The stroke being drawn right now (empty between gestures).
    live: State<Rc<Stroke>>,
    caption: State<Arc<str>>,
    brush: State<f64>,
    /// Where the pointer is, for the crosshair.
    pointer: State<Option<Point>>,
}

impl Sketch {
    fn clear(&self) {
        self.strokes.set(Rc::new(Vec::new()));
        self.live.set(Rc::new(Vec::new()));
    }

    fn extend(&self, at: Point) {
        let mut live = (*self.live.get()).clone();
        live.push(at);
        self.live.set(Rc::new(live));
    }

    fn seal(&self) {
        let live = self.live.get();
        if live.len() > 1 {
            let mut strokes = (*self.strokes.get()).clone();
            strokes.push((*live).clone());
            self.strokes.set(Rc::new(strokes));
        }
        self.live.set(Rc::new(Vec::new()));
    }
}

// MARK: - The box

impl CustomElement for Sketch {
    fn name(&self) -> &str {
        "sketch"
    }

    /// The caption takes the keyboard, so the box does.
    fn accepts_keys(&self) -> bool {
        true
    }

    fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
        let size = ctx.size();
        painter.fill(ctx.bounds(), palette::WELL);

        // the grid: only the dots the clip lets through are painted
        let from = (ctx.visible.origin.y / GRID).floor().max(0.0) as usize;
        let rows = (ctx.visible.size.height / GRID).ceil() as usize + 1;
        let columns = (size.width / GRID).ceil() as usize;
        for row in from..from + rows {
            for column in 0..columns {
                painter.fill(
                    Rect {
                        origin: Point { x: column as Px * GRID, y: row as Px * GRID },
                        size: Size { width: 1.0, height: 1.0 },
                    },
                    palette::GRID,
                );
            }
        }

        let brush = self.brush.get();
        let mut ink = |stroke: &Stroke, color: Color| {
            // the framework has no path primitive yet, so the app draws
            // its own ink with what exists: a rounded dab per step
            for point in stroke {
                painter.fill_rounded(
                    Rect {
                        origin: Point { x: point.x - brush / 2.0, y: point.y - brush / 2.0 },
                        size: Size { width: brush, height: brush },
                    },
                    color,
                    brush / 2.0,
                );
            }
        };
        for stroke in self.strokes.get().iter() {
            ink(stroke, palette::INK);
        }
        ink(&self.live.get(), palette::LIVE);

        // the crosshair follows the pointer — proof the moves arrive
        if let Some(at) = self.pointer.get() {
            painter.fill(
                Rect {
                    origin: Point { x: at.x - 8.0, y: at.y },
                    size: Size { width: 16.0, height: 1.0 },
                },
                palette::CROSS,
            );
            painter.fill(
                Rect {
                    origin: Point { x: at.x, y: at.y - 8.0 },
                    size: Size { width: 1.0, height: 16.0 },
                },
                palette::CROSS,
            );
        }

        // the caption strip: focus, a caret of our own, and the text
        let strip = Rect {
            origin: Point { x: 0.0, y: size.height - CAPTION_H },
            size: Size { width: size.width, height: CAPTION_H },
        };
        painter.fill(strip, palette::STRIP);
        let caption = self.caption.get();
        let origin = Point { x: 14.0, y: strip.origin.y + 9.0 };
        painter.text(
            origin,
            caption.clone(),
            if ctx.focused { palette::FG } else { palette::FG_FAINT },
        );
        if ctx.caret_visible {
            painter.fill(
                Rect {
                    origin: Point {
                        x: origin.x + ctx.metrics.width(&caption) + 1.0,
                        y: origin.y,
                    },
                    size: Size { width: 1.5, height: ctx.metrics.line_height() },
                },
                palette::CARET,
            );
        }
        painter.text(
            Point { x: size.width - 96.0, y: origin.y },
            format!("brush {brush:.0}"),
            palette::FG_FAINT,
        );
    }

    fn event(&self, event: &ElementEvent, _ctx: &EventCtx) -> Response {
        match event {
            ElementEvent::PointerDown { at, .. } => {
                self.pointer.set(Some(*at));
                self.extend(*at);
                Response::handled()
            }
            ElementEvent::PointerMoved { at, pressed } => {
                self.pointer.set(Some(*at));
                if *pressed {
                    self.extend(*at);
                }
                Response::handled()
            }
            ElementEvent::PointerUp { .. } => {
                self.seal();
                Response::handled()
            }
            ElementEvent::PointerExited => {
                self.pointer.set(None);
                Response::handled()
            }
            // the box scrolls nothing: the wheel is the brush size
            ElementEvent::Wheel { dy, .. } => {
                self.brush.set((self.brush.get() + dy * 0.1).clamp(1.0, 40.0));
                Response::handled()
            }
            ElementEvent::Text(text) => {
                self.caption.set(Arc::from(format!("{}{text}", self.caption.get())));
                Response::handled()
            }
            ElementEvent::Key(stroke) => match stroke.pattern.key {
                Key::Backspace => {
                    let mut caption = self.caption.get().to_string();
                    caption.pop();
                    self.caption.set(Arc::from(caption));
                    Response::handled()
                }
                Key::Delete => {
                    self.clear();
                    Response::handled()
                }
                _ => Response::ignored(),
            },
            _ => Response::ignored(),
        }
    }

    /// What the input system reads while the caption has the keyboard —
    /// the caret rides at the end of the line.
    fn ime(&self, metrics: &Metrics) -> Option<ImeContext> {
        let caption = self.caption.get();
        let caret = caption.chars().map(char::len_utf16).sum();
        Some(ImeContext {
            selected: (caret, 0),
            marked: None,
            caret_rect: Rect {
                origin: Point { x: 14.0 + metrics.width(&caption), y: 9.0 },
                size: Size { width: 1.5, height: metrics.line_height() },
            },
            text: caption.to_string(),
        })
    }
}

mod palette {
    use bunny_ui::layout::Color;

    pub const WELL: Color = Color::hex(0x121319);
    pub const GRID: Color = Color::hex(0x23262F);
    pub const INK: Color = Color::hex(0x7FD1C8);
    pub const LIVE: Color = Color::hex(0xE879F9);
    pub const CROSS: Color = Color::hex(0x3B4252);
    pub const STRIP: Color = Color::hex(0x191B22);
    pub const FG: Color = Color::hex(0xD5DAE4);
    pub const FG_FAINT: Color = Color::hex(0x5A6172);
    pub const CARET: Color = Color::hex(0xE879F9);
}

// MARK: - The window

#[derive(Clone, Copy)]
struct App {
    sketch: Sketch,
}

impl Component for App {
    fn body(self, _ctx: &Context) -> impl View {
        let bar = hstack!(
            spacer().frame(LIGHTS_W, 1.0),
            text("sketch").bold(),
            text("drag to draw · wheel sizes the brush · click the strip and type")
                .font_size(11.0)
                .foreground_color(theme::fg_faint()),
            spacer(),
            text(format!("{} strokes", self.sketch.strokes.get().len()))
                .font_size(11.0)
                .foreground_color(theme::fg_secondary()),
        )
        .spacing(10.0)
        .alignment(VerticalAlignment::Center)
        .padding_edge(Edge::Trailing, 14.0)
        .frame_height(BAR_H)
        .window_drag_region();

        vstack!(bar, custom(self.sketch)).spacing(0.0)
    }
}

#[cfg(target_os = "linux")]
fn main() {
    theme::install(Theme::dark());
    let runtime = Runtime::new()
        .text_engine(Rc::new(bunny_ui_linux::FreeTypeEngine::new()))
        .image_engine(Rc::new(bunny_ui_linux::LinuxImageEngine::new()));
    // the scene-drawn chrome arrives in its own phase; the box rides
    // under the native title bar until then
    bunny_ui_linux::run_window_with(
        "bunny — a box the app owns",
        Size { width: 820.0, height: 600.0 },
        runtime,
        App {
            sketch: Sketch {
                strokes: State::new(Rc::new(Vec::new())),
                live: State::new(Rc::new(Vec::new())),
                caption: State::new(Arc::from("a caption the app draws")),
                brush: State::new(6.0),
                pointer: State::new(None),
            },
        },
    );
}

#[cfg(not(target_os = "linux"))]
fn main() {} // this example is Linux-only
