//! A live box presents on a surface of its own, and that surface sits
//! above the whole scene. Paint that follows the box in the list would
//! then land UNDER it — a sheet laid over a looping sky would show the
//! sky through it. So a box that later paint reaches stays in the scene,
//! and its clock steps repaint the frame; a box nothing covers keeps its
//! own surface.

extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use bunny_ui::custom::{CustomElement, PaintCtx, Painter, canvas, custom};
use bunny_ui::layout::{Color, DrawCommand, Point, Proposal, Rect, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;

const WINDOW: Size = Size { width: 200.0, height: 120.0 };

/// A sky whose colour IS its phase: the red channel counts the loop.
fn sky(width: f64, height: f64) -> impl View {
    canvas(|ctx, p| {
        let red = (ctx.phase * 255.0).round() as u8;
        p.fill(ctx.bounds(), Color::rgba(red, 0, 40, 255));
    })
    .looping(Loop::secs(1.0).fps(4.0))
    .frame(width, height)
}

/// The sheet's own colour — nothing else paints it.
const SHEET: Color = Color { r: 250, g: 250, b: 251, a: 255 };

fn reds_then_sheet(display: &bunny_ui::layout::DisplayList) -> (Option<usize>, Option<usize>) {
    let sky = display.iter().position(|command| matches!(command, DrawCommand::FillRect { color, .. } if color.b == 40));
    let sheet = display.iter().position(|command| matches!(command, DrawCommand::FillRect { color, .. } if *color == SHEET));
    (sky, sheet)
}

#[test]
fn a_looping_box_under_a_later_sibling_stays_in_the_scene() {
    #[derive(Clone, Copy)]
    struct SignIn;
    impl Component for SignIn {
        fn body(self) -> impl View {
            zstack!(
                sky(WINDOW.width, WINDOW.height),
                rectangle().foreground_color(SHEET).frame(80.0, 60.0)
            )
            .frame(WINDOW.width, WINDOW.height)
        }
    }

    let runtime = Runtime::new();
    let scene = runtime.display_frame(&SignIn, WINDOW);
    let (sky_at, sheet_at) = reds_then_sheet(&scene);
    assert!(sky_at.expect("the sky paints") < sheet_at.expect("the sheet paints"), "the sheet is laid over the sky");
    assert!(
        runtime.live_slices().is_empty(),
        "a surface of its own would sit over the sheet: the sky stays in the scene"
    );
    assert!(runtime.live_paths().is_empty());
    assert!(runtime.live_islands_all(1).is_empty(), "no layer is seeded for it");

    // its clock still runs, and a step is a frame of the scene
    let moved = runtime.tick(0.3);
    assert!(moved.scene, "a covered loop's step repaints the frame");
    assert!(runtime.live_islands(1).is_empty(), "and never a layer");
    let stepped = runtime.animation_frame(&SignIn, WINDOW);
    let red = stepped
        .iter()
        .find_map(|command| match command {
            DrawCommand::FillRect { color, .. } if color.b == 40 => Some(color.r),
            _ => None,
        })
        .expect("the sky paints");
    assert_eq!(red, 64, "the frame carries the quarter phase");
    let (sky_at, sheet_at) = reds_then_sheet(&stepped);
    assert!(sky_at < sheet_at, "and keeps the sheet above it");
}

#[test]
fn a_looping_box_with_nothing_above_it_keeps_its_own_surface() {
    #[derive(Clone, Copy)]
    struct Mark;
    impl Component for Mark {
        fn body(self) -> impl View {
            // the sheet is laid FIRST, the sky over it, and a footer below
            // the sky that never reaches it
            zstack!(
                rectangle().foreground_color(SHEET).frame(WINDOW.width, WINDOW.height),
                vstack!(
                    sky(60.0, 40.0),
                    rectangle().foreground_color(Color::rgba(9, 9, 9, 255)).frame(60.0, 20.0)
                )
            )
            .frame(WINDOW.width, WINDOW.height)
        }
    }

    let runtime = Runtime::new();
    let _ = runtime.display_frame(&Mark, WINDOW);
    assert_eq!(runtime.live_slices().len(), 1, "the sky is the topmost paint over its box");
    let moved = runtime.tick(0.3);
    assert!(moved.islands && !moved.scene, "a step repaints the box alone");
    assert_eq!(runtime.live_islands(1).len(), 1);
}

#[test]
fn a_label_beside_a_spinner_does_not_cover_it() {
    #[derive(Clone, Copy)]
    struct Saving;
    impl Component for Saving {
        fn body(self) -> impl View {
            // no spacing: the label's line starts exactly where the
            // spinner ends, and a line's ink never reaches back past its
            // origin
            hstack!(sky(16.0, 16.0), text("Saving…"), text("and more"))
        }
    }

    let runtime = Runtime::new();
    let _ = runtime.display_frame(&Saving, WINDOW);
    assert_eq!(runtime.live_slices().len(), 1, "the label sits beside the box, not over it");
}

#[test]
fn a_label_laid_over_a_looping_box_covers_it() {
    #[derive(Clone, Copy)]
    struct Banner;
    impl Component for Banner {
        fn body(self) -> impl View {
            zstack!(sky(160.0, 40.0), text("Welcome"))
        }
    }

    let runtime = Runtime::new();
    let _ = runtime.display_frame(&Banner, WINDOW);
    assert!(runtime.live_slices().is_empty(), "the words would sink under the sky's layer");
}

#[test]
fn paint_clipped_away_from_the_box_does_not_cover_it() {
    #[derive(Clone, Copy)]
    struct Spill;
    impl Component for Spill {
        fn body(self) -> impl View {
            // the second box paints a fill far wider than itself, back over
            // the sky — and its own clip cuts all of that away
            hstack!(
                sky(40.0, 40.0),
                canvas(|_, p| {
                    p.fill(
                        Rect { origin: Point { x: -100.0, y: 0.0 }, size: Size { width: 140.0, height: 40.0 } },
                        Color::rgba(9, 9, 9, 255),
                    );
                })
                .frame(40.0, 40.0)
                .clipped()
            )
        }
    }

    let runtime = Runtime::new();
    let scene = runtime.display_frame(&Spill, WINDOW);
    assert!(
        scene.iter().any(|command| matches!(command, DrawCommand::FillRect { rect, .. } if rect.origin.x < 0.0)),
        "the spill's rect does reach back over the sky"
    );
    assert_eq!(runtime.live_slices().len(), 1, "but the clip it was drawn under keeps it off");
}

/// An editor's shape: a white ground, and a caret on an overlay of its own.
struct Editor;

const CARET: Rect = Rect { origin: Point { x: 10.0, y: 4.0 }, size: Size { width: 2.0, height: 16.0 } };

impl CustomElement for Editor {
    fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
        painter.fill(Rect { origin: Point::ZERO, size: ctx.frame.size }, Color::WHITE);
        if !ctx.overlay_layered {
            let _ = self.paint_overlay(ctx, painter);
        }
    }

    fn paint_overlay(&self, _ctx: &PaintCtx, painter: &mut Painter) -> Option<Rect> {
        painter.fill(CARET, Color::BLACK);
        Some(CARET)
    }
}

#[test]
fn a_covered_overlay_is_painted_in_the_scene() {
    let runtime = Runtime::new();
    runtime.set_overlay_layers(true);
    let size = Size { width: 120.0, height: 40.0 };
    // a menu-like card laid over the editor, caret and all (centred:
    // 10 to 110 across)
    let view = zstack!(
        custom(Editor).frame(size.width, size.height).id("editor"),
        rectangle().foreground_color(SHEET).frame(100.0, 40.0)
    )
    .frame(size.width, size.height);
    let result = runtime.settled_layout(&view, Proposal::exact(size));
    assert!(result.customs.iter().any(|placement| placement.overlay), "the overlay is still placed");
    assert!(runtime.live_slices().is_empty(), "the card is above the caret: no layer may lift it over");
    runtime.focus("editor");
    assert!(runtime.blink_overlay(1).is_none(), "the blink takes the frame's road");
}

