//! The world ruler: what a world costs the CPU each frame.
//!
//! ```sh
//! cargo run --release -p bunny-ui-core --features 3d --example bench_world
//! ```
//!
//! Four worlds, each with the shape of a real load: a shadowed still
//! life, thousands of instances of one model, one large mesh, and a crowd
//! of skinned figures that walk and play a clip. The same boxes added one
//! by one, each with a material of its own, show what sharing a model
//! saves: a draw for each box in view. For each world it times what
//! the CPU does for a frame a tier draws — `World::frame`: the cull, the
//! levels of detail, the sort, the instancing, the near and far planes,
//! the poses, the shadow's fit — and a pick at the centre. Frames of
//! the crowd also time `damage::reuse`, which finds the rects a redraw
//! needs, and a parallel eye that pans at Low times the slide.
//!
//! A second table draws each world on the CPU, with the software
//! rasterizer every tier is held against and that draws where no tier
//! is: its time is the floor's frame.
//!
//! The GPU's own time is not here: each tier reads it with its timers,
//! and the governor weighs it.

extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use std::cell::RefCell;
use std::rc::Rc;

use bunny_ui::prelude::*;
use bunny_ui::world::damage;
use bunny_ui::world::math::Quat;
use bunny_ui::world::skin::{Channel, Clip, Path, Play, Skeleton, SkeletonNode, Skin};
use bunny_ui::world::{Background, Camera, Light, Material, Mesh, Model, Quality, Transform, Vec3, World, WorldFrame, WorldStyle, soft, vec3};

// the stage table is the frame bench's: a world has no stages of its own
#[allow(dead_code)]
#[path = "support/harness.rs"]
mod harness;

use harness::{CountingAllocator, Report, measure, print_reports};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const WARMUP: usize = 20;
const FRAMES: usize = 300;
/// The software rasterizer draws every pixel: fewer frames.
const RASTER_WARMUP: usize = 2;
const RASTER_FRAMES: usize = 12;

/// The target a tier draws: a box of 1280 × 720 pixels.
const TARGET: (u32, u32) = (1280, 720);
/// The CPU's own picture, as the floor draws it.
const RASTER: (u32, u32) = (640, 360);
/// One beat of a 60 Hz display.
const BEAT: f64 = 1.0 / 60.0;

fn style(quality: Quality) -> WorldStyle {
    WorldStyle { background: Background::Color(Color::rgb(236, 233, 226)), quality, ..WorldStyle::default() }
}

fn floor(world: &World, side: f64) {
    world.add(Mesh::plane(side, side), Material::color(Color::rgb(200, 196, 188))).set_position(vec3(0.0, -0.5, 0.0));
}

/// A floor, forty boxes and ten balls under a sun that casts.
fn still_life() -> World {
    let world = World::new();
    floor(&world, 24.0);
    let cube = Mesh::cuboid(1.0, 1.0, 1.0);
    let ball = Mesh::sphere(0.45);
    for index in 0..40 {
        let (x, z) = ((index % 8) as f64 * 2.5 - 8.75, (index / 8) as f64 * 2.5 - 5.0);
        let tint = Color::rgb(160 + (index * 7 % 90) as u8, 120, 60 + (index * 13 % 120) as u8);
        world.add(cube.clone(), Material::color(tint)).set_position(vec3(x, 0.0, z));
    }
    for index in 0..10 {
        let x = index as f64 * 2.0 - 9.0;
        world.add(ball.clone(), Material::color(Color::rgb(70, 110, 200)).roughness(0.3).metallic(0.2)).set_position(vec3(x, -0.05, 7.0));
    }
    world.add_light(Light::sun().toward(vec3(0.6, -1.0, 0.45)).with_shadows());
    world
}

