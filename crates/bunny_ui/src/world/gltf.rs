//! The glTF door: models, materials and the tree of nodes from a GLB
//! file or a glTF text with its buffers inside.
//!
//! What it reads: triangle meshes with positions, normals, the first
//! texture coordinates, the first vertex colours and a baked occlusion
//! (`_AO`); quantized attributes (`KHR_mesh_quantization`); metal-rough
//! materials with a base colour texture, emissive light
//! (`KHR_materials_emissive_strength`), alpha modes, two sides and
//! unlit surfaces (`KHR_materials_unlit`); nodes with a matrix or a
//! translation, rotation and scale; skins (`JOINTS_0`, `WEIGHTS_0`, the
//! inverse binds) and the animations that move their nodes, keys held
//! (`STEP`) or blended (`LINEAR`); up to two morph targets a mesh (their
//! places and normals, weighed by the app); and the `extras` of nodes and
//! meshes, handed on as raw JSON.
//!
//! What it refuses, by name: buffers in files outside the asset, sparse
//! accessors, cubic spline keys (sample them at a steady rate), a skin
//! of more than 256 joints, and any input that is not what it claims —
//! always an error, never a panic.

use std::rc::Rc;

use super::json::{Json, JsonError};
use super::material::{AlphaMode, Material, Model};
use super::math::{Mat4, Quat, Transform, Vec3, vec3};
use super::mesh::{MAX_MORPHS, Mesh, MeshData, Morph};
use super::skin::{Channel, Clip, Path, Skeleton, SkeletonNode, Skin};
use super::shade::encode;
use super::store::{Entity, EntitySpec, Shape, World};
use crate::image_engine::ImageSource;
use crate::layout::Color;

/// Why an asset could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GltfError {
    /// The bytes are not a GLB container (and not a glTF text).
    NotGltf,
    /// A glTF version this reader does not know.
    Version(u32),
    Json(JsonError),
    /// A required part is missing or names nothing.
    Missing(&'static str),
    /// Data runs past the end of its buffer.
    OutOfRange(&'static str),
    /// A feature this reader refuses.
    Unsupported(&'static str),
}

impl std::fmt::Display for GltfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GltfError::NotGltf => write!(f, "not a glTF asset"),
            GltfError::Version(v) => write!(f, "glTF version {v} is not supported"),
            GltfError::Json(e) => write!(f, "bad JSON: {e}"),
            GltfError::Missing(what) => write!(f, "missing {what}"),
            GltfError::OutOfRange(what) => write!(f, "{what} runs past its buffer"),
            GltfError::Unsupported(what) => write!(f, "{what} is not supported"),
        }
    }
}

impl std::error::Error for GltfError {}

/// One node of an asset's tree.
#[derive(Clone, Debug)]
pub struct AssetNode {
    pub name: Option<Rc<str>>,
    pub transform: Transform,
    /// Into [`Asset::models`].
    pub model: Option<usize>,
    pub children: Vec<usize>,
    /// The node's `extras`, as compact JSON text.
    pub extras: Option<Rc<str>>,
}

/// What a glTF asset holds: a model per mesh, the tree of nodes that
/// places them, and the nodes at the top of the scene; for a figure, its
/// skeleton, its skins and its clips.
#[derive(Clone, Debug)]
pub struct Asset {
    /// A model per mesh, then one per node that hangs a mesh from a skin.
    pub models: Vec<Model>,
    /// The `extras` of each model's mesh, beside it.
    pub model_extras: Vec<Option<Rc<str>>>,
    pub nodes: Vec<AssetNode>,
    pub roots: Vec<usize>,
    /// The asset's nodes as a skeleton, when a skin names them.
    pub skeleton: Option<Skeleton>,
    pub skins: Vec<Skin>,
    /// The animations, each a clip of the skeleton's nodes.
    pub clips: Vec<Clip>,
}

impl Asset {
    /// The clip named `name`.
    pub fn clip(&self, name: &str) -> Option<&Clip> {
        self.clips.iter().find(|clip| clip.name() == Some(name))
    }

    /// Reads a GLB file, or a glTF text whose buffers and images are
    /// `data:` URIs.
    pub fn from_bytes(bytes: &[u8]) -> Result<Asset, GltfError> {
        if bytes.len() >= 12 && &bytes[0..4] == b"glTF" {
            let (json, bin) = split_glb(bytes)?;
            return read(&json, bin);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| GltfError::NotGltf)?;
        if !text.trim_start().starts_with('{') {
            return Err(GltfError::NotGltf);
        }
        let json = Json::parse(text).map_err(GltfError::Json)?;
        read(&json, None)
    }
}

impl World {
    /// Places an asset in the world: a group entity at the origin, and an
    /// entity under it for every node, models shared — an asset spawned
    /// twice draws as one instanced mesh. Move, turn or remove the asset by
    /// its group.
    pub fn spawn(&self, asset: &Asset) -> Entity {
        let root = self.add_group();
        let mut stack: Vec<(usize, Entity)> = asset.roots.iter().rev().map(|&node| (node, root)).collect();
        let mut seen = vec![false; asset.nodes.len()];
        while let Some((index, parent)) = stack.pop() {
            let Some(node) = asset.nodes.get(index) else { continue };
            if std::mem::replace(&mut seen[index], true) {
                continue; // a node the tree names twice is placed once
            }
            let shape = node.model.and_then(|m| asset.models.get(m)).map_or(Shape::Empty, |model| Shape::Model(model.clone()));
            let mut spec = EntitySpec::new(shape);
            spec.transform = node.transform;
            spec.extras = node.extras.clone().or_else(|| node.model.and_then(|m| asset.model_extras.get(m).cloned().flatten()));
            let entity = self.add_spec(spec).set_parent(Some(parent));
            stack.extend(node.children.iter().rev().map(|&child| (child, entity)));
        }
        root
    }
}

