//! A world in a box, in a window.
//!
//! ```sh
//! cargo run -p bunny-ui-linux --features 3d --example world_window_linux
//! ```
//!
//! A few shapes on a floor, under a sun that casts shadows and a soft
//! sky. Drag to turn the view around what lies under the pointer; shift
//! and a drag pan it; the wheel or a pinch zooms toward the pointer. A
//! click selects a shape — it takes a tint, and a label that follows
//! the camera — and a click on nothing clears it. The buttons fly the
//! camera: a flight keeps the frames coming until it lands, and a still
//! world asks for none.
//!
//! `BUNNY_PRESENT=cpu` shows the same world drawn by the CPU.

#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
use bunny_ui::world::{
    Background, Camera, Entity, Light, LineDepth, LineStyle, Material, Mesh, Quality, World, WorldStyle, scene_view,
    vec2, vec3,
};

const INK: Color = Color::rgb(44, 46, 54);
const SELECTED: Color = Color::rgb(255, 214, 150);

#[derive(Clone, Copy)]
struct App {
    world: World,
    camera: State<Camera>,
    picked: State<Option<Entity>>,
    quality: State<Quality>,
}

impl Component for App {
    fn body(self) -> impl View {
        let quality = self.quality.get();
        let style = WorldStyle {
            background: Background::Gradient { top: Color::rgb(214, 224, 238), bottom: Color::rgb(244, 241, 236) },
            quality,
            ..WorldStyle::default()
        };
        vstack!(
            hstack!(
                button(text("Perspective"), move || self.fly(perspective())),
                button(text("Isometric"), move || self.fly(isometric())),
                button(text("Above"), move || self.fly(above())),
                spacer(),
                button(text(quality_name(quality)), move || self.quality.set(next(quality))),
            )
            .spacing(8.0)
            .padding(),
            scene_view(self.world, self.camera)
                .orbit()
                .style(style)
                .on_tap(move |hit| self.pick(hit.map(|hit| hit.entity)))
                .label(move || self.picked.get(), "Selected"),
        )
        .spacing(0.0)
    }
}

impl App {
    /// The camera flies from where it is to `to`.
    fn fly(self, to: Camera) {
        self.camera.set(self.camera.get().fly_to(to, 0.6));
    }

    /// The selection moves: the shape it leaves loses its tint.
    fn pick(self, entity: Option<Entity>) {
        if let Some(old) = self.picked.get() {
            old.set_tint(Color::WHITE);
        }
        if let Some(new) = entity {
            new.set_tint(SELECTED);
        }
        self.picked.set(entity);
    }
}

fn perspective() -> Camera {
    Camera::orbiting(vec3(0.0, 0.6, 0.0), 32.0, 24.0, 9.0)
}

/// The classic three-quarter view, with a parallel lens.
fn isometric() -> Camera {
    Camera::orbiting(vec3(0.0, 0.6, 0.0), 45.0, 35.264, 12.0).parallel()
}

fn above() -> Camera {
    Camera::orbiting(vec3(0.0, 0.0, 0.0), 0.0, 89.0, 12.0).parallel()
}

fn quality_name(quality: Quality) -> &'static str {
    match quality {
        Quality::Low => "Quality: low",
        Quality::Medium => "Quality: medium",
        Quality::High => "Quality: high",
    }
}

fn next(quality: Quality) -> Quality {
    match quality {
        Quality::Low => Quality::Medium,
        Quality::Medium => Quality::High,
        Quality::High => Quality::Low,
    }
}

/// A floor, a box, two balls, a column and a low square ring, with the
/// edges a drawing shows.
fn still_life() -> World {
    let world = World::new();
    world.add(Mesh::plane(16.0, 16.0), Material::color(Color::rgb(228, 224, 216)).roughness(0.95));
    let at = vec3(-1.7, 0.6, 0.3);
    let cube = Mesh::cuboid(1.2, 1.2, 1.2);
    world.add(cube.clone(), Material::color(Color::rgb(226, 114, 60))).set_position(at);
    world.add_lines(cube.edges(30.0), LineStyle::solid(INK, 1.5)).set_position(at);
    world.add_lines(cube.edges(30.0), LineStyle::solid(INK, 1.0).dashed(4.0, 4.0).depth(LineDepth::Hidden)).set_position(at);
    world.add(Mesh::sphere(0.7), Material::color(Color::rgb(60, 104, 214)).roughness(0.3)).set_position(vec3(0.5, 0.7, -0.3));
    world
        .add(Mesh::sphere(0.45), Material::color(Color::rgb(214, 218, 226)).metallic(1.0).roughness(0.22))
        .set_position(vec3(1.9, 0.45, 1.2));
    world.add(Mesh::cylinder(0.45, 1.8), Material::color(Color::rgb(66, 158, 112))).set_position(vec3(-0.4, 0.9, -2.4));
    let outline = [vec2(0.0, 0.0), vec2(2.4, 0.0), vec2(2.4, 2.4), vec2(0.0, 2.4)];
    let hole = [vec2(0.6, 0.6), vec2(1.8, 0.6), vec2(1.8, 1.8), vec2(0.6, 1.8)];
    let ring = Mesh::extrude(&outline, &[&hole], 0.4);
    let at = vec3(1.2, 0.0, -2.8);
    world.add(ring.clone(), Material::color(Color::rgb(246, 244, 238))).set_position(at);
    world.add_lines(ring.edges(30.0), LineStyle::solid(INK, 1.2)).set_position(at);
    world.add_light(Light::sun().toward(vec3(-1.0, -2.2, -1.4)).with_shadows());
    world
}

#[cfg(target_os = "linux")]
fn main() {
    let app = App {
        world: still_life(),
        camera: State::new(perspective()),
        picked: State::new(None),
        quality: State::new(Quality::Medium),
    };
    bunny_ui_linux::run_window("bunny — a world in a box", Size { width: 960.0, height: 680.0 }, app);
}

#[cfg(not(target_os = "linux"))]
fn main() {} // this example is Linux-only
