//! The frame: what one picture of a world asks of a tier.
//!
//! The core decides everything here, once, on the CPU — which entities
//! the camera sees, which meshes share a draw, the order of the draws,
//! the near and far planes, the lights in the eye's space. A tier
//! uploads what it lacks and runs the draws in order; the software
//! rasterizer runs the same list. Two tiers can only disagree on how a
//! pixel is shaded, never on what is drawn.

use std::collections::HashMap;
use std::hash::Hasher;
use std::rc::Rc;

use super::camera::{Camera, Lens};
use super::material::{AlphaMode, Light, LineDepth, Material, PartState, SunFrame};
use super::math::{Aabb, Frustum, Mat4, Plane, Vec3, vec3, vec4};
use super::mesh::{Lines, Mesh};
use super::shade::decode;
use super::shadow;
use super::store::{EntityId, Shape, WorldData};
use super::wire::{
    FrameUniforms, InstanceData, LineStyleData, MAX_POINTS, MaterialData, NO_JOINTS, NO_PARTS, PartRow, Row, ShadowCaster, instance_flag,
    material_flag,
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
    /// A sky over a ground, by where the view looks: `horizon` at eye
    /// level, `zenith` straight up, `ground` below the horizon. The
    /// horizon moves as the camera tilts; a parallel lens sees one colour.
    Sky { zenith: Color, horizon: Color, ground: Color },
}

/// What a picture of the world may cost, from the least to the most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Quality {
    /// One sample a pixel and a small pixel budget: a weak GPU, a phone,
    /// a busy page.
    Low,
    /// Four samples a pixel.
    #[default]
    Medium,
    /// Four samples a pixel and the largest budget.
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

    /// The most pixels a GPU tier draws a picture with. A box inside the
    /// budget is drawn at its own size; a larger one is drawn smaller,
    /// and the compositor stretches it to the box.
    pub fn pixel_budget(self) -> u32 {
        match self {
            Quality::Low => 1_200_000,
            Quality::Medium => 2_200_000,
            Quality::High => 4_000_000,
        }
    }

    /// The side of the sun's shadow map, in texels.
    pub fn shadow_size(self) -> u32 {
        match self {
            Quality::Low => 1024,
            Quality::Medium | Quality::High => 2048,
        }
    }

    /// The side of the second shadow map, of what moves (a figure that
    /// walks), drawn again each frame it moves: none at Low, where what
    /// moves casts no shadow (a blob under it stands in).
    pub fn moving_shadow_size(self) -> Option<u32> {
        match self {
            Quality::Low => None,
            Quality::Medium => Some(1024),
            Quality::High => Some(2048),
        }
    }

    /// The lookups a pixel makes into the second map: one, or four.
    pub fn moving_shadow_taps(self) -> u32 {
        match self {
            Quality::Low | Quality::Medium => 1,
            Quality::High => 4,
        }
    }

    /// The lookups a pixel makes into the sun's shadow map — each one
    /// four texels compared and filtered: one, four or nine.
    pub fn shadow_taps(self) -> u32 {
        match self {
            Quality::Low => 1,
            Quality::Medium => 4,
            Quality::High => 9,
        }
    }

    /// The size a GPU tier draws a box of `box_px` pixels at: the box's
    /// own, or the largest of the same shape inside the budget.
    pub fn target_size(self, box_px: (u32, u32)) -> (u32, u32) {
        let pixels = box_px.0 as f64 * box_px.1 as f64;
        let budget = self.pixel_budget() as f64;
        if pixels <= budget {
            return (box_px.0.max(1), box_px.1.max(1));
        }
        let scale = (budget / pixels).sqrt();
        (((box_px.0 as f64 * scale).floor() as u32).max(1), ((box_px.1 as f64 * scale).floor() as u32).max(1))
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
    /// Every surface drawn see-through, so what stands behind it shows,
    /// and every edge with it: the look of an editor over a busy model.
    pub xray: bool,
    /// The colour grade of every surface, after the tone curve: a day's
    /// warmth, a night's blue. `None` leaves the colours alone.
    pub grade: Option<Grade>,
    /// How strongly what glows spills its light around itself, in
    /// `[0, 1]`: 0 is none, 0.6 a soft glow. Only [`Quality::High`] draws
    /// it; a lamp's emissive light decides how much it glows.
    pub bloom: f32,
    /// The most pictures a second an animation may ask for.
    pub max_fps: Option<f64>,
}

impl Default for WorldStyle {
    fn default() -> WorldStyle {
        WorldStyle {
            background: Background::Transparent,
            quality: Quality::Medium,
            section: None,
            exposure: 1.0,
            xray: false,
            grade: None,
            bloom: 0.0,
            max_fps: None,
        }
    }
}

/// A colour grade: a table of 16 × 16 × 16 points from the sRGB codes a
/// surface would show to the codes it shows. A colour between the points
/// blends four of them (the corners of the tetrahedron that holds it), so
/// a grade costs four reads a pixel. One table serves every frame that
/// names it.
#[derive(Clone, Debug, PartialEq)]
pub struct Grade {
    /// Red runs fastest, then green, then blue.
    points: Rc<[[u8; 3]]>,
    /// A hash of the points, for the picture's identity.
    hash: u64,
}

impl Grade {
    /// The points along each channel.
    pub const SIDE: usize = 16;

    /// The grade that `f` draws: from a colour in `[0, 1]` sRGB to the
    /// colour it becomes, sampled at the table's points.
    pub fn from_fn(f: impl Fn([f32; 3]) -> [f32; 3]) -> Grade {
        let side = Grade::SIDE;
        let at = |index: usize| index as f32 / (side - 1) as f32;
        let mut points = Vec::with_capacity(side * side * side);
        for b in 0..side {
            for g in 0..side {
                for r in 0..side {
                    let out = f([at(r), at(g), at(b)]);
                    points.push(out.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8));
                }
            }
        }
        Grade::from_points(points)
    }

    /// A grade of its `16³` points, red running fastest; `None` for
    /// another count.
    pub fn new(points: Vec<[u8; 3]>) -> Option<Grade> {
        (points.len() == Grade::SIDE.pow(3)).then(|| Grade::from_points(points))
    }

    fn from_points(points: Vec<[u8; 3]>) -> Grade {
        let mut hasher = motor::hash::FxHasher::default();
        for point in &points {
            hasher.write(point);
        }
        Grade { points: points.into(), hash: hasher.finish() }
    }

    /// A hash of the grade's points.
    pub fn hash(&self) -> u64 {
        self.hash
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

/// The sun's shadow map of a frame: what a tier draws into it, from the
/// sun, before the frame's own draws. The casters are the solid surfaces
/// of every entity that casts a shadow, in view or not — a shadow falls
/// into the view from outside it. A cut-out casts its whole shape.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowFrame {
    /// A hash of what the map holds — the casters, their meshes and the
    /// sun — and of nothing else: a tier that holds a map of this
    /// generation draws nothing into it, whatever the camera did.
    pub generation: u64,
    /// The map's side, in texels.
    pub size: u32,
    /// Each caster ([`super::wire::shadow_instance_bytes`]).
    pub instances: Vec<ShadowCaster>,
    /// In order. Each one draws both faces and keeps the nearest depth.
    pub draws: Vec<ShadowDraw>,
}

