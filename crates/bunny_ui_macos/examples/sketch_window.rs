//! A box the application owns: it draws, it listens, it types.
//!
//! The framework hands over a rectangle, the paint vocabulary it uses
//! everywhere else, the pointer, the keyboard and the input system.
//! What happens inside is the app's: the ink, the brush, the caption,
//! the caret. Nothing here is a built-in view — and nothing here needs
//! a new one.
//!
//! The ink is a RAMP, not a colour: `Painter::path` takes the same
//! `Gradient` a box paints behind itself, declared in the mark's own
//! proportions — so a stroke shows where it began and where it went,
//! at any size, and every rendering gets the same pixels.
//!
//! Drag to draw. The wheel changes the brush. Click the caption strip
//! and type (composition included); Backspace erases, Escape drops the
//! keyboard, and the ink clears with Delete.
//!
//! Command-press anywhere on the well and the box opens a MENU at that
//! point — the scene's own menu, asked for from inside the box's own
//! event, because a painted scene has no views to hang one on and the
//! choice is always about the point the hand touched.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example sketch_window
//! ```

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use std::rc::Rc;
use std::sync::Arc;

use bunny_ui::layout::{Color, Corners, Point, Px, Rect, Size};
use bunny_ui::prelude::*;
#[cfg(target_os = "macos")]
use bunny_ui_macos::Chrome;

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

    /// A single mark, where the menu was asked for — the one item that
    /// is about the POINT and not about the box.
    fn dab(&self, at: Point) {
        let mut strokes = (*self.strokes.get()).clone();
        strokes.push(vec![at]);
        self.strokes.set(Rc::new(strokes));
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
        // the ink is a RAMP, not a colour: declared in the stroke's own
        // box, so it holds whatever the hand draws, at whatever size
        let mut ink = |stroke: &Stroke, color: Gradient| {
            match stroke.len() {
                0 => {}
                // a single tap has no direction: it is a dab — a pen
                // put down and lifted at the same point, so it wears
                // the ramp like every other mark
                1 => painter.path(
                    &[
                        Verb::Move(stroke[0].x as f32, stroke[0].y as f32),
                        Verb::Line(stroke[0].x as f32, stroke[0].y as f32),
                    ],
                    Paint::Stroke { width: brush as f32 },
                    color,
                ),
                // the ink is ONE path the app assembles from the points
                // the pointer passed through — every sample becomes the
                // control of a quadratic and the midpoints become the
                // anchors, which is how a hand-drawn line stops looking
                // like a chain of segments
                _ => {
                    let mut verbs = Vec::with_capacity(stroke.len() + 1);
                    verbs.push(Verb::Move(stroke[0].x as f32, stroke[0].y as f32));
                    for pair in stroke.windows(2) {
                        let (from, to) = (pair[0], pair[1]);
                        verbs.push(Verb::Quad(
                            from.x as f32,
                            from.y as f32,
                            ((from.x + to.x) / 2.0) as f32,
                            ((from.y + to.y) / 2.0) as f32,
                        ));
                    }
                    let last = stroke[stroke.len() - 1];
                    verbs.push(Verb::Line(last.x as f32, last.y as f32));
                    painter.path(&verbs, Paint::Stroke { width: brush as f32 }, color);
                }
            }
        };
        for stroke in self.strokes.get().iter() {
            ink(stroke, palette::ink());
        }
        ink(&self.live.get(), palette::live());

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

    fn event(&self, event: &ElementEvent, ctx: &EventCtx) -> Response {
        match event {
            // the menu the framework cannot anchor for us: there is no
            // view under this point, only paint, so the box asks
            ElementEvent::PointerDown { at, modifiers, .. } if modifiers.command => {
                let (here, sketch) = (*at, *self);
                ctx.open_menu(
                    here,
                    vec![
                        menu_item("Dab here", move || sketch.dab(here)),
                        menu_divider(),
                        menu_item("Fine brush", move || sketch.brush.set(3.0)),
                        menu_item("Broad brush", move || sketch.brush.set(18.0)),
                        menu_divider(),
                        menu_item("Clear", move || sketch.clear()),
                    ],
                );
                Response::handled()
            }
            ElementEvent::PointerDown { at, .. } => {
                self.pointer.set(Some(*at));
                self.extend(*at);
                Response::handled()
            }
            ElementEvent::PointerMoved { at, pressed, .. } => {
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
    pub const INK_END: Color = Color::hex(0x4C7BE8);
    pub const LIVE: Color = Color::hex(0xE879F9);
    pub const LIVE_END: Color = Color::hex(0xFBBF24);

    /// The settled ink: a ramp along the mark it draws, so a long
    /// stroke shows where it started and where it went.
    pub fn ink() -> bunny_ui::layout::Gradient {
        ramp(INK, INK_END)
    }

    /// The stroke under the hand, in its own two colours.
    pub fn live() -> bunny_ui::layout::Gradient {
        ramp(LIVE, LIVE_END)
    }

    fn ramp(from: Color, to: Color) -> bunny_ui::layout::Gradient {
        use bunny_ui::layout::UnitPoint;
        bunny_ui::layout::Gradient::linear(from, to)
            .direction(UnitPoint::TOP_LEADING, UnitPoint::BOTTOM_TRAILING)
    }
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

impl App {
    /// One cell of the brush picker. The picker is ONE figure cut into
    /// three, so the cells that end it round outward and the middle
    /// one stays square — four corners, not one.
    fn cell(&self, label: &str, width: Px, corners: Corners) -> impl View + use<> {
        let brush = self.sketch.brush;
        let chosen = (brush.get() - width).abs() < 0.01;
        text(label)
            .font_size(11.0)
            .foreground_color(if chosen { Color::WHITE } else { theme::fg_secondary() })
            .foreground_hovered(Color::WHITE)
            .padding_edge(Edge::Leading, 9.0)
            .padding_edge(Edge::Trailing, 9.0)
            .padding_edge(Edge::Top, 5.0)
            .padding_edge(Edge::Bottom, 5.0)
            .background_color(if chosen { theme::accent() } else { theme::control() })
            .background_hovered(theme::row_hover())
            .corner_radius(corners)
            .on_click(move || brush.set(width))
    }
}

impl Component for App {
    fn body(self, _ctx: &Context) -> impl View {
        // the picker: the first cell rounds its LEFT, the last its
        // RIGHT, and the one between them rounds nothing
        let picker = hstack!(
            self.cell("fine", 2.0, Corners::left(7.0)),
            self.cell("medium", 6.0, Corners::ZERO),
            self.cell("broad", 14.0, Corners::right(7.0)),
        )
        .spacing(1.0);

        let bar = hstack!(
            spacer().frame(LIGHTS_W, 1.0),
            text("sketch").bold(),
            text("drag to draw · wheel sizes the brush · click the strip and type")
                .font_size(11.0)
                .foreground_color(theme::fg_faint()),
            spacer(),
            picker,
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

#[cfg(target_os = "macos")]
fn main() {
    theme::install(Theme::dark());
    let runtime = Runtime::new()
        .text_engine(Rc::new(bunny_ui_macos::CoreTextEngine::new()))
        .image_engine(Rc::new(bunny_ui_macos::CoreGraphicsImageEngine::new()));
    bunny_ui_macos::run_window_chrome(
        "bunny — a box the app owns",
        Size { width: 820.0, height: 600.0 },
        Chrome::Scene,
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

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
