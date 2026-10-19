//! A world in a box: one write is one repaint, a still world is still,
//! a flying camera keeps the frames coming until it lands.

#![cfg(feature = "3d")]

use bunny_ui_core as bunny_ui;

use std::cell::Cell;

use bunny_ui::custom::canvas;
use bunny_ui::image_engine::ImageSource;
use bunny_ui::layout::{Color, DrawCommand, Rect, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;
use bunny_ui::world::{Camera, Material, Mesh, World, vec3};

const SIZE: Size = Size { width: 320.0, height: 200.0 };

thread_local! {
    static BODIES: Cell<u32> = const { Cell::new(0) };
}

#[derive(Clone, Copy)]
struct Viewer {
    world: World,
    camera: State<Camera>,
    twice: bool,
}

impl Component for Viewer {
    fn body(self) -> impl View {
        BODIES.with(|bodies| bodies.set(bodies.get() + 1));
        canvas(move |ctx, painter| {
            painter.world(ctx.bounds(), self.world, self.camera);
            if self.twice {
                let half = Rect { origin: ctx.bounds().origin, size: Size { width: ctx.size().width * 0.5, height: ctx.size().height } };
                painter.world(half, self.world, self.camera);
            }
        })
        .frame(SIZE.width, SIZE.height)
    }
}

fn mount(twice: bool) -> (Runtime, Viewer, bunny_ui::world::Entity) {
    let world = World::new();
    let cube = world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(200, 90, 50)));
    let viewer = Viewer { world, camera: State::new(Camera::orbiting(vec3(0.0, 0.0, 0.0), 30.0, 25.0, 5.0)), twice };
    let runtime = Runtime::new();
    let _ = runtime.display_frame(&viewer, SIZE);
    let _ = runtime.display_frame(&viewer, SIZE);
    BODIES.with(|bodies| bodies.set(0));
    (runtime, viewer, cube)
}

/// The world pictures of a display list: (slot, generation, veil).
fn worlds(list: &bunny_ui::layout::DisplayList) -> Vec<(u64, u64, u8)> {
    list.iter()
        .filter_map(|command| match command {
            DrawCommand::Image { source: ImageSource::World { key, generation, veil, .. }, .. } => Some((*key, *generation, *veil)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_world_write_is_one_repaint_and_no_body() {
    let (runtime, viewer, cube) = mount(false);
    let before = worlds(&runtime.display_frame(&viewer, SIZE));
    cube.set_position(vec3(0.5, 0.0, 0.0));
    let need = runtime.frame_need();
    assert!(need.paints && !need.dirty, "the box's paint read the world: {need:?}");
    let after = worlds(&runtime.display_frame(&viewer, SIZE));
    assert_eq!(BODIES.with(Cell::get), 0, "no body ran");
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].0, after[0].0, "the same box keeps its slot");
    assert_ne!(before[0].1, after[0].1, "the picture moved");
}

#[test]
fn an_unrelated_frame_keeps_the_world_identity() {
    let (runtime, viewer, _) = mount(false);
    let first = worlds(&runtime.display_frame(&viewer, SIZE));
    let second = worlds(&runtime.display_frame(&viewer, SIZE));
    assert_eq!(first, second, "nothing changed: the same picture, nothing to draw again");
}

#[test]
fn a_world_at_rest_asks_for_no_frame() {
    let (runtime, _, _) = mount(false);
    assert!(!runtime.needs_frame(), "{:?}", runtime.frame_need());
    assert!(!runtime.wants_frame());
    assert_eq!(runtime.frame_pace(), FramePace::Idle);
}

#[test]
fn a_flying_camera_keeps_frames_until_it_lands() {
    let (runtime, viewer, _) = mount(false);
    let to = Camera::orbiting(vec3(0.0, 0.0, 0.0), 120.0, 25.0, 5.0);
    viewer.camera.set(viewer.camera.get().fly_to(to, 0.25));
    let mut generations = vec![worlds(&runtime.display_frame(&viewer, SIZE))[0].1];
    assert!(runtime.wants_frame(), "a flight asks for the next beat");
    let mut beats = 0;
    while runtime.wants_frame() && beats < 100 {
        let _ = runtime.tick(1.0 / 60.0);
        assert!(runtime.frame_need().paints, "each beat repaints the flying box");
        generations.push(worlds(&runtime.display_frame(&viewer, SIZE))[0].1);
        beats += 1;
    }
    assert!((14..=17).contains(&beats), "a quarter second at 60 Hz: {beats} beats");
    generations.dedup();
    assert!(generations.len() > 10, "the picture moved with the flight");
    assert!(!runtime.wants_frame() && !runtime.needs_frame(), "landed: the driver parks");
    assert_eq!(BODIES.with(Cell::get), 0, "a flight runs no body");
}

#[test]
fn two_worlds_in_one_box_take_two_slots() {
    let (runtime, viewer, _) = mount(true);
    let pictures = worlds(&runtime.display_frame(&viewer, SIZE));
    assert_eq!(pictures.len(), 2);
    assert_ne!(pictures[0].0, pictures[1].0);
}

#[test]
fn a_fade_keeps_a_world_a_world() {
    let world = World::new();
    world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::WHITE));
    let camera = State::new(Camera::orbiting(vec3(0.0, 0.0, 0.0), 0.0, 20.0, 4.0));
    let page = canvas(move |ctx, painter| painter.world(ctx.bounds(), world, camera)).frame(80.0, 60.0).opacity(0.5);
    let runtime = Runtime::new();
    let list = runtime.display_frame(&page, Size { width: 80.0, height: 60.0 });
    let pictures = worlds(&list);
    assert_eq!(pictures.len(), 1, "still a world, never a veil around one: {list:?}");
    assert_eq!(pictures[0].2, 128);
}

#[test]
fn a_world_rasters_once_per_generation() {
    let (runtime, viewer, _) = mount(false);
    let list = runtime.display_frame(&viewer, SIZE);
    let source = list
        .iter()
        .find_map(|command| match command {
            DrawCommand::Image { source: source @ ImageSource::World { .. }, .. } => Some(source.clone()),
            _ => None,
        })
        .expect("a world");
    let engine = bunny_ui::image_engine::RawImages::default();
    let first = bunny_ui::image_engine::raster_source(&engine, &source, 64, 40).expect("the oracle draws");
    let second = bunny_ui::image_engine::raster_source(&engine, &source, 64, 40).expect("again");
    assert!(std::rc::Rc::ptr_eq(&first, &second), "once per picture and size");
    assert_eq!(first.rgba.len(), 64 * 40 * 4);
    assert!(first.rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 255), "the cube is there");
}