/// One draw into the shadow map: a run of a mesh's indices, once per
/// caster.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShadowDraw {
    /// Into [`WorldFrame::meshes`].
    pub mesh: u16,
    pub first_index: u32,
    pub index_count: u32,
    /// Into [`ShadowFrame::instances`].
    pub first_instance: u32,
    pub instance_count: u32,
}

/// What one instance of a frame shows, so two frames of one target can be
/// told apart ([`super::damage`]): a key that changes with anything of
/// its picture — its shape, its place, its look, its parts, its pose —
/// and the pixels its picture may reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cover {
    pub key: u64,
    /// `[left, top, right, bottom]` in pixels of the target, y down: the
    /// whole target when the lens cannot tell.
    pub rect: [f32; 4],
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
    /// How far each opaque surface is pushed back, in pixels of its own
    /// depth slope, so an edge drawn on it wins across its whole width
    /// even where the surface turns away from the eye. Zero when no
    /// line of the frame meets the depth, or no surface writes one.
    pub face_bias: f32,
    /// The sun's shadow map, when the frame's sun casts shadows and
    /// something casts one: of what stands still, so a camera that moves
    /// and a figure that walks leave it as it is.
    pub shadow: Option<ShadowFrame>,
    /// The second map, of what moves in view (a figure that walks, a
    /// clip that plays): drawn again each frame, small and fitted to it.
    /// Medium and High only.
    pub moving_shadow: Option<ShadowFrame>,
    /// Into [`WorldFrame::textures`]: the world's floor map, when one is
    /// laid ([`FrameUniforms::floor_u`] places it).
    pub floor: Option<u16>,
    /// The states of the parts of every instance that sets them, one row a
    /// part its mesh names ([`InstanceData::part_base`]).
    pub parts: Vec<PartRow>,
    /// How strongly what glows spills, in 256ths: 0 draws no bloom
    /// ([`super::bloom`]). The pass then draws each pixel's glow beside
    /// its colour.
    pub bloom: u16,
    /// The palettes of the skinned instances, three rows a joint
    /// ([`InstanceData::joint_base`]): each pose worked out once, the
    /// clothes of a figure sharing it.
    pub joints: Vec<Row>,
    /// What each instance shows, by the index of its instance.
    pub covers: Vec<Cover>,
    /// Where the shadow of each thing in the map of what moves may fall,
    /// `[left, top, right, bottom]` in pixels.
    pub moving_covers: Vec<[f32; 4]>,
    /// A hash of all the frame's instances share — the eye, the style,
    /// the lights, the still map, the floor map — so two frames with the
    /// same one differ only where their covers do.
    pub scene: u64,
}

impl WorldFrame {
    /// The backdrop's block, for a backdrop a pass draws: six rows —
    /// `[0]` the top's or the zenith's codes, `[1]` the bottom's or the
    /// horizon's, `[2]` `(width, height, 1 for a sky, 0)`, `[3]` the
    /// ground's codes, `[4]` the world's up in the eye's space, `[5]`
    /// `(1/focal x, 1/focal y, 1 for a parallel lens, 0)`. `None` for a
    /// backdrop the clear alone draws.
    pub fn backdrop(&self) -> Option<[Row; 6]> {
        let codes = |c: Color| [c.r as f32, c.g as f32, c.b as f32, c.a as f32];
        let size = [self.size.0 as f32, self.size.1 as f32];
        let u = &self.uniforms;
        match self.background {
            Background::Transparent | Background::Color(_) => None,
            Background::Gradient { top, bottom } => {
                Some([codes(top), codes(bottom), [size[0], size[1], 0.0, 0.0], [0.0; 4], [0.0; 4], [0.0; 4]])
            }
            Background::Sky { zenith, horizon, ground } => Some([
                codes(zenith),
                codes(horizon),
                [size[0], size[1], 1.0, 0.0],
                codes(ground),
                [u.sky_up[0], u.sky_up[1], u.sky_up[2], 0.0],
                [1.0 / u.clip_from_eye[0], 1.0 / u.clip_from_eye[5], u.params[3], 0.0],
            ]),
        }
    }

    /// The backdrop's block that paints the frame's background over a
    /// part of a target, where a clear cannot: [`WorldFrame::backdrop`],
    /// or for a flat background a ramp from its colour to itself — the
    /// same codes the clear gives.
    pub fn fill(&self) -> [Row; 6] {
        self.backdrop().unwrap_or_else(|| {
            let color = match self.background {
                Background::Color(color) => color,
                _ => Color::rgba(0, 0, 0, 0),
            };
            let codes = [color.r as f32, color.g as f32, color.b as f32, color.a as f32];
            [codes, codes, [self.size.0 as f32, self.size.1 as f32, 0.0, 0.0], [0.0; 4], [0.0; 4], [0.0; 4]]
        })
    }

    /// Whether `draw` may reach `rect` (`[x, y, width, height]` in pixels):
    /// a draw none of whose instances reach it changes nothing there.
    pub fn draw_reaches(&self, draw: &Draw, rect: [u32; 4]) -> bool {
        let (left, top, right, bottom) = (rect[0] as f32, rect[1] as f32, (rect[0] + rect[2]) as f32, (rect[1] + rect[3]) as f32);
        let first = draw.first_instance as usize;
        match self.covers.get(first..first + draw.instance_count as usize) {
            Some(covers) => covers.iter().any(|cover| cover.rect[0] < right && cover.rect[2] > left && cover.rect[1] < bottom && cover.rect[3] > top),
            None => true,
        }
    }