// MARK: - The container

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, GltfError> {
    bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or(GltfError::OutOfRange("the GLB header"))
}

fn split_glb(bytes: &[u8]) -> Result<(Json, Option<&[u8]>), GltfError> {
    let version = u32_at(bytes, 4)?;
    if version != 2 {
        return Err(GltfError::Version(version));
    }
    let length = (u32_at(bytes, 8)? as usize).min(bytes.len());
    let mut at = 12;
    let mut json = None;
    let mut bin = None;
    while at + 8 <= length {
        let size = u32_at(bytes, at)? as usize;
        let kind = u32_at(bytes, at + 4)?;
        let body = bytes.get(at + 8..at + 8 + size).ok_or(GltfError::OutOfRange("a GLB chunk"))?;
        match kind {
            0x4E4F_534A => {
                let text = std::str::from_utf8(body).map_err(|_| GltfError::NotGltf)?;
                json = Some(Json::parse(text.trim_end_matches(['\0', ' '])).map_err(GltfError::Json)?);
            }
            0x004E_4942 if bin.is_none() => bin = Some(body),
            _ => {}
        }
        at += 8 + size.div_ceil(4) * 4;
    }
    Ok((json.ok_or(GltfError::Missing("the JSON chunk"))?, bin))
}

fn base64(text: &str) -> Result<Vec<u8>, GltfError> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return Err(GltfError::Unsupported("a malformed data URI")),
        };
        acc = (acc << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// The bytes of a `data:` URI.
fn data_uri(uri: &str) -> Result<Vec<u8>, GltfError> {
    let Some(rest) = uri.strip_prefix("data:") else { return Err(GltfError::Unsupported("a buffer in a file outside the asset")) };
    let Some((_, payload)) = rest.split_once(";base64,") else { return Err(GltfError::Unsupported("a data URI that is not base64")) };
    base64(payload)
}

// MARK: - The document

struct Document<'a> {
    json: &'a Json,
    buffers: Vec<Rc<[u8]>>,
}

