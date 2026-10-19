//! What a surface is made of, how a line is drawn, and what light the
//! world holds.

use std::rc::Rc;

use super::math::{Mat4, Vec2, Vec3};
use super::mesh::{Mesh, MeshData};
use crate::image_engine::ImageSource;
use crate::layout::Color;

/// How a surface treats the alpha of its colour.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum AlphaMode {
    /// Alpha is ignored: the surface hides what is behind it.
    #[default]
    Opaque,
    /// A cut-out: alpha below the cutoff is a hole, above it is solid.
    Mask(f32),
    /// See-through by alpha. Drawn after the opaque surfaces, back to
    /// front.
    Blend,
    /// Adds its light to what is behind it — a glow, a halo.
    Add,
}

/// The look of a surface.
///
/// Colours are sRGB, as everywhere in the framework; the shading works
/// in linear light and gives sRGB back.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// The base colour. With a texture, the two multiply.
    pub color: Color,
    /// The colour of the back of the surface. `None` = the back is not
    /// drawn at all, which is right for a closed shape.
    pub back: Option<Color>,
    /// 0 = a mirror-like gloss, 1 = fully matte.
    pub roughness: f32,
    /// 0 = a dielectric (paint, wood, cloth), 1 = a bare metal.
    pub metallic: f32,
    /// Light the surface gives off by itself.
    pub emissive: Color,
    /// How strong that light is: 1 is the colour itself, more is brighter
    /// than any lit surface.
    pub emissive_strength: f32,
    /// The base colour's texture, sampled at the mesh's UVs.
    pub texture: Option<ImageSource>,
    pub alpha: AlphaMode,
    /// Shows the colour as it is, untouched by any light — a marker, a
    /// screen, a guide.
    pub unlit: bool,
}

impl Default for Material {
    fn default() -> Material {
        Material::color(Color::WHITE)
    }
}

impl Material {
    /// A matte surface of one colour.
    pub fn color(color: Color) -> Material {
        Material {
            color,
            back: None,
            roughness: 0.8,
            metallic: 0.0,
            emissive: Color::rgb(0, 0, 0),
            emissive_strength: 1.0,
            texture: None,
            alpha: AlphaMode::Opaque,
            unlit: false,
        }
    }

    /// A colour that no light changes.
    pub fn unlit(color: Color) -> Material {
        Material { unlit: true, ..Material::color(color) }
    }

    pub fn roughness(self, roughness: f32) -> Material {
        Material { roughness: roughness.clamp(0.0, 1.0), ..self }
    }

    pub fn metallic(self, metallic: f32) -> Material {
        Material { metallic: metallic.clamp(0.0, 1.0), ..self }
    }

    /// The surface gives off `color` at `strength`.
    pub fn emissive(self, color: Color, strength: f32) -> Material {
        Material { emissive: color, emissive_strength: strength.max(0.0), ..self }
    }

    pub fn texture(self, texture: impl Into<ImageSource>) -> Material {
        Material { texture: Some(texture.into()), ..self }
    }

    /// Both sides drawn; the back in `back`.
    pub fn two_sided(self, back: Color) -> Material {
        Material { back: Some(back), ..self }
    }

    /// See-through by the alpha of the colour.
    pub fn blend(self) -> Material {
        Material { alpha: AlphaMode::Blend, ..self }
    }

    /// A cut-out at `cutoff` alpha (0…1).
    pub fn mask(self, cutoff: f32) -> Material {
        Material { alpha: AlphaMode::Mask(cutoff.clamp(0.0, 1.0)), ..self }
    }

    /// Adds its light to what is behind it.
    pub fn additive(self) -> Material {
        Material { alpha: AlphaMode::Add, ..self }
    }
}

/// A mesh with the materials its vertices name by slot. Shared and
/// compared by identity, like the mesh itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub mesh: Mesh,
    pub materials: Rc<[Material]>,
}

impl Model {
    pub fn new(mesh: Mesh, materials: impl Into<Rc<[Material]>>) -> Model {
        Model { mesh, materials: materials.into() }
    }

    /// One material for every vertex.
    pub fn single(mesh: Mesh, material: Material) -> Model {
        Model { mesh, materials: Rc::from(vec![material]) }
    }

