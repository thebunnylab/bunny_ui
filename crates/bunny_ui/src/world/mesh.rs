//! Meshes: the triangles of a shape, the attributes of each corner,
//! and the lines that draw its edges.
//!
//! A [`Mesh`] is immutable and shared. Its identity is a number and a
//! generation: a tier keeps one GPU copy per identity and uploads again
//! only when the generation moves. Edit a shape with [`Mesh::revise`] —
//! the identity stays, the generation climbs, and every entity that
//! shows the mesh shows the new shape.

use std::rc::Rc;

use super::math::{Aabb, Vec2, Vec3, vec3};
use super::next_id;
use super::polygon::triangulate;

// MARK: - The vertex on the wire

/// Bytes per vertex of a mesh without a skin.
pub const VERTEX_STRIDE: usize = 32;
/// Bytes per vertex of a skinned mesh: the plain vertex, then four
/// joint indices and four weights.
pub const SKINNED_VERTEX_STRIDE: usize = 40;

/// Where each attribute starts inside a vertex. Every tier reads these
/// offsets; a test pins them.
pub mod offset {
    /// `f32 × 3`, model space, meters.
    pub const POSITION: usize = 0;
    /// `i16 × 2`, the unit normal folded onto an octahedron.
    pub const NORMAL: usize = 12;
    /// `f32 × 2`; the first row of a texture is `v = 0`.
    pub const UV: usize = 16;
    /// `u8 × 4`, sRGB-encoded, straight alpha.
    pub const COLOR: usize = 24;
    /// `u8`: the slot of the vertex's material in its model.
    pub const MATERIAL: usize = 28;
    /// `u8`: baked light from the surroundings, 255 = nothing in the way.
    pub const OCCLUSION: usize = 29;
    /// `u16`, little-endian: the part of the mesh the vertex belongs to.
    pub const PART: usize = 30;
    /// `u8 × 4`: the joints that move a skinned vertex.
    pub const JOINTS: usize = 32;
    /// `u8 × 4`, unit-normalized: how much each joint moves it.
    pub const WEIGHTS: usize = 36;
}

/// A unit normal folded onto the octahedron and quantized to two
/// signed 16-bit numbers. The shader and the CPU rasterizer unfold it
/// the same way ([`unfold_normal`]).
pub fn fold_normal(normal: [f32; 3]) -> [i16; 2] {
    let [x, y, z] = normal;
    let sum = x.abs() + y.abs() + z.abs();
    if sum <= 0.0 || !sum.is_finite() {
        return [0, 32767];
    }
    let (mut u, mut v) = (x / sum, y / sum);
    if z < 0.0 {
        let (fu, fv) = (u, v);
        u = (1.0 - fv.abs()) * if fu >= 0.0 { 1.0 } else { -1.0 };
        v = (1.0 - fu.abs()) * if fv >= 0.0 { 1.0 } else { -1.0 };
    }
    let quantize = |value: f32| (value.clamp(-1.0, 1.0) * 32767.0).round() as i16;
    [quantize(u), quantize(v)]
}

/// The inverse of [`fold_normal`], to unit length.
pub fn unfold_normal(folded: [i16; 2]) -> [f32; 3] {
    let u = (folded[0] as f32 / 32767.0).max(-1.0);
    let v = (folded[1] as f32 / 32767.0).max(-1.0);
    let z = 1.0 - u.abs() - v.abs();
    let (mut x, mut y) = (u, v);
    if z < 0.0 {
        x = (1.0 - v.abs()) * if u >= 0.0 { 1.0 } else { -1.0 };
        y = (1.0 - u.abs()) * if v >= 0.0 { 1.0 } else { -1.0 };
    }
    let length = (x * x + y * y + z * z).sqrt();
    [x / length, y / length, z / length]
}

// MARK: - The data

