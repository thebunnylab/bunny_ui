//! The world's parity catalog: the frames every tier's world pass is held
//! against, and what each may differ by.
//!
//! One list, so "this tier passes six worlds" means the same sentence on
//! every backend. The CPU rasterizer draws each frame too, and the
//! difference between the two pictures is the certification.

use std::rc::Rc;

use crate::image_engine::ImageSource;
use crate::layout::Color;
use crate::world::camera::Camera;
use crate::world::frame::{Background, Quality, WorldFrame, WorldStyle};
use crate::world::material::{FloorMap, Light, LineDepth, LineStyle, Material, Model, PartState};
use crate::world::math::Mat4;
use crate::world::math::{Vec3, vec2, vec3};
use crate::world::mesh::{Mesh, MeshData};
use crate::world::store::World;

/// How many worlds the catalog holds.
pub const COUNT: u32 = 15;

/// What a tier's picture may differ from the CPU's by: the share of
/// pixels whose worst channel is more than three codes apart, and more
/// than sixteen.
#[derive(Clone, Copy, Debug)]
pub struct Tolerance {
    pub beyond_three: f64,
    pub beyond_sixteen: f64,
}

/// Interiors agree to the dither's step; the pixels a triangle's edge
/// crosses may fall to either side.
const EXACT: Tolerance = Tolerance { beyond_three: 0.02, beyond_sixteen: 0.01 };

/// A shadow's edge is a filtered blend of comparisons, and a texture
/// unit keeps fewer bits of the blend than the CPU: the penumbra may sit
/// a few codes apart.
const PENUMBRA: Tolerance = Tolerance { beyond_three: 0.03, beyond_sixteen: 0.01 };

/// One world of the catalog: the frame, its name, its tolerance.
pub struct WorldScene {
    pub frame: Rc<WorldFrame>,
    pub name: &'static str,
    pub tolerance: Tolerance,
    /// The textures the frame names, as pictures any image engine reads.
    pub pictures: Vec<ImageSource>,
}

fn style(quality: Quality) -> WorldStyle {
    WorldStyle { background: Background::Color(Color::rgb(236, 233, 226)), quality, ..WorldStyle::default() }
}

/// A box, a ball and a floor under a sun: every term of the shading.
fn still_life() -> World {
    let world = World::new();
    world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(222, 120, 60)));
    world.add(Mesh::plane(6.0, 6.0), Material::color(Color::rgb(200, 196, 188))).set_position(vec3(0.0, -0.5, 0.0));
    world.add(Mesh::sphere(0.45), Material::color(Color::rgb(70, 110, 200)).roughness(0.3).metallic(0.2)).set_position(vec3(1.25, -0.05, 0.5));
    world.add_light(Light::sun().toward(vec3(-1.0, -2.0, -1.5)));
    world
}

/// The still life under a sun that casts: the box's shadow falls on the
/// floor and on the ball.
fn shadowed_still_life() -> World {
    let world = World::new();
    world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(222, 120, 60)));
    world.add(Mesh::plane(6.0, 6.0), Material::color(Color::rgb(200, 196, 188))).set_position(vec3(0.0, -0.5, 0.0));
    world.add(Mesh::sphere(0.45), Material::color(Color::rgb(70, 110, 200)).roughness(0.3).metallic(0.2)).set_position(vec3(1.25, -0.05, 0.5));
    world.add_light(Light::sun().toward(vec3(0.6, -1.0, 0.45)).with_shadows());
    world
}

/// A floor map of `side` texels over a `span`-meter square centred on the
/// origin: the shade a box at the origin leaves under itself, and a warm
/// pool of baked lamp light off to one side.
pub fn floor_map(side: u32, span: f64) -> FloorMap {
    let mut rgba = Vec::with_capacity((side * side * 4) as usize);
    for row in 0..side {
        for column in 0..side {
            let at = |index: u32| -span * 0.5 + (index as f64 + 0.5) / side as f64 * span;
            let (x, z) = (at(column), at(row));
            let near = (x * x + z * z).sqrt();
            let shade = (0.2 + 0.8 * (near - 0.5) / 0.8).clamp(0.2, 1.0);
            let (dx, dz) = (x - 1.4, z + 0.9);
            let pool = (1.0 - (dx * dx + dz * dz).sqrt() / 1.4).clamp(0.0, 1.0);
            rgba.extend_from_slice(&[(150.0 * pool) as u8, (96.0 * pool) as u8, (40.0 * pool) as u8, (255.0 * shade) as u8]);
        }
    }
    FloorMap {
        picture: ImageSource::rgba(0xF100_0000_0000_0001, (side, side), rgba),
        origin: vec2(-span * 0.5, -span * 0.5),
        size: vec2(span, span),
    }
}

