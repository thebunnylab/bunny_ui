//! The frame: what one picture of a world asks of a tier.
//!
//! The core decides everything here, once, on the CPU — which entities
//! the camera sees, which meshes share a draw, the order of the draws,
//! the near and far planes, the lights in the eye's space. A tier
//! uploads what it lacks and runs the draws in order; the software
//! rasterizer runs the same list. Two tiers can only disagree on how a
//! pixel is shaded, never on what is drawn.

use std::rc::Rc;

use super::camera::{Camera, Lens};
use super::material::{AlphaMode, Light, LineDepth, Material, SunFrame};
use super::math::{Aabb, Frustum, Mat4, Plane, Vec3, vec3};
use super::mesh::{Lines, Mesh};
use super::shade::decode;
use super::store::{Shape, WorldData};
use super::wire::{
    FrameUniforms, InstanceData, LineStyleData, MAX_POINTS, MaterialData, instance_flag, material_flag,
};
use crate::image_engine::ImageSource;
use crate::layout::Color;

// MARK: - How a box shows its world

/// What stands behind the world.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Background {
    /// Nothing: the box's own background shows through.
    #[default]
    Transparent,
    Color(Color),
    /// A vertical ramp, top to bottom.
    Gradient { top: Color, bottom: Color },
}

/// What a picture of the world may cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Quality {
    /// Fewer pixels, no multisampling: a weak GPU, a phone, a busy page.
    Low,
    #[default]
    Medium,
    High,
}

impl Quality {
    /// Samples per pixel the picture asks for.
    pub fn samples(self) -> u8 {
        match self {
            Quality::Low => 1,
            Quality::Medium | Quality::High => 4,
        }
    }

    /// The share of the box's pixels the picture is drawn at; the
    /// compositor stretches it to the box.
    pub fn render_scale(self) -> f64 {
        match self {
            Quality::Low => 0.75,
            Quality::Medium | Quality::High => 1.0,
        }
    }
}

/// How a box shows its world: the backdrop, the cost, a cut.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldStyle {
    pub background: Background,
    pub quality: Quality,
    /// The world beyond this plane is cut away — the side its normal
    /// points to stays.
    pub section: Option<Plane>,
    /// Multiplies all light before the tone curve.
    pub exposure: f32,
    /// The most pictures a second an animation may ask for.
    pub max_fps: Option<f64>,
}

impl Default for WorldStyle {
    fn default() -> WorldStyle {
        WorldStyle { background: Background::Transparent, quality: Quality::Medium, section: None, exposure: 1.0, max_fps: None }
    }
}

// MARK: - The frame

/// The passes of a frame, run in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pass {
    /// Opaque and cut-out surfaces, front to back. They write depth.
    Opaque,
    /// Lines hidden by the surfaces in front of them.
    Lines,
    /// See-through and glowing surfaces, back to front. No depth write.
    Transparent,
    /// Lines shown only where a surface stands in front of them.
    HiddenLines,
    /// Lines over everything.
    Overlay,
}

/// How a draw's colour meets the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    /// Replaces it.
    Replace,
    /// Over it, by alpha (premultiplied).
    Alpha,
    /// Adds to it.
    Add,
}

/// What one draw draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DrawKind {
    /// A run of a mesh's indices, once per instance.
    Mesh {
        /// Into [`WorldFrame::meshes`].
        mesh: u16,
        first_index: u32,
        index_count: u32,
        /// Into [`WorldFrame::textures`].
        texture: Option<u16>,
        /// Back faces are dropped.
        cull_back: bool,
        /// The instances mirror the mesh: their front faces turn clockwise.
        mirrored: bool,
        blend: Blend,
        /// Cut-out: a fragment below its material's cutoff is discarded.
        mask: bool,
    },
    /// Every segment of a set of lines, once, for one instance.
    Lines {
        /// Into [`WorldFrame::lines`].
        lines: u16,
        /// Into [`WorldFrame::line_styles`].
        style: u16,
    },
}

/// One draw: a kind and a run of instances.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Draw {
    pub pass: Pass,
    pub kind: DrawKind,
    pub first_instance: u32,
    pub instance_count: u32,
}