/// 4 096 boxes of one mesh on a grid, each tinted its own way. With
/// `shared`, they share one model and the boxes in view draw as one;
/// without, each is added with a material of its own.
fn instances(shared: bool) -> World {
    let world = World::new();
    floor(&world, 140.0);
    let cube = Mesh::cuboid(0.8, 0.8, 0.8);
    let model = Model::single(cube.clone(), Material::color(Color::WHITE));
    for index in 0..4096 {
        let (x, z) = ((index % 64) as f64 * 2.0 - 63.0, (index / 64) as f64 * 2.0 - 63.0);
        let tint = Color::rgb(90 + (index % 160) as u8, 120, 200 - (index / 64 % 160) as u8);
        let entity = if shared { world.add_model(model.clone()) } else { world.add(cube.clone(), Material::color(Color::WHITE)) };
        entity.set_position(vec3(x, -0.1, z)).set_tint(tint);
    }
    world.add_light(Light::sun().toward(vec3(-1.0, -2.0, -1.5)).with_shadows());
    world
}

/// One ball of 262 144 triangles over a floor.
fn large_mesh() -> World {
    let world = World::new();
    floor(&world, 12.0);
    world.add(Mesh::sphere_with(2.0, 512, 256), Material::color(Color::rgb(222, 120, 60)).roughness(0.4)).set_position(vec3(0.0, 1.5, 0.0));
    world.add_light(Light::sun().toward(vec3(-1.0, -2.0, -1.5)).with_shadows());
    world
}

/// `count` figures of two joints, eight to a row: each walks across the
/// floor and sways with a clip that loops, and the sun casts their poses.
fn crowd(count: usize) -> World {
    let world = World::new();
    floor(&world, 40.0);
    let skeleton = Skeleton::new(vec![
        SkeletonNode { name: None, parent: None, rest: Transform::IDENTITY },
        SkeletonNode { name: None, parent: Some(0), rest: Transform::IDENTITY },
    ]);
    let skin = Skin::new(skeleton, vec![0, 1], Vec::new()).expect("two joints");
    let mut data = Mesh::cuboid(0.4, 1.6, 0.4).data().clone();
    data.joints = vec![[0, 1, 0, 0]; data.positions.len()];
    data.weights = data.positions.iter().map(|p| if p[1] > 0.0 { [0, 255, 0, 0] } else { [255, 0, 0, 0] }).collect();
    let body = Model::single(Mesh::new(data), Material::color(Color::rgb(70, 110, 200))).skinned(skin);
    let lean = Quat::from_axis_angle(Vec3::Z, 0.3);
    let sway = Clip::new(
        None,
        vec![Channel {
            node: 1,
            path: Path::Rotation,
            step: false,
            times: Rc::from([0.0, 0.5, 1.0]),
            values: Rc::from([0.0, 0.0, 0.0, 1.0, lean.x as f32, lean.y as f32, lean.z as f32, lean.w as f32, 0.0, 0.0, 0.0, 1.0]),
        }],
    );
    let now = motor::task::now();
    for index in 0..count {
        let (x, z) = ((index % 8) as f64 * 3.0 - 10.5, (index / 8) as f64 * 3.0 - 10.5);
        let figure = world.add_model(body.clone());
        figure.set_position(vec3(x, 0.3, z)).move_to(vec3(x + 6.0, 0.3, z), 0.5);
        figure.set_play(Some(Play { clip: sway.clone(), start: now - index as f64 * 0.1, speed: 1.0, looped: true, from: None }));
    }
    world.add_light(Light::sun().toward(vec3(1.0, -0.9, 0.6)).with_shadows());
    world
}

/// The frame a tier would draw, timed; the clock moves a beat first.
fn frame_report(label: &str, world: &World, camera: Camera, quality: Quality, moves: bool) -> Report {
    let style = style(quality);
    measure(
        label,
        WARMUP,
        FRAMES,
        || {
            if moves {
                motor::task::advance(BEAT);
            }
        },
        || world.frame(&camera, &style, TARGET, 1.0).draws.len(),
    )
}

/// A pick at the centre of the box.
fn pick_report(label: &str, world: &World, camera: Camera) -> Report {
    let size = Size { width: TARGET.0 as f64, height: TARGET.1 as f64 };
    let centre = Point { x: size.width / 2.0, y: size.height / 2.0 };
    measure(label, WARMUP, FRAMES, || {}, || usize::from(world.pick(camera, centre, size).is_some()))
}