/// The geometry of a mesh, as the app builds it or a file brings it.
///
/// `positions` and `indices` are the shape: each three indices are one
/// triangle, its corners counter-clockwise when the front faces the
/// eye. Every other list is OPTIONAL: an empty list gives each vertex
/// the default, and a list of the wrong length counts as empty.
///
/// Without normals the mesh shows flat faces — each triangle gets its
/// own corners and the normal of its plane.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Default `[0, 0]`.
    pub uvs: Vec<[f32; 2]>,
    /// sRGB, straight alpha. Default white.
    pub colors: Vec<[u8; 4]>,
    /// The slot of each vertex's material in its model. Default 0.
    pub materials: Vec<u8>,
    /// Baked light from the surroundings, 255 = nothing in the way.
    /// Default 255.
    pub occlusion: Vec<u8>,
    /// The part each vertex belongs to — one piece of a merged mesh, one
    /// region of a figure. A part can take its own tint or hide, with no
    /// new upload. Default 0.
    pub parts: Vec<u16>,
    pub indices: Vec<u32>,
}

impl MeshData {
    fn channel<T: Copy>(list: &[T], count: usize, index: usize, default: T) -> T {
        if list.len() == count { list[index] } else { default }
    }

    /// A copy where each triangle owns its three corners and they carry
    /// the normal of its plane. Attributes travel with their corners.
    pub fn with_flat_normals(&self) -> MeshData {
        let count = self.positions.len();
        let mut out = MeshData::default();
        let triangles = self.indices.chunks_exact(3);
        let corners = triangles.len() * 3;
        out.positions.reserve(corners);
        out.normals.reserve(corners);
        for triangle in self.indices.chunks_exact(3) {
            if triangle.iter().any(|&i| i as usize >= count) {
                continue;
            }
            let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|i| self.positions[i as usize]);
            let normal = face_normal(a, b, c);
            for &index in triangle {
                let i = index as usize;
                out.indices.push(out.positions.len() as u32);
                out.positions.push(self.positions[i]);
                out.normals.push(normal);
                out.uvs.push(Self::channel(&self.uvs, count, i, [0.0, 0.0]));
                out.colors.push(Self::channel(&self.colors, count, i, [255; 4]));
                out.materials.push(Self::channel(&self.materials, count, i, 0));
                out.occlusion.push(Self::channel(&self.occlusion, count, i, 255));
                out.parts.push(Self::channel(&self.parts, count, i, 0));
            }
        }
        out
    }
}

fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let (a, b, c) = (to_vec(a), to_vec(b), to_vec(c));
    (b - a).cross(c - a).normalized().to_f32()
}

fn to_vec(p: [f32; 3]) -> Vec3 {
    vec3(p[0] as f64, p[1] as f64, p[2] as f64)
}

// MARK: - The mesh

/// An immutable shape, cheap to clone and to share between entities.
#[derive(Clone)]
pub struct Mesh(Rc<MeshInner>);

struct MeshInner {
    id: u64,
    generation: u64,
    data: MeshData,
    bounds: Aabb,
}

impl std::fmt::Debug for Mesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // never the blob: a scene dump must stay readable
        f.debug_struct("Mesh")
            .field("id", &self.0.id)
            .field("generation", &self.0.generation)
            .field("vertices", &self.0.data.positions.len())
            .field("triangles", &(self.0.data.indices.len() / 3))
            .finish()
    }
}

impl PartialEq for Mesh {
    /// Two meshes are equal when they are the same revision of the same
    /// shape — compared by identity, never by content.
    fn eq(&self, other: &Mesh) -> bool {
        self.0.id == other.0.id && self.0.generation == other.0.generation
    }
}

impl Mesh {
    /// A mesh of new identity. A triangle that names a vertex the data
    /// does not hold is dropped; data without normals shows flat faces.
    pub fn new(data: MeshData) -> Mesh {
        Mesh::with_identity(next_id(), 0, data)
    }

    fn with_identity(id: u64, generation: u64, data: MeshData) -> Mesh {
        let data = sanitize(data);
        let bounds = Aabb::from_points(data.positions.iter().map(|&p| to_vec(p)));
        Mesh(Rc::new(MeshInner { id, generation, data, bounds }))
    }

    /// The same mesh with a new shape: the identity stays, the
    /// generation climbs. Entities that show this mesh pick up the new
    /// shape with their next frame.
    pub fn revise(&self, data: MeshData) -> Mesh {
        Mesh::with_identity(self.0.id, self.0.generation + 1, data)
    }

    pub fn id(&self) -> u64 {
        self.0.id
    }

