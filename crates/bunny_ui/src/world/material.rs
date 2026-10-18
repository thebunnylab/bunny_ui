//! What a surface is made of, how a line is drawn, and what light the
//! world holds.

use std::rc::Rc;

use super::math::Vec3;
use super::mesh::Mesh;
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

    /// Light from the sky and the ground.
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