/// One picture of a world, decided: everything a tier needs to draw it.
#[derive(Clone, Debug)]
pub struct WorldFrame {
    /// The picture, in pixels.
    pub size: (u32, u32),
    pub samples: u8,
    pub background: Background,
    pub uniforms: FrameUniforms,
    /// The meshes the draws name, each once.
    pub meshes: Vec<Mesh>,
    /// The textures the draws name, each once.
    pub textures: Vec<ImageSource>,
    /// The line sets the draws name, each once.
    pub lines: Vec<Lines>,
    pub instances: Vec<InstanceData>,
    pub materials: Vec<MaterialData>,
    pub line_styles: Vec<LineStyleData>,
    /// In order: by pass, then as each pass wants them.
    pub draws: Vec<Draw>,
    /// The planes the projection uses, in meters from the eye.
    pub near: f64,
    pub far: f64,
}

impl WorldFrame {
    /// One number for everything a tier reads: two frames with the same
    /// checksum draw the same picture.
    pub fn checksum(&self) -> u64 {
        let mut hasher = motor::hash::FxHasher::default();
        use std::hash::Hasher;
        hasher.write_u32(self.size.0);
        hasher.write_u32(self.size.1);
        hasher.write_u8(self.samples);
        hasher.write(&self.uniforms.to_bytes());
        hasher.write(&super::wire::instance_bytes(&self.instances));
        hasher.write(&super::wire::material_bytes(&self.materials));
        hasher.write(&super::wire::line_style_bytes(&self.line_styles));
        for mesh in &self.meshes {
            hasher.write_u64(mesh.id());
            hasher.write_u64(mesh.generation());
        }
        for texture in &self.textures {
            hasher.write_u64(texture.key());
        }
        for lines in &self.lines {
            hasher.write_u64(lines.id());
            hasher.write_u64(lines.generation());
        }
        for draw in &self.draws {
            std::hash::Hash::hash(draw, &mut hasher);
        }
        hasher.finish()
    }
}

// MARK: - Building

/// A colour as linear light, alpha straight.
fn linear(color: Color) -> [f32; 4] {
    [decode(color.r), decode(color.g), decode(color.b), color.a as f32 / 255.0]
}

fn scaled(color: Color, intensity: f32) -> [f32; 4] {
    let [r, g, b, _] = linear(color);
    [r * intensity, g * intensity, b * intensity, 0.0]
}

fn rows(matrix: &Mat4) -> [[f32; 4]; 3] {
    [0, 1, 2].map(|row| {
        let r = matrix.row(row);
        [r.x as f32, r.y as f32, r.z as f32, r.w as f32]
    })
}

fn material_data(material: &Material) -> MaterialData {
    let mut flags = 0;
    if material.unlit {
        flags |= material_flag::UNLIT;
    }
    if material.texture.is_some() {
        flags |= material_flag::TEXTURED;
    }
    let cutoff = match material.alpha {
        AlphaMode::Mask(cutoff) => {
            flags |= material_flag::MASK;
            cutoff
        }
        _ => 0.0,
    };
    let back = material.back.map_or([0.0; 4], |back| {
        let [r, g, b, _] = linear(back);
        [r, g, b, 1.0]
    });
    MaterialData {
        base: linear(material.color),
        back,
        emissive: scaled(material.emissive, material.emissive_strength),
        params: [material.roughness, material.metallic, cutoff, flags as f32],
    }
}

/// The studio light a world without lights gets: a soft sky, and a key
/// light from the upper left of the view.
const STUDIO_SUN: [f32; 3] = [0.62, 0.595, 0.558];
const STUDIO_SKY: [f32; 3] = [0.414, 0.442, 0.488];
const STUDIO_GROUND: [f32; 3] = [0.360, 0.340, 0.313];
const STUDIO_TOWARD_SUN: [f64; 3] = [-0.575, 0.814, 0.082];

struct Seen {
    entity: u32,
    eye_from_model: Mat4,
    bounds: Aabb,
    tint: Color,
    flags: u32,
}