    pub fn generation(&self) -> u64 {
        self.0.generation
    }

    pub fn data(&self) -> &MeshData {
        &self.0.data
    }

    /// The box around every vertex, in model space.
    pub fn bounds(&self) -> Aabb {
        self.0.bounds
    }

    pub fn vertex_count(&self) -> usize {
        self.0.data.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.0.data.indices.len() / 3
    }

    /// The vertices in the layout every tier reads ([`VERTEX_STRIDE`]
    /// bytes each, little-endian, offsets in [`offset`]).
    pub fn vertex_bytes(&self) -> Vec<u8> {
        let data = &self.0.data;
        let count = data.positions.len();
        let mut bytes = Vec::with_capacity(count * VERTEX_STRIDE);
        for i in 0..count {
            let p = data.positions[i];
            for value in p {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            for value in fold_normal(data.normals[i]) {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            for value in MeshData::channel(&data.uvs, count, i, [0.0, 0.0]) {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&MeshData::channel(&data.colors, count, i, [255; 4]));
            bytes.push(MeshData::channel(&data.materials, count, i, 0));
            bytes.push(MeshData::channel(&data.occlusion, count, i, 255));
            bytes.extend_from_slice(&MeshData::channel(&data.parts, count, i, 0).to_le_bytes());
        }
        bytes
    }

    /// The triangles as little-endian `u32` indices.
    pub fn index_bytes(&self) -> Vec<u8> {
        self.0.data.indices.iter().flat_map(|i| i.to_le_bytes()).collect()
    }

    /// The edges a drawing shows: an edge with one face (the rim of an
    /// open surface), an edge with three or more faces, and an edge
    /// where the two faces meet at more than `crease_degrees`. Corners at
    /// the same place count as one, so a cube of 24 vertices still has
    /// 12 edges.
    pub fn edges(&self, crease_degrees: f64) -> Lines {
        let data = &self.0.data;
        let cos_limit = crease_degrees.to_radians().cos();
        // weld by exact position: one id per place
        let mut place_of = std::collections::HashMap::<[u32; 3], u32>::new();
        let welded: Vec<u32> = data
            .positions
            .iter()
            .map(|p| {
                let key = p.map(f32::to_bits);
                let next = place_of.len() as u32;
                *place_of.entry(key).or_insert(next)
            })
            .collect();
        let mut places = vec![[0.0f32; 3]; place_of.len()];
        for (index, &place) in welded.iter().enumerate() {
            places[place as usize] = data.positions[index];
        }
        let mut faces_at = std::collections::HashMap::<(u32, u32), Vec<Vec3>>::new();
        for triangle in data.indices.chunks_exact(3) {
            let corners = [triangle[0], triangle[1], triangle[2]].map(|i| welded[i as usize]);
            if corners[0] == corners[1] || corners[1] == corners[2] || corners[0] == corners[2] {
                continue;
            }
            let normal = {
                let [a, b, c] = corners.map(|place| to_vec(places[place as usize]));
                (b - a).cross(c - a).normalized()
            };
            for (a, b) in [(corners[0], corners[1]), (corners[1], corners[2]), (corners[2], corners[0])] {
                faces_at.entry((a.min(b), a.max(b))).or_default().push(normal);
            }
        }
        let mut keys: Vec<(u32, u32)> = faces_at
            .iter()
            .filter(|(_, normals)| match normals.as_slice() {
                [_, ] => true,
                [a, b] => a.dot(*b) < cos_limit,
                _ => true,
            })
            .map(|(key, _)| *key)
            .collect();
        // a stable order: the same mesh always gives the same lines
        keys.sort_unstable();
        Lines::new(keys.into_iter().map(|(a, b)| [places[a as usize], places[b as usize]]).collect())
    }
}

/// Drops triangles that name a missing vertex and folds missing normals
/// into flat faces.
fn sanitize(mut data: MeshData) -> MeshData {
    let count = data.positions.len();
    if !data.indices.len().is_multiple_of(3) {
        data.indices.truncate(data.indices.len() / 3 * 3);
    }
    if data.indices.iter().any(|&i| i as usize >= count) {
        let kept: Vec<u32> = data
            .indices
            .chunks_exact(3)
            .filter(|t| t.iter().all(|&i| (i as usize) < count))
            .flatten()
            .copied()
            .collect();
        data.indices = kept;
    }
    if data.normals.len() != count {
        data = data.with_flat_normals();
    }
    data
}

// MARK: - Shapes

/// One flat four-cornered face: `center`, half extents along `u` and
/// `v` (with `u × v` facing out), counter-clockwise seen from the front.
fn quad(data: &mut MeshData, center: Vec3, u: Vec3, v: Vec3) {
    let normal = u.cross(v).normalized().to_f32();
    let base = data.positions.len() as u32;
    let corners = [(center - u - v, [0.0, 1.0]), (center + u - v, [1.0, 1.0]), (center + u + v, [1.0, 0.0]), (center - u + v, [0.0, 0.0])];
    for (corner, uv) in corners {
        data.positions.push(corner.to_f32());
        data.normals.push(normal);
        data.uvs.push(uv);
    }
    data.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

impl Mesh {
    /// A box `width × height × depth` meters, centered on the origin,
    /// each face flat with its own corners.
    pub fn cuboid(width: f64, height: f64, depth: f64) -> Mesh {
        let (x, y, z) = (width * 0.5, height * 0.5, depth * 0.5);
        let mut data = MeshData::default();
        quad(&mut data, vec3(x, 0.0, 0.0), vec3(0.0, 0.0, -z), vec3(0.0, y, 0.0));
        quad(&mut data, vec3(-x, 0.0, 0.0), vec3(0.0, 0.0, z), vec3(0.0, y, 0.0));
        quad(&mut data, vec3(0.0, y, 0.0), vec3(x, 0.0, 0.0), vec3(0.0, 0.0, -z));
        quad(&mut data, vec3(0.0, -y, 0.0), vec3(x, 0.0, 0.0), vec3(0.0, 0.0, z));
        quad(&mut data, vec3(0.0, 0.0, z), vec3(x, 0.0, 0.0), vec3(0.0, y, 0.0));
        quad(&mut data, vec3(0.0, 0.0, -z), vec3(-x, 0.0, 0.0), vec3(0.0, y, 0.0));
        Mesh::new(data)
    }

    /// A flat rectangle `width × depth` meters on the ground plane,
    /// centered on the origin, facing up.
    pub fn plane(width: f64, depth: f64) -> Mesh {
        let mut data = MeshData::default();
        quad(&mut data, Vec3::ZERO, vec3(width * 0.5, 0.0, 0.0), vec3(0.0, 0.0, -depth * 0.5));
        Mesh::new(data)
    }

    /// A smooth ball of `radius` meters, centered on the origin.
    pub fn sphere(radius: f64) -> Mesh {
        Mesh::sphere_with(radius, 32, 16)
    }

    /// A ball with `segments` around and `rings` from pole to pole.
    pub fn sphere_with(radius: f64, segments: u32, rings: u32) -> Mesh {
        let (segments, rings) = (segments.max(3), rings.max(2));
        let mut data = MeshData::default();
        for ring in 0..=rings {
            let theta = std::f64::consts::PI * ring as f64 / rings as f64;
            for segment in 0..=segments {
                // the last column and the poles land EXACTLY where they
                // close: sin 2π and sin π are not zero in floating point,
                // and a seam that misses by a hair is an open seam
                let phi = std::f64::consts::TAU * (segment % segments) as f64 / segments as f64;
                let direction = match ring {
                    0 => Vec3::Y,
                    r if r == rings => -Vec3::Y,
                    _ => vec3(theta.sin() * phi.sin(), theta.cos(), theta.sin() * phi.cos()),
                };
                data.positions.push((direction * radius).to_f32());
                data.normals.push(direction.to_f32());
                data.uvs.push([segment as f32 / segments as f32, ring as f32 / rings as f32]);
            }
        }
        let row = segments + 1;
        for ring in 0..rings {
            for segment in 0..segments {
                let a = ring * row + segment;
                let (b, c, d) = (a + 1, a + row + 1, a + row);
                // at each pole one triangle of the pair has no area: its two
                // pole corners stand at the same place
                if ring != rings - 1 {
                    data.indices.extend_from_slice(&[a, d, c]);
                }
                if ring != 0 {
                    data.indices.extend_from_slice(&[a, c, b]);
                }
            }
        }
        Mesh::new(data)
    }

    /// An upright cylinder of `radius` and `height` meters, centered on
    /// the origin: a smooth side and two flat caps.
    pub fn cylinder(radius: f64, height: f64) -> Mesh {
        Mesh::cylinder_with(radius, height, 32)
    }

    /// A cylinder with `segments` around.
    pub fn cylinder_with(radius: f64, height: f64, segments: u32) -> Mesh {
        let segments = segments.max(3);
        let half = height * 0.5;
        let mut data = MeshData::default();
        let around = |segment: u32| {
            // the last column closes exactly on the first
            let phi = std::f64::consts::TAU * (segment % segments) as f64 / segments as f64;
            vec3(phi.sin(), 0.0, phi.cos())
        };
        // the side: two rows of smooth corners
        for segment in 0..=segments {
            let direction = around(segment);
            for y in [half, -half] {
                data.positions.push((direction * radius + vec3(0.0, y, 0.0)).to_f32());
                data.normals.push(direction.to_f32());
                data.uvs.push([segment as f32 / segments as f32, if y > 0.0 { 0.0 } else { 1.0 }]);
            }
        }
        for segment in 0..segments {
            let (a, d) = (segment * 2, segment * 2 + 1);
            let (b, c) = (a + 2, d + 2);
            data.indices.extend_from_slice(&[a, d, c, a, c, b]);
        }
        // the caps: a fan around each center, flat
        for (y, up) in [(half, true), (-half, false)] {
            let normal = if up { [0.0, 1.0, 0.0] } else { [0.0, -1.0, 0.0] };
            let center = data.positions.len() as u32;
            data.positions.push([0.0, y as f32, 0.0]);
            data.normals.push(normal);
            data.uvs.push([0.5, 0.5]);
            for segment in 0..segments {
                let direction = around(segment);
                data.positions.push((direction * radius + vec3(0.0, y, 0.0)).to_f32());
                data.normals.push(normal);
                data.uvs.push([0.5 + 0.5 * direction.x as f32, 0.5 + 0.5 * direction.z as f32]);
            }
            for segment in 0..segments {
                let (p, q) = (center + 1 + segment, center + 1 + (segment + 1) % segments);
                if up {
                    data.indices.extend_from_slice(&[center, p, q]);
                } else {
                    data.indices.extend_from_slice(&[center, q, p]);
                }
            }
        }
        Mesh::new(data)
    }

    /// A prism: the polygon `outline` (with `holes`) on the ground plane,
    /// raised `height` meters. Points are `(x, z)` in meters; the turn of
    /// the rings does not matter. The walls are flat, one face per edge.
    pub fn extrude(outline: &[Vec2], holes: &[&[Vec2]], height: f64) -> Mesh {
        let mut points: Vec<Vec2> = outline.to_vec();
        for hole in holes {
            points.extend_from_slice(hole);
        }
        let triangles = triangulate(outline, holes);
        let mut data = MeshData::default();
        let at = |p: Vec2, y: f64| [p.x as f32, y as f32, p.y as f32];
        // the caps: counter-clockwise in (x, z) faces DOWN, because +z
        // runs toward the eye of someone looking down at the ground
        for (y, normal, flip) in [(height, [0.0, 1.0, 0.0], true), (0.0, [0.0, -1.0, 0.0], false)] {
            let base = data.positions.len() as u32;
            for &p in &points {
                data.positions.push(at(p, y));
                data.normals.push(normal);
                data.uvs.push([p.x as f32, p.y as f32]);
            }
            for t in &triangles {
                let [a, b, c] = t.map(|i| base + i);
                data.indices.extend_from_slice(&if flip { [a, c, b] } else { [a, b, c] });
            }
        }
        // the walls: each ring walked so that the solid stays on one side
        let mut rings: Vec<(Vec<Vec2>, bool)> = vec![(outline.to_vec(), true)];
        rings.extend(holes.iter().map(|hole| (hole.to_vec(), false)));
        for (ring, is_outline) in rings {
            if ring.len() < 3 {
                continue;
            }
            let area = super::polygon::signed_area(&ring);
            // the outline counter-clockwise, a hole clockwise, in (x, z)
            let reverse = (is_outline && area < 0.0) || (!is_outline && area > 0.0);
            let ring: Vec<Vec2> = if reverse { ring.into_iter().rev().collect() } else { ring };
            for i in 0..ring.len() {
                let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
                if p == q {
                    continue;
                }
                // walking p → q counter-clockwise in (x, z), the solid lies
                // on the left of the walk and the outside on its right
                let normal = vec3(q.y - p.y, 0.0, p.x - q.x).normalized().to_f32();
                let base = data.positions.len() as u32;
                // seen from outside, q stands on the left and p on the right;
                // the corners are the caps' own points, so the seams close
                let corners = [(q, 0.0, [0.0, 1.0]), (p, 0.0, [1.0, 1.0]), (p, height, [1.0, 0.0]), (q, height, [0.0, 0.0])];
                for (point, y, uv) in corners {
                    data.positions.push(at(point, y));
                    data.normals.push(normal);
                    data.uvs.push(uv);
                }
                data.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
        Mesh::new(data)
    }
}

// MARK: - Lines

/// Segments in model space — the edges of a shape, a grid, an axis.
/// Shared and immutable like a [`Mesh`], with the same identity rule.
#[derive(Clone)]
pub struct Lines(Rc<LinesInner>);

struct LinesInner {
    id: u64,
    generation: u64,
    segments: Vec<[[f32; 3]; 2]>,
    bounds: Aabb,
}

impl std::fmt::Debug for Lines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lines")
            .field("id", &self.0.id)
            .field("generation", &self.0.generation)
            .field("segments", &self.0.segments.len())
            .finish()
    }
}

impl PartialEq for Lines {
    fn eq(&self, other: &Lines) -> bool {
        self.0.id == other.0.id && self.0.generation == other.0.generation
    }
}

impl Lines {
    pub fn new(segments: Vec<[[f32; 3]; 2]>) -> Lines {
        Lines::with_identity(next_id(), 0, segments)
    }

    fn with_identity(id: u64, generation: u64, segments: Vec<[[f32; 3]; 2]>) -> Lines {
        let segments: Vec<[[f32; 3]; 2]> =
            segments.into_iter().filter(|s| s.iter().flatten().all(|v| v.is_finite())).collect();
        let bounds = Aabb::from_points(segments.iter().flatten().map(|&p| to_vec(p)));
        Lines(Rc::new(LinesInner { id, generation, segments, bounds }))
    }

    /// A path through `points`, closed back to the first when `closed`.
    pub fn path(points: &[Vec3], closed: bool) -> Lines {
        let mut segments: Vec<[[f32; 3]; 2]> =
            points.windows(2).map(|pair| [pair[0].to_f32(), pair[1].to_f32()]).collect();
        if closed && points.len() > 2 {
            segments.push([points[points.len() - 1].to_f32(), points[0].to_f32()]);
        }
        Lines::new(segments)
    }

    /// The same identity with new segments, one generation later.
    pub fn revise(&self, segments: Vec<[[f32; 3]; 2]>) -> Lines {
        Lines::with_identity(self.0.id, self.0.generation + 1, segments)
    }

    pub fn id(&self) -> u64 {
        self.0.id
    }

    pub fn generation(&self) -> u64 {
        self.0.generation
    }

    pub fn segments(&self) -> &[[[f32; 3]; 2]] {
        &self.0.segments
    }

    pub fn bounds(&self) -> Aabb {
        self.0.bounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::math::vec2;

    /// The volume a closed mesh holds, by the divergence theorem:
    /// positive when every face turns counter-clockwise seen from
    /// outside.
    fn signed_volume(mesh: &Mesh) -> f64 {
        let data = mesh.data();
        data.indices
            .chunks_exact(3)
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| to_vec(data.positions[i as usize]));
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    /// Every edge, welded by place, is shared by exactly two triangles
    /// that walk it in opposite directions.
    fn is_closed(mesh: &Mesh) -> bool {
        let data = mesh.data();
        let key = |i: u32| data.positions[i as usize].map(f32::to_bits);
        let mut directed = std::collections::HashMap::<([u32; 3], [u32; 3]), i32>::new();
        for t in data.indices.chunks_exact(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *directed.entry((key(a), key(b))).or_default() += 1;
            }
        }
        directed.iter().all(|(&(a, b), &count)| count == 1 && directed.get(&(b, a)) == Some(&1))
    }

    #[test]
    fn a_cuboid_is_closed_and_faces_outward() {
        let mesh = Mesh::cuboid(2.0, 3.0, 4.0);
        assert_eq!(mesh.vertex_count(), 24);
        assert_eq!(mesh.triangle_count(), 12);
        assert!(is_closed(&mesh));
        assert!((signed_volume(&mesh) - 24.0).abs() < 1e-6, "{}", signed_volume(&mesh));
        let bounds = mesh.bounds();
        assert_eq!((bounds.min, bounds.max), (vec3(-1.0, -1.5, -2.0), vec3(1.0, 1.5, 2.0)));
        // each stored normal points the way its face turns
        let data = mesh.data();
        for t in data.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| data.positions[i as usize]);
            let geometric = to_vec(face_normal(a, b, c));
            assert!((geometric - to_vec(data.normals[t[0] as usize])).length() < 1e-6);
        }
    }

    #[test]
    fn a_sphere_has_unit_normals() {
        let mesh = Mesh::sphere(2.0);
        assert!(mesh.data().normals.iter().all(|n| (to_vec(*n).length() - 1.0).abs() < 1e-6));
        let volume = signed_volume(&mesh);
        let exact = 4.0 / 3.0 * std::f64::consts::PI * 8.0;
        assert!(volume > 0.97 * exact && volume < exact, "faces out, close to the ball: {volume} vs {exact}");
        // and the triangles turn the way the normals point
        let data = mesh.data();
        for t in data.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| data.positions[i as usize]);
            let center = (to_vec(a) + to_vec(b) + to_vec(c)) / 3.0;
            assert!(to_vec(face_normal(a, b, c)).dot(center) > 0.0);
        }
    }

    #[test]
    fn a_cylinder_is_closed_and_faces_outward() {
        let mesh = Mesh::cylinder_with(1.0, 2.0, 24);
        let volume = signed_volume(&mesh);
        let exact = std::f64::consts::PI * 2.0;
        assert!(volume > 0.97 * exact && volume < exact, "{volume} vs {exact}");
    }

    #[test]
    fn an_extruded_l_has_its_volume() {
        let l = [vec2(0.0, 0.0), vec2(4.0, 0.0), vec2(4.0, 1.0), vec2(1.0, 1.0), vec2(1.0, 3.0), vec2(0.0, 3.0)];
        for outline in [l.to_vec(), l.iter().rev().copied().collect()] {
            let mesh = Mesh::extrude(&outline, &[], 2.5);
            assert!(is_closed(&mesh), "the walls meet the caps");
            assert!((signed_volume(&mesh) - 6.0 * 2.5).abs() < 1e-5, "{}", signed_volume(&mesh));
        }
        let ring = [vec2(0.0, 0.0), vec2(5.0, 0.0), vec2(5.0, 5.0), vec2(0.0, 5.0)];
        let hole = [vec2(2.0, 2.0), vec2(3.0, 2.0), vec2(3.0, 3.0), vec2(2.0, 3.0)];
        let mesh = Mesh::extrude(&ring, &[&hole], 1.0);
        assert!(is_closed(&mesh), "the hole's walls close it too");
        assert!((signed_volume(&mesh) - 24.0).abs() < 1e-5, "{}", signed_volume(&mesh));
    }

    #[test]
    fn a_revision_keeps_the_id_and_moves_the_generation() {
        let first = Mesh::cuboid(1.0, 1.0, 1.0);
        let second = first.revise(Mesh::cuboid(2.0, 1.0, 1.0).data().clone());
        assert_eq!(first.id(), second.id());
        assert_eq!(second.generation(), first.generation() + 1);
        assert_ne!(first, second, "a new revision is a new picture");
        assert_eq!(first, first.clone());
        assert_ne!(Mesh::cuboid(1.0, 1.0, 1.0).id(), first.id(), "a new mesh is a new identity");
    }

    #[test]
    fn missing_normals_become_flat_faces_and_bad_triangles_go() {
        let data = MeshData {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            colors: vec![[10, 20, 30, 255]; 4],
            indices: vec![0, 1, 2, 1, 3, 2, 0, 1, 99],
            ..MeshData::default()
        };
        let mesh = Mesh::new(data);
        assert_eq!(mesh.triangle_count(), 2, "the triangle naming vertex 99 is dropped");
        assert_eq!(mesh.vertex_count(), 6, "each flat triangle owns its corners");
        assert!(mesh.data().normals.iter().all(|n| *n == [0.0, 0.0, 1.0]));
        assert!(mesh.data().colors.iter().all(|c| *c == [10, 20, 30, 255]), "colors travel with corners");
    }

    #[test]
    fn vertex_bytes_follow_the_wire_layout() {
        let data = MeshData {
            positions: vec![[1.5, -2.0, 3.25]],
            normals: vec![[0.0, 0.0, -1.0]],
            uvs: vec![[0.25, 0.75]],
            colors: vec![[1, 2, 3, 4]],
            materials: vec![7],
            occlusion: vec![200],
            parts: vec![513],
            indices: vec![],
        };
        let bytes = Mesh::new(data).vertex_bytes();
        assert_eq!(bytes.len(), VERTEX_STRIDE);
        let f32_at = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let i16_at = |at: usize| i16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
        assert_eq!([f32_at(offset::POSITION), f32_at(offset::POSITION + 4), f32_at(offset::POSITION + 8)], [1.5, -2.0, 3.25]);
        let folded = [i16_at(offset::NORMAL), i16_at(offset::NORMAL + 2)];
        assert_eq!(unfold_normal(folded), [0.0, 0.0, -1.0]);
        assert_eq!([f32_at(offset::UV), f32_at(offset::UV + 4)], [0.25, 0.75]);
        assert_eq!(&bytes[offset::COLOR..offset::COLOR + 4], &[1, 2, 3, 4]);
        assert_eq!(bytes[offset::MATERIAL], 7);
        assert_eq!(bytes[offset::OCCLUSION], 200);
        assert_eq!(u16::from_le_bytes([bytes[offset::PART], bytes[offset::PART + 1]]), 513);
    }

    #[test]
    fn a_folded_normal_unfolds_to_itself() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        for _ in 0..2000 {
            let n = vec3(next(), next(), next()).normalized();
            if n == Vec3::ZERO {
                continue;
            }
            let back = to_vec(unfold_normal(fold_normal(n.to_f32())));
            assert!(back.dot(n) > 0.999_99, "{n:?} → {back:?}");
        }
        for axis in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
            let back = to_vec(unfold_normal(fold_normal(axis.to_f32())));
            assert!((back - axis).length() < 1e-6, "{axis:?} → {back:?}");
        }
    }

    #[test]
    fn a_cube_has_twelve_feature_edges() {
        assert_eq!(Mesh::cuboid(1.0, 2.0, 3.0).edges(30.0).segments().len(), 12);
        assert_eq!(Mesh::plane(2.0, 2.0).edges(30.0).segments().len(), 4, "the rim of an open sheet");
        assert_eq!(Mesh::sphere(1.0).edges(30.0).segments().len(), 0, "a smooth ball has no crease");
        let cylinder = Mesh::cylinder_with(1.0, 1.0, 16).edges(30.0);
        assert_eq!(cylinder.segments().len(), 32, "two rims of sixteen edges");
        // the same mesh gives the same lines, in the same order
        let mesh = Mesh::cuboid(1.0, 1.0, 1.0);
        assert_eq!(mesh.edges(30.0).segments(), mesh.edges(30.0).segments());
    }

    #[test]
    fn a_path_closes_when_asked() {
        let square = [Vec3::ZERO, Vec3::X, vec3(1.0, 0.0, 1.0), Vec3::Z];
        assert_eq!(Lines::path(&square, false).segments().len(), 3);
        assert_eq!(Lines::path(&square, true).segments().len(), 4);
        let lines = Lines::path(&square, true);
        assert_eq!(lines.bounds(), Aabb::new(Vec3::ZERO, vec3(1.0, 0.0, 1.0)));
    }
}