impl Document<'_> {
    fn list(&self, name: &str) -> &[Json] {
        self.json.get(name).map_or(&[], Json::as_array)
    }

    fn item(&self, name: &'static str, index: usize) -> Result<&Json, GltfError> {
        self.list(name).get(index).ok_or(GltfError::Missing(name))
    }

    /// The bytes of a buffer view.
    fn view(&self, index: usize) -> Result<(&[u8], Option<usize>), GltfError> {
        let view = self.item("bufferViews", index)?;
        let buffer = view.get("buffer").and_then(Json::as_index).ok_or(GltfError::Missing("bufferView.buffer"))?;
        let buffer = self.buffers.get(buffer).ok_or(GltfError::Missing("buffers"))?;
        let offset = view.get("byteOffset").and_then(Json::as_index).unwrap_or(0);
        let length = view.get("byteLength").and_then(Json::as_index).ok_or(GltfError::Missing("bufferView.byteLength"))?;
        let bytes = buffer.get(offset..offset.checked_add(length).ok_or(GltfError::OutOfRange("a buffer view"))?).ok_or(GltfError::OutOfRange("a buffer view"))?;
        Ok((bytes, view.get("byteStride").and_then(Json::as_index)))
    }

    /// An accessor's values as `f32`, normalized integers mapped as the
    /// glTF rules say: `count` items of `components` each.
    fn floats(&self, index: usize) -> Result<(Vec<f32>, usize), GltfError> {
        let accessor = self.item("accessors", index)?;
        if accessor.get("sparse").is_some() {
            return Err(GltfError::Unsupported("a sparse accessor"));
        }
        let components = match accessor.get("type").and_then(Json::as_str) {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            Some("MAT4") => 16,
            _ => return Err(GltfError::Unsupported("an accessor type")),
        };
        let kind = accessor.get("componentType").and_then(Json::as_index).ok_or(GltfError::Missing("accessor.componentType"))?;
        let size = match kind {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return Err(GltfError::Unsupported("a component type")),
        };
        let normalized = accessor.get("normalized").and_then(Json::as_bool).unwrap_or(false);
        let count = accessor.get("count").and_then(Json::as_index).ok_or(GltfError::Missing("accessor.count"))?;
        let Some(view) = accessor.get("bufferView").and_then(Json::as_index) else {
            return Ok((vec![0.0; count * components], components)); // all zeros, as the rules say
        };
        let (bytes, stride) = self.view(view)?;
        let offset = accessor.get("byteOffset").and_then(Json::as_index).unwrap_or(0);
        let stride = stride.unwrap_or(size * components);
        let mut out = Vec::with_capacity(count * components);
        for item in 0..count {
            for component in 0..components {
                let at = offset + item * stride + component * size;
                let raw = bytes.get(at..at + size).ok_or(GltfError::OutOfRange("an accessor"))?;
                let value = match kind {
                    5120 => {
                        let v = raw[0] as i8 as f32;
                        if normalized { (v / 127.0).max(-1.0) } else { v }
                    }
                    5121 => {
                        let v = raw[0] as f32;
                        if normalized { v / 255.0 } else { v }
                    }
                    5122 => {
                        let v = i16::from_le_bytes([raw[0], raw[1]]) as f32;
                        if normalized { (v / 32767.0).max(-1.0) } else { v }
                    }
                    5123 => {
                        let v = u16::from_le_bytes([raw[0], raw[1]]) as f32;
                        if normalized { v / 65535.0 } else { v }
                    }
                    5125 => u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as f32,
                    _ => f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]),
                };
                out.push(value);
            }
        }
        Ok((out, components))
    }

    /// An index accessor, exactly (a `u32` index survives no `f32`).
    fn indices(&self, index: usize) -> Result<Vec<u32>, GltfError> {
        let accessor = self.item("accessors", index)?;
        let kind = accessor.get("componentType").and_then(Json::as_index).ok_or(GltfError::Missing("accessor.componentType"))?;
        let size = match kind {
            5121 => 1,
            5123 => 2,
            5125 => 4,
            _ => return Err(GltfError::Unsupported("an index type")),
        };
        let count = accessor.get("count").and_then(Json::as_index).ok_or(GltfError::Missing("accessor.count"))?;
        let view = accessor.get("bufferView").and_then(Json::as_index).ok_or(GltfError::Missing("accessor.bufferView"))?;
        let (bytes, stride) = self.view(view)?;
        let offset = accessor.get("byteOffset").and_then(Json::as_index).unwrap_or(0);
        let stride = stride.unwrap_or(size);
        (0..count)
            .map(|item| {
                let at = offset + item * stride;
                let raw = bytes.get(at..at + size).ok_or(GltfError::OutOfRange("an index accessor"))?;
                Ok(match size {
                    1 => raw[0] as u32,
                    2 => u16::from_le_bytes([raw[0], raw[1]]) as u32,
                    _ => u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]),
                })
            })
            .collect()
    }

    fn image(&self, index: usize) -> Result<ImageSource, GltfError> {
        let image = self.item("images", index)?;
        if let Some(view) = image.get("bufferView").and_then(Json::as_index) {
            let (bytes, _) = self.view(view)?;
            return Ok(ImageSource::from_bytes(bytes.to_vec()));
        }
        let uri = image.get("uri").and_then(Json::as_str).ok_or(GltfError::Missing("image.uri"))?;
        Ok(ImageSource::from_bytes(data_uri(uri)?))
    }

    fn material(&self, index: usize) -> Result<Material, GltfError> {
        let json = self.item("materials", index)?;
        let pbr = json.get("pbrMetallicRoughness");
        let factor = pbr.and_then(|p| p.get("baseColorFactor")).and_then(Json::as_numbers).unwrap_or_else(|| vec![1.0; 4]);
        let linear = |i: usize| factor.get(i).copied().unwrap_or(1.0) as f32;
        let color = Color { r: encode(linear(0)), g: encode(linear(1)), b: encode(linear(2)), a: (linear(3).clamp(0.0, 1.0) * 255.0 + 0.5) as u8 };
        let mut material = Material::color(color)
            .metallic(pbr.and_then(|p| p.get("metallicFactor")).and_then(Json::as_f64).unwrap_or(1.0) as f32)
            .roughness(pbr.and_then(|p| p.get("roughnessFactor")).and_then(Json::as_f64).unwrap_or(1.0) as f32);
        let texture = pbr.and_then(|p| p.get("baseColorTexture")).and_then(|t| t.get("index")).and_then(Json::as_index);
        if let Some(texture) = texture {
            let source = self.item("textures", texture)?.get("source").and_then(Json::as_index).ok_or(GltfError::Missing("texture.source"))?;
            material = material.texture(self.image(source)?);
        }
        let emissive = json.get("emissiveFactor").and_then(Json::as_numbers).unwrap_or_default();
        if emissive.len() == 3 && emissive.iter().any(|v| *v > 0.0) {
            let strength = json
                .get("extensions")
                .and_then(|e| e.get("KHR_materials_emissive_strength"))
                .and_then(|e| e.get("emissiveStrength"))
                .and_then(Json::as_f64)
                .unwrap_or(1.0);
            let glow = Color { r: encode(emissive[0] as f32), g: encode(emissive[1] as f32), b: encode(emissive[2] as f32), a: 255 };
            material = material.emissive(glow, strength as f32);
        }
        material.alpha = match json.get("alphaMode").and_then(Json::as_str) {
            Some("MASK") => AlphaMode::Mask(json.get("alphaCutoff").and_then(Json::as_f64).unwrap_or(0.5) as f32),
            Some("BLEND") => AlphaMode::Blend,
            _ => AlphaMode::Opaque,
        };
        if json.get("doubleSided").and_then(Json::as_bool).unwrap_or(false) {
            material.back = Some(color);
        }
        if json.get("extensions").and_then(|e| e.get("KHR_materials_unlit")).is_some() {
            material.unlit = true;
        }
        Ok(material)
    }
}

/// The glTF default material: white, metal, rough.
fn default_material() -> Material {
    Material::color(Color::WHITE).metallic(1.0).roughness(1.0)
}