/// Decides the picture of `world` through `camera` for a target of
/// `size` pixels, `px_per_point` pixels to a point.
pub(crate) fn build_frame(
    world: &WorldData,
    camera: &Camera,
    style: &WorldStyle,
    size: (u32, u32),
    px_per_point: f32,
    time: f64,
) -> WorldFrame {
    let aspect = if size.1 > 0 { size.0.max(1) as f64 / size.1 as f64 } else { 1.0 };
    let view = camera.view();
    let parallel = matches!(camera.lens, Lens::Parallel { .. });
    // the sides of the view, without a near or far yet
    let open = if parallel { camera.projection(aspect, -1.0e12, 1.0e12) } else { camera.projection(aspect, 1.0e-9, 1.0e12) };
    let sides = Frustum::from_matrix(&open);

    let mut models: Vec<(Seen, Model)> = Vec::new();
    let mut line_sets: Vec<(Seen, Lines, super::material::LineStyle)> = Vec::new();
    let mut lights: Vec<(Light, Mat4)> = Vec::new();
    for (id, data) in world.iter() {
        if !world.shown(id) {
            continue;
        }
        let Some(matrix) = world.world_matrix(id) else { continue };
        let eye_from_model = view * matrix;
        let seen = |bounds: Aabb| Seen {
            entity: id.index,
            eye_from_model,
            bounds,
            tint: data.spec.tint,
            flags: if data.spec.casts_shadow { instance_flag::CASTS_SHADOW } else { 0 },
        };
        match &data.spec.shape {
            Shape::Model(model) => {
                let bounds = model.mesh.bounds().transformed(&eye_from_model);
                if sides.may_see(&bounds) && (parallel || bounds.min.z < 0.0) {
                    models.push((seen(bounds), Model { mesh: model.mesh.clone(), materials: Rc::clone(&model.materials) }));
                }
            }
            Shape::Lines(lines, line_style) => {
                let bounds = lines.bounds().transformed(&eye_from_model);
                if sides.may_see(&bounds) && (parallel || bounds.min.z < 0.0) {
                    line_sets.push((seen(bounds), lines.clone(), *line_style));
                }
            }
            Shape::Light(light) => lights.push((*light, eye_from_model)),
            Shape::Empty => {}
        }
    }

    // near and far hug what is in view
    let reach = models
        .iter()
        .map(|(seen, _)| seen.bounds)
        .chain(line_sets.iter().map(|(seen, _, _)| seen.bounds))
        .fold(None::<(f64, f64)>, |range, bounds| {
            let (near, far) = (-bounds.max.z, -bounds.min.z);
            Some(range.map_or((near, far), |(a, b)| (a.min(near), b.max(far))))
        });
    let (mut near, mut far) = match (reach, parallel) {
        (None, false) => (0.1, 100.0),
        (None, true) => (-100.0, 100.0),
        (Some((near, far)), true) => {
            let margin = ((far - near) * 0.01).max(1e-3);
            (near - margin, far + margin)
        }
        (Some((near, far)), false) => {
            let far = (far * 1.01).max(1e-6);
            (near.max(far * 1e-5) * 0.99, far)
        }
    };
    if let Some(fixed) = camera.near {
        near = fixed;
    }
    if let Some(fixed) = camera.far {
        far = fixed;
    }
    if far <= near {
        far = near + 1e-3;
    }
    let projection = camera.projection(aspect, near, far);

    let mut frame = WorldFrame {
        size,
        samples: style.quality.samples(),
        background: style.background,
        uniforms: FrameUniforms::default(),
        meshes: Vec::new(),
        textures: Vec::new(),
        lines: Vec::new(),
        instances: Vec::new(),
        materials: Vec::new(),
        line_styles: Vec::new(),
        draws: Vec::new(),
        near,
        far,
    };

    // models: one group per (mesh, materials, handedness), drawn instanced
    struct Group {
        mesh: u16,
        material_base: u32,
        mirrored: bool,
        members: Vec<usize>,
        depth: f64,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut group_of = std::collections::HashMap::<(u64, u64, usize, bool), usize>::new();
    let mut mesh_slot = std::collections::HashMap::<(u64, u64), u16>::new();
    let mut material_slot = std::collections::HashMap::<usize, u32>::new();
    for (index, (seen, model)) in models.iter().enumerate() {
        let mirrored = seen.eye_from_model.determinant() < 0.0;
        let materials_key = Rc::as_ptr(&model.materials) as *const Material as usize;
        let key = (model.mesh.id(), model.mesh.generation(), materials_key, mirrored);
        let group = *group_of.entry(key).or_insert_with(|| {
            let mesh = *mesh_slot.entry((model.mesh.id(), model.mesh.generation())).or_insert_with(|| {
                frame.meshes.push(model.mesh.clone());
                (frame.meshes.len() - 1) as u16
            });
            let material_base = *material_slot.entry(materials_key).or_insert_with(|| {
                let base = frame.materials.len() as u32;
                if model.materials.is_empty() {
                    frame.materials.push(material_data(&Material::default()));
                }
                frame.materials.extend(model.materials.iter().map(material_data));
                base
            });
            groups.push(Group { mesh, material_base, mirrored, members: Vec::new(), depth: f64::INFINITY });
            groups.len() - 1
        });
        groups[group].members.push(index);
        groups[group].depth = groups[group].depth.min(-seen.bounds.max.z);
    }
    // the nearest group first: the depth test then rejects the most
    groups.sort_by(|a, b| a.depth.total_cmp(&b.depth));

    let texture_of = |frame: &mut WorldFrame, texture: &Option<ImageSource>| -> Option<u16> {
        let texture = texture.as_ref()?;
        Some(match frame.textures.iter().position(|known| known.key() == texture.key()) {
            Some(at) => at as u16,
            None => {
                frame.textures.push(texture.clone());
                (frame.textures.len() - 1) as u16
            }
        })
    };

    let mut transparent: Vec<(f64, Draw)> = Vec::new();
    for group in &groups {
        let first_instance = frame.instances.len() as u32;
        let mut members: Vec<usize> = group.members.clone();
        members.sort_by(|&a, &b| (-models[a].0.bounds.max.z).total_cmp(&(-models[b].0.bounds.max.z)));
        for &member in &members {
            let seen = &models[member].0;
            frame.instances.push(instance_of(seen, group.material_base));
        }
        let (_, model) = &models[group.members[0]];
        // runs of the same draw key become one draw
        let mut runs: Vec<(Material, u32, u32)> = Vec::new();
        for &(slot, first, count) in model.mesh.material_runs() {
            let material = model.materials.get(slot as usize).or(model.materials.last()).cloned().unwrap_or_default();
            match runs.last_mut() {
                Some((last, _, run_count)) if same_draw(last, &material) => *run_count += count,
                _ => runs.push((material, first, count)),
            }
        }
        for (material, first_index, index_count) in runs {
            let texture = texture_of(&mut frame, &material.texture);
            let (pass, blend) = match material.alpha {
                AlphaMode::Opaque | AlphaMode::Mask(_) => (Pass::Opaque, Blend::Replace),
                AlphaMode::Blend => (Pass::Transparent, Blend::Alpha),
                AlphaMode::Add => (Pass::Transparent, Blend::Add),
            };
            let kind = DrawKind::Mesh {
                mesh: group.mesh,
                first_index,
                index_count,
                texture,
                cull_back: material.back.is_none(),
                mirrored: group.mirrored,
                blend,
                mask: matches!(material.alpha, AlphaMode::Mask(_)),
            };
            if pass == Pass::Opaque {
                frame.draws.push(Draw { pass, kind, first_instance, instance_count: members.len() as u32 });
            } else {
                // each see-through instance on its own, so they can be sorted
                for (offset, &member) in members.iter().enumerate() {
                    let depth = -models[member].0.bounds.center().z;
                    transparent.push((depth, Draw { pass, kind, first_instance: first_instance + offset as u32, instance_count: 1 }));
                }
            }
        }
    }

    // lines: one draw per set, in the pass its depth asks for
    let mut line_draws: Vec<Draw> = Vec::new();
    for (seen, lines, line_style) in &line_sets {
        let set = match frame.lines.iter().position(|known| known == lines) {
            Some(at) => at as u16,
            None => {
                frame.lines.push(lines.clone());
                (frame.lines.len() - 1) as u16
            }
        };
        let data = line_style_data(line_style, px_per_point);
        let style_index = match frame.line_styles.iter().position(|known| *known == data) {
            Some(at) => at as u16,
            None => {
                frame.line_styles.push(data);
                (frame.line_styles.len() - 1) as u16
            }
        };
        let pass = match line_style.depth {
            LineDepth::Tested => Pass::Lines,
            LineDepth::Hidden => Pass::HiddenLines,
            LineDepth::Always => Pass::Overlay,
        };
        frame.instances.push(instance_of(seen, 0));
        line_draws.push(Draw {
            pass,
            kind: DrawKind::Lines { lines: set, style: style_index },
            first_instance: frame.instances.len() as u32 - 1,
            instance_count: 1,
        });
    }
    // the farthest see-through surface first
    transparent.sort_by(|a, b| b.0.total_cmp(&a.0));
    frame.draws.extend(transparent.into_iter().map(|(_, draw)| draw));
    frame.draws.extend(line_draws);
    // stable: within a pass, the order above stands
    frame.draws.sort_by_key(|draw| draw.pass);

    frame.uniforms = uniforms(&Eye { camera, view, projection }, style, size, px_per_point, time, &lights);
    frame
}

impl super::store::World {
    /// The picture of the world through `camera`, decided: the draws a
    /// tier runs for a target of `size` pixels, `px_per_point` pixels to
    /// a point. A flying camera is sampled at the engine clock. Reads
    /// the world.
    pub fn frame(&self, camera: &Camera, style: &WorldStyle, size: (u32, u32), px_per_point: f32) -> WorldFrame {
        let time = motor::task::now();
        let camera = camera.at(time);
        self.with(|data| build_frame(data, &camera, style, size, px_per_point, time))
    }
}

/// What a model entity carries into the frame.
struct Model {
    mesh: Mesh,
    materials: Rc<[Material]>,
}

fn same_draw(a: &Material, b: &Material) -> bool {
    let key = |m: &Material| {
        (
            m.texture.as_ref().map(ImageSource::key),
            m.back.is_none(),
            match m.alpha {
                AlphaMode::Opaque | AlphaMode::Mask(_) => 0,
                AlphaMode::Blend => 1,
                AlphaMode::Add => 2,
            },
            matches!(m.alpha, AlphaMode::Mask(_)),
        )
    };
    key(a) == key(b)
}

fn instance_of(seen: &Seen, material_base: u32) -> InstanceData {
    let normal = seen.eye_from_model.normal_matrix();
    InstanceData {
        eye_from_model: rows(&seen.eye_from_model),
        normal: normal.map(|row| [row[0] as f32, row[1] as f32, row[2] as f32, 0.0]),
        tint: [seen.tint.r, seen.tint.g, seen.tint.b, seen.tint.a],
        material_base,
        entity: seen.entity,
        flags: seen.flags,
    }
}

fn line_style_data(style: &super::material::LineStyle, px_per_point: f32) -> LineStyleData {
    let (dash, gap) = style.dash.unwrap_or((0.0, 0.0));
    let mode = match style.depth {
        LineDepth::Tested => 0.0,
        LineDepth::Hidden => 1.0,
        LineDepth::Always => 2.0,
    };
    LineStyleData {
        color: linear(style.color),
        shape: [style.width * px_per_point, dash * px_per_point, gap * px_per_point, mode],
    }
}

/// The eye a frame is drawn from.
struct Eye<'a> {
    camera: &'a Camera,
    view: Mat4,
    projection: Mat4,
}