/// A checker of `cells` squares a side over a clear band.
pub fn checker(cells: u32, side: u32) -> ImageSource {
    let mut rgba = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let on = ((x * cells / side) + (y * cells / side)).is_multiple_of(2);
            let pixel = match (y < side / 8, on) {
                (true, _) => [0, 0, 0, 0],
                (false, true) => [230, 70, 40, 255],
                (false, false) => [250, 240, 220, 255],
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    ImageSource::rgba(0xC4EC_0000_0000_0001, (side, side), rgba)
}

/// World `index` of the catalog.
pub fn world_scene(index: u32) -> WorldScene {
    let frame = |world: &World, camera: &Camera, quality: Quality, size: (u32, u32)| Rc::new(world.frame(camera, &style(quality), size, 1.0));
    // a quality's shadow, drawn at one sample like the CPU's picture
    let one_sample = |world: &World, camera: &Camera, quality: Quality, size: (u32, u32)| {
        let mut frame = world.frame(camera, &style(quality), size, 1.0);
        frame.samples = 1;
        Rc::new(frame)
    };
    match index % COUNT {
        0 => WorldScene {
            frame: frame(&still_life(), &Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0), Quality::Low, (160, 120)),
            name: "a lit still life",
            tolerance: EXACT,
            pictures: Vec::new(),
        },
        1 => WorldScene {
            frame: frame(&still_life(), &Camera::orbiting(Vec3::ZERO, -45.0, 35.264, 8.0).parallel(), Quality::Low, (128, 96)),
            name: "a parallel lens",
            tolerance: EXACT,
            pictures: Vec::new(),
        },
        2 => {
            let world = World::new();
            world.add(Mesh::plane(16.0, 16.0), Material::color(Color::rgb(228, 224, 216)));
            let outline = [vec2(0.0, 0.0), vec2(2.4, 0.0), vec2(2.4, 2.4), vec2(0.0, 2.4)];
            let hole = [vec2(0.6, 0.6), vec2(1.8, 0.6), vec2(1.8, 1.8), vec2(0.6, 1.8)];
            let ring = Mesh::extrude(&outline, &[&hole], 0.4);
            let at = vec3(1.2, 0.0, -2.8);
            world.add(ring.clone(), Material::color(Color::rgb(246, 244, 238))).set_position(at);
            world.add_lines(ring.edges(30.0), LineStyle::solid(Color::rgb(20, 20, 24), 1.2)).set_position(at);
            let cube = Mesh::cuboid(1.2, 1.2, 1.2);
            let cube_at = vec3(-1.7, 0.6, 0.3);
            world.add(cube.clone(), Material::color(Color::rgb(226, 114, 60))).set_position(cube_at);
            world.add_lines(cube.edges(30.0), LineStyle::solid(Color::rgb(44, 46, 54), 1.5)).set_position(cube_at);
            world
                .add_lines(cube.edges(30.0), LineStyle::solid(Color::rgb(44, 46, 54), 1.0).dashed(4.0, 4.0).depth(LineDepth::Hidden))
                .set_position(cube_at);
            WorldScene {
                frame: frame(&world, &Camera::orbiting(vec3(0.0, 0.6, 0.0), 32.0, 24.0, 9.0), Quality::Low, (240, 140)),
                name: "edges, hidden and dashed",
                tolerance: Tolerance { beyond_three: 0.03, beyond_sixteen: 0.015 },
                pictures: Vec::new(),
            }
        }
        3 => {
            let world = still_life();
            world.add(Mesh::cuboid(2.4, 0.8, 0.1), Material::color(Color::rgba(90, 160, 220, 110)).blend()).set_position(vec3(0.0, 0.0, 1.2));
            world
                .add(Mesh::sphere(0.2), Material::unlit(Color::rgb(255, 200, 80)).emissive(Color::rgb(255, 200, 80), 2.0))
                .set_position(vec3(-1.2, 0.0, 0.6));
            WorldScene {
                frame: frame(&world, &Camera::orbiting(Vec3::ZERO, 10.0, 20.0, 5.0), Quality::Low, (160, 120)),
                name: "see-through and glowing",
                tolerance: EXACT,
                pictures: Vec::new(),
            }
        }
        4 => {
            let world = World::new();
            let floor = Mesh::new(MeshData {
                positions: vec![[-4.0, 0.0, 4.0], [4.0, 0.0, 4.0], [4.0, 0.0, -40.0], [-4.0, 0.0, -40.0]],
                normals: vec![[0.0, 1.0, 0.0]; 4],
                uvs: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 22.0], [0.0, 22.0]],
                indices: vec![0, 1, 2, 0, 2, 3],
                ..MeshData::default()
            });
            let picture = checker(8, 64);
            world.add(floor, Material::color(Color::WHITE).texture(picture.clone()).mask(0.5));
            WorldScene {
                frame: frame(&world, &Camera::looking_at(vec3(0.0, 1.2, 5.0), vec3(0.0, 0.0, -6.0)), Quality::Low, (160, 120)),
                name: "a textured floor, far and cut out",
                // the hardware picks a level per two-by-two quad, the CPU
                // per pixel: the far rows may sit a few codes apart
                tolerance: Tolerance { beyond_three: 0.25, beyond_sixteen: 0.03 },
                pictures: vec![picture],
            }
        }
        6 => WorldScene {
            frame: frame(&shadowed_still_life(), &Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0), Quality::Low, (160, 120)),
            name: "a cast shadow",
            tolerance: PENUMBRA,
            pictures: Vec::new(),
        },
        7 => WorldScene {
            frame: one_sample(&shadowed_still_life(), &Camera::orbiting(Vec3::ZERO, 20.0, 35.0, 4.0), Quality::Medium, (200, 150)),
            name: "a shadow of four lookups",
            tolerance: PENUMBRA,
            pictures: Vec::new(),
        },
        14 => {
            // the cut-out floor at four samples: its edges become coverage,
            // which the CPU's one sample cannot show
            let world = World::new();
            let floor = Mesh::new(MeshData {
                positions: vec![[-4.0, 0.0, 4.0], [4.0, 0.0, 4.0], [4.0, 0.0, -40.0], [-4.0, 0.0, -40.0]],
                normals: vec![[0.0, 1.0, 0.0]; 4],
                uvs: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 22.0], [0.0, 22.0]],
                indices: vec![0, 1, 2, 0, 2, 3],
                ..MeshData::default()
            });
            let picture = checker(8, 64);
            world.add(floor, Material::color(Color::WHITE).texture(picture.clone()).mask(0.5));
            WorldScene {
                frame: frame(&world, &Camera::looking_at(vec3(0.0, 1.2, 5.0), vec3(0.0, 0.0, -6.0)), Quality::Medium, (160, 120)),
                name: "a cut-out at four samples",
                tolerance: Tolerance { beyond_three: 0.25, beyond_sixteen: 0.08 },
                pictures: vec![picture],
            }
        }
        13 => {
            // a sky over a ground, the horizon across the middle: the
            // backdrop follows the view's tilt, not the picture's rows
            let world = World::new();
            world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(222, 120, 60))).set_position(vec3(0.0, 0.0, -2.0));
            let sky = WorldStyle {
                background: Background::Sky { zenith: Color::rgb(92, 140, 210), horizon: Color::rgb(222, 230, 238), ground: Color::rgb(150, 144, 132) },
                ..style(Quality::Low)
            };
            WorldScene {
                frame: Rc::new(world.frame(&Camera::looking_at(vec3(0.0, 1.0, 4.0), vec3(0.0, 1.6, -4.0)), &sky, (200, 140), 1.0)),
                name: "a sky over a ground",
                tolerance: EXACT,
                pictures: Vec::new(),
            }
        }
        12 => {
            // three boxes merged into one mesh: the middle one hides (and
            // casts nothing), the last takes a tint; a ball beside them
            // stands in a blue room's ambient
            let world = World::new();
            world.add(Mesh::plane(6.0, 6.0), Material::color(Color::rgb(214, 210, 202))).set_position(vec3(0.0, -0.5, 0.0));
            let piece = Model::single(Mesh::cuboid(0.7, 1.0, 0.7), Material::color(Color::rgb(222, 120, 60)));
            let merged = Model::merge((0..3u16).map(|part| (piece.clone(), Mat4::translation(vec3(part as f64 * 1.0 - 1.0, 0.0, 0.0)), part)));
            let row = world.add_model(merged);
            row.set_parts(Some(std::rc::Rc::from(vec![PartState::default(), PartState::hidden(), PartState::tinted(Color::rgb(120, 200, 255))])));
            world
                .add(Mesh::sphere(0.4), Material::color(Color::rgb(236, 232, 224)))
                .set_position(vec3(0.0, -0.1, 1.1))
                .set_ambient(Color::rgb(90, 140, 255));
            world.add_light(Light::sun().toward(vec3(0.6, -1.0, -0.5)).with_shadows());
            WorldScene {
                frame: frame(&world, &Camera::orbiting(Vec3::ZERO, 15.0, 30.0, 5.0), Quality::Low, (200, 140)),
                name: "parts that hide and tint",
                tolerance: PENUMBRA,
                pictures: Vec::new(),
            }
        }
        11 => {
            let world = World::new();
            let map = floor_map(32, 6.0);
            let picture = map.picture.clone();
            world.set_floor_map(Some(map));
            world.add(Mesh::plane(6.0, 6.0), Material::color(Color::rgb(214, 210, 202))).set_position(vec3(0.0, -0.5, 0.0)).set_reads_floor_map(true);
            world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(90, 140, 210)));
            world.add_light(Light::sun().toward(vec3(-1.0, -2.0, -1.5)));
            WorldScene {
                frame: frame(&world, &Camera::orbiting(Vec3::ZERO, 25.0, 40.0, 6.0), Quality::Low, (200, 140)),
                name: "a floor map",
                tolerance: EXACT,
                pictures: vec![picture],
            }
        }
        10 => {
            // two rooms at night, a lamp in each: each lamp reaches its own
            // room's group alone, though the light would reach both
            let world = World::new();
            world.add_light(Light::sky(Color::rgb(30, 34, 46), Color::rgb(20, 18, 16)));
            let floor = Mesh::plane(3.0, 3.0);
            let block = Mesh::cuboid(0.8, 0.8, 0.8);
            for (room, x) in [(1u8, -1.6), (2u8, 1.6)] {
                world.add(floor.clone(), Material::color(Color::rgb(210, 204, 196))).set_position(vec3(x, 0.0, 0.0)).set_light_group(room);
                world.add(block.clone(), Material::color(Color::rgb(222, 120, 60))).set_position(vec3(x, 0.4, -0.4)).set_light_group(room);
                let lamp = if room == 1 { Color::rgb(255, 196, 140) } else { Color::rgb(150, 190, 255) };
                world.add_light(Light::point(lamp).intensity(2.5).range(4.5).radius(0.4).groups(1 << room)).set_position(vec3(x * 0.4, 1.2, 0.6));
            }
            WorldScene {
                frame: frame(&world, &Camera::orbiting(vec3(0.0, 0.3, 0.0), 0.0, 38.0, 6.0), Quality::Low, (200, 120)),
                name: "lamps by group",
                tolerance: EXACT,
                pictures: Vec::new(),
            }
        }
        9 => {
            let world = World::new();
            world.add(Mesh::plane(8.0, 8.0), Material::color(Color::rgb(228, 224, 216)));
            let outline = [vec2(0.0, 0.0), vec2(2.4, 0.0), vec2(2.4, 2.4), vec2(0.0, 2.4)];
            let hole = [vec2(0.6, 0.6), vec2(1.8, 0.6), vec2(1.8, 1.8), vec2(0.6, 1.8)];
            let ring = Mesh::extrude(&outline, &[&hole], 0.4);
            let at = vec3(-1.2, 0.0, 1.2);
            world.add(ring.clone(), Material::color(Color::rgb(226, 114, 60))).set_position(at);
            world.add_lines(ring.edges(30.0), LineStyle::solid(Color::rgb(30, 32, 40), 1.2)).set_position(at);
            let xray = WorldStyle { xray: true, ..style(Quality::Low) };
            WorldScene {
                frame: Rc::new(world.frame(&Camera::orbiting(vec3(0.0, 0.2, 0.0), 28.0, 32.0, 5.5), &xray, (200, 140), 1.0)),
                name: "an X-ray",
                tolerance: Tolerance { beyond_three: 0.03, beyond_sixteen: 0.015 },
                pictures: Vec::new(),
            }
        }
        8 => WorldScene {
            frame: one_sample(&shadowed_still_life(), &Camera::orbiting(vec3(0.5, 0.0, 0.3), 50.0, 30.0, 3.5).parallel(), Quality::High, (200, 150)),
            name: "a shadow of nine lookups",
            tolerance: PENUMBRA,
            pictures: Vec::new(),
        },
        _ => WorldScene {
            frame: frame(&still_life(), &Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0), Quality::Medium, (160, 120)),
            name: "four samples",
            // the CPU takes one sample: only the edges may move
            tolerance: Tolerance { beyond_three: 0.08, beyond_sixteen: 0.06 },
            pictures: Vec::new(),
        },
    }
}