    /// A soft dark disc `radius` meters across its half, for under
    /// something that moves — a person, a cart — where a shadow map would
    /// be drawn again every frame: one see-through quad a hair above its
    /// origin, `darkness` (0 to 1) at its heart and nothing at its rim. It
    /// casts nothing. Parent it to what it follows, at the floor's height.
    pub fn blob_shadow(radius: f64, darkness: f32) -> Model {
        // a hair above the floor it lies on, so the two never fight
        const LIFT: f64 = 0.005;
        let mut data = Mesh::plane(radius * 2.0, radius * 2.0).data().clone();
        for position in &mut data.positions {
            position[1] += LIFT as f32;
        }
        let ink = Color::rgba(0, 0, 0, (darkness.clamp(0.0, 1.0) * 255.0).round() as u8);
        Model::single(Mesh::new(data), Material::unlit(ink).texture(blob_falloff()).blend())
    }

    /// Many pieces as ONE model: one mesh, so one draw for all of them,
    /// whatever their materials. Each piece is a model, placed by a
    /// matrix, and its vertices take `part` as their part, so a piece can
    /// still take a tint of its own or hide. Equal materials are kept
    /// once. A model holds at most 256 materials: past that, a piece
    /// takes the last one.
    pub fn merge(pieces: impl IntoIterator<Item = (Model, Mat4, u16)>) -> Model {
        let mut data = MeshData::default();
        let mut materials: Vec<Material> = Vec::new();
        for (model, matrix, part) in pieces {
            let source = model.mesh.data();
            let count = source.positions.len();
            let base = data.positions.len() as u32;
            let remap: Vec<u8> = model
                .materials
                .iter()
                .map(|material| match materials.iter().position(|known| known == material) {
                    Some(at) => at.min(255) as u8,
                    None => {
                        materials.push(material.clone());
                        (materials.len() - 1).min(255) as u8
                    }
                })
                .collect();
            let normal = matrix.normal_matrix();
            let turn = |n: [f32; 3]| {
                let n = [n[0] as f64, n[1] as f64, n[2] as f64];
                let row = |r: [f64; 3]| r[0] * n[0] + r[1] * n[1] + r[2] * n[2];
                Vec3 { x: row(normal[0]), y: row(normal[1]), z: row(normal[2]) }.normalized().to_f32()
            };
            for i in 0..count {
                let p = source.positions[i];
                data.positions.push(matrix.transform_point(Vec3 { x: p[0] as f64, y: p[1] as f64, z: p[2] as f64 }).to_f32());
                data.normals.push(turn(source.normals[i]));
                data.uvs.push(if source.uvs.len() == count { source.uvs[i] } else { [0.0, 0.0] });
                data.colors.push(if source.colors.len() == count { source.colors[i] } else { [255; 4] });
                data.occlusion.push(if source.occlusion.len() == count { source.occlusion[i] } else { 255 });
                let slot = if source.materials.len() == count { source.materials[i] as usize } else { 0 };
                data.materials.push(remap.get(slot).or(remap.last()).copied().unwrap_or(0));
                data.parts.push(part);
            }
            // a mirroring matrix turns the faces around: the corners turn back
            let mirrored = matrix.determinant() < 0.0;
            for triangle in source.indices.as_chunks::<3>().0 {
                let [a, b, c] = triangle.map(|i| base + i);
                data.indices.extend_from_slice(&if mirrored { [a, c, b] } else { [a, b, c] });
            }
        }
        if materials.is_empty() {
            materials.push(Material::default());
        }
        Model::new(Mesh::new(data), materials)
    }
}

/// Where a line stands against the surfaces of the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LineDepth {
    /// Hidden where a surface is in front of it — an edge, a grid.
    #[default]
    Tested,
    /// Shown ONLY where a surface is in front of it — the hidden half of
    /// an edge, drawn dashed in an x-ray view.
    Hidden,
    /// Always on top — a guide, an axis lock.
    Always,
}

/// How a set of lines is drawn. Widths are in points, so a line keeps
/// its width when the camera moves away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineStyle {
    pub color: Color,
    pub width: f32,
    /// `(dash, gap)` in points; `None` = solid.
    pub dash: Option<(f32, f32)>,
    pub depth: LineDepth,
}