    /// One number for everything a tier reads: two frames with the same
    /// checksum draw the same picture.
    pub fn checksum(&self) -> u64 {
        let mut hasher = motor::hash::FxHasher::default();
        hasher.write_u32(self.size.0);
        hasher.write_u32(self.size.1);
        hasher.write_u8(self.samples);
        hasher.write(&self.uniforms.to_bytes());
        hasher.write(&super::wire::instance_bytes(&self.instances));
        hasher.write(&super::wire::material_bytes(&self.materials));
        hasher.write(&super::wire::line_style_bytes(&self.line_styles));
        hasher.write_u32(self.face_bias.to_bits());
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
        if let Some(shadow) = &self.shadow {
            hasher.write_u64(shadow.generation);
        }
        if let Some(shadow) = &self.moving_shadow {
            hasher.write_u64(shadow.generation);
        }
        if let Some(floor) = self.floor {
            hasher.write_u16(floor);
        }
        hasher.write(&super::wire::part_bytes(&self.parts));
        hasher.write_u16(self.bloom);
        for value in self.joints.iter().flatten() {
            hasher.write_u32(value.to_bits());
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

/// The nearest a fitted near plane may come, as a share of the far
/// plane: with a 24-bit or a float depth, two surfaces a few millimetres
/// apart at the far end still draw in their order.
const NEAR_PER_FAR: f64 = 1.0 / 2000.0;

/// The steepest a depth slope reads across a pixel, against the slope
/// along the axes: a diagonal is √2 longer.
const FACE_BIAS_PER_PIXEL: f32 = std::f32::consts::SQRT_2;

/// The share of light an X-rayed surface stops, in alpha codes.
const XRAY_ALPHA: u32 = 128;

/// The studio light: a soft sky every world gets until it holds a sky of
/// its own, and a key light from the upper left of the view for a world
/// with no light at all.
const STUDIO_SUN: [f32; 3] = [0.62, 0.595, 0.558];
const STUDIO_SKY: [f32; 3] = [0.414, 0.442, 0.488];
const STUDIO_GROUND: [f32; 3] = [0.360, 0.340, 0.313];
const STUDIO_TOWARD_SUN: [f64; 3] = [-0.575, 0.814, 0.082];

struct Seen {
    entity: u32,
    world_from_model: Mat4,
    eye_from_model: Mat4,
    bounds: Aabb,
    tint: Color,
    ambient: Color,
    parts: Option<Rc<[PartState]>>,
    flags: u32,
    joint_base: u32,
    /// How many rows its palette takes.
    joint_rows: u32,
    morph: u32,
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
    let mut casters: Vec<Caster> = Vec::new();
    let mut line_sets: Vec<(Seen, Lines, super::material::LineStyle)> = Vec::new();
    let mut lights: Vec<(Light, Mat4)> = Vec::new();
    // each pose once per skeleton and the entity that plays it, each
    // palette once per skin and pose: a figure's clothes share both
    let mut poses = HashMap::<(Option<EntityId>, u64), Rc<Vec<Mat4>>>::new();
    let mut palettes = HashMap::<(Option<EntityId>, u64), u32>::new();
    let mut joints: Vec<Row> = Vec::new();
    // the world's box around what is in view: no shadow falls past it
    let mut receivers = Aabb::EMPTY;
    let mut levels = world.levels.borrow_mut();
    for (id, data) in world.iter() {
        if !world.shown(id) {
            continue;
        }
        let Some(matrix) = world.world_matrix_at(id, time) else { continue };
        let eye_from_model = view * matrix;
        // the mesh a model draws with: a coarser one as it stands small
        let mesh = match &data.spec.shape {
            Shape::Model(model) if !model.lods.is_empty() => {
                let level = model.level(height_on_target(&model.mesh.bounds().transformed(&eye_from_model), &open, parallel, size), levels.get(&id).copied());
                levels.insert(id, level);
                Some(model.mesh_at(level).clone())
            }
            Shape::Model(model) => Some(model.mesh.clone()),
            _ => None,
        };
        let (joint_base, joint_rows) = match (&data.spec.shape, &mesh) {
            (Shape::Model(model), Some(mesh)) => match &model.skin {
                Some(skin) if mesh.is_skinned() => {
                    let played = world.play_of(id);
                    let owner = played.map(|(owner, _)| owner);
                    let skeleton = skin.skeleton();
                    let pose = Rc::clone(
                        poses.entry((owner, skeleton.id())).or_insert_with(|| Rc::new(skeleton.pose(played.map(|(_, play)| play), time))),
                    );
                    let base = *palettes.entry((owner, skin.id())).or_insert_with(|| {
                        let base = joints.len() as u32;
                        joints.extend(skin.palette(&pose));
                        base
                    });
                    (base, skin.joints().len() as u32 * 3)
                }
                _ => (NO_JOINTS, 0),
            },
            _ => (NO_JOINTS, 0),
        };
        let seen = |bounds: Aabb| Seen {
            entity: id.index,
            world_from_model: matrix,
            eye_from_model,
            bounds,
            tint: data.spec.tint,
            ambient: data.spec.ambient,
            parts: data.spec.parts.clone(),
            flags: if data.spec.casts_shadow { instance_flag::CASTS_SHADOW } else { 0 }
                | if data.spec.reads_floor_map { instance_flag::FLOOR_MAP } else { 0 }
                | (data.spec.light_group as u32 & instance_flag::GROUP_MASK) << instance_flag::GROUP_SHIFT,
            joint_base,
            joint_rows,
            morph: super::mesh::pack_morph(data.spec.morph),
        };
        match &data.spec.shape {
            Shape::Model(model) => {
                let Some(mesh) = mesh else { continue };
                // a skinned mesh leaves its rest pose: its box grows by
                // half its longest side each way
                let local = if joint_base == NO_JOINTS { mesh.bounds() } else { skinned_bounds(mesh.bounds()) };
                let bounds = local.transformed(&eye_from_model);
                let in_view = sides.may_see(&bounds) && (parallel || bounds.min.z < 0.0);
                if data.spec.casts_shadow {
                    casters.push(Caster {
                        world_from_model: matrix,
                        model: Model { mesh: mesh.clone(), materials: Rc::clone(&model.materials) },
                        parts: data.spec.parts.clone(),
                        joint_base,
                        joint_rows,
                        morph: super::mesh::pack_morph(data.spec.morph),
                        bounds: local,
                        moving: world.moves(id, time),
                        in_view,
                    });
                }
                if in_view {
                    receivers = receivers.union(local.transformed(&matrix));
                    models.push((seen(bounds), Model { mesh, materials: Rc::clone(&model.materials) }));
                }
            }
            Shape::Lines(lines, line_style) => {
                let bounds = lines.bounds().transformed(&eye_from_model);
                if sides.may_see(&bounds) && (parallel || bounds.min.z < 0.0) {
                    receivers = receivers.union(lines.bounds().transformed(&matrix));
                    line_sets.push((seen(bounds), lines.clone(), *line_style));
                }
            }
            Shape::Light(light) => lights.push((*light, eye_from_model)),
            Shape::Empty => {}
        }
    }

    drop(levels);
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
            // a floor that runs under the eye would pull the near plane to
            // nothing, and the depth would lose every step it can tell
            // apart: the near plane stays within a ratio of the far one
            (near.max(far * NEAR_PER_FAR) * 0.99, far)
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
        face_bias: 0.0,
        shadow: None,
        moving_shadow: None,
        floor: None,
        parts: Vec::new(),
        bloom: if style.quality == Quality::High && style.bloom > 0.0 { (style.bloom.min(1.0) * 256.0).round() as u16 } else { 0 },
        joints,
        covers: Vec::new(),
        moving_covers: Vec::new(),
        scene: 0,
    };
    // the pixels an eye-space box may reach on the target
    let reach = |bounds: &Aabb, margin: f64| reach_of(bounds, &projection, size, margin);
    // what each set of materials and each palette looks like, hashed once
    let mut looks = HashMap::<usize, u64>::new();
    let mut poses_seen = HashMap::<u32, u64>::new();
    // each set of part states once, as many rows as its mesh names parts
    let mut part_bases = HashMap::<(usize, u32), u32>::new();

    // models: one group per (mesh, materials, handedness), drawn instanced
    struct Group {
        mesh: u16,
        material_base: u32,
        mirrored: bool,
        members: Vec<usize>,
        depth: f64,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut group_of = HashMap::<(u64, u64, usize, bool), usize>::new();
    let mut mesh_slot = HashMap::<(u64, u64), u16>::new();
    let mut material_slot = HashMap::<usize, u32>::new();
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
            let (seen, model) = &models[member];
            let mut instance = instance_of(seen, group.material_base, part_base_of(&mut frame.parts, &mut part_bases, &seen.parts, &model.mesh));
            if style.xray {
                instance.tint[3] = ((instance.tint[3] as u32 * XRAY_ALPHA + 127) / 255) as u8;
            }
            frame.instances.push(instance);
            let look = *looks.entry(Rc::as_ptr(&model.materials) as *const Material as usize).or_insert_with(|| look_of(&model.materials));
            let pose = pose_of(&frame.joints, &mut poses_seen, seen);
            let key = cover_key(seen, (model.mesh.id(), model.mesh.generation()), look, pose);
            frame.covers.push(Cover { key, rect: reach(&seen.bounds, 1.0) });
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
                // X-ray: every solid surface see-through, writing no depth
                AlphaMode::Opaque | AlphaMode::Mask(_) if style.xray => (Pass::Transparent, Blend::Alpha),
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
            // an X-ray's surfaces write no depth: its edges go on top of
            // them, still tested against each other's nothing
            LineDepth::Tested if style.xray => Pass::Overlay,
            LineDepth::Tested => Pass::Lines,
            LineDepth::Hidden => Pass::HiddenLines,
            LineDepth::Always => Pass::Overlay,
        };
        frame.instances.push(instance_of(seen, 0, NO_PARTS));
        // a line reaches half its width past its segments, and its soft
        // rim a pixel more
        let mut look = motor::hash::FxHasher::default();
        look.write(&super::wire::line_style_bytes(&[data]));
        let look = look.finish();
        let key = cover_key(seen, (lines.id(), lines.generation()), look, 0);
        frame.covers.push(Cover { key, rect: reach(&seen.bounds, data.shape[0] as f64 * 0.5 + 2.0) });
        line_draws.push(Draw {
            pass,
            kind: DrawKind::Lines { lines: set, style: style_index },
            first_instance: frame.instances.len() as u32 - 1,
            instance_count: 1,
        });
    }
    // a line's solid core reaches half its width from its edge, and its
    // soft rim half a pixel more: the surfaces under it step back as far
    // as the core and a quarter of the rim, in their own slope, with room
    // for a slope across the diagonal. A larger step would let hidden
    // edges show near the corners they leave from
    let widest = line_draws
        .iter()
        .filter(|draw| matches!(draw.pass, Pass::Lines | Pass::HiddenLines))
        .filter_map(|draw| match draw.kind {
            DrawKind::Lines { style, .. } => Some(frame.line_styles[style as usize].shape[0]),
            DrawKind::Mesh { .. } => None,
        })
        .fold(None, |widest: Option<f32>, width| Some(widest.map_or(width, |w| w.max(width))));
    // and only where a surface writes a depth for them to meet
    let solid = frame.draws.iter().any(|draw| draw.pass == Pass::Opaque);
    if let Some(width) = widest
        && solid
    {
        frame.face_bias = (width * 0.5 + 0.25) * FACE_BIAS_PER_PIXEL;
    }
    // the farthest see-through surface first
    transparent.sort_by(|a, b| b.0.total_cmp(&a.0));
    frame.draws.extend(transparent.into_iter().map(|(_, draw)| draw));
    frame.draws.extend(line_draws);
    // stable: within a pass, the order above stands
    frame.draws.sort_by_key(|draw| draw.pass);

    // the light and shade baked on the ground
    let floor_map = world.floor_map.clone();
    if let Some(map) = &floor_map {
        frame.floor = texture_of(&mut frame, &Some(map.picture.clone()));
    }

    // the frame's sun is its first, and it casts when it says so
    let world_from_eye = view.inverse().unwrap_or(Mat4::IDENTITY);
    let sun = lights.iter().find_map(|(light, _)| match light {
        Light::Sun(sun) => Some(*sun),
        _ => None,
    });
    let travel = sun
        .filter(|sun| sun.shadows)
        .and_then(|sun| {
            match sun.frame {
                SunFrame::World => sun.direction,
                SunFrame::View => world_from_eye.transform_vector(sun.direction),
            }
            .try_normalized()
        });
    // what stands still casts into the cached map; what moves, into the
    // second one, at Medium and High, when it is in view
    let (still, moving): (Vec<&Caster>, Vec<&Caster>) = casters.iter().partition(|caster| !caster.moving);
    let fit = travel.and_then(|travel| {
        let (shadow, fit) = shadow_frame(&mut frame, &mut mesh_slot, &mut part_bases, &still, travel, style.quality.shadow_size())?;
        frame.shadow = Some(shadow);
        Some(fit)
    });
    let moving: Vec<&Caster> = moving.into_iter().filter(|caster| caster.in_view).collect();
    let moving_fit = travel.zip(style.quality.moving_shadow_size()).and_then(|(travel, side)| {
        let (shadow, fit) = shadow_frame(&mut frame, &mut mesh_slot, &mut part_bases, &moving, travel, side)?;
        frame.moving_shadow = Some(shadow);
        // where each one's shadow may fall: its box, grown by the
        // lookups' reach, swept along the sun's way out of what is seen
        let grow = fit.texel * (shadow::reach(style.quality.moving_shadow_taps()) + 2.0);
        frame.moving_covers = moving
            .iter()
            .filter_map(|caster| {
                let bounds = caster.bounds.transformed(&caster.world_from_model);
                let grown = Aabb::new(bounds.min - vec3(grow, grow, grow), bounds.max + vec3(grow, grow, grow));
                let swept = shadow_sweep(grown, travel, &receivers)?;
                Some(reach(&swept.transformed(&view), 1.0))
            })
            .collect();
        Some(fit)
    });

    frame.uniforms = uniforms(&Eye { camera, view, projection }, style, size, px_per_point, time, &lights);
    // the grade rides the part table, after the parts: `tone.w` names
    // where it starts, plus one
    if let Some(grade) = &style.grade {
        frame.uniforms.tone[3] = frame.parts.len() as f32 + 1.0;
        frame.parts.extend(grade.points.iter().map(|&[r, g, b]| [r, g, b, 255]));
    }
    if let Some(map) = floor_map.filter(|map| map.size.x > 0.0 && map.size.y > 0.0) {
        // u runs with the world's x from the map's corner, v with its z;
        // the eye's own place is taken out in f64
        let along = |row: usize, origin: f64, span: f64| {
            let r = world_from_eye.row(row);
            [(r.x / span) as f32, (r.y / span) as f32, (r.z / span) as f32, ((r.w - origin) / span) as f32]
        };
        frame.uniforms.floor_u = along(0, map.origin.x, map.size.x);
        frame.uniforms.floor_v = along(2, map.origin.y, map.size.y);
        frame.uniforms.counts[1] = 1.0;
    }
    // each map's u runs with its x, its v down from its top
    let uv = Mat4::from_cols(vec4(0.5, 0.0, 0.0, 0.0), vec4(0.0, -0.5, 0.0, 0.0), vec4(0.0, 0.0, 1.0, 0.0), vec4(0.5, 0.5, 0.0, 1.0));
    let lookup = |fit: &shadow::Fit, size: u32, taps: u32| {
        ((uv * fit.map_from_world * world_from_eye).to_f32(), [1.0 / size as f32, (fit.texel * shadow::reach(taps)) as f32, shadow::depth_bias(fit) as f32, taps as f32])
    };
    if let Some(fit) = fit {
        (frame.uniforms.shadow_from_eye, frame.uniforms.shadow) = lookup(&fit, style.quality.shadow_size(), style.quality.shadow_taps());
    }
    if let (Some(fit), Some(size)) = (moving_fit, style.quality.moving_shadow_size()) {
        (frame.uniforms.moving_from_eye, frame.uniforms.moving) = lookup(&fit, size, style.quality.moving_shadow_taps());
    }
    frame.scene = scene_of(&frame, &view, &projection, style, px_per_point);
    frame
}

/// The pixels an eye-space box may reach on a target of `size` through
/// `projection`, `[left, top, right, bottom]` with y down, grown by
/// `margin` pixels: the hull of its corners, which holds all of it that
/// lies before the eye — the whole target when a corner lies behind it.
fn reach_of(bounds: &Aabb, projection: &Mat4, size: (u32, u32), margin: f64) -> [f32; 4] {
    let (width, height) = (size.0 as f64, size.1 as f64);
    let whole = [0.0, 0.0, width as f32, height as f32];
    if bounds.is_empty() {
        return [0.0; 4];
    }
    let mut out = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for corner in bounds.corners() {
        let clip = projection.transform(corner.extend(1.0));
        if clip.w.is_nan() || clip.w <= 0.0 {
            return whole;
        }
        let (x, y) = ((clip.x / clip.w + 1.0) * 0.5 * width, (1.0 - clip.y / clip.w) * 0.5 * height);
        out = [out[0].min(x), out[1].min(y), out[2].max(x), out[3].max(y)];
    }
    let clamp = |value: f64, end: f64| value.clamp(0.0, end) as f32;
    [clamp(out[0] - margin, width), clamp(out[1] - margin, height), clamp(out[2] + margin, width), clamp(out[3] + margin, height)]
}

/// The box a shadow from `caster` (a world box) may fall in: swept along
/// `travel` while some of it can still meet what is in `receivers`, and
/// cut to it. `None` when it meets nothing.
fn shadow_sweep(caster: Aabb, travel: Vec3, receivers: &Aabb) -> Option<Aabb> {
    if caster.is_empty() || receivers.is_empty() {
        return None;
    }
    // past the first side whose far wall even its last corner has
    // crossed, nothing of it lies in the box any more
    let exits = [(travel.x, caster.min.x, caster.max.x, receivers.min.x, receivers.max.x), (travel.y, caster.min.y, caster.max.y, receivers.min.y, receivers.max.y), (travel.z, caster.min.z, caster.max.z, receivers.min.z, receivers.max.z)];
    let reach = exits
        .iter()
        .filter(|(way, ..)| way.abs() > 1e-12)
        .map(|&(way, low, high, wall_low, wall_high)| if way > 0.0 { (wall_high - low) / way } else { (wall_low - high) / way })
        .fold(f64::INFINITY, f64::min);
    let reach = if reach.is_finite() { reach.max(0.0) } else { 0.0 };
    let moved = Aabb::new(caster.min + travel * reach, caster.max + travel * reach);
    let swept = caster.union(moved);
    let cut = Aabb::new(swept.min.max(receivers.min), swept.max.min(receivers.max));
    (cut.min.x <= cut.max.x && cut.min.y <= cut.max.y && cut.min.z <= cut.max.z).then_some(cut)
}

/// A hash of what a set of materials looks like: each one's colours,
/// alpha mode and texture.
fn look_of(materials: &[Material]) -> u64 {
    let mut hasher = motor::hash::FxHasher::default();
    for material in materials {
        hasher.write(&super::wire::material_bytes(&[material_data(material)]));
        hasher.write_u8(match material.alpha {
            AlphaMode::Opaque => 0,
            AlphaMode::Mask(_) => 1,
            AlphaMode::Blend => 2,
            AlphaMode::Add => 3,
        });
        hasher.write_u64(material.texture.as_ref().map_or(0, ImageSource::key));
    }
    hasher.finish()
}

/// A hash of an instance's palette, once a palette.
fn pose_of(joints: &[Row], seen: &mut HashMap<u32, u64>, instance: &Seen) -> u64 {
    if instance.joint_base == NO_JOINTS {
        return 0;
    }
    *seen.entry(instance.joint_base).or_insert_with(|| {
        let mut hasher = motor::hash::FxHasher::default();
        let (start, end) = (instance.joint_base as usize, (instance.joint_base + instance.joint_rows) as usize);
        for value in joints.get(start..end).unwrap_or(&[]).iter().flatten() {
            hasher.write_u32(value.to_bits());
        }
        hasher.finish()
    })
}

/// The key of what an instance shows: `shape` is its mesh's or its
/// lines' identity and revision, `look` its materials' or its line
/// style's hash, `pose` its palette's.
fn cover_key(seen: &Seen, shape: (u64, u64), look: u64, pose: u64) -> u64 {
    let mut hasher = motor::hash::FxHasher::default();
    hasher.write_u32(seen.entity);
    hasher.write_u64(shape.0);
    hasher.write_u64(shape.1);
    for value in seen.world_from_model.m {
        hasher.write_u64(value.to_bits());
    }
    hasher.write(&[seen.tint.r, seen.tint.g, seen.tint.b, seen.tint.a, seen.ambient.r, seen.ambient.g, seen.ambient.b]);
    hasher.write_u32(seen.flags);
    hasher.write_u32(seen.morph);
    for part in seen.parts.iter().flat_map(|parts| parts.iter()) {
        hasher.write(&[part.tint.r, part.tint.g, part.tint.b, part.tint.a, part.shown as u8]);
    }
    hasher.write_u64(look);
    hasher.write_u64(pose);
    hasher.finish()
}

/// The hash of all a frame's instances share: the eye and the lens, the
/// style, the uniforms but for the clock, the grade's place and the map
/// of what moves, the still map, the floor map and the grade's points.
fn scene_of(frame: &WorldFrame, view: &Mat4, projection: &Mat4, style: &WorldStyle, px_per_point: f32) -> u64 {
    let mut hasher = motor::hash::FxHasher::default();
    hasher.write_u32(frame.size.0);
    hasher.write_u32(frame.size.1);
    hasher.write_u8(frame.samples);
    hasher.write_u32(px_per_point.to_bits());
    for value in view.m.iter().chain(&projection.m) {
        hasher.write_u64(value.to_bits());
    }
    hasher.write_u64(super::view::style_hash(style));
    let mut uniforms = frame.uniforms;
    uniforms.params[1] = 0.0;
    uniforms.tone[3] = 0.0;
    uniforms.moving_from_eye = [0.0; 16];
    uniforms.moving = [0.0; 4];
    hasher.write(&uniforms.to_bytes());
    hasher.write_u32(frame.face_bias.to_bits());
    hasher.write_u16(frame.bloom);
    hasher.write_u64(frame.shadow.as_ref().map_or(0, |shadow| shadow.generation));
    hasher.write_u64(frame.floor.and_then(|index| frame.textures.get(index as usize)).map_or(0, ImageSource::key));
    if frame.uniforms.tone[3] > 0.0 {
        for row in frame.parts.get(frame.uniforms.tone[3] as usize - 1..).unwrap_or(&[]) {
            hasher.write(row);
        }
    }
    hasher.finish()
}

/// An entity that casts the sun's shadow: where it stands, what it is.
struct Caster {
    world_from_model: Mat4,
    model: Model,
    parts: Option<Rc<[PartState]>>,
    joint_base: u32,
    /// How many rows its palette takes.
    joint_rows: u32,
    morph: u32,
    /// The box it casts from, in its own space (grown for a skin).
    bounds: Aabb,
    /// It moves by itself: it walks, or its skin plays a clip.
    moving: bool,
    in_view: bool,
}

/// How many pixels tall a box in the eye's space stands on a target of
/// `size` through `projection` (the lens without its planes).
fn height_on_target(bounds: &Aabb, projection: &Mat4, parallel: bool, size: (u32, u32)) -> f64 {
    let tall = (bounds.max.y - bounds.min.y) * projection.m[5] * size.1 as f64 * 0.5;
    if parallel {
        return tall;
    }
    let depth = -bounds.center().z;
    if depth > 0.0 { tall / depth } else { f64::INFINITY }
}

/// The box a skinned mesh may reach from its rest pose's: half its
/// longest side more each way.
fn skinned_bounds(bounds: Aabb) -> Aabb {
    if bounds.is_empty() {
        return bounds;
    }
    let reach = Vec3::splat(bounds.size().max_element() * 0.5);
    Aabb::new(bounds.min - reach, bounds.max + reach)
}

/// Where a set of part states starts in the frame's part table, its rows
/// laid there the first time: one a part `mesh` names, a part past the
/// set's end in its own look. [`NO_PARTS`] for no set.
fn part_base_of(table: &mut Vec<PartRow>, bases: &mut HashMap<(usize, u32), u32>, parts: &Option<Rc<[PartState]>>, mesh: &Mesh) -> u32 {
    let Some(parts) = parts else { return NO_PARTS };
    let count = mesh.part_count();
    let key = (Rc::as_ptr(parts) as *const PartState as usize, count);
    *bases.entry(key).or_insert_with(|| {
        let base = table.len() as u32;
        table.extend((0..count as usize).map(|part| {
            let state = parts.get(part).copied().unwrap_or_default();
            [state.tint.r, state.tint.g, state.tint.b, if state.shown { 255 } else { 0 }]
        }));
        base
    })
}

/// Casters that share a mesh and its solid runs: one instanced draw a
/// run.
struct CasterGroup {
    runs: Vec<(u32, u32)>,
    members: Vec<usize>,
}

/// The runs of a model's triangles that light does not pass through —
/// opaque and cut-out surfaces — as `(first index, count)`, neighbours
/// joined.
fn solid_runs(model: &Model) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &(slot, first, count) in model.mesh.material_runs() {
        let alpha = model.materials.get(slot as usize).or(model.materials.last()).map_or(AlphaMode::Opaque, |material| material.alpha);
        if !matches!(alpha, AlphaMode::Opaque | AlphaMode::Mask(_)) {
            continue;
        }
        match runs.last_mut() {
            Some((start, length)) if *start + *length == first => *length += count,
            _ => runs.push((first, count)),
        }
    }
    runs
}