fn uniforms(eye: &Eye<'_>, style: &WorldStyle, size: (u32, u32), px_per_point: f32, time: f64, lights: &[(Light, Mat4)]) -> FrameUniforms {
    let (camera, view) = (eye.camera, &eye.view);
    let mut out = FrameUniforms { clip_from_eye: eye.projection.to_f32(), ..FrameUniforms::default() };
    let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
    out.viewport = [w, h, 1.0 / w, 1.0 / h];
    let lens = if matches!(camera.lens, Lens::Parallel { .. }) { 1.0 } else { 0.0 };
    out.params = [px_per_point, time as f32, style.quality.samples() as f32, lens];
    let to_eye = |direction: Vec3| view.transform_vector(direction).normalized();
    if let Some(plane) = style.section {
        // n·p_world + d = (R n)·p_eye + (n·eye + d), for a unit n
        let unit = plane.normalized();
        let normal = to_eye(unit.normal);
        out.section = [normal.x as f32, normal.y as f32, normal.z as f32, (unit.d + unit.normal.dot(camera.eye)) as f32];
    }
    let up = to_eye(Vec3::Y);
    out.sky_up = [up.x as f32, up.y as f32, up.z as f32, camera.eye.y as f32];
    out.tone = [style.exposure, 0.8, 1.0, 0.0];

    let mut has_light = false;
    let mut points = 0usize;
    for (light, eye_from_model) in lights {
        has_light = true;
        match light {
            Light::Sun(sun) if out.sun_direction[3] == 0.0 => {
                let toward = match sun.frame {
                    SunFrame::World => to_eye(-sun.direction),
                    SunFrame::View => (-sun.direction).normalized(),
                };
                out.sun_direction = [toward.x as f32, toward.y as f32, toward.z as f32, 1.0];
                out.sun_color = scaled(sun.color, sun.intensity);
            }
            Light::Sky { sky, ground, intensity } => {
                out.sky_color = scaled(*sky, *intensity);
                out.ground_color = scaled(*ground, *intensity);
            }
            Light::Point(point) if points < MAX_POINTS => {
                let at = eye_from_model.transform_point(Vec3::ZERO);
                out.point_position[points] = [at.x as f32, at.y as f32, at.z as f32, point.range];
                let [r, g, b, _] = scaled(point.color, point.intensity);
                out.point_color[points] = [r, g, b, point.radius];
                out.point_groups[points / 4][points % 4] = point.groups;
                points += 1;
            }
            _ => {}
        }
    }
    out.counts = [points as f32, 0.0, 0.0, 0.0];
    if !has_light {
        let toward = vec3(STUDIO_TOWARD_SUN[0], STUDIO_TOWARD_SUN[1], STUDIO_TOWARD_SUN[2]).normalized();
        out.sun_direction = [toward.x as f32, toward.y as f32, toward.z as f32, 1.0];
        out.sun_color = [STUDIO_SUN[0], STUDIO_SUN[1], STUDIO_SUN[2], 0.0];
        out.sky_color = [STUDIO_SKY[0], STUDIO_SKY[1], STUDIO_SKY[2], 0.0];
        out.ground_color = [STUDIO_GROUND[0], STUDIO_GROUND[1], STUDIO_GROUND[2], 0.0];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::material::{LineStyle, Material};
    use crate::world::store::World;

    fn build(world: &World, camera: &Camera) -> WorldFrame {
        world.frame(camera, &WorldStyle::default(), (640, 480), 2.0)
    }

    fn gray() -> Material {
        Material::color(Color::rgb(128, 128, 128))
    }

    #[test]
    fn culled_entities_leave_no_draw() {
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray());
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray()).set_position(vec3(0.0, 0.0, 40.0)); // behind the eye
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray()).set_position(vec3(500.0, 0.0, 0.0)); // far to the side
        let camera = Camera::looking_at(vec3(0.0, 0.0, 10.0), Vec3::ZERO);
        let frame = build(&world, &camera);
        assert_eq!(frame.instances.len(), 1, "only the cube in view");
        assert_eq!(frame.draws.len(), 1);
    }

    #[test]
    fn shared_meshes_become_one_instanced_draw() {
        let world = World::new();
        let mesh = Mesh::cuboid(1.0, 1.0, 1.0);
        let material = gray();
        let model = super::super::material::Model::single(mesh.clone(), material);
        for i in 0..10 {
            world.add_model(model.clone()).set_position(vec3(i as f64 * 2.0 - 9.0, 0.0, 0.0));
        }
        let camera = Camera::looking_at(vec3(0.0, 5.0, 30.0), Vec3::ZERO);
        let frame = build(&world, &camera);
        assert_eq!(frame.meshes.len(), 1);
        assert_eq!(frame.draws.len(), 1, "ten entities, one draw");
        assert_eq!(frame.draws[0].instance_count, 10);
        assert_eq!(frame.materials.len(), 1, "one material table for the shared model");
    }

    #[test]
    fn transparent_draws_come_back_to_front() {
        let world = World::new();
        let glass = Material::color(Color::rgba(200, 220, 255, 100)).blend();
        let model = super::super::material::Model::single(Mesh::cuboid(1.0, 1.0, 1.0), glass);
        for z in [-6.0, 2.0, -2.0] {
            world.add_model(model.clone()).set_position(vec3(0.0, 0.0, z));
        }
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray()).set_position(vec3(3.0, 0.0, 0.0));
        let camera = Camera::looking_at(vec3(0.0, 0.0, 10.0), Vec3::ZERO);
        let frame = build(&world, &camera);
        let passes: Vec<Pass> = frame.draws.iter().map(|d| d.pass).collect();
        assert_eq!(passes, vec![Pass::Opaque, Pass::Transparent, Pass::Transparent, Pass::Transparent]);
        let depths: Vec<f32> = frame.draws[1..]
            .iter()
            .map(|d| -frame.instances[d.first_instance as usize].eye_from_model[2][3])
            .collect();
        assert!(depths.windows(2).all(|pair| pair[0] > pair[1]), "farthest first: {depths:?}");
    }

    #[test]
    fn instance_translations_are_eye_relative() {
        let world = World::new();
        let far = vec3(3.0e6, 10.0, -4.0e6);
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray()).set_position(far);
        let camera = Camera::looking_at(far + vec3(0.0, 0.0, 5.0), far);
        let frame = build(&world, &camera);
        let translation = [0, 1, 2].map(|row| frame.instances[0].eye_from_model[row][3]);
        assert_eq!(translation, [0.0, 0.0, -5.0], "the GPU never sees the large number");
    }

    #[test]
    fn near_and_far_hug_the_visible_world() {
        let world = World::new();
        world.add(Mesh::cuboid(2.0, 2.0, 2.0), gray());
        let camera = Camera::looking_at(vec3(0.0, 0.0, 10.0), Vec3::ZERO);
        let frame = build(&world, &camera);
        assert!(frame.near > 8.5 && frame.near < 9.0, "{}", frame.near);
        assert!(frame.far > 11.0 && frame.far < 11.2, "{}", frame.far);
        let fixed = build(&world, &camera.clip_planes(0.5, 50.0));
        assert_eq!((fixed.near, fixed.far), (0.5, 50.0));
    }

    #[test]
    fn the_same_shot_builds_the_same_bytes() {
        let world = World::new();
        world.add(Mesh::sphere(1.0), gray());
        world.add_lines(Mesh::cuboid(1.0, 1.0, 1.0).edges(30.0), LineStyle::solid(Color::BLACK, 1.5));
        let camera = Camera::orbiting(Vec3::ZERO, 30.0, 20.0, 6.0);
        assert_eq!(build(&world, &camera).checksum(), build(&world, &camera).checksum());
        let moved = Camera::orbiting(Vec3::ZERO, 31.0, 20.0, 6.0);
        assert_ne!(build(&world, &camera).checksum(), build(&world, &moved).checksum());
    }

    #[test]
    fn lines_draw_in_the_pass_their_depth_asks_for() {
        let world = World::new();
        let edges = Mesh::cuboid(1.0, 1.0, 1.0).edges(30.0);
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray());
        world.add_lines(edges.clone(), LineStyle::solid(Color::BLACK, 1.0));
        world.add_lines(edges.clone(), LineStyle::solid(Color::BLACK, 1.0).dashed(4.0, 3.0).depth(LineDepth::Hidden));
        world.add_lines(edges, LineStyle::solid(Color::WHITE, 2.0).depth(LineDepth::Always));
        let frame = build(&world, &Camera::orbiting(Vec3::ZERO, 30.0, 20.0, 6.0));
        let passes: Vec<Pass> = frame.draws.iter().map(|d| d.pass).collect();
        assert_eq!(passes, vec![Pass::Opaque, Pass::Lines, Pass::HiddenLines, Pass::Overlay]);
        assert_eq!(frame.lines.len(), 1, "one segment set shared by three draws");
        assert_eq!(frame.line_styles[0].shape[0], 2.0, "a point is two pixels here");
    }

    #[test]
    fn a_mirrored_instance_draws_apart() {
        let world = World::new();
        let model = super::super::material::Model::single(Mesh::cuboid(1.0, 1.0, 1.0), gray());
        world.add_model(model.clone());
        world.add_model(model).set_position(vec3(2.0, 0.0, 0.0)).set_scale(vec3(-1.0, 1.0, 1.0));
        let frame = build(&world, &Camera::looking_at(vec3(1.0, 0.0, 10.0), vec3(1.0, 0.0, 0.0)));
        assert_eq!(frame.draws.len(), 2);
        let mirrored: Vec<bool> = frame.draws.iter().map(|d| matches!(d.kind, DrawKind::Mesh { mirrored: true, .. })).collect();
        assert!(mirrored.contains(&true) && mirrored.contains(&false));
    }

    #[test]
    fn a_world_without_light_gets_the_studio_rig() {
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray());
        let frame = build(&world, &Camera::orbiting(Vec3::ZERO, 0.0, 20.0, 6.0));
        assert_eq!(frame.uniforms.sun_direction[3], 1.0);
        assert!(frame.uniforms.sky_color[0] > 0.0);
        world.add_light(Light::sun().toward(vec3(0.0, -1.0, 0.0)));
        let frame = build(&world, &Camera::orbiting(Vec3::ZERO, 0.0, 0.0, 6.0));
        assert_eq!(frame.uniforms.sky_color, [0.0; 4], "a world with lights keeps exactly its lights");
        let up = frame.uniforms.sun_direction;
        assert!((up[1] - 1.0).abs() < 1e-6, "toward the sun is up, in the eye of a level camera: {up:?}");
    }
}
