//! A box the app paints can decline the pointer: it still paints, and the
//! press, the hover, the wheel and the cursor reach what lies under it —
//! a glyph inside a button, marks over a control.
extern crate bunny_ui_core as bunny_ui;

use std::cell::RefCell;

use bunny_ui::layout::{Cursor, LayoutResult, Proposal, Rect};
use bunny_ui::prelude::*;

const WINDOW: Size = Size { width: 300.0, height: 200.0 };

fn laid(runtime: &Runtime, view: &impl View) -> LayoutResult {
    runtime.layout(view, Proposal::exact(WINDOW))
}

/// What a box heard, by kind: a pass-through box must hear nothing of the
/// pointer.
#[derive(Default)]
struct Heard {
    presses: usize,
    moves: usize,
    wheels: usize,
    keys: usize,
}

thread_local! {
    static HEARD: RefCell<Heard> = RefCell::new(Heard::default());
}

fn heard<T>(read: impl FnOnce(&Heard) -> T) -> T {
    HEARD.with(|heard| read(&heard.borrow()))
}

fn forget() {
    HEARD.with(|heard| *heard.borrow_mut() = Heard::default());
}

/// A 16 pt glyph that would rather be the pointer's — unless told not to.
struct Glyph {
    takes_pointer: bool,
    accepts_keys: bool,
}

impl CustomElement for Glyph {
    fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
        painter.fill(ctx.bounds(), Color::rgba(20, 20, 20, 255));
    }

    fn measure(&self, _proposal: Proposal, _metrics: &Metrics) -> Size {
        Size { width: 16.0, height: 16.0 }
    }

    fn flexible(&self, _axis: bunny_ui::layout::Axis) -> bool {
        false
    }

    fn event(&self, event: &ElementEvent, _ctx: &EventCtx) -> Response {
        HEARD.with(|heard| {
            let mut heard = heard.borrow_mut();
            match event {
                ElementEvent::PointerDown { .. } => heard.presses += 1,
                ElementEvent::PointerMoved { .. } => heard.moves += 1,
                ElementEvent::Wheel { .. } => heard.wheels += 1,
                ElementEvent::Key(_) => heard.keys += 1,
                _ => {}
            }
        });
        // a box that answers everything, so a pass is the framework's
        // doing and not the box shrugging
        Response::handled()
    }

    fn cursor(&self, _at: Point, _visible: Rect) -> Option<Cursor> {
        Some(Cursor::Crosshair)
    }

    fn accepts_keys(&self) -> bool {
        self.accepts_keys
    }

    fn takes_pointer(&self) -> bool {
        self.takes_pointer
    }

    fn name(&self) -> &str {
        "glyph"
    }
}

/// A button whose label is the glyph — the toolbar icon of a browser.
#[derive(Clone, Copy)]
struct IconButton {
    presses: State<u32>,
    takes_pointer: bool,
}

impl Component for IconButton {
    fn body(self) -> impl View {
        let presses = self.presses;
        let glyph = custom(Glyph { takes_pointer: self.takes_pointer, accepts_keys: false });
        button(glyph, move || presses.update(|n| *n += 1)).id("reload")
    }
}

fn center(layout: &LayoutResult, id: &str) -> (f64, f64) {
    let (_, rect) = layout.hits.iter().find(|(path, _)| path.contains(id)).expect("the target");
    (rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0)
}

#[test]
fn a_button_hears_the_click_on_a_glyph_that_declines_the_pointer() {
    for (takes_pointer, presses, glyph_presses) in [(true, 0, 1), (false, 1, 0)] {
        forget();
        let runtime = Runtime::new();
        let view = IconButton { presses: State::new(0), takes_pointer };
        runtime.render_stable(&view);
        let layout = laid(&runtime, &view);
        // the press lands on the glyph, in the middle of the button
        let (x, y) = center(&layout, "[reload]");
        runtime.pointer_pressed(x, y);
        runtime.pointer_released(x, y);
        runtime.render_stable(&view);
        assert_eq!(view.presses.get(), presses, "takes the pointer: {takes_pointer}");
        assert_eq!(heard(|heard| heard.presses), glyph_presses, "takes the pointer: {takes_pointer}");
    }
}

