//! A split's seam is the layout's, not a body's. A drag writes the seam's
//! state on every pointer move; read in the body that holds the split,
//! each write re-ran that body — the whole window's, for a dock — and
//! every body under it before a frame could lay out. Read by the layout,
//! a write is a frame and nothing else.
use bunny_ui::action::Modifiers;
use bunny_ui::layout::{Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;

const SIZE: Size = Size { width: 600.0, height: 200.0 };

#[derive(Clone)]
struct Room {
    seam: State<f64>,
}

impl Component for Room {
    fn body(self, _ctx: &Context) -> impl View {
        hsplit(self.seam.binding(), text!("a").on_click(|| {}).id("a"), text!("b").on_click(|| {}).id("b"))
            .min_sizes(50.0, 50.0)
    }
}

fn x_of(runtime: &Runtime, view: &impl View, name: &str) -> f64 {
    runtime
        .settled_layout(view, Proposal::exact(SIZE))
        .hits
        .iter()
        .find(|(path, _)| path.contains(name))
        .map(|(_, rect)| rect.origin.x)
        .expect("the lane is a hit")
}

#[test]
fn a_seam_written_lays_out_with_no_body_between() {
    let seam = State::new(200.0);
    let runtime = Runtime::new();
    let room = Room { seam };
    let _ = runtime.display_frame(&room, SIZE);
    let before = x_of(&runtime, &room, "[b]");
    seam.set(300.0);
    let _ = runtime.display_frame(&room, SIZE);
    assert!(runtime.body_runs().is_empty(), "a seam write ran a body: {:?}", runtime.body_runs());
    let after = x_of(&runtime, &room, "[b]");
    assert!((after - before - 100.0).abs() < 1.0, "lane B moved with the seam: {before} -> {after}");
}

#[test]
fn a_drag_of_the_grip_moves_the_seam_and_runs_no_body() {
    let seam = State::new(200.0);
    let runtime = Runtime::new();
    let room = Room { seam };
    let _ = runtime.display_frame(&room, SIZE);
    assert!(runtime.pointer_pressed(200.5, 100.0), "the grip took the press");
    let _ = runtime.pointer_moved(240.5, 100.0, Modifiers::default());
    let _ = runtime.display_frame(&room, SIZE);
    assert!(runtime.body_runs().is_empty(), "a seam drag ran a body: {:?}", runtime.body_runs());
    assert!((seam.get() - 240.0).abs() < 1.5, "the seam followed the hand: {}", seam.get());
    runtime.pointer_released(240.5, 100.0);
    let b = x_of(&runtime, &room, "[b]");
    assert!((b - 241.0).abs() < 1.5, "lane B stands past the seam: {b}");
}