fn read(json: &Json, bin: Option<&[u8]>) -> Result<Asset, GltfError> {
    let version = json.get("asset").and_then(|a| a.get("version")).and_then(Json::as_str).ok_or(GltfError::Missing("asset.version"))?;
    if !version.starts_with("2.") {
        return Err(GltfError::Version(version.split('.').next().and_then(|v| v.parse().ok()).unwrap_or(0)));
    }
    for required in json.get("extensionsRequired").map_or(&[][..], Json::as_array) {
        match required.as_str() {
            Some("KHR_mesh_quantization" | "KHR_materials_emissive_strength" | "KHR_materials_unlit") => {}
            _ => return Err(GltfError::Unsupported("a required extension")),
        }
    }
    let mut buffers: Vec<Rc<[u8]>> = Vec::new();
    for (i, buffer) in json.get("buffers").map_or(&[][..], Json::as_array).iter().enumerate() {
        let bytes: Vec<u8> = match buffer.get("uri").and_then(Json::as_str) {
            Some(uri) => data_uri(uri)?,
            None if i == 0 => bin.ok_or(GltfError::Missing("the BIN chunk"))?.to_vec(),
            None => return Err(GltfError::Missing("buffer.uri")),
        };
        buffers.push(Rc::from(bytes));
    }
    let document = Document { json, buffers };

    let mut materials_cache: Vec<Option<Material>> = vec![None; document.list("materials").len()];
    let mut material = |index: Option<usize>| -> Result<Material, GltfError> {
        let Some(index) = index else { return Ok(default_material()) };
        if index >= materials_cache.len() {
            return Err(GltfError::Missing("materials"));
        }
        if materials_cache[index].is_none() {
            materials_cache[index] = Some(document.material(index)?);
        }
        Ok(materials_cache[index].clone().expect("just read"))
    };

    let mut models = Vec::new();
    let mut model_extras = Vec::new();
    for mesh in document.list("meshes") {
        let mut data = MeshData::default();
        let mut slots: Vec<Option<usize>> = Vec::new();
        let mut materials: Vec<Material> = Vec::new();
        let mut morphs: Vec<Morph> = Vec::new();
        for primitive in mesh.get("primitives").map_or(&[][..], Json::as_array) {
            if primitive.get("mode").and_then(Json::as_index).unwrap_or(4) != 4 {
                continue; // lines and points have no surface to draw
            }
            let attributes = primitive.get("attributes").ok_or(GltfError::Missing("primitive.attributes"))?;
            let attribute = |name: &str| attributes.get(name).and_then(Json::as_index);
            let position = attribute("POSITION").ok_or(GltfError::Missing("POSITION"))?;
            let (positions, components) = document.floats(position)?;
            if components != 3 {
                return Err(GltfError::Unsupported("a POSITION that is not VEC3"));
            }
            let count = positions.len() / 3;
            let base = data.positions.len() as u32;
            let material_index = primitive.get("material").and_then(Json::as_index);
            let slot = match slots.iter().position(|known| *known == material_index) {
                Some(slot) => slot,
                None => {
                    slots.push(material_index);
                    materials.push(material(material_index)?);
                    slots.len() - 1
                }
            };
            data.positions
                .extend(positions.as_chunks::<3>().0.iter().copied());
            let normals = match attribute("NORMAL") {
                Some(index) => document.floats(index)?.0.as_chunks::<3>().0.to_vec(),
                None => Vec::new(),
            };
            let uvs = match attribute("TEXCOORD_0") {
                Some(index) => document.floats(index)?.0.as_chunks::<2>().0.to_vec(),
                None => Vec::new(),
            };
            let colors = match attribute("COLOR_0") {
                Some(index) => {
                    let (values, components) = document.floats(index)?;
                    values
                        .chunks_exact(components.max(1))
                        .map(|c| {
                            let alpha = if components == 4 { c[3] } else { 1.0 };
                            [encode(c[0]), encode(c[1]), encode(c[2]), (alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8]
                        })
                        .collect::<Vec<_>>()
                }
                None => Vec::new(),
            };
            let occlusion = match attribute("_AO") {
                Some(index) => document.floats(index)?.0.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect::<Vec<_>>(),
                None => Vec::new(),
            };
            // the joints a vertex hangs from, a byte each, and their shares
            let joints = match attribute("JOINTS_0") {
                Some(index) => {
                    let (values, components) = document.floats(index)?;
                    if components != 4 {
                        return Err(GltfError::Unsupported("JOINTS_0 that is not VEC4"));
                    }
                    if values.iter().any(|&joint| !(0.0..256.0).contains(&joint)) {
                        return Err(GltfError::Unsupported("a skin of more than 256 joints"));
                    }
                    values.as_chunks::<4>().0.iter().map(|j| j.map(|joint| joint as u8)).collect::<Vec<_>>()
                }
                None => Vec::new(),
            };
            let weights = match attribute("WEIGHTS_0") {
                Some(index) => {
                    let (values, components) = document.floats(index)?;
                    if components != 4 {
                        return Err(GltfError::Unsupported("WEIGHTS_0 that is not VEC4"));
                    }
                    values.as_chunks::<4>().0.iter().map(|w| w.map(|share| (share.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect::<Vec<_>>()
                }
                None => Vec::new(),
            };
            // an attribute of the wrong length is no attribute
            let fill = |list: Vec<[f32; 3]>| if list.len() == count { list } else { Vec::new() };
            let normals = fill(normals);
            data.normals.extend(if normals.is_empty() { vec![[0.0, 0.0, 0.0]; count] } else { normals });
            data.uvs.extend(if uvs.len() == count { uvs } else { vec![[0.0, 0.0]; count] });
            data.colors.extend(if colors.len() == count { colors } else { vec![[255; 4]; count] });
            data.occlusion.extend(if occlusion.len() == count { occlusion } else { vec![255; count] });
            // a skin's lists stay whole across the primitives, or go
            if joints.len() == count && weights.len() == count && data.joints.len() == base as usize {
                data.joints.extend(joints);
                data.weights.extend(weights);
            } else {
                data.joints.clear();
                data.weights.clear();
            }
            data.materials.extend(std::iter::repeat_n(slot.min(255) as u8, count));
            // morph targets: each primitive's vertices in each target, a
            // primitive without one adding no change
            let targets = primitive.get("targets").map_or(&[][..], Json::as_array);
            if targets.len() > MAX_MORPHS {
                return Err(GltfError::Unsupported("more than two morph targets"));
            }
            while morphs.len() < targets.len() {
                morphs.push(Morph { positions: vec![[0.0; 3]; base as usize], normals: vec![[0.0; 3]; base as usize] });
            }
            for (k, morph) in morphs.iter_mut().enumerate() {
                let target = targets.get(k);
                let read = |name: &str| -> Result<Vec<[f32; 3]>, GltfError> {
                    match target.and_then(|t| t.get(name)).and_then(Json::as_index) {
                        Some(index) => {
                            let (values, components) = document.floats(index)?;
                            if components != 3 || values.len() != count * 3 {
                                return Err(GltfError::Unsupported("a morph target that does not match its primitive"));
                            }
                            Ok(values.as_chunks::<3>().0.to_vec())
                        }
                        None => Ok(vec![[0.0; 3]; count]),
                    }
                };
                morph.positions.extend(read("POSITION")?);
                morph.normals.extend(read("NORMAL")?);
            }
            match primitive.get("indices").and_then(Json::as_index) {
                Some(index) => {
                    for i in document.indices(index)? {
                        if i as usize >= count {
                            return Err(GltfError::OutOfRange("an index"));
                        }
                        data.indices.push(base + i);
                    }
                }
                None => data.indices.extend(base..base + count as u32),
            }
        }
        // a mesh whose primitives gave no normal shows flat faces
        if data.normals.iter().all(|n| *n == [0.0, 0.0, 0.0]) {
            data.normals.clear();
        }
        if materials.is_empty() {
            materials.push(default_material());
        }
        data.morphs = morphs;
        models.push(Model::new(Mesh::new(data), materials));
        model_extras.push(mesh.get("extras").map(|e| Rc::from(e.to_text())));
    }

    let mut nodes = Vec::new();
    for node in document.list("nodes") {
        let transform = match node.get("matrix").and_then(Json::as_numbers) {
            Some(m) if m.len() == 16 => {
                let mut matrix = Mat4::IDENTITY;
                matrix.m.copy_from_slice(&m);
                Transform::from_matrix(&matrix)
            }
            _ => {
                let numbers = |name: &str| node.get(name).and_then(Json::as_numbers);
                let t = numbers("translation").filter(|v| v.len() == 3).unwrap_or_else(|| vec![0.0; 3]);
                let r = numbers("rotation").filter(|v| v.len() == 4).unwrap_or_else(|| vec![0.0, 0.0, 0.0, 1.0]);
                let s = numbers("scale").filter(|v| v.len() == 3).unwrap_or_else(|| vec![1.0; 3]);
                Transform {
                    translation: vec3(t[0], t[1], t[2]),
                    rotation: Quat { x: r[0], y: r[1], z: r[2], w: r[3] }.normalized(),
                    scale: Vec3 { x: s[0], y: s[1], z: s[2] },
                }
            }
        };
        let model = node.get("mesh").and_then(Json::as_index).filter(|&m| m < models.len());
        let children = node.get("children").map_or(&[][..], Json::as_array).iter().filter_map(Json::as_index).collect();
        nodes.push(AssetNode {
            name: node.get("name").and_then(Json::as_str).map(Rc::from),
            transform,
            model,
            children,
            extras: node.get("extras").map(|e| Rc::from(e.to_text())),
        });
    }

    let scene = json.get("scene").and_then(Json::as_index).unwrap_or(0);
    let roots: Vec<usize> = match json.get("scenes").map(Json::as_array).and_then(|scenes| scenes.get(scene)) {
        Some(scene) => scene.get("nodes").map_or(&[][..], Json::as_array).iter().filter_map(Json::as_index).filter(|&n| n < nodes.len()).collect(),
        None => {
            // no scene: every node nobody names as a child is a root
            let mut child = vec![false; nodes.len()];
            for node in &nodes {
                for &c in &node.children {
                    if let Some(flag) = child.get_mut(c) {
                        *flag = true;
                    }
                }
            }
            (0..nodes.len()).filter(|&n| !child[n]).collect()
        }
    };
    // the skins: the asset's nodes as one skeleton, each skin naming its
    // joints among them; a node that hangs a mesh from a skin takes a
    // model of its own, placed where the node stands in the skeleton
    let skin_list = document.list("skins");
    let (skeleton, skins) = if skin_list.is_empty() {
        (None, Vec::new())
    } else {
        let mut parent = vec![None; nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            for &child in &node.children {
                if let Some(slot) = parent.get_mut(child) {
                    *slot = Some(index as u32);
                }
            }
        }
        let skeleton = Skeleton::new(
            nodes.iter().zip(&parent).map(|(node, &parent)| SkeletonNode { name: node.name.clone(), parent, rest: node.transform }).collect(),
        );
        let mut skins = Vec::new();
        for skin in skin_list {
            let joints: Vec<u32> = skin.get("joints").map_or(&[][..], Json::as_array).iter().filter_map(Json::as_index).map(|j| j as u32).collect();
            if joints.iter().any(|&joint| joint as usize >= nodes.len()) {
                return Err(GltfError::Missing("a skin's joint node"));
            }
            let inverse_bind = match skin.get("inverseBindMatrices").and_then(Json::as_index) {
                Some(index) => {
                    let (values, components) = document.floats(index)?;
                    if components != 16 {
                        return Err(GltfError::Unsupported("inverse binds that are not MAT4"));
                    }
                    values
                        .as_chunks::<16>()
                        .0
                        .iter()
                        .map(|m| {
                            let mut matrix = Mat4::IDENTITY;
                            for (at, value) in m.iter().enumerate() {
                                matrix.m[at] = *value as f64;
                            }
                            matrix
                        })
                        .collect()
                }
                None => Vec::new(),
            };
            skins.push(Skin::new(skeleton.clone(), joints, inverse_bind).ok_or(GltfError::Unsupported("a skin of more than 256 joints"))?);
        }
        (Some(skeleton), skins)
    };
    if let Some(skeleton) = &skeleton {
        let pose = skeleton.pose(None, 0.0);
        for (index, node) in document.list("nodes").iter().enumerate() {
            let Some(skin) = node.get("skin").and_then(Json::as_index) else { continue };
            let skin = skins.get(skin).ok_or(GltfError::Missing("skins"))?;
            let Some(model) = nodes[index].model else { continue };
            let skinned = models[model].clone().skinned(skin.placed(&pose[index]));
            models.push(skinned);
            model_extras.push(model_extras[model].clone());
            nodes[index].model = Some(models.len() - 1);
        }
    }

    // the animations: each a clip of the skeleton's nodes
    let mut clips = Vec::new();
    for animation in document.list("animations") {
        let samplers = animation.get("samplers").map_or(&[][..], Json::as_array);
        let mut channels = Vec::new();
        for channel in animation.get("channels").map_or(&[][..], Json::as_array) {
            let target = channel.get("target").ok_or(GltfError::Missing("channel.target"))?;
            let Some(node) = target.get("node").and_then(Json::as_index).filter(|&node| node < nodes.len()) else { continue };
            let path = match target.get("path").and_then(Json::as_str) {
                Some("translation") => Path::Translation,
                Some("rotation") => Path::Rotation,
                Some("scale") => Path::Scale,
                _ => continue, // morph weights move no node
            };
            let sampler = channel.get("sampler").and_then(Json::as_index).and_then(|at| samplers.get(at)).ok_or(GltfError::Missing("animation.sampler"))?;
            let step = match sampler.get("interpolation").and_then(Json::as_str).unwrap_or("LINEAR") {
                "LINEAR" => false,
                "STEP" => true,
                _ => return Err(GltfError::Unsupported("a cubic spline animation")),
            };
            let input = sampler.get("input").and_then(Json::as_index).ok_or(GltfError::Missing("sampler.input"))?;
            let output = sampler.get("output").and_then(Json::as_index).ok_or(GltfError::Missing("sampler.output"))?;
            let (times, _) = document.floats(input)?;
            let (values, _) = document.floats(output)?;
            channels.push(Channel { node: node as u32, path, step, times: Rc::from(times), values: Rc::from(values) });
        }
        clips.push(Clip::new(animation.get("name").and_then(Json::as_str).map(Rc::from), channels));
    }
    Ok(Asset { models, model_extras, nodes, roots, skeleton, skins, clips })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GLB of `json` and `bin`, each chunk padded to four bytes.
    fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut json = json.as_bytes().to_vec();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut bin = bin.to_vec();
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        out.extend_from_slice(&json);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        out.extend_from_slice(&bin);
        out
    }

    /// One triangle: positions as `f32`, then `u16` indices.
    fn triangle_bin() -> Vec<u8> {
        let mut bin = Vec::new();
        for value in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
        for index in [0u16, 1, 2] {
            bin.extend_from_slice(&index.to_le_bytes());
        }
        bin
    }

    fn triangle_json(material: &str, extras: &str) -> String {
        format!(
            r#"{{"asset":{{"version":"2.0"}},"buffers":[{{"byteLength":42}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":6}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},{{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}}],
            "materials":[{material}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1,"material":0}}]}}],
            "nodes":[{{"mesh":0,"translation":[1,2,3]{extras}}}],
            "scenes":[{{"nodes":[0]}}],"scene":0}}"#
        )
    }

    #[test]
    fn a_glb_triangle_imports_with_its_material() {
        let material = r#"{"pbrMetallicRoughness":{"baseColorFactor":[0.5,0.25,1.0,1.0],"metallicFactor":0.0,"roughnessFactor":0.4},"doubleSided":true}"#;
        let asset = Asset::from_bytes(&glb(&triangle_json(material, ""), &triangle_bin())).expect("a valid asset");
        assert_eq!(asset.models.len(), 1);
        let model = &asset.models[0];
        assert_eq!(model.mesh.triangle_count(), 1);
        let m = &model.materials[0];
        assert_eq!(m.color, Color { r: encode(0.5), g: encode(0.25), b: 255, a: 255 }, "linear factors become sRGB");
        assert_eq!((m.metallic, m.roughness), (0.0, 0.4));
        assert!(m.back.is_some(), "double-sided draws its back");
        assert_eq!(asset.nodes[0].transform.translation, vec3(1.0, 2.0, 3.0));
        assert_eq!(asset.roots, vec![0]);
    }

    #[test]
    fn extras_arrive_as_raw_json() {
        let extras = r#","extras":{"seat":{"height":0.45},"kind":"chair"}"#;
        let asset = Asset::from_bytes(&glb(&triangle_json("{}", extras), &triangle_bin())).expect("valid");
        assert_eq!(asset.nodes[0].extras.as_deref(), Some(r#"{"seat":{"height":0.45},"kind":"chair"}"#));
        let world = World::new();
        let root = world.spawn(&asset);
        let child = world.with(|data| data.iter().find(|(_, e)| e.parent == Some(root.id())).map(|(id, _)| id)).expect("a child");
        let extras = world.with(|data| data.get(child).and_then(|e| e.spec.extras.clone()));
        assert_eq!(extras.as_deref(), Some(r#"{"seat":{"height":0.45},"kind":"chair"}"#));
    }

    #[test]
    fn one_asset_spawned_twice_shares_meshes() {
        let asset = Asset::from_bytes(&glb(&triangle_json("{}", ""), &triangle_bin())).expect("valid");
        let world = World::new();
        world.spawn(&asset).set_position(vec3(-2.0, 0.0, 0.0));
        world.spawn(&asset).set_position(vec3(2.0, 0.0, 0.0));
        let camera = super::super::camera::Camera::looking_at(vec3(0.0, 1.0, 12.0), vec3(0.0, 1.0, 0.0));
        let frame = world.frame(&camera, &super::super::frame::WorldStyle::default(), (320, 200), 1.0);
        assert_eq!(frame.meshes.len(), 1, "one mesh");
        assert_eq!(frame.draws.len(), 1, "one instanced draw");
        assert_eq!(frame.draws[0].instance_count, 2);
    }

    #[test]
    fn a_skinned_glb_imports_its_skin_and_clip() {
        let mut bin = Vec::new();
        let floats = |bin: &mut Vec<u8>, values: &[f32]| values.iter().for_each(|v| bin.extend_from_slice(&v.to_le_bytes()));
        floats(&mut bin, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]); // 0: positions
        for index in [0u16, 1, 2, 0] {
            bin.extend_from_slice(&index.to_le_bytes()); // 36: indices, padded
        }
        for _ in 0..3 {
            bin.extend_from_slice(&[1, 0, 0, 0]); // 44: every vertex on the skin's second joint
        }
        for _ in 0..3 {
            floats(&mut bin, &[1.0, 0.0, 0.0, 0.0]); // 56: weights
        }
        let identity = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        floats(&mut bin, &identity); // 104: the hips' inverse bind
        floats(&mut bin, &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 1.0]); // the arm's
        floats(&mut bin, &[0.0, 1.0]); // 232: key times
        floats(&mut bin, &[0.0, 1.0, 0.0, 1.0, 1.0, 0.0]); // 240: the arm's place at each
        let json = r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":264}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":6},
              {"buffer":0,"byteOffset":44,"byteLength":12},{"buffer":0,"byteOffset":56,"byteLength":48},
              {"buffer":0,"byteOffset":104,"byteLength":128},{"buffer":0,"byteOffset":232,"byteLength":8},
              {"buffer":0,"byteOffset":240,"byteLength":24}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"},{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"},
              {"bufferView":2,"componentType":5121,"count":3,"type":"VEC4"},{"bufferView":3,"componentType":5126,"count":3,"type":"VEC4"},
              {"bufferView":4,"componentType":5126,"count":2,"type":"MAT4"},{"bufferView":5,"componentType":5126,"count":2,"type":"SCALAR"},
              {"bufferView":6,"componentType":5126,"count":2,"type":"VEC3"}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0,"JOINTS_0":2,"WEIGHTS_0":3},"indices":1}]}],
            "skins":[{"joints":[1,2],"inverseBindMatrices":4}],
            "animations":[{"name":"wave","channels":[{"sampler":0,"target":{"node":2,"path":"translation"}}],
              "samplers":[{"input":5,"output":6,"interpolation":"LINEAR"}]}],
            "nodes":[{"mesh":0,"skin":0},{"name":"hips","children":[2]},{"name":"arm","translation":[0,1,0]}],
            "scenes":[{"nodes":[0,1]}],"scene":0}"#;
        let asset = Asset::from_bytes(&glb(json, &bin)).expect("a valid skinned asset");
        assert_eq!(asset.skins.len(), 1);
        assert_eq!(asset.skins[0].joints(), &[1, 2]);
        let clip = asset.clip("wave").expect("the clip by name").clone();
        assert_eq!(clip.duration(), 1.0);
        let model = &asset.models[asset.nodes[0].model.expect("the mesh node's model")];
        assert!(model.skin.is_some() && model.mesh.is_skinned(), "the node hangs its mesh from the skin");
        // played to its end and held: the arm one meter along x from its bind
        let world = World::new();
        let figure = world.spawn(&asset);
        figure.set_play(Some(super::super::skin::Play { clip, start: motor::task::now() - 2.0, speed: 1.0, looped: false, from: None }));
        let camera = super::super::camera::Camera::looking_at(vec3(0.5, 0.5, 6.0), vec3(0.5, 0.5, 0.0));
        let frame = world.frame(&camera, &super::super::frame::WorldStyle::default(), (64, 64), 1.0);
        assert_eq!(frame.joints.len(), 6, "two joints, three rows each");
        assert_eq!(&frame.joints[3..6], &[[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]);
        assert_eq!(frame.instances[0].joint_base, 0);
        // cubic spline keys are refused by name
        let cubic = json.replace(r#""interpolation":"LINEAR""#, r#""interpolation":"CUBICSPLINE""#);
        assert_eq!(Asset::from_bytes(&glb(&cubic, &bin)).err(), Some(GltfError::Unsupported("a cubic spline animation")));
    }

    #[test]
    fn a_mesh_with_targets_imports_its_morphs() {
        let mut bin = triangle_bin();
        bin.extend_from_slice(&[0, 0]); // the indices padded to four bytes
        for value in [0.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0] {
            bin.extend_from_slice(&value.to_le_bytes()); // 44: the second corner moves toward +z
        }
        let json = r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":80}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":6},{"buffer":0,"byteOffset":44,"byteLength":36}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"},{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"},
              {"bufferView":2,"componentType":5126,"count":3,"type":"VEC3"}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1,"targets":[{"POSITION":2}]}]}],
            "nodes":[{"mesh":0}]}"#;
        let asset = Asset::from_bytes(&glb(json, &bin)).expect("a valid asset");
        let mesh = &asset.models[0].mesh;
        assert_eq!(mesh.morph_count(), 1);
        // the flat faces split the corners; the morph travels with them
        let data = mesh.data();
        let moved = data.positions.iter().zip(&data.morphs[0].positions).find(|(p, _)| **p == [1.0, 0.0, 0.0]).map(|(_, m)| *m);
        assert_eq!(moved, Some([0.0, 0.0, 1.0]));
        let three = json.replace(r#""targets":[{"POSITION":2}]"#, r#""targets":[{"POSITION":2},{"POSITION":2},{"POSITION":2}]"#);
        assert_eq!(Asset::from_bytes(&glb(&three, &bin)).err(), Some(GltfError::Unsupported("more than two morph targets")));
    }

    #[test]
    fn quantized_positions_dequantize_through_the_node() {
        // positions as normalized i16, the node scaling them back to meters
        let mut bin = Vec::new();
        for value in [0i16, 0, 0, 32767, 0, 0, 0, 32767, 0, 0] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
        let json = r#"{"asset":{"version":"2.0"},"extensionsRequired":["KHR_mesh_quantization"],"extensionsUsed":["KHR_mesh_quantization"],
            "buffers":[{"byteLength":20}],
            "bufferViews":[{"buffer":0,"byteLength":18,"byteStride":6}],
            "accessors":[{"bufferView":0,"componentType":5122,"normalized":true,"count":3,"type":"VEC3"}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],
            "nodes":[{"mesh":0,"scale":[4,4,4]}]}"#;
        let asset = Asset::from_bytes(&glb(json, &bin)).expect("valid");
        assert_eq!(asset.models[0].mesh.data().positions[1], [1.0, 0.0, 0.0]);
        assert_eq!(asset.nodes[0].transform.scale, vec3(4.0, 4.0, 4.0));
        assert_eq!(asset.roots, vec![0], "no scene: the node nobody names is a root");
    }

    #[test]
    fn a_text_asset_with_data_uris_loads() {
        let bin = triangle_bin();
        let encoded = {
            const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = String::new();
            for chunk in bin.chunks(3) {
                let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (*b as u32) << (16 - 8 * i));
                for i in 0..4 {
                    if i <= chunk.len() {
                        out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
                    } else {
                        out.push('=');
                    }
                }
            }
            out
        };
        let json = triangle_json("{}", "").replace(r#""buffers":[{"byteLength":42}]"#, &format!(r#""buffers":[{{"byteLength":42,"uri":"data:application/octet-stream;base64,{encoded}"}}]"#));
        let asset = Asset::from_bytes(json.as_bytes()).expect("a text asset");
        assert_eq!(asset.models[0].mesh.data().positions[2], [0.0, 1.0, 0.0]);
    }

    /// Loads the GLB named by `BUNNY_WORLD_GLB`, frames it and draws it;
    /// with `BUNNY_WORLD_DUMP` naming a folder, writes the picture there
    /// as raw RGBA for a person to look at.
    #[test]
    #[ignore = "reads a file named by the environment"]
    fn a_glb_from_the_environment_loads_and_draws() {
        let Ok(path) = std::env::var("BUNNY_WORLD_GLB") else { return };
        let bytes = std::fs::read(&path).expect("the file reads");
        let asset = Asset::from_bytes(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        let world = World::new();
        world.spawn(&asset);
        let size = (256, 256);
        let camera = super::super::camera::Camera::orbiting(Vec3::ZERO, 45.0, 30.0, 10.0)
            .framing(world.bounds(), crate::layout::Size { width: 256.0, height: 256.0 });
        let style = super::super::frame::WorldStyle {
            background: super::super::frame::Background::Color(Color::rgb(236, 238, 242)),
            ..super::super::frame::WorldStyle::default()
        };
        let picture = world.render_with(&crate::image_engine::RawImages::default(), &camera, &style, size, 1.0);
        let triangles: usize = asset.models.iter().map(|m| m.mesh.triangle_count()).sum();
        eprintln!("{path}: {} models, {} nodes, {triangles} triangles", asset.models.len(), asset.nodes.len());
        if let Ok(folder) = std::env::var("BUNNY_WORLD_DUMP") {
            let name = std::path::Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("asset");
            let _ = std::fs::write(std::path::Path::new(&folder).join(format!("{name}-256x256.rgba")), &picture.rgba);
        }
        assert!(
            picture
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| *p != [236, 238, 242, 255]),
            "something drew"
        );
    }

    #[test]
    fn malformed_input_is_an_error_never_a_panic() {
        let good = glb(&triangle_json("{}", ""), &triangle_bin());
        // every truncation of a good file
        for cut in 0..good.len() {
            let _ = Asset::from_bytes(&good[..cut]);
        }
        // seeded garbage, and garbage behind a valid header
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        for round in 0..300 {
            let mut bytes: Vec<u8> = (0..(round % 97 + 1))
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    seed as u8
                })
                .collect();
            if round % 2 == 0 {
                let mut header = good[..20].to_vec();
                header.append(&mut bytes);
                bytes = header;
            }
            let _ = Asset::from_bytes(&bytes);
        }
        assert_eq!(Asset::from_bytes(b"not a model").unwrap_err(), GltfError::NotGltf);
        let external = triangle_json("{}", "").replace(r#""buffers":[{"byteLength":42}]"#, r#""buffers":[{"byteLength":42,"uri":"model.bin"}]"#);
        assert_eq!(Asset::from_bytes(external.as_bytes()).unwrap_err(), GltfError::Unsupported("a buffer in a file outside the asset"));
        let bad_index = glb(&triangle_json("{}", ""), &{
            let mut bin = triangle_bin();
            bin[36] = 9;
            bin
        });
        assert_eq!(Asset::from_bytes(&bad_index).unwrap_err(), GltfError::OutOfRange("an index"));
    }
}