/// The same glyph drawn by a `canvas`, which wears the answer from outside.
#[derive(Clone, Copy)]
struct CanvasButton {
    presses: State<u32>,
}

impl Component for CanvasButton {
    fn body(self) -> impl View {
        let presses = self.presses;
        let glyph = canvas(|ctx, p| p.fill(ctx.bounds(), Color::rgba(20, 20, 20, 255)))
            .frame(16.0, 16.0)
            .allows_hit_testing(false);
        button(glyph, move || presses.update(|n| *n += 1)).id("star")
    }
}

/// A box with no id of its own has the path of the button around it. The
/// declined hit must not come back through that shared path.
#[test]
fn a_canvas_inside_a_button_that_declines_hit_testing_lets_the_click_through() {
    let runtime = Runtime::new();
    let view = CanvasButton { presses: State::new(0) };
    runtime.render_stable(&view);
    let layout = laid(&runtime, &view);
    let (x, y) = center(&layout, "[star]");
    runtime.pointer_pressed(x, y);
    runtime.pointer_released(x, y);
    runtime.render_stable(&view);
    assert_eq!(view.presses.get(), 1);
}

/// Marks drawn over a control, the whole size of it.
#[derive(Clone, Copy)]
struct Marked {
    inside: State<bool>,
    takes_pointer: bool,
}

impl Component for Marked {
    fn body(self) -> impl View {
        let inside = self.inside;
        zstack!(
            button(text("Under"), || {}).frame(200.0, 100.0).on_hover(move |hover| inside.set(hover)).id("under"),
            custom(Glyph { takes_pointer: self.takes_pointer, accepts_keys: false }).frame(200.0, 100.0).id("marks"),
        )
    }
}

#[test]
fn the_hover_and_the_cursor_reach_the_control_under_a_pass_through_box() {
    for takes_pointer in [true, false] {
        forget();
        let runtime = Runtime::new();
        let view = Marked { inside: State::new(false), takes_pointer };
        runtime.render_stable(&view);
        laid(&runtime, &view);
        // the middle of the button, where the 16 pt glyph sits
        runtime.pointer_moved(100.0, 50.0, bunny_ui::action::Modifiers::default());
        runtime.render_stable(&view);
        laid(&runtime, &view);
        let cursor = runtime.hovered_cursor();
        if takes_pointer {
            assert!(!view.inside.get(), "the box covers the button");
            assert_eq!(cursor, Some(Cursor::Crosshair), "the box's own cursor");
            assert!(heard(|heard| heard.moves) > 0);
        } else {
            assert!(view.inside.get(), "the button under the box is hovered");
            assert_ne!(cursor, Some(Cursor::Crosshair), "the box says nothing of the cursor");
            assert_eq!(heard(|heard| heard.moves), 0);
        }
    }
}

/// Marks over a region of rows: the wheel is the region's.
#[derive(Clone, Copy)]
struct Over {
    takes_pointer: bool,
}

impl Component for Over {
    fn body(self) -> impl View {
        zstack!(
            scroll(vstack!(for_each(
                (0..40).collect::<Vec<usize>>(),
                |row| row.to_string(),
                |row| text(format!("row {row}")).frame(300.0, 20.0),
            )))
            .id("rows"),
            custom(Glyph { takes_pointer: self.takes_pointer, accepts_keys: false }).frame(300.0, 200.0).id("marks"),
        )
    }
}

#[test]
fn the_wheel_reaches_the_region_under_a_pass_through_box() {
    for takes_pointer in [true, false] {
        forget();
        let runtime = Runtime::new();
        let view = Over { takes_pointer };
        runtime.render_stable(&view);
        let layout = laid(&runtime, &view);
        let region = layout.scrolls.iter().find(|region| region.path.ends_with("[rows]")).expect("the rows");
        runtime.wheel(150.0, 100.0, 0.0, -40.0);
        let travelled = runtime.scroll_offset(&region.path).y;
        if takes_pointer {
            assert_eq!(heard(|heard| heard.wheels), 1, "the box took the turn");
            assert_eq!(travelled, 0.0);
        } else {
            assert_eq!(heard(|heard| heard.wheels), 0);
            assert_eq!(travelled, 40.0, "the region under the box scrolled");
        }
    }
}