/// The rects a frame redraws over the last, timed apart from the frame:
/// `step` moves the world or the camera between them. A frame drawn
/// whole counts no rects.
fn reuse_report(label: &str, world: &World, mut camera: impl FnMut() -> Camera, quality: Quality, mut step: impl FnMut()) -> Report {
    let style = style(quality);
    let held = RefCell::new(Rc::new(world.frame(&camera(), &style, TARGET, 1.0)));
    let next: RefCell<Option<Rc<WorldFrame>>> = RefCell::new(None);
    measure(
        label,
        WARMUP,
        FRAMES,
        || {
            step();
            if let Some(done) = next.borrow_mut().take() {
                *held.borrow_mut() = done;
            }
            *next.borrow_mut() = Some(Rc::new(world.frame(&camera(), &style, TARGET, 1.0)));
        },
        || {
            let next = next.borrow();
            let next = next.as_ref().expect("a frame to reuse into");
            damage::reuse(&held.borrow(), next).map_or(0, |reuse| reuse.rects.len())
        },
    )
}

/// The world drawn on the CPU at the floor's size.
fn raster_report(label: &str, world: &World, camera: Camera) -> Report {
    // the CPU draws one sample a pixel, at Low
    let style = style(Quality::Low);
    measure(
        label,
        RASTER_WARMUP,
        RASTER_FRAMES,
        || {},
        || {
            let frame = world.frame(&camera, &style, RASTER, 1.0);
            soft::draw(&frame).rgba.len() / 4
        },
    )
}

fn main() {
    let still = still_life();
    let still_eye = Camera::orbiting(Vec3::ZERO, 30.0, 30.0, 18.0);
    let many = instances(true);
    let apart = instances(false);
    let many_eye = Camera::orbiting(Vec3::ZERO, 30.0, 35.0, 90.0);
    let large = large_mesh();
    let large_eye = Camera::orbiting(vec3(0.0, 1.0, 0.0), 30.0, 20.0, 9.0);
    let walkers = crowd(64);
    let walkers_eye = Camera::orbiting(Vec3::ZERO, 20.0, 35.0, 30.0);
    let few = crowd(4);

    let frames = vec![
        frame_report("frame · still life, sun casts (Medium)", &still, still_eye, Quality::Medium, false),
        frame_report("frame · 4 096 boxes, one model (Medium)", &many, many_eye, Quality::Medium, false),
        frame_report("frame · 4 096 boxes, added apart (Medium)", &apart, many_eye, Quality::Medium, false),
        frame_report("frame · 262 144 triangles (Medium)", &large, large_eye, Quality::Medium, false),
        frame_report("frame · 64 skinned walkers (Medium)", &walkers, walkers_eye, Quality::Medium, true),
        frame_report("frame · 64 skinned walkers (High)", &walkers, walkers_eye, Quality::High, true),
        pick_report("pick · 4 096 boxes", &many, many_eye),
        pick_report("pick · 262 144 triangles", &large, large_eye),
        reuse_report("reuse · 4 walkers, a beat apart (Low)", &few, || walkers_eye, Quality::Low, || {
            motor::task::advance(BEAT);
        }),
        {
            // a parallel eye over the still life pans by a pixel a beat
            let pan = std::cell::Cell::new(0.0);
            let height = 12.0;
            reuse_report(
                "reuse · a parallel pan at Low (slide)",
                &still,
                || Camera::orbiting(vec3(pan.get(), 0.0, 0.0), 0.0, 35.0, 18.0).parallel_height(height),
                Quality::Low,
                || pan.set(pan.get() + height / TARGET.1 as f64),
            )
        },
    ];
    print_reports(&format!("World frames on the CPU, {} × {} target (cmds = draws, or rects)", TARGET.0, TARGET.1), &frames);

    let rasters = vec![
        raster_report("draw · still life, sun casts", &still, still_eye),
        raster_report("draw · 4 096 boxes", &many, many_eye),
        raster_report("draw · 262 144 triangles", &large, large_eye),
        raster_report("draw · 64 skinned walkers", &walkers, walkers_eye),
    ];
    print_reports(&format!("The CPU's own picture, {} × {} at Low (cmds = pixels)", RASTER.0, RASTER.1), &rasters);
}