impl LineStyle {
    pub fn solid(color: Color, width: f32) -> LineStyle {
        LineStyle { color, width: width.max(0.0), dash: None, depth: LineDepth::Tested }
    }

    pub fn dashed(self, dash: f32, gap: f32) -> LineStyle {
        LineStyle { dash: Some((dash.max(0.0), gap.max(0.0))), ..self }
    }

    pub fn depth(self, depth: LineDepth) -> LineStyle {
        LineStyle { depth, ..self }
    }
}

/// The frame a sun's direction is measured in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SunFrame {
    /// The direction is the world's: the light stays put while the
    /// camera moves around.
    #[default]
    World,
    /// The direction is the camera's (x right, y up, z toward the eye):
    /// the light turns with the camera, like a studio turntable, so every
    /// view is lit the same way.
    View,
}

/// Light from very far away, all of it in one direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sun {
    /// The way the light travels.
    pub direction: Vec3,
    pub frame: SunFrame,
    pub color: Color,
    pub intensity: f32,
    pub shadows: bool,
}

impl Sun {
    /// Light that travels toward `direction`, in the world's frame.
    pub fn toward(self, direction: Vec3) -> Sun {
        Sun { direction, frame: SunFrame::World, ..self }
    }

    /// Light that travels toward `direction` in the camera's frame.
    pub fn from_view(self, direction: Vec3) -> Sun {
        Sun { direction, frame: SunFrame::View, ..self }
    }

    pub fn color(self, color: Color) -> Sun {
        Sun { color, ..self }
    }

    pub fn intensity(self, intensity: f32) -> Sun {
        Sun { intensity: intensity.max(0.0), ..self }
    }

    /// The sun casts shadows. One sun in a world can.
    pub fn with_shadows(self) -> Sun {
        Sun { shadows: true, ..self }
    }
}

/// Light from a point, fading to nothing at `range` meters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointLight {
    pub color: Color,
    pub intensity: f32,
    pub range: f32,
    /// The size of the source: a small radius gives a hot centre, a
    /// large one a soft lamp.
    pub radius: f32,
    /// The groups of surfaces this light reaches: a lamp in one room
    /// does not light the next. All groups by default.
    pub groups: u32,
}

impl PointLight {
    pub fn intensity(self, intensity: f32) -> PointLight {
        PointLight { intensity: intensity.max(0.0), ..self }
    }

    pub fn range(self, range: f32) -> PointLight {
        PointLight { range: range.max(0.0), ..self }
    }

    pub fn radius(self, radius: f32) -> PointLight {
        PointLight { radius: radius.max(0.0), ..self }
    }

    pub fn groups(self, groups: u32) -> PointLight {
        PointLight { groups, ..self }
    }
}

/// How one part of a mesh looks ([`super::mesh::MeshData::parts`]): a
/// tint over its own colours, or not shown at all. A merged mesh keeps
/// one upload and one draw while its pieces change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartState {
    pub tint: Color,
    pub shown: bool,
}

impl Default for PartState {
    fn default() -> PartState {
        PartState { tint: Color::WHITE, shown: true }
    }
}

impl PartState {
    /// The part, tinted.
    pub fn tinted(tint: Color) -> PartState {
        PartState { tint, shown: true }
    }

    /// The part, not shown.
    pub fn hidden() -> PartState {
        PartState { tint: Color::WHITE, shown: false }
    }
}

/// The falloff of a blob shadow: white, its alpha `(1 − r²)²` from the
/// centre to the rim. One picture for every blob, so they share a
/// texture and a draw.
fn blob_falloff() -> ImageSource {
    const SIDE: u32 = 64;
    let mut rgba = Vec::with_capacity((SIDE * SIDE * 4) as usize);
    for row in 0..SIDE {
        for column in 0..SIDE {
            let at = |index: u32| (index as f64 + 0.5) / SIDE as f64 * 2.0 - 1.0;
            let r2 = (at(column) * at(column) + at(row) * at(row)).min(1.0);
            let alpha = (1.0 - r2) * (1.0 - r2);
            rgba.extend_from_slice(&[255, 255, 255, (alpha * 255.0).round() as u8]);
        }
    }
    ImageSource::rgba(0xB10B_0000_0000_0001, (SIDE, SIDE), rgba)
}