/// Declining the pointer is not declining the keyboard.
#[derive(Clone, Copy)]
struct Typist;

impl Component for Typist {
    fn body(self) -> impl View {
        custom(Glyph { takes_pointer: false, accepts_keys: true }).id("typist")
    }
}

#[test]
fn a_box_that_declines_the_pointer_still_takes_the_keyboard() {
    forget();
    let runtime = Runtime::new();
    runtime.render_stable(&Typist);
    laid(&runtime, &Typist);
    assert!(runtime.focus_named("typist"), "a focus stop all the same");
    runtime.key_stroke(KeyPattern::key(Key::Down));
    assert_eq!(heard(|heard| heard.keys), 1);
}

/// On the web the browser runs the hit test: the island of a box that
/// declines the pointer says `pointer-events:none`, so the press reaches
/// the element under it and the canvas never captures the pointer.
#[test]
fn a_served_pass_through_box_says_pointer_events_none() {
    for takes_pointer in [true, false] {
        let page = bunny_ui::ssr::render(
            &custom(Glyph { takes_pointer, accepts_keys: false }).id("marks"),
            WINDOW,
        );
        assert_eq!(page.css.contains("pointer-events:none"), !takes_pointer, "takes the pointer: {takes_pointer}");
    }
}

/// An exact interior captures its elements by frame, apart from the flow
/// lowering: the declined pointer must survive that capture too.
#[test]
fn a_pass_through_box_in_an_exact_interior_says_pointer_events_none() {
    for takes_pointer in [true, false] {
        let page = bunny_ui::ssr::render(
            &vstack!(custom(Glyph { takes_pointer, accepts_keys: false }).id("marks"))
                .layout(bunny_ui::layout::LayoutMode::Exact),
            WINDOW,
        );
        assert_eq!(page.css.contains("pointer-events:none"), !takes_pointer, "takes the pointer: {takes_pointer}");
    }
}

/// A toolbar drawn as ONE canvas island on the web: inside it the
/// framework routes the browser's canvas point itself, and a glyph that
/// declines the pointer is glass on that road too.
#[derive(Clone, Copy)]
struct IslandBar {
    presses: State<u32>,
}

impl Component for IslandBar {
    fn body(self) -> impl View {
        let presses = self.presses;
        let glyph = custom(Glyph { takes_pointer: false, accepts_keys: false });
        hstack!(button(glyph, move || presses.update(|n| *n += 1)).id("reload"))
            .frame(120.0, 40.0)
            .rendering(Rendering::Gpu)
    }
}

#[test]
fn a_glyph_that_declines_the_pointer_inside_a_web_island_lets_its_button_hear_the_click() {
    forget();
    let runtime = Runtime::new();
    let view = IslandBar { presses: State::new(0) };
    let mount = runtime.dom_frame(&view, WINDOW);
    let island = mount
        .iter()
        .find_map(|patch| match patch {
            bunny_ui::dom::DomPatch::Create { id, kind: bunny_ui::dom::CreateKind::Canvas, .. } => Some(*id),
            _ => None,
        })
        .expect("the island mounted");
    let _ = runtime.dom_islands(1);
    // the middle of the island is the middle of the button and its glyph
    assert!(runtime.dom_island_pointer(island, 0, 60.0, 20.0, false), "the button armed");
    assert!(runtime.dom_island_pointer(island, 2, 60.0, 20.0, false));
    let _ = runtime.dom_frame(&view, WINDOW);
    assert_eq!(view.presses.get(), 1);
    assert_eq!(heard(|heard| heard.presses), 0, "the glyph heard nothing");
}