/// The shadow map of a sun whose light travels along `travel`: every
/// caster's solid surfaces as the sun sees them, the map fitted around
/// them. Casters that share a mesh and its runs draw instanced. `None`
/// when nothing casts.
fn shadow_frame(
    frame: &mut WorldFrame,
    mesh_slot: &mut HashMap<(u64, u64), u16>,
    part_bases: &mut HashMap<(usize, u32), u32>,
    casters: &[&Caster],
    travel: Vec3,
    size: u32,
) -> Option<(ShadowFrame, shadow::Fit)> {
    let light = shadow::light_view(travel);
    let mut bounds = Aabb::EMPTY;
    let mut groups: Vec<CasterGroup> = Vec::new();
    let mut group_of = HashMap::<(u64, u64, Vec<(u32, u32)>), usize>::new();
    for (index, caster) in casters.iter().enumerate() {
        let runs = solid_runs(&caster.model);
        if runs.is_empty() {
            continue;
        }
        bounds = bounds.union(caster.bounds.transformed(&(light * caster.world_from_model)));
        let mesh = &caster.model.mesh;
        let group = *group_of.entry((mesh.id(), mesh.generation(), runs.clone())).or_insert_with(|| {
            groups.push(CasterGroup { runs, members: Vec::new() });
            groups.len() - 1
        });
        groups[group].members.push(index);
    }
    if groups.is_empty() || bounds.is_empty() {
        return None;
    }
    let fit = shadow::Fit::around(&light, &bounds, size);
    let mut out = ShadowFrame { generation: 0, size, instances: Vec::new(), draws: Vec::new() };
    let mut hasher = motor::hash::FxHasher::default();
    hasher.write_u32(size);
    for CasterGroup { runs, members } in &groups {
        let mesh = &casters[members[0]].model.mesh;
        let slot = *mesh_slot.entry((mesh.id(), mesh.generation())).or_insert_with(|| {
            frame.meshes.push(mesh.clone());
            (frame.meshes.len() - 1) as u16
        });
        let first_instance = out.instances.len() as u32;
        for &member in members {
            let caster = casters[member];
            let part_base = part_base_of(&mut frame.parts, part_bases, &caster.parts, &caster.model.mesh);
            out.instances.push(ShadowCaster {
                rows: rows(&(fit.map_from_world * caster.world_from_model)),
                part_base,
                joint_base: caster.joint_base,
                morph: caster.morph,
            });
        }
        for &(first_index, index_count) in runs {
            out.draws.push(ShadowDraw { mesh: slot, first_index, index_count, first_instance, instance_count: members.len() as u32 });
            hasher.write_u64(mesh.id());
            hasher.write_u64(mesh.generation());
            for value in [first_index, index_count, first_instance, members.len() as u32] {
                hasher.write_u32(value);
            }
        }
    }
    for caster in &out.instances {
        for value in caster.rows.iter().flatten() {
            hasher.write_u32(value.to_bits());
        }
        // a caster's morph weights and its pose move the map
        hasher.write_u32(caster.morph);
        if caster.joint_base != NO_JOINTS {
            let rows = casters.iter().find(|known| known.joint_base == caster.joint_base).map_or(0, |known| known.joint_rows);
            for value in frame.joints.iter().skip(caster.joint_base as usize).take(rows as usize).flatten() {
                hasher.write_u32(value.to_bits());
            }
        }
        // a part that hides casts nothing: which parts hide moves the map,
        // and a tint does not
        if caster.part_base != NO_PARTS {
            let count = frame.parts.len().saturating_sub(caster.part_base as usize);
            for (index, row) in frame.parts.iter().skip(caster.part_base as usize).take(count).enumerate() {
                if row[3] == 0 {
                    hasher.write_u32(index as u32);
                }
            }
        }
    }
    out.generation = hasher.finish();
    Some((out, fit))
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

fn instance_of(seen: &Seen, material_base: u32, part_base: u32) -> InstanceData {
    let normal = seen.eye_from_model.normal_matrix();
    InstanceData {
        eye_from_model: rows(&seen.eye_from_model),
        normal: normal.map(|row| [row[0] as f32, row[1] as f32, row[2] as f32, 0.0]),
        tint: [seen.tint.r, seen.tint.g, seen.tint.b, seen.tint.a],
        material_base,
        entity: seen.entity,
        flags: seen.flags,
        ambient: [seen.ambient.r, seen.ambient.g, seen.ambient.b, 255],
        part_base,
        joint_base: seen.joint_base,
        morph: seen.morph,
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
    let mut has_sky = false;
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
                has_sky = true;
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
    // a side the sun misses is never black: the studio sky fills it until
    // the world brings a sky of its own (a black sky is a choice)
    if !has_sky {
        out.sky_color = [STUDIO_SKY[0], STUDIO_SKY[1], STUDIO_SKY[2], 0.0];
        out.ground_color = [STUDIO_GROUND[0], STUDIO_GROUND[1], STUDIO_GROUND[2], 0.0];
    }
    if !has_light {
        let toward = vec3(STUDIO_TOWARD_SUN[0], STUDIO_TOWARD_SUN[1], STUDIO_TOWARD_SUN[2]).normalized();
        out.sun_direction = [toward.x as f32, toward.y as f32, toward.z as f32, 1.0];
        out.sun_color = [STUDIO_SUN[0], STUDIO_SUN[1], STUDIO_SUN[2], 0.0];
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
    fn what_walks_casts_into_a_map_of_its_own() {
        let world = World::new();
        world.add(Mesh::plane(8.0, 8.0), gray()).set_position(vec3(0.0, -0.5, 0.0));
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray());
        let walker = world.add(Mesh::cuboid(0.5, 1.5, 0.5), gray()).set_position(vec3(-1.0, 0.25, 1.5));
        world.add_light(crate::world::material::Light::sun().toward(vec3(1.0, -0.9, 0.6)).with_shadows());
        let camera = Camera::orbiting(Vec3::ZERO, 20.0, 30.0, 8.0);
        let at = |quality: Quality| world.frame(&camera, &WorldStyle { quality, ..WorldStyle::default() }, (200, 150), 1.0);
        let still = at(Quality::Medium);
        assert!(still.moving_shadow.is_none(), "nothing walks yet");
        let generation = still.shadow.as_ref().expect("a still map").generation;
        walker.move_to(vec3(2.0, 0.25, 1.5), 1.0);
        let walking = at(Quality::Medium);
        let held = walking.shadow.as_ref().expect("the still map");
        assert_eq!(held.instances.len(), 2, "the floor and the box, the walker apart");
        assert_ne!(held.generation, generation, "the walker left the still map once");
        let moving = walking.moving_shadow.as_ref().expect("a map of what moves");
        assert_eq!((moving.instances.len(), moving.size), (1, 1024));
        assert_eq!(walking.uniforms.moving[3], 1.0, "one lookup at Medium");
        // the walk goes on, the still map stays
        assert_eq!(at(Quality::Medium).shadow.as_ref().expect("still").generation, held.generation);
        // at Low what walks casts nothing: a blob stands in
        let low = at(Quality::Low);
        assert!(low.moving_shadow.is_none() && low.uniforms.moving[3] == 0.0);
        assert_eq!(low.shadow.as_ref().expect("still").instances.len(), 2);
    }

    #[test]
    fn a_small_figure_takes_its_coarse_mesh_and_holds_it_near_the_step() {
        use crate::world::material::Model;
        let (fine, middle, coarse) = (Mesh::cuboid(1.0, 1.0, 1.0), Mesh::cuboid(1.0, 1.0, 1.0), Mesh::cuboid(1.0, 1.0, 1.0));
        let model = Model::single(fine.clone(), gray()).with_lods([(coarse.clone(), 120.0), (middle.clone(), 300.0)]);
        // the levels run from the fine mesh down, whatever order they came in
        assert_eq!(model.mesh_at(1), &middle);
        assert_eq!(model.mesh_at(2), &coarse);
        assert_eq!(model.level(310.0, None), 0);
        assert_eq!(model.level(290.0, Some(0)), 0, "within a tenth of the step: held");
        assert_eq!(model.level(260.0, Some(0)), 1, "past it: the next level");
        assert_eq!(model.level(115.0, Some(1)), 1);
        assert_eq!(model.level(100.0, Some(1)), 2);
        let world = World::new();
        world.add_model(model);
        // a parallel lens: a meter-tall box spans `200 / height` pixels of 200
        let drawn = |height: f64| {
            let camera = Camera::looking_at(vec3(0.0, 0.0, 5.0), Vec3::ZERO).parallel_height(height);
            let frame = world.frame(&camera, &WorldStyle::default(), (200, 200), 1.0);
            frame.meshes[0].clone()
        };
        assert_eq!(drawn(0.5), fine, "four hundred pixels: the fine mesh");
        assert_eq!(drawn(1.0), middle, "two hundred: the middle one");
        assert_eq!(drawn(2.5), coarse, "eighty: the coarse one");
        assert_eq!(drawn(1.6), coarse, "125 pixels: held at the coarse one");
        assert_eq!(drawn(1.4), middle, "143: past a tenth, back to the middle");
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
        // a sun of its own replaces the key light, and the studio sky
        // stays to fill the side the sun misses
        world.add_light(Light::sun().toward(vec3(0.0, -1.0, 0.0)).color(Color::rgb(255, 0, 0)));
        let frame = build(&world, &Camera::orbiting(Vec3::ZERO, 0.0, 0.0, 6.0));
        assert_eq!(frame.uniforms.sun_color[1], 0.0, "the world's own sun");
        assert!(frame.uniforms.sky_color[0] > 0.0, "the studio sky fills the shade");
        let up = frame.uniforms.sun_direction;
        assert!((up[1] - 1.0).abs() < 1e-6, "toward the sun is up, in the eye of a level camera: {up:?}");
        // a sky of its own replaces the studio's, even a black one
        world.add_light(Light::sky(Color::rgb(0, 0, 0), Color::rgb(0, 0, 0)));
        let frame = build(&world, &Camera::orbiting(Vec3::ZERO, 0.0, 0.0, 6.0));
        assert_eq!(frame.uniforms.sky_color, [0.0; 4]);
        assert_eq!(frame.uniforms.ground_color, [0.0; 4]);
    }

    #[test]
    fn a_floor_under_the_eye_keeps_the_near_plane_within_reach() {
        let world = World::new();
        world.add(Mesh::plane(400.0, 400.0), gray());
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray()).set_position(vec3(0.0, 0.5, 0.0));
        let frame = build(&world, &Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 6.0));
        // the far corner of the floor is near 290 m away: the near plane
        // stays a seventh of a metre out, where the depth still parts two
        // surfaces a centimetre apart at that distance
        assert!(frame.far > 250.0 && frame.near > 0.12, "near {} far {}", frame.near, frame.far);
        // a box alone in view keeps the near plane it hugs
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray());
        let frame = build(&world, &Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 6.0));
        assert!(frame.near > 5.0 && frame.near < 6.0, "near {}", frame.near);
    }

    #[test]
    fn surfaces_step_back_only_under_edges_that_meet_the_depth() {
        let world = World::new();
        let cube = Mesh::cuboid(1.0, 1.0, 1.0);
        world.add(cube.clone(), gray());
        let camera = Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 6.0);
        assert_eq!(build(&world, &camera).face_bias, 0.0, "no edges, no step");
        world.add_lines(cube.edges(30.0), LineStyle::solid(Color::rgb(0, 0, 0), 3.0).depth(LineDepth::Always));
        assert_eq!(build(&world, &camera).face_bias, 0.0, "an overlay meets no depth");
        world.add_lines(cube.edges(30.0), LineStyle::solid(Color::rgb(0, 0, 0), 1.5));
        // `build` draws at two pixels a point: a 1.5 pt line is 3 px wide
        let bias = build(&world, &camera).face_bias;
        assert!((bias - (1.5 + 0.25) * std::f32::consts::SQRT_2).abs() < 1e-4, "{bias}");
    }

    fn sunny(shadows: bool) -> (World, crate::world::store::Entity) {
        let world = World::new();
        let cube = world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray());
        world.add(Mesh::plane(8.0, 8.0), gray()).set_position(vec3(0.0, -0.5, 0.0));
        let sun = Light::sun().toward(vec3(0.5, -1.0, 0.7));
        world.add_light(if shadows { sun.with_shadows() } else { sun });
        (world, cube)
    }

    #[test]
    fn a_sun_without_shadows_casts_none() {
        let (world, _) = sunny(false);
        let frame = build(&world, &Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0));
        assert!(frame.shadow.is_none());
        assert_eq!(frame.uniforms.shadow, [0.0; 4], "no lookups");
    }

    #[test]
    fn the_shadow_holds_still_while_the_camera_moves() {
        let (world, _) = sunny(true);
        let generation = |camera: &Camera| {
            let frame = build(&world, camera);
            let shadow = frame.shadow.as_ref().expect("a sun with shadows over casters");
            assert_eq!(shadow.size, Quality::Medium.shadow_size());
            assert_eq!(frame.uniforms.shadow[3], 4.0, "four lookups at the default quality");
            (shadow.generation, frame.uniforms.shadow_from_eye)
        };
        let (first, from_eye) = generation(&Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0));
        for (camera, eye_moved) in [
            (Camera::orbiting(Vec3::ZERO, 80.0, 40.0, 5.0), true),
            (Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 2.5), true),
            (Camera::orbiting(vec3(1.5, 0.0, -2.0), 35.0, 28.0, 5.0), true),
            (Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0).parallel(), false),
        ] {
            let (again, seen_from) = generation(&camera);
            assert_eq!(again, first, "the camera moved, the map did not");
            // the eye reads the same map from where it stands now
            assert_eq!(seen_from != from_eye, eye_moved);
        }
    }

    #[test]
    fn a_caster_that_moves_draws_the_shadow_again() {
        let (world, cube) = sunny(true);
        let camera = Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0);
        let before = build(&world, &camera).shadow.expect("a shadow").generation;
        cube.set_position(vec3(0.0, 0.25, 0.0));
        let after = build(&world, &camera).shadow.expect("a shadow").generation;
        assert_ne!(before, after);
        // a second sun changes nothing: the first is the frame's
        let second = world.add_light(Light::sun().toward(vec3(-0.5, -1.0, 0.7)).with_shadows());
        assert_eq!(build(&world, &camera).shadow.unwrap().generation, after);
        // and a sun that turns draws it again
        second.remove();
        world.with(|data| {
            let sun = data.iter().find(|(_, entity)| matches!(entity.spec.shape, Shape::Light(Light::Sun(_)))).map(|(id, _)| id);
            assert!(sun.is_some());
        });
        let turned = World::new();
        turned.add(Mesh::cuboid(1.0, 1.0, 1.0), gray()).set_position(vec3(0.0, 0.25, 0.0));
        turned.add(Mesh::plane(8.0, 8.0), gray()).set_position(vec3(0.0, -0.5, 0.0));
        turned.add_light(Light::sun().toward(vec3(0.6, -1.0, 0.7)).with_shadows());
        assert_ne!(build(&turned, &camera).shadow.unwrap().generation, after);
    }

    #[test]
    fn a_caster_out_of_view_still_casts() {
        let (world, _) = sunny(true);
        let far = world.add(Mesh::sphere(0.5), gray()).set_position(vec3(-60.0, 0.0, 0.0));
        let camera = Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0);
        let frame = build(&world, &camera);
        let shadow = frame.shadow.as_ref().expect("a shadow");
        assert_eq!(shadow.draws.len(), 3, "the cube, the floor, and the ball out of view");
        assert_eq!(frame.draws.len(), 2, "only what the camera sees is drawn");
        assert!(shadow.draws.iter().all(|draw| (draw.mesh as usize) < frame.meshes.len()));
        far.remove();
        assert_eq!(build(&world, &camera).shadow.unwrap().draws.len(), 2);
    }

    #[test]
    fn what_lets_light_through_casts_nothing() {
        let world = World::new();
        world.add(Mesh::plane(8.0, 8.0), gray()).set_position(vec3(0.0, -0.5, 0.0)).set_casts_shadow(false);
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgba(200, 220, 255, 100)).blend());
        world.add_light(Light::sun().toward(vec3(0.5, -1.0, 0.7)).with_shadows());
        let frame = build(&world, &Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0));
        assert!(frame.shadow.is_none(), "glass and a floor that casts no shadow: no map");
        assert_eq!(frame.uniforms.shadow[3], 0.0);
        world.add(Mesh::cuboid(0.5, 0.5, 0.5), gray()).set_position(vec3(2.0, 0.0, 0.0));
        let frame = build(&world, &Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0));
        assert_eq!(frame.shadow.expect("the solid box casts").draws.len(), 1);
    }

    #[test]
    fn a_sun_fixed_to_the_view_draws_its_shadow_again_when_the_camera_turns() {
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray());
        world.add(Mesh::plane(8.0, 8.0), gray()).set_position(vec3(0.0, -0.5, 0.0));
        world.add_light(Light::sun().from_view(vec3(0.3, -1.0, -0.4)).with_shadows());
        let at = |yaw: f64| build(&world, &Camera::orbiting(Vec3::ZERO, yaw, 28.0, 5.0)).shadow.unwrap().generation;
        assert_ne!(at(35.0), at(60.0), "the sun turned with the camera");
        assert_eq!(at(35.0), at(35.0));
    }

    #[test]
    fn an_xray_draws_every_surface_see_through() {
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray());
        world.add(Mesh::plane(8.0, 8.0), gray()).set_position(vec3(0.0, -0.5, 0.0));
        world.add_lines(Mesh::cuboid(1.0, 1.0, 1.0).edges(30.0), LineStyle::solid(Color::BLACK, 1.0));
        let camera = Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 6.0);
        let plain = build(&world, &camera);
        assert!(plain.draws.iter().any(|draw| draw.pass == Pass::Opaque));
        let style = WorldStyle { xray: true, ..WorldStyle::default() };
        let xray = world.frame(&camera, &style, (640, 480), 2.0);
        let surfaces: Vec<&Draw> = xray.draws.iter().filter(|draw| matches!(draw.kind, DrawKind::Mesh { .. })).collect();
        assert_eq!(surfaces.len(), 2);
        assert!(surfaces.iter().all(|draw| draw.pass == Pass::Transparent && matches!(draw.kind, DrawKind::Mesh { blend: Blend::Alpha, .. })));
        let tints: Vec<u8> = xray.instances.iter().take(2).map(|instance| instance.tint[3]).collect();
        assert_eq!(tints, [128, 128], "half the light stopped");
        assert_eq!(xray.face_bias, 0.0, "no surface writes a depth an edge must beat");
        assert_eq!(xray.draws.last().map(|draw| draw.pass), Some(Pass::Overlay), "the edges go on top");
        assert_ne!(view_style_hash(&style), view_style_hash(&WorldStyle::default()), "an X-ray is a new picture");
    }

    fn view_style_hash(style: &WorldStyle) -> u64 {
        crate::world::view::style_hash(style)
    }

    #[test]
    fn a_night_of_lamps_gets_no_studio_sun() {
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), gray());
        world.add_light(Light::point(Color::rgb(255, 200, 150)));
        let frame = build(&world, &Camera::orbiting(Vec3::ZERO, 0.0, 20.0, 6.0));
        assert_eq!(frame.uniforms.sun_direction[3], 0.0);
        assert_eq!(frame.uniforms.counts[0], 1.0);
    }
}
