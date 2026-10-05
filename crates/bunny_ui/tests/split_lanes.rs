//! A split's lane can hide: the other takes the whole room and the seam goes
//! with it, and both lanes keep their place in the tree — the lane that
//! stays is the same lane before, during and after, and the one that
//! returns comes back to its own path.
use bunny_ui::layout::{Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;

const SIZE: Size = Size { width: 600.0, height: 300.0 };

fn lane_a() -> impl View<Arity = bunny_ui::view::Single> {
    text!("the surface").on_click(|| {}).id("surface")
}

fn lane_b() -> impl View<Arity = bunny_ui::view::Single> {
    text!("the dock").on_click(|| {}).id("dock")
}

/// The room with a dock that opens and closes beside the surface.
#[derive(Clone)]
struct Room {
    seam: State<f64>,
    dock_open: State<bool>,
}

impl Component for Room {
    fn body(self, _ctx: &Context) -> impl View {
        let open = self.dock_open.get();
        hsplit(self.seam.binding(), lane_a(), lane_b()).min_sizes(100.0, 100.0).hide_trailing(!open)
    }
}

fn hit_path(runtime: &Runtime, view: &impl View, name: &str) -> Option<String> {
    runtime
        .settled_layout(view, Proposal::exact(SIZE))
        .hits
        .iter()
        .find(|(path, _)| path.contains(name))
        .map(|(path, _)| path.clone())
}

#[test]
fn a_hidden_trailing_lane_gives_the_leading_one_the_whole_room() {
    let seam = State::new(200.0);
    let runtime = Runtime::new();
    let split = hsplit(seam.binding(), text!("alone"), text!("the dock")).hide_trailing(true);
    let shown = runtime.display_frame(&split, SIZE);
    let alone = Runtime::new().display_frame(&text!("alone"), SIZE);
    assert_eq!(shown.as_slice(), alone.as_slice(), "the leading lane alone is the whole picture");
}

#[test]
fn a_hidden_leading_lane_gives_the_trailing_one_the_whole_room() {
    let seam = State::new(200.0);
    let runtime = Runtime::new();
    let split = hsplit(seam.binding(), text!("the dock"), text!("alone")).hide_leading(true);
    let shown = runtime.display_frame(&split, SIZE);
    let alone = Runtime::new().display_frame(&text!("alone"), SIZE);
    assert_eq!(shown.as_slice(), alone.as_slice(), "the trailing lane alone is the whole picture");
}

#[test]
fn a_lane_keeps_its_place_while_the_other_hides_and_returns() {
    let seam = State::new(400.0);
    let dock_open = State::new(true);
    let runtime = Runtime::new();
    let room = Room { seam, dock_open };
    let _ = runtime.display_frame(&room, SIZE);
    let surface = hit_path(&runtime, &room, "[surface]").expect("the surface is a hit");
    assert!(hit_path(&runtime, &room, "[dock]").is_some(), "the dock shows");
    dock_open.set(false);
    let _ = runtime.display_frame(&room, SIZE);
    assert_eq!(hit_path(&runtime, &room, "[surface]").as_deref(), Some(surface.as_str()), "the surface stayed where it was");
    assert!(hit_path(&runtime, &room, "[dock]").is_none(), "the dock is gone");
    dock_open.set(true);
    let _ = runtime.display_frame(&room, SIZE);
    assert_eq!(hit_path(&runtime, &room, "[surface]").as_deref(), Some(surface.as_str()), "the surface is still the same lane");
    assert!(hit_path(&runtime, &room, "[dock]").is_some(), "the dock came back");
}