/// Light and shade baked flat onto the ground: a picture laid on the
/// world's x–z plane and read by the surfaces that ask for it
/// ([`super::store::EntitySpec::reads_floor_map`]). Its colour is light
/// added to theirs — a room's lamps, baked once — and its alpha the share
/// of the sky's light that reaches them: the shade a chair leaves under
/// itself. A surface reads it for the price of one texel a pixel.
#[derive(Clone, Debug, PartialEq)]
pub struct FloorMap {
    /// sRGB colour, straight alpha; an alpha of 255 is nothing in the way.
    pub picture: ImageSource,
    /// The world `(x, z)` of the picture's first texel corner: its first
    /// row lies along `z = origin.y`, its first column along `x = origin.x`.
    pub origin: Vec2,
    /// The meters the picture spans: along x, then along z.
    pub size: Vec2,
}

/// The light a world holds. A light is an entity: a point light stands
/// where its entity stands and follows its parent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Light {
    Sun(Sun),
    /// Light from the whole sky: `sky` from above, `ground` bounced from
    /// below, blended by the way a surface faces.
    Sky { sky: Color, ground: Color, intensity: f32 },
    Point(PointLight),
}

impl Light {
    /// A white sun straight from above, without shadows.
    pub fn sun() -> Sun {
        Sun {
            direction: Vec3 { x: 0.0, y: -1.0, z: 0.0 },
            frame: SunFrame::World,
            color: Color::WHITE,
            intensity: 1.0,
            shadows: false,
        }
    }

    /// Light from the sky and the ground. A world without one gets a
    /// soft studio sky, so a side no sun reaches is never black; a world
    /// that wants a black sky says so with this.
    pub fn sky(sky: Color, ground: Color) -> Light {
        Light::Sky { sky, ground, intensity: 1.0 }
    }

    /// A lamp of `color`.
    pub fn point(color: Color) -> PointLight {
        PointLight { color, intensity: 1.0, range: 5.0, radius: 0.5, groups: u32::MAX }
    }
}

impl From<Sun> for Light {
    fn from(sun: Sun) -> Light {
        Light::Sun(sun)
    }
}

impl From<PointLight> for Light {
    fn from(point: PointLight) -> Light {
        Light::Point(point)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::math::{Quat, Transform, vec3};

    #[test]
    fn merged_pieces_are_one_mesh_with_their_materials_and_parts() {
        let red = Material::color(Color::rgb(200, 0, 0));
        let blue = Material::color(Color::rgb(0, 0, 200));
        let cube = Mesh::cuboid(1.0, 1.0, 1.0);
        let pieces = vec![
            (Model::single(cube.clone(), red.clone()), Mat4::translation(vec3(-2.0, 0.0, 0.0)), 1),
            (Model::single(cube.clone(), blue.clone()), Mat4::IDENTITY, 2),
            (Model::single(cube.clone(), red.clone()), Mat4::translation(vec3(2.0, 0.0, 0.0)), 3),
        ];
        let merged = Model::merge(pieces);
        assert_eq!(merged.materials.len(), 2, "equal materials kept once");
        assert_eq!(merged.mesh.triangle_count(), 36);
        assert_eq!(merged.mesh.material_runs().len(), 2, "one run per material");
        let bounds = merged.mesh.bounds();
        assert_eq!((bounds.min.x, bounds.max.x), (-2.5, 2.5));
        let parts: std::collections::BTreeSet<u16> = merged.mesh.data().parts.iter().copied().collect();
        assert_eq!(parts.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn a_mirrored_piece_still_faces_out() {
        let cube = Model::single(Mesh::cuboid(1.0, 1.0, 1.0), Material::default());
        let mirror = Transform::IDENTITY.with_scale(vec3(-1.0, 1.0, 1.0)).with_rotation(Quat::IDENTITY).matrix();
        let merged = Model::merge(vec![(cube, mirror, 0)]);
        let data = merged.mesh.data();
        let volume: f64 = data
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| {
                    let p = data.positions[i as usize];
                    vec3(p[0] as f64, p[1] as f64, p[2] as f64)
                });
                a.dot(b.cross(c)) / 6.0
            })
            .sum();
        assert!((volume - 1.0).abs() < 1e-6, "faces out after the mirror: {volume}");
    }
}