/// How two pictures differ: the worst channel, and how many pixels sit
/// more than one, three and sixteen codes apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spread {
    pub worst: u8,
    pub beyond: [usize; 3],
    pub pixels: usize,
}

impl Spread {
    pub fn of(ours: &[u8], theirs: &[u8]) -> Spread {
        let mut out = Spread { worst: 0, beyond: [0; 3], pixels: ours.len().min(theirs.len()) / 4 };
        for (p, q) in ours.as_chunks::<4>().0.iter().zip(theirs.as_chunks::<4>().0) {
            let delta = (0..4).map(|k| p[k].abs_diff(q[k])).max().unwrap_or(0);
            out.worst = out.worst.max(delta);
            for (slot, limit) in [1u8, 3, 16].iter().enumerate() {
                if delta > *limit {
                    out.beyond[slot] += 1;
                }
            }
        }
        out
    }

    /// Whether the spread is inside `tolerance`.
    pub fn within(&self, tolerance: Tolerance) -> bool {
        let share = |count: usize| count as f64 / self.pixels.max(1) as f64;
        share(self.beyond[1]) <= tolerance.beyond_three && share(self.beyond[2]) <= tolerance.beyond_sixteen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_world_of_the_catalog_draws_on_the_cpu() {
        for index in 0..COUNT {
            let scene = world_scene(index);
            let picture = crate::world::soft::draw(&scene.frame);
            assert_eq!((picture.width, picture.height), scene.frame.size, "{}", scene.name);
            let painted = picture.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count();
            assert!(painted > 0, "{} painted nothing", scene.name);
            // the CPU against itself is the zero of the scale
            assert!(Spread::of(&picture.rgba, &picture.rgba).within(scene.tolerance));
            if let Some(dump) = std::env::var_os("BUNNY_WORLD_DUMP") {
                let (w, h) = scene.frame.size;
                let _ = std::fs::write(std::path::Path::new(&dump).join(format!("catalog-{index}-{w}x{h}.rgba")), &picture.rgba);
            }
        }
    }

    #[test]
    fn every_shadow_of_the_catalog_shows() {
        for index in 0..COUNT {
            let scene = world_scene(index);
            if scene.frame.shadow.is_none() {
                continue;
            }
            let shaded = crate::world::soft::draw(&scene.frame);
            let mut sunlit = (*scene.frame).clone();
            sunlit.shadow = None;
            sunlit.uniforms.shadow = [0.0; 4];
            let plain = crate::world::soft::draw(&sunlit);
            let darker = shaded.rgba.as_chunks::<4>().0.iter().zip(plain.rgba.as_chunks::<4>().0).filter(|(a, b)| a[0] as i32 + 20 < b[0] as i32).count();
            let share = darker as f64 / scene.frame.size.0 as f64 / scene.frame.size.1 as f64;
            assert!(share > 0.02, "{}: {share} of the picture in clear shadow", scene.name);
        }
    }
}
