//! The software rasterizer: a world frame drawn on the CPU.
//!
//! It is the oracle every GPU tier is held against, and the floor where
//! no tier exists. It runs the frame's draws in order, with the rules a
//! GPU follows: triangles clipped against the near and far planes in
//! homogeneous space, corners snapped to 1/256 of a pixel, integer edge
//! functions with the top-left rule, depth linear on the screen,
//! attributes corrected for perspective, colour blended as an 8-bit
//! target blends it. Lines are quads of analytic coverage, as their
//! shader draws them. The sun's shadow map is drawn first, by the same
//! rules, and read the way a comparing sampler reads it
//! (`super::shadow`).
//!
//! The picture is what a tier's target holds: premultiplied, sRGB-encoded
//! RGBA8, the top row first. The same frame gives the same bytes on every
//! machine.

use std::rc::Rc;

use super::frame::{Background, Blend, DrawKind, WorldFrame};
use super::mesh::{fold_normal, unfold_normal};
use super::shade::{self, Surface, V3, decode, encode_code, quantize};
use super::shadow::{self as sun_shadow, ShadowMap};
use super::texture::{Texture, log2};
use super::wire::{FrameUniforms, InstanceData, NO_JOINTS, NO_PARTS, PartRow, Row, instance_flag, material_flag};

/// A picture of a world: premultiplied, sRGB-encoded RGBA8, top row
/// first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Picture {
    /// The pixel at `(x, y)`.
    pub fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    /// The same picture with straight alpha — what the 2D compositor
    /// blends. A clear pixel becomes black and clear.
    pub fn unpremultiplied(&self) -> Vec<u8> {
        let mut out = self.rgba.clone();
        for pixel in out.as_chunks_mut::<4>().0 {
            let alpha = pixel[3] as u32;
            if alpha == 0 {
                pixel[..3].fill(0);
            } else if alpha < 255 {
                for channel in &mut pixel[..3] {
                    *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
        }
        out
    }
}

/// Draws the frame, with no texture: a textured surface shows its colour.
pub fn draw(frame: &WorldFrame) -> Picture {
    draw_with(frame, &|_| None)
}

/// Draws the frame; `textures` gives the texture of each of the frame's
/// textures by index, `None` while it is not decoded.
pub fn draw_with(frame: &WorldFrame, textures: &dyn Fn(usize) -> Option<Rc<Texture>>) -> Picture {
    let (width, height) = (frame.size.0.max(1), frame.size.1.max(1));
    let mut target = Target {
        width,
        height,
        color: vec![[0; 4]; (width * height) as usize],
        depth: vec![1.0; (width * height) as usize],
        glow: if frame.bloom > 0 { vec![0; (width * height) as usize] } else { Vec::new() },
    };
    target.clear(frame);
    let shadow = sun_shadow::map_of(frame);
    let floor = frame.floor.and_then(|index| textures(index as usize));
    let projection = frame.uniforms.clip_from_eye;
    for draw in &frame.draws {
        match draw.kind {
            DrawKind::Mesh { mesh, first_index, index_count, texture, cull_back, mirrored, blend, mask } => {
                let texture = texture.and_then(|index| textures(index as usize));
                let mesh = &frame.meshes[mesh as usize];
                let corners = Corners::of(mesh);
                let indices = &mesh.data().indices[first_index as usize..(first_index + index_count) as usize];
                // an opaque surface steps back under the frame's edges
                let bias = if blend == Blend::Replace { frame.face_bias } else { 0.0 };
                for instance in &frame.instances[draw.first_instance as usize..(draw.first_instance + draw.instance_count) as usize] {
                    let job = Job {
                        frame,
                        instance,
                        cull_back,
                        mirrored,
                        blend,
                        mask,
                        bias,
                        texture: texture.as_deref(),
                        shadow: shadow.as_deref(),
                        floor: floor.as_deref(),
                    };
                    for triangle in indices.as_chunks::<3>().0 {
                        // a part that hides: its corners leave the picture
                        if part_hidden(frame, instance.part_base, mesh.data(), triangle[0] as usize) {
                            continue;
                        }
                        let vertices = [0, 1, 2].map(|k| corners.vertex(triangle[k] as usize, instance, &projection, &frame.joints));
                        job.triangle(&mut target, vertices);
                    }
                }
            }
            DrawKind::Lines { lines, style } => {
                let instance = &frame.instances[draw.first_instance as usize];
                let style = &frame.line_styles[style as usize];
                for segment in frame.lines[lines as usize].segments() {
                    line(&mut target, frame, instance, &projection, style, segment);
                }
            }
        }
    }
    if frame.bloom > 0 {
        super::bloom::apply(&mut target.color, &target.glow, (width, height), frame.bloom as u32);
    }
    Picture { width, height, rgba: target.color.into_iter().flatten().collect() }
}

impl super::store::World {
    /// The picture of the world through `camera`, drawn on the CPU: a
    /// thumbnail, an export, a test. `px_per_point` scales line widths.
    pub fn render(&self, camera: &super::camera::Camera, style: &super::frame::WorldStyle, size: (u32, u32), px_per_point: f32) -> Picture {
        let mut style = style.clone();
        // the CPU draws one sample a pixel
        if style.quality != super::frame::Quality::Low {
            style.quality = super::frame::Quality::Low;
        }
        draw(&self.frame(camera, &style, size, px_per_point))
    }

    /// [`World::render`] with textures, decoded by `images`.
    pub fn render_with(
        &self,
        images: &dyn crate::image_engine::ImageEngine,
        camera: &super::camera::Camera,
        style: &super::frame::WorldStyle,
        size: (u32, u32),
        px_per_point: f32,
    ) -> Picture {
        let style = super::frame::WorldStyle { quality: super::frame::Quality::Low, ..style.clone() };
        let frame = self.frame(camera, &style, size, px_per_point);
        draw_with(&frame, &|index| super::texture::texture_of(images, &frame.textures[index]))
    }
}

// MARK: - The target

struct Target {
    width: u32,
    height: u32,
    /// Premultiplied, sRGB-encoded.
    color: Vec<[u8; 4]>,
    depth: Vec<f32>,
    /// How much each pixel glows, for a frame with a bloom; empty without.
    glow: Vec<u8>,
}

impl Target {
    fn clear(&mut self, frame: &WorldFrame) {
        match frame.background {
            Background::Transparent => {}
            Background::Color(c) => {
                let pixel = premultiply([c.r, c.g, c.b], c.a);
                self.color.fill(pixel);
            }
            Background::Gradient { top, bottom } => {
                for y in 0..self.height {
                    let t = (y as f32 + 0.5) / self.height as f32;
                    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t + 0.5).floor().clamp(0.0, 255.0) as u8;
                    let alpha = lerp(top.a, bottom.a);
                    let pixel = premultiply([lerp(top.r, bottom.r), lerp(top.g, bottom.g), lerp(top.b, bottom.b)], alpha);
                    let row = (y * self.width) as usize;
                    self.color[row..row + self.width as usize].fill(pixel);
                }
            }
            Background::Sky { .. } => {
                let Some(ramp) = frame.backdrop() else { return };
                for y in 0..self.height {
                    for x in 0..self.width {
                        let at = (y * self.width + x) as usize;
                        self.color[at] = sky(&ramp, x as f32 + 0.5, y as f32 + 0.5);
                    }
                }
            }
        }
    }

    /// Blends a premultiplied colour into the pixel, the way an 8-bit
    /// target does: in floating point, rounded to the nearest code. The
    /// glow, premultiplied in codes too, blends as the colour does.
    fn blend(&mut self, index: usize, source: [f32; 4], glow: f32, blend: Blend) {
        if let Some(below) = self.glow.get(index).map(|&g| g as f32) {
            let out = match blend {
                Blend::Replace => glow,
                Blend::Alpha => glow + below * (1.0 - source[3] / 255.0),
                Blend::Add => glow + below,
            };
            self.glow[index] = (out + 0.5).floor().clamp(0.0, 255.0) as u8;
        }
        let destination = self.color[index];
        let out = match blend {
            Blend::Replace => source,
            Blend::Alpha => {
                let keep = 1.0 - source[3] / 255.0;
                [0, 1, 2, 3].map(|k| source[k] + destination[k] as f32 * keep)
            }
            Blend::Add => [source[0] + destination[0] as f32, source[1] + destination[1] as f32, source[2] + destination[2] as f32, destination[3] as f32 + source[3] * (1.0 - destination[3] as f32 / 255.0)],
        };
        self.color[index] = out.map(|value| (value + 0.5).floor().clamp(0.0, 255.0) as u8);
    }
}

/// The sky's colour at a pixel centre `(x, y)` (y down): the view's ray
/// through it, against the world's up — the shaders' backdrop step for
/// step.
fn sky(ramp: &[[f32; 4]; 6], x: f32, y: f32) -> [u8; 4] {
    let nx = x / ramp[2][0] * 2.0 - 1.0;
    let ny = 1.0 - y / ramp[2][1] * 2.0;
    let ray = if ramp[5][2] > 0.5 { [0.0, 0.0, -1.0] } else { shade::normalize([nx * ramp[5][0], ny * ramp[5][1], -1.0]) };
    let e = shade::dot(ray, [ramp[4][0], ramp[4][1], ramp[4][2]]);
    let (from, to, t) = if e >= 0.0 { (ramp[1], ramp[0], e * (2.0 - e)) } else { (ramp[1], ramp[3], (-e * 8.0).min(1.0)) };
    let c = [0, 1, 2, 3].map(|k| (from[k] + (to[k] - from[k]) * t + 0.5).floor().clamp(0.0, 255.0));
    let rgb = [0, 1, 2].map(|k| ((c[k] * c[3] + 127.0) / 255.0).floor() as u8);
    [rgb[0], rgb[1], rgb[2], c[3] as u8]
}

fn premultiply(rgb: [u8; 3], alpha: u8) -> [u8; 4] {
    let scale = |c: u8| ((c as u32 * alpha as u32 + 127) / 255) as u8;
    [scale(rgb[0]), scale(rgb[1]), scale(rgb[2]), alpha]
}

// MARK: - Vertices

/// The corners of a mesh as a shader reads them: the normal through the
/// same fold the wire uses, the colour decoded.
struct Corners<'a> {
    data: &'a super::mesh::MeshData,
    normals: Vec<[f32; 3]>,
}

impl<'a> Corners<'a> {
    fn of(mesh: &'a super::mesh::Mesh) -> Corners<'a> {
        let data = mesh.data();
        Corners { data, normals: data.normals.iter().map(|&n| unfold_normal(fold_normal(n))).collect() }
    }

    fn vertex(&self, index: usize, instance: &InstanceData, projection: &[f32; 16], palette: &[Row]) -> Vertex {
        let data = self.data;
        let count = data.positions.len();
        let mut p = data.positions[index];
        let mut n = self.normals[index];
        // a skinned vertex moves to its pose before it is placed
        if instance.joint_base != NO_JOINTS && data.joints.len() == count && data.weights.len() == count {
            (p, n) = super::skin::skin_vertex(palette, instance.joint_base, data.joints[index], data.weights[index], p, n);
        }
        let row = |r: [f32; 4]| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3];
        let eye = [row(instance.eye_from_model[0]), row(instance.eye_from_model[1]), row(instance.eye_from_model[2])];
        let turn = |r: [f32; 4]| r[0] * n[0] + r[1] * n[1] + r[2] * n[2];
        let normal = [turn(instance.normal[0]), turn(instance.normal[1]), turn(instance.normal[2])];
        let color = if data.colors.len() == count { data.colors[index] } else { [255; 4] };
        Vertex {
            clip: clip_of(projection, eye),
            eye,
            normal,
            uv: if data.uvs.len() == count { data.uvs[index] } else { [0.0, 0.0] },
            color: [decode(color[0]), decode(color[1]), decode(color[2]), color[3] as f32 / 255.0],
            material: if data.materials.len() == count { data.materials[index] as u32 } else { 0 },
            occlusion: if data.occlusion.len() == count { data.occlusion[index] as f32 / 255.0 } else { 1.0 },
            part: part_of(data, index) as u32,
        }
    }
}

/// The part a mesh's vertex belongs to.
fn part_of(data: &super::mesh::MeshData, vertex: usize) -> usize {
    if data.parts.len() == data.positions.len() { data.parts[vertex] as usize } else { 0 }
}

/// The state of `part` for an instance whose states start at
/// `part_base`; `None` when it keeps its own look.
fn part_row(frame: &WorldFrame, part_base: u32, part: usize) -> Option<PartRow> {
    if part_base == NO_PARTS {
        return None;
    }
    frame.parts.get(part_base as usize + part).copied()
}

/// Whether the part of a mesh's vertex hides, for an instance whose part
/// states start at `part_base`.
pub(super) fn part_hidden(frame: &WorldFrame, part_base: u32, data: &super::mesh::MeshData, vertex: usize) -> bool {
    part_row(frame, part_base, part_of(data, vertex)).is_some_and(|row| row[3] == 0)
}

fn clip_of(m: &[f32; 16], p: [f32; 3]) -> [f32; 4] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
        m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15],
    ]
}

#[derive(Clone, Copy, Debug)]
struct Vertex {
    clip: [f32; 4],
    eye: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    /// Linear, straight alpha.
    color: [f32; 4],
    /// The slot in the instance's materials (flat across a triangle).
    material: u32,
    occlusion: f32,
    /// The part of the mesh (flat across a triangle).
    part: u32,
}

fn lerp_vertex(a: &Vertex, b: &Vertex, t: f32) -> Vertex {
    let l = |x: f32, y: f32| x + (y - x) * t;
    Vertex {
        clip: [0, 1, 2, 3].map(|k| l(a.clip[k], b.clip[k])),
        eye: [0, 1, 2].map(|k| l(a.eye[k], b.eye[k])),
        normal: [0, 1, 2].map(|k| l(a.normal[k], b.normal[k])),
        uv: [0, 1].map(|k| l(a.uv[k], b.uv[k])),
        color: [0, 1, 2, 3].map(|k| l(a.color[k], b.color[k])),
        material: a.material,
        occlusion: l(a.occlusion, b.occlusion),
        part: a.part,
    }
}

/// The clip planes: near (z ≥ 0), far (z ≤ w), and a guard band four
/// views wide on each side, so no snapped corner overflows.
const GUARD: f32 = 4.0;

fn clip_polygon(vertices: [Vertex; 3]) -> Vec<Vertex> {
    let planes: [fn(&[f32; 4]) -> f32; 6] = [
        |c| c[2],
        |c| c[3] - c[2],
        |c| GUARD * c[3] + c[0],
        |c| GUARD * c[3] - c[0],
        |c| GUARD * c[3] + c[1],
        |c| GUARD * c[3] - c[1],
    ];
    let mut polygon: Vec<Vertex> = vertices.to_vec();
    for plane in planes {
        if polygon.is_empty() {
            break;
        }
        if polygon.iter().all(|v| plane(&v.clip) >= 0.0) {
            continue;
        }
        let mut out = Vec::with_capacity(polygon.len() + 2);
        for i in 0..polygon.len() {
            let (a, b) = (&polygon[i], &polygon[(i + 1) % polygon.len()]);
            let (da, db) = (plane(&a.clip), plane(&b.clip));
            if da >= 0.0 {
                out.push(*a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                out.push(lerp_vertex(a, b, da / (da - db)));
            }
        }
        polygon = out;
    }
    polygon
}

// MARK: - Triangles

/// What every triangle of one instance shares.
struct Job<'a> {
    frame: &'a WorldFrame,
    instance: &'a InstanceData,
    cull_back: bool,
    mirrored: bool,
    blend: Blend,
    mask: bool,
    /// How many pixels of its own depth slope the triangle steps back.
    bias: f32,
    texture: Option<&'a Texture>,
    /// The sun's shadow map of the frame.
    shadow: Option<&'a ShadowMap>,
    /// The floor map of the frame, decoded.
    floor: Option<&'a Texture>,
}

/// A corner on the screen: fixed-point position, depth, `1/w`.
#[derive(Clone, Copy)]
struct Screen {
    x: i64,
    y: i64,
    z: f32,
    inv_w: f32,
}

/// The edge function: positive when `p` lies to the right of `a → b`
/// on a y-down screen.
pub(super) fn edge(a: (i64, i64), b: (i64, i64), p: (i64, i64)) -> i64 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

/// A top edge (flat, going right) or a left edge (going up) of a
/// clockwise triangle: a pixel centre exactly on it belongs to it.
pub(super) fn top_left(a: (i64, i64), b: (i64, i64)) -> bool {
    (a.1 == b.1 && b.0 > a.0) || b.1 < a.1
}

impl Job<'_> {
    fn triangle(&self, target: &mut Target, vertices: [Vertex; 3]) {
        let polygon = clip_polygon(vertices);
        if polygon.len() < 3 {
            return;
        }
        let (w, h) = (target.width as f32, target.height as f32);
        let screen: Vec<Screen> = polygon
            .iter()
            .map(|v| {
                let inv_w = 1.0 / v.clip[3];
                let (x, y) = ((v.clip[0] * inv_w + 1.0) * 0.5 * w, (1.0 - v.clip[1] * inv_w) * 0.5 * h);
                Screen { x: snap(x), y: snap(y), z: v.clip[2] * inv_w, inv_w }
            })
            .collect();
        for k in 1..polygon.len() - 1 {
            self.fill(target, [&polygon[0], &polygon[k], &polygon[k + 1]], [screen[0], screen[k], screen[k + 1]]);
        }
    }

    fn fill(&self, target: &mut Target, vertices: [&Vertex; 3], screen: [Screen; 3]) {
        let p = |s: &Screen| (s.x, s.y);
        let area = edge(p(&screen[0]), p(&screen[1]), p(&screen[2]));
        if area == 0 {
            return;
        }
        // a turn keeps its sense on the screen, but the edge function's sign
        // flips with the y-down axis: counter-clockwise reads negative here
        let mut front = area < 0;
        if self.mirrored {
            front = !front;
        }
        if self.cull_back && !front {
            return;
        }
        let (mut vertices, mut screen, mut area) = (vertices, screen, area);
        if area < 0 {
            vertices.swap(1, 2);
            screen.swap(1, 2);
            area = -area;
        }
        let (s0, s1, s2) = (screen[0], screen[1], screen[2]);
        let min_x = s0.x.min(s1.x).min(s2.x);
        let max_x = s0.x.max(s1.x).max(s2.x);
        let min_y = s0.y.min(s1.y).min(s2.y);
        let max_y = s0.y.max(s1.y).max(s2.y);
        let (x0, x1) = (((min_x - 128) >> 8).max(0), ((max_x - 128) >> 8).min(target.width as i64 - 1));
        let (y0, y1) = (((min_y - 128) >> 8).max(0), ((max_y - 128) >> 8).min(target.height as i64 - 1));
        if x0 > x1 || y0 > y1 {
            return;
        }
        let edges = [(p(&s1), p(&s2)), (p(&s2), p(&s0)), (p(&s0), p(&s1))];
        let owns = edges.map(|(a, b)| top_left(a, b));
        let inv_area = 1.0 / area as f64;
        // the slope-scaled step back of a GPU: the steeper of the depth's
        // two slopes on the screen, per pixel, times the bias
        let step_back = if self.bias > 0.0 {
            let (x1, y1) = ((s1.x - s0.x) as f64 / 256.0, (s1.y - s0.y) as f64 / 256.0);
            let (x2, y2) = ((s2.x - s0.x) as f64 / 256.0, (s2.y - s0.y) as f64 / 256.0);
            let (z1, z2) = ((s1.z - s0.z) as f64, (s2.z - s0.z) as f64);
            let det = x1 * y2 - x2 * y1;
            let dzdx = (z1 * y2 - z2 * y1) / det;
            let dzdy = (z2 * x1 - z1 * x2) / det;
            (self.bias as f64 * dzdx.abs().max(dzdy.abs())) as f32
        } else {
            0.0
        };
        for py in y0..=y1 {
            for px in x0..=x1 {
                let at = (px * 256 + 128, py * 256 + 128);
                let e = edges.map(|(a, b)| edge(a, b, at));
                if (0..3).any(|k| e[k] < 0 || (e[k] == 0 && !owns[k])) {
                    continue;
                }
                let lambda = e.map(|value| (value as f64 * inv_area) as f32);
                let z = lambda[0] * s0.z + lambda[1] * s1.z + lambda[2] * s2.z;
                let index = (py as u32 * target.width + px as u32) as usize;
                let opaque = self.blend == Blend::Replace;
                if !(0.0..=1.0).contains(&z) {
                    continue;
                }
                // a surface that steps back stops short of the far plane: it
                // stays in the picture, behind everything drawn nearer
                let z = if self.bias > 0.0 { (z + step_back + DEPTH_TIE).min(DEPTH_CEILING) } else { z };
                if z >= target.depth[index] {
                    continue;
                }
                let correct = |e: [i64; 3]| {
                    let l = e.map(|value| (value as f64 * inv_area) as f32);
                    let w = [l[0] * s0.inv_w, l[1] * s1.inv_w, l[2] * s2.inv_w];
                    let sum = w[0] + w[1] + w[2];
                    w.map(|weight| weight / sum)
                };
                let b = correct(e);
                let texel = self.texture.map(|texture| {
                    let uv_at = |b: [f32; 3]| [0, 1].map(|k| vertices[0].uv[k] * b[0] + vertices[1].uv[k] * b[1] + vertices[2].uv[k] * b[2]);
                    let uv = uv_at(b);
                    // the screen derivatives of uv, by the neighbours' centres
                    let right = uv_at(correct(edges.map(|(a, c)| edge(a, c, (at.0 + 256, at.1)))));
                    let below = uv_at(correct(edges.map(|(a, c)| edge(a, c, (at.0, at.1 + 256)))));
                    let level = &texture.levels[0];
                    let (w, h) = (level.width as f32, level.height as f32);
                    let dx = ((right[0] - uv[0]) * w, (right[1] - uv[1]) * h);
                    let dy = ((below[0] - uv[0]) * w, (below[1] - uv[1]) * h);
                    let rho2 = (dx.0 * dx.0 + dx.1 * dx.1).max(dy.0 * dy.0 + dy.1 * dy.1);
                    texture.sample(uv[0], uv[1], 0.5 * log2(rho2.max(1e-12)))
                });
                let Some((color, glow)) = self.shade(vertices, b, front, texel, px as u32, py as u32) else { continue };
                if opaque {
                    target.depth[index] = z;
                }
                target.blend(index, color, glow, self.blend);
            }
        }
    }

    /// The colour of a fragment, premultiplied in codes, and its glow in a
    /// code premultiplied too; `None` when it is discarded.
    fn shade(&self, v: [&Vertex; 3], b: [f32; 3], front: bool, texel: Option<[f32; 4]>, x: u32, y: u32) -> Option<([f32; 4], f32)> {
        let u: &FrameUniforms = &self.frame.uniforms;
        let mix3 = |f: fn(&Vertex) -> [f32; 3]| -> V3 {
            let (a, c, d) = (f(v[0]), f(v[1]), f(v[2]));
            [0, 1, 2].map(|k| a[k] * b[0] + c[k] * b[1] + d[k] * b[2])
        };
        let position = mix3(|v| v.eye);
        let section = u.section;
        if section != [0.0; 4] && section[0] * position[0] + section[1] * position[1] + section[2] * position[2] + section[3] < 0.0 {
            return None;
        }
        let material = &self.frame.materials[(self.instance.material_base + v[0].material) as usize];
        let flags = material.params[3] as u32;
        let tint = self.instance.tint;
        // the instance's tint, and its part's over it
        let part = part_row(self.frame, self.instance.part_base, v[0].part as usize).unwrap_or([255; 4]);
        let tinted = [0, 1, 2].map(|k| decode(tint[k]) * decode(part[k]));
        let mut color = [0, 1, 2, 3].map(|k| v[0].color[k] * b[0] + v[1].color[k] * b[1] + v[2].color[k] * b[2]);
        if let Some(texel) = texel {
            color = [0, 1, 2, 3].map(|k| color[k] * texel[k]);
        }
        let two_sided = material.back[3] > 0.5;
        let surface = if front || !two_sided { material.base } else { [material.back[0], material.back[1], material.back[2], material.base[3]] };
        let base = [0, 1, 2].map(|k| surface[k] * tinted[k] * color[k]);
        let mut alpha = surface[3] * (tint[3] as f32 / 255.0) * color[3];
        if self.mask || flags & material_flag::MASK != 0 {
            if alpha < material.params[2] {
                return None;
            }
            alpha = 1.0;
        }
        if self.blend == Blend::Replace {
            alpha = 1.0;
        }
        let (linear, dither) = if flags & material_flag::UNLIT != 0 {
            (base, 0.0)
        } else {
            let normal = shade::normalize(mix3(|v| v.normal));
            let normal = if front { normal } else { shade::scale(normal, -1.0) };
            let occlusion = v[0].occlusion * b[0] + v[1].occlusion * b[1] + v[2].occlusion * b[2];
            let shadow = self.shadow.map_or(1.0, |map| sun_shadow::visibility(u, map, position, normal));
            let floor = match self.floor {
                Some(map) if self.instance.flags & instance_flag::FLOOR_MAP != 0 => floor_light(u, map, position),
                _ => shade::NO_FLOOR,
            };
            let lit = shade::lit(
                u,
                &Surface {
                    position,
                    normal,
                    base,
                    roughness: material.params[0],
                    metallic: material.params[1],
                    emissive: [material.emissive[0], material.emissive[1], material.emissive[2]],
                    occlusion,
                    groups: 1 << ((self.instance.flags >> instance_flag::GROUP_SHIFT) & instance_flag::GROUP_MASK),
                    shadow,
                    floor,
                    ambient: [decode(self.instance.ambient[0]), decode(self.instance.ambient[1]), decode(self.instance.ambient[2])],
                },
            );
            let dither = if u.tone[2] > 0.5 { shade::bayer(x, y) } else { 0.0 };
            (shade::tone(lit, u.tone[0], u.tone[1]), dither)
        };
        let codes = graded(self.frame, linear.map(encode_code)).map(|code| quantize(code * alpha, dither) as f32);
        let glow = super::bloom::glow([material.emissive[0], material.emissive[1], material.emissive[2]]);
        Some(([codes[0], codes[1], codes[2], (alpha * 255.0 + 0.5).floor()], (glow * alpha * 255.0 + 0.5).floor()))
    }
}

/// What the floor map gives a point at `position` (the eye's space): its
/// texel, read as a shader reads it — bilinear in linear light from the
/// base level, never past the picture's edge — or nothing off the map.
fn floor_light(u: &FrameUniforms, map: &Texture, position: V3) -> [f32; 4] {
    if u.counts[1] < 0.5 {
        return shade::NO_FLOOR;
    }
    let row = |r: [f32; 4]| r[0] * position[0] + r[1] * position[1] + r[2] * position[2] + r[3];
    let (s, t) = (row(u.floor_u), row(u.floor_v));
    if !((0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t)) {
        return shade::NO_FLOOR;
    }
    let base = &map.levels[0];
    let (half_s, half_t) = (0.5 / base.width as f32, 0.5 / base.height as f32);
    map.sample(s.clamp(half_s, 1.0 - half_s), t.clamp(half_t, 1.0 - half_t), 0.0)
}

/// A surface's continuous codes through the frame's grade: the four
/// points of the table that the walk from the cube's low corner to its
/// high one crosses, blended along that walk — the shaders' order.
fn graded(frame: &WorldFrame, code: [f32; 3]) -> [f32; 3] {
    let start = frame.uniforms.tone[3];
    if start < 0.5 {
        return code;
    }
    let g = code.map(|c| (c / 255.0 * 15.0).clamp(0.0, 15.0));
    let i = g.map(|v| v.floor().min(14.0));
    let mut w = [g[0] - i[0], g[1] - i[1], g[2] - i[2]];
    let mut s = [1usize, 16, 256];
    // the walk from the cube's low corner to its high one, the largest
    // share first: the same three swaps as the shaders'
    for (a, b) in [(0, 1), (1, 2), (0, 1)] {
        if w[a] < w[b] {
            w.swap(a, b);
            s.swap(a, b);
        }
    }
    let low = start as usize - 1 + i[0] as usize + 16 * i[1] as usize + 256 * i[2] as usize;
    let at = |index: usize| {
        let row = frame.parts.get(index).copied().unwrap_or([0; 4]);
        [row[0] as f32, row[1] as f32, row[2] as f32]
    };
    let (p0, p1, p2, p3) = (at(low), at(low + s[0]), at(low + s[0] + s[1]), at(low + s[0] + s[1] + s[2]));
    [0, 1, 2].map(|k| p0[k] + (p1[k] - p0[k]) * w[0] + (p2[k] - p1[k]) * w[1] + (p3[k] - p2[k]) * w[2])
}

/// The deepest a surface that steps back may go: the largest number
/// below the far plane's 1.
pub const DEPTH_CEILING: f32 = f32::from_bits(0x3F7F_FFFF);

/// The constant part of a surface's step back, `2⁻²²` — four steps of a
/// float depth near the far plane: it breaks the ties an edge and its
/// surface are left in where the slope gives nothing, as the constant
/// term of a GPU's depth bias does.
pub const DEPTH_TIE: f32 = 2.384_185_8e-7;

/// Snaps a pixel coordinate to 1/256, rounding half to even.
pub(super) fn snap(value: f32) -> i64 {
    (value as f64 * 256.0).round_ties_even() as i64
}

// MARK: - Lines

/// Pulls a line toward the eye before it meets the depth buffer, so an
/// edge wins against the faces it bounds. In the eye's space: the share
/// of the distance for a perspective lens, a fixed step for a parallel one.
const LINE_PULL: f32 = 2.0e-3;

fn line(target: &mut Target, frame: &WorldFrame, instance: &InstanceData, projection: &[f32; 16], style: &super::wire::LineStyleData, segment: &[[f32; 3]; 2]) {
    let u = &frame.uniforms;
    let parallel = u.params[3] > 0.5;
    let to_eye = |p: [f32; 3]| {
        let row = |r: [f32; 4]| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3];
        [row(instance.eye_from_model[0]), row(instance.eye_from_model[1]), row(instance.eye_from_model[2])]
    };
    let (mut a, mut b) = (to_eye(segment[0]), to_eye(segment[1]));
    if !parallel {
        // keep the part in front of the near plane
        let near = -frame.near as f32;
        let (da, db) = (near - a[2], near - b[2]);
        if da < 0.0 && db < 0.0 {
            return;
        }
        if da < 0.0 {
            a = [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * (da / (da - db)));
        } else if db < 0.0 {
            b = [0, 1, 2].map(|k| b[k] + (a[k] - b[k]) * (db / (db - da)));
        }
    }
    let pull = |p: [f32; 3]| {
        if parallel {
            [p[0], p[1], p[2] + LINE_PULL * (frame.far - frame.near) as f32]
        } else {
            shade::scale(p, 1.0 - LINE_PULL)
        }
    };
    let (ea, eb) = (pull(a), pull(b));
    let (w, h) = (target.width as f32, target.height as f32);
    let project = |p: [f32; 3]| {
        let c = clip_of(projection, p);
        let inv_w = 1.0 / c[3];
        ((c[0] * inv_w + 1.0) * 0.5 * w, (1.0 - c[1] * inv_w) * 0.5 * h, c[2] * inv_w, inv_w)
    };
    let (ax, ay, az, aw) = project(ea);
    let (bx, by, bz, bw) = project(eb);
    let [width, dash, gap, mode] = style.shape;
    let half = width * 0.5;
    let reach = half + 1.0;
    let (x0, x1) = ((ax.min(bx) - reach).floor().max(0.0) as i64, (ax.max(bx) + reach).ceil().min(w - 1.0) as i64);
    let (y0, y1) = ((ay.min(by) - reach).floor().max(0.0) as i64, (ay.max(by) + reach).ceil().min(h - 1.0) as i64);
    let (dx, dy) = (bx - ax, by - ay);
    let length2 = dx * dx + dy * dy;
    let length = length2.sqrt();
    let codes = [encode_code(style.color[0]), encode_code(style.color[1]), encode_code(style.color[2])];
    let section = u.section;
    for py in y0..=y1 {
        for px in x0..=x1 {
            let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
            let t = if length2 > 0.0 { (((cx - ax) * dx + (cy - ay) * dy) / length2).clamp(0.0, 1.0) } else { 0.0 };
            let (nx, ny) = (ax + dx * t - cx, ay + dy * t - cy);
            let coverage = shade::saturate(half + 0.5 - (nx * nx + ny * ny).sqrt());
            if coverage <= 0.0 {
                continue;
            }
            if dash > 0.0 {
                let along = t * length;
                let period = dash + gap;
                if along - (along / period).floor() * period >= dash {
                    continue;
                }
            }
            let z = az + (bz - az) * t;
            let index = (py as u32 * target.width + px as u32) as usize;
            let stored = target.depth[index];
            let passes = match mode as u32 {
                0 => z <= stored,
                1 => z > stored,
                _ => true,
            };
            if !passes || !(0.0..=1.0).contains(&z) {
                continue;
            }
            if section != [0.0; 4] {
                // the eye-space point under the pixel, corrected for perspective
                let (wa, wb) = ((1.0 - t) * aw, t * bw);
                let sum = wa + wb;
                let p = [0, 1, 2].map(|k| (ea[k] * wa + eb[k] * wb) / sum);
                if section[0] * p[0] + section[1] * p[1] + section[2] * p[2] + section[3] < 0.0 {
                    continue;
                }
            }
            let alpha = coverage * style.color[3];
            let source = [codes[0] * alpha, codes[1] * alpha, codes[2] * alpha, alpha * 255.0];
            target.blend(index, source, 0.0, Blend::Alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Color;
    use crate::world::camera::Camera;
    use crate::world::frame::{Background, Quality, WorldStyle};
    use crate::world::material::{LineDepth, LineStyle, Material};
    use crate::world::math::{Plane, Vec3, vec2, vec3};
    use crate::world::mesh::{Mesh, MeshData};
    use crate::world::store::{Entity, World};

    fn style(background: Background) -> WorldStyle {
        WorldStyle { background, quality: Quality::Low, ..WorldStyle::default() }
    }

    /// Writes the picture as raw RGBA when `BUNNY_WORLD_DUMP` names a
    /// folder — for a person to look at, never for the test to read.
    fn dump(name: &str, picture: &Picture) {
        if let Ok(folder) = std::env::var("BUNNY_WORLD_DUMP") {
            let path = std::path::Path::new(&folder).join(format!("{name}-{}x{}.rgba", picture.width, picture.height));
            let _ = std::fs::write(path, &picture.rgba);
        }
    }

    fn fnv(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, &byte| (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3))
    }

    fn square() -> Mesh {
        // two triangles sharing the diagonal, facing +Z
        Mesh::new(MeshData {
            positions: vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            uvs: vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..MeshData::default()
        })
    }

    fn front_camera() -> Camera {
        Camera::looking_at(vec3(0.0, 0.0, 3.0), Vec3::ZERO)
    }

    #[test]
    fn two_triangles_sharing_an_edge_cover_each_pixel_once() {
        let world = World::new();
        // half see-through: a pixel covered twice would show the double blend
        world.add(square(), Material::unlit(Color::rgba(255, 0, 0, 128)).blend());
        let picture = world.render(&front_camera(), &style(Background::Transparent), (64, 64), 1.0);
        let inside: Vec<[u8; 4]> = (16..48).map(|i| picture.at(i, i)).chain((16..48).map(|i| picture.at(i, 63 - i))).collect();
        let first = picture.at(32, 20);
        assert!(first[3] > 100 && first[3] < 140, "half alpha: {first:?}");
        assert!(inside.iter().all(|p| *p == first), "the diagonal is blended once like the rest");
    }

    #[test]
    fn the_nearer_face_wins() {
        let world = World::new();
        let near = world.add(square(), Material::unlit(Color::rgb(0, 255, 0))).set_position(vec3(0.0, 0.0, 0.5));
        world.add(square(), Material::unlit(Color::rgb(255, 0, 0)));
        let picture = world.render(&front_camera(), &style(Background::Transparent), (32, 32), 1.0);
        assert_eq!(picture.at(16, 16), [0, 255, 0, 255]);
        // the same with the far one drawn first and the near one last
        near.remove();
        world.add(square(), Material::unlit(Color::rgb(0, 255, 0))).set_position(vec3(0.0, 0.0, 0.5));
        let picture = world.render(&front_camera(), &style(Background::Transparent), (32, 32), 1.0);
        assert_eq!(picture.at(16, 16), [0, 255, 0, 255]);
    }

    #[test]
    fn colour_is_interpolated_for_perspective() {
        // a long strip on the ground, black near the eye, white far away
        let world = World::new();
        let strip = Mesh::new(MeshData {
            positions: vec![[-1.0, 0.0, 2.0], [1.0, 0.0, 2.0], [1.0, 0.0, -20.0], [-1.0, 0.0, -20.0]],
            normals: vec![[0.0, 1.0, 0.0]; 4],
            colors: vec![[0, 0, 0, 255], [0, 0, 0, 255], [255, 255, 255, 255], [255, 255, 255, 255]],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..MeshData::default()
        });
        world.add(strip, Material::unlit(Color::WHITE));
        let camera = Camera::looking_at(vec3(0.0, 1.0, 4.0), vec3(0.0, 0.0, -9.0));
        let picture = world.render(&camera, &style(Background::Transparent), (64, 64), 1.0);
        let size = crate::layout::Size { width: 64.0, height: 64.0 };
        let ground = Plane::new(Vec3::ZERO, Vec3::Y);
        let mut checked = 0;
        for y in 0..64 {
            let pixel = picture.at(32, y);
            if pixel[3] == 0 {
                continue;
            }
            // where the ray through the pixel's centre meets the ground
            let ray = camera.ray(crate::layout::Point { x: 32.5, y: y as f64 + 0.5 }, size);
            let Some(t) = ray.hits_plane(&ground) else { continue };
            let z = ray.at(t).z;
            if !(-19.0..=1.0).contains(&z) {
                continue;
            }
            let expected = ((2.0 - z) / 22.0) as f32;
            let linear = decode(pixel[0]);
            assert!((linear - expected).abs() < 0.012, "row {y}: {linear} vs {expected} (z = {z})");
            checked += 1;
        }
        assert!(checked > 10, "the strip covers rows: {checked}");
    }

    #[test]
    fn a_triangle_through_the_near_plane_keeps_its_visible_part() {
        let world = World::new();
        world.add(Mesh::plane(400.0, 400.0), Material::unlit(Color::rgb(30, 60, 90)));
        let camera = Camera::looking_at(vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, -10.0));
        let picture = world.render(&camera, &style(Background::Transparent), (48, 48), 1.0);
        assert_eq!(picture.at(24, 47), [30, 60, 90, 255], "the ground under the eye");
        assert_eq!(picture.at(24, 0)[3], 0, "the sky above the horizon is empty");
    }

    #[test]
    fn back_faces_take_the_back_colour_or_vanish() {
        let world = World::new();
        let sheet = world.add(square(), Material::unlit(Color::rgb(200, 0, 0)));
        let behind = Camera::looking_at(vec3(0.0, 0.0, -3.0), Vec3::ZERO);
        let picture = world.render(&behind, &style(Background::Transparent), (16, 16), 1.0);
        assert_eq!(picture.at(8, 8), [0, 0, 0, 0], "a closed shape's back is never drawn");
        sheet.set_shape(crate::world::store::Shape::Model(crate::world::material::Model::single(
            square(),
            Material::unlit(Color::rgb(200, 0, 0)).two_sided(Color::rgb(0, 0, 200)),
        )));
        let picture = world.render(&behind, &style(Background::Transparent), (16, 16), 1.0);
        assert_eq!(picture.at(8, 8), [0, 0, 200, 255]);
    }

    #[test]
    fn a_far_cube_paints_like_a_near_one() {
        let paint = |at: Vec3| {
            let world = World::new();
            world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(180, 120, 60))).set_position(at);
            let camera = Camera::orbiting(at, 35.0, 25.0, 4.0);
            world.render(&camera, &style(Background::Color(Color::rgb(240, 240, 240))), (40, 30), 1.0)
        };
        assert_eq!(paint(Vec3::ZERO), paint(vec3(1.0e6, 0.0, -1.0e6)));
    }

    #[test]
    fn the_oracle_is_byte_stable() {
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(200, 80, 40)));
        world.add(Mesh::sphere(0.6), Material::color(Color::rgb(60, 120, 220)).roughness(0.3)).set_position(vec3(1.4, 0.0, 0.0));
        world.add(Mesh::plane(6.0, 6.0), Material::color(Color::rgb(235, 235, 230))).set_position(vec3(0.0, -0.5, 0.0));
        world.add_lines(Mesh::cuboid(1.0, 1.0, 1.0).edges(30.0), LineStyle::solid(Color::rgb(20, 20, 25), 1.5));
        let camera = Camera::orbiting(vec3(0.5, 0.0, 0.0), 30.0, 25.0, 6.0);
        let backdrop = Background::Gradient { top: Color::rgb(200, 215, 235), bottom: Color::rgb(245, 245, 245) };
        let first = world.render(&camera, &style(backdrop), (96, 64), 1.0);
        let second = world.render(&camera, &style(backdrop), (96, 64), 1.0);
        dump("oracle", &first);
        assert_eq!(first, second);
        // the same bytes on every machine: this number is the picture's
        // fingerprint, and a platform that rounds one pixel differently
        // fails here
        assert_eq!(fnv(&first.rgba), 15_732_496_699_468_962_662, "the picture's fingerprint moved");
    }

    /// A low square ring with a hole, its edges drawn, seen from above
    /// and in front: the corners of the hole are where an edge meets a
    /// surface that turns away, and where a hidden edge leaves a corner.
    fn ring_with_edges() -> (World, Camera, Vec3) {
        let world = World::new();
        world.add(Mesh::plane(16.0, 16.0), Material::color(Color::rgb(228, 224, 216)));
        let outline = [vec2(0.0, 0.0), vec2(2.4, 0.0), vec2(2.4, 2.4), vec2(0.0, 2.4)];
        let hole = [vec2(0.6, 0.6), vec2(1.8, 0.6), vec2(1.8, 1.8), vec2(0.6, 1.8)];
        let ring = Mesh::extrude(&outline, &[&hole], 0.4);
        let at = vec3(1.2, 0.0, -2.8);
        world.add(ring.clone(), Material::color(Color::rgb(246, 244, 238))).set_position(at);
        world.add_lines(ring.edges(30.0), LineStyle::solid(Color::rgb(20, 20, 24), 1.2)).set_position(at);
        (world, Camera::orbiting(vec3(0.0, 0.6, 0.0), 32.0, 24.0, 9.0), at)
    }

    fn ink(picture: &Picture, x: f64, y: f64, reach: i64) -> bool {
        let (cx, cy) = (x.floor() as i64, y.floor() as i64);
        (-reach..=reach).any(|dy| {
            (-reach..=reach).any(|dx| {
                let (px, py) = (cx + dx, cy + dy);
                px >= 0 && py >= 0 && (px as u32) < picture.width && (py as u32) < picture.height && picture.at(px as u32, py as u32)[0] < 120
            })
        })
    }

    #[test]
    fn an_edge_on_a_surface_that_turns_away_stays_whole() {
        let (world, camera, at) = ring_with_edges();
        let size = crate::layout::Size { width: 480.0, height: 280.0 };
        let picture = world.render(&camera, &style(Background::Transparent), (480, 280), 1.0);
        dump("ring", &picture);
        // the near edge of the hole: the top on one side, a wall that faces
        // away on the other
        let from = camera.project(at + vec3(0.6, 0.4, 1.8), size).expect("in view");
        let to = camera.project(at + vec3(1.8, 0.4, 1.8), size).expect("in view");
        let steps = 40;
        for step in 2..steps - 1 {
            let t = step as f64 / steps as f64;
            let (x, y) = (from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            assert!(ink(&picture, x, y, 0), "the edge has a gap at ({x:.1}, {y:.1})");
        }
    }

    #[test]
    fn an_edge_behind_a_surface_stays_hidden_up_to_its_corner() {
        let (world, camera, at) = ring_with_edges();
        let size = crate::layout::Size { width: 480.0, height: 280.0 };
        let picture = world.render(&camera, &style(Background::Transparent), (480, 280), 1.0);
        // the vertical edge at the hole's far right corner is behind the
        // ring's right side all the way down
        for y in [0.05, 0.15, 0.25] {
            let p = camera.project(at + vec3(1.8, y, 0.6), size).expect("in view");
            assert!(!ink(&picture, p.x, p.y, 0), "a hidden edge shows at height {y}");
        }
    }

    #[test]
    fn an_xray_shows_the_edges_behind_its_surfaces() {
        let (world, camera, at) = ring_with_edges();
        let size = crate::layout::Size { width: 480.0, height: 280.0 };
        let xray = WorldStyle { xray: true, ..style(Background::Transparent) };
        let picture = world.render(&camera, &xray, (480, 280), 1.0);
        dump("ring-xray", &picture);
        // the edge at the hole's far right corner, hidden behind the ring
        // in the plain picture, shows through it
        for y in [0.05, 0.15, 0.25] {
            let p = camera.project(at + vec3(1.8, y, 0.6), size).expect("in view");
            assert!(ink(&picture, p.x, p.y, 1), "the hidden edge stays hidden at height {y}");
        }
        // and a surface lets the backdrop through
        let top = camera.project(at + vec3(0.3, 0.4, 0.3), size).expect("in view");
        let pixel = picture.at(top.x as u32, top.y as u32);
        assert!(pixel[3] > 100 && pixel[3] < 255, "a see-through top: {pixel:?}");
    }

    #[test]
    fn a_grade_turns_the_colours() {
        use crate::world::frame::Grade;
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::unlit(Color::rgb(200, 120, 60)));
        let camera = Camera::looking_at(vec3(0.0, 0.0, 4.0), Vec3::ZERO).parallel_height(2.0);
        let medium = WorldStyle { quality: Quality::Medium, ..style(Background::Transparent) };
        let plain = world.render(&camera, &medium, (32, 32), 1.0);
        // the identity: the table's points blend back to the codes
        let same = WorldStyle { grade: Some(Grade::from_fn(|c| c)), ..medium.clone() };
        let picture = world.render(&camera, &same, (32, 32), 1.0);
        let worst = plain.rgba.iter().zip(&picture.rgba).map(|(a, b)| a.abs_diff(*b)).max();
        assert!(worst <= Some(1), "an identity grade moves a code by {worst:?}");
        // red and blue trade places
        let swap = WorldStyle { grade: Some(Grade::from_fn(|[r, g, b]| [b, g, r])), ..medium.clone() };
        let swapped = world.render(&camera, &swap, (32, 32), 1.0).at(16, 16);
        let [r, g, b, _] = plain.at(16, 16);
        assert!(swapped[0].abs_diff(b) <= 1 && swapped[1].abs_diff(g) <= 1 && swapped[2].abs_diff(r) <= 1, "{swapped:?}");
        assert!(Grade::new(vec![[0; 3]; 15]).is_none(), "a table of another count");
    }

    #[test]
    fn a_glowing_ball_spills_its_light_at_high() {
        let world = World::new();
        world.add(Mesh::sphere(0.3), Material::unlit(Color::rgb(255, 190, 110)).emissive(Color::rgb(255, 190, 110), 3.0));
        world.add(Mesh::cuboid(0.4, 0.4, 0.4), Material::color(Color::rgb(90, 140, 200))).set_position(vec3(1.2, 0.0, 0.0));
        let camera = Camera::looking_at(vec3(0.0, 0.0, 4.0), Vec3::ZERO).parallel_height(3.0);
        let dark = Background::Color(Color::rgb(16, 18, 24));
        let high = WorldStyle { quality: Quality::High, bloom: 0.6, ..style(dark) };
        let frame = world.frame(&camera, &high, (96, 96), 1.0);
        assert_eq!(frame.bloom, 154, "0.6 in 256ths");
        let glowing = draw(&frame);
        let plain = draw(&world.frame(&camera, &WorldStyle { bloom: 0.0, ..high.clone() }, (96, 96), 1.0));
        dump("bloom", &glowing);
        // just past the ball's edge (its radius is ten pixels), the light
        // spills; a far corner keeps its colour
        let (near, far) = ((48 + 12, 48), (2, 2));
        assert!(glowing.at(near.0, near.1)[0] > plain.at(near.0, near.1)[0] + 20, "{:?} over {:?}", glowing.at(near.0, near.1), plain.at(near.0, near.1));
        assert_eq!(glowing.at(far.0, far.1), plain.at(far.0, far.1));
        assert!(glowing.rgba.chunks(4).all(|pixel| pixel[3] == 255), "an opaque picture stays opaque");
        // the box does not glow: its middle gains only what spills from the ball
        let middle = glowing.at(48 + 38, 48);
        assert!(middle[2] >= plain.at(48 + 38, 48)[2], "a screen never darkens");
        // below High, no bloom
        let medium = world.frame(&camera, &WorldStyle { quality: Quality::Medium, ..high }, (96, 96), 1.0);
        assert_eq!(medium.bloom, 0);
    }

    /// A tall box whose top hangs from an arm that a clip carries one
    /// meter along x; the bottom stays on the root.
    fn swaying_box() -> (World, crate::world::skin::Clip, Entity) {
        use crate::world::material::Model;
        use crate::world::math::Transform;
        use crate::world::skin::{Channel, Clip, Path, Skeleton, SkeletonNode, Skin};
        let skeleton = Skeleton::new(vec![
            SkeletonNode { name: Some("root".into()), parent: None, rest: Transform::IDENTITY },
            SkeletonNode { name: Some("arm".into()), parent: Some(0), rest: Transform::IDENTITY },
        ]);
        let skin = Skin::new(skeleton, vec![0, 1], Vec::new()).expect("a skin");
        let mut data = Mesh::cuboid(0.4, 2.0, 0.4).data().clone();
        data.joints = vec![[0, 1, 0, 0]; data.positions.len()];
        data.weights = data.positions.iter().map(|p| if p[1] > 0.0 { [0, 255, 0, 0] } else { [255, 0, 0, 0] }).collect();
        let world = World::new();
        let entity = world.add_model(Model::single(Mesh::new(data), Material::unlit(Color::rgb(220, 90, 40))).skinned(skin));
        let sway = Channel { node: 1, path: Path::Translation, step: false, times: Rc::from([0.0, 1.0]), values: Rc::from([0.0, 0.0, 0.0, 1.0, 0.0, 0.0]) };
        (world, Clip::new(Some("sway".into()), vec![sway]), entity)
    }

    #[test]
    fn a_skinned_box_follows_its_clip() {
        use crate::world::skin::Play;
        let (world, clip, entity) = swaying_box();
        let camera = Camera::looking_at(vec3(0.0, 0.0, 5.0), Vec3::ZERO).parallel_height(4.0);
        let rest = world.render(&camera, &style(Background::Transparent), (64, 64), 1.0);
        // at rest the box stands straight: its top half on the middle column
        assert!(rest.at(32, 18)[3] > 0 && rest.at(48, 18)[3] == 0, "{:?} {:?}", rest.at(32, 18), rest.at(48, 18));
        entity.set_play(Some(Play { clip, start: motor::task::now() - 5.0, speed: 1.0, looped: false }));
        let moved = world.render(&camera, &style(Background::Transparent), (64, 64), 1.0);
        dump("skinned-box", &moved);
        // the clip held at its end: the top a meter to the right, the
        // bottom where it stood
        assert!(moved.at(48, 18)[3] > 0 && moved.at(32, 18)[3] == 0, "{:?} {:?}", moved.at(48, 18), moved.at(32, 18));
        assert_eq!(moved.at(32, 46), rest.at(32, 46), "the root's half stays");
    }

    #[test]
    fn garments_share_their_wearers_palette() {
        use crate::world::material::Model;
        let (world, clip, body) = swaying_box();
        let model = body.spec().and_then(|spec| match spec.shape {
            crate::world::store::Shape::Model(model) => Some(model),
            _ => None,
        });
        let model: Model = model.expect("the body's model");
        // a coat: the same mesh and skin, hung under the body
        let coat = world.add_model(Model { materials: Rc::from(vec![Material::unlit(Color::rgb(40, 60, 200))]), ..model }).set_parent(Some(body));
        body.play(&clip);
        let camera = Camera::looking_at(vec3(0.0, 0.0, 5.0), Vec3::ZERO).parallel_height(4.0);
        let frame = world.frame(&camera, &style(Background::Transparent), (64, 64), 1.0);
        assert_eq!(frame.joints.len(), 6, "one palette for the body and its coat");
        assert!(frame.instances.iter().all(|instance| instance.joint_base == 0));
        assert!(coat.is_alive());
        // a clip that plays asks for frames; one held at its end does not
        let moving = |world: &World| world.with(|data| data.moving(motor::task::now()));
        assert!(moving(&world));
        body.set_play(Some(crate::world::skin::Play { clip, start: motor::task::now() - 5.0, speed: 1.0, looped: false }));
        assert!(!moving(&world));
    }

    #[test]
    fn edges_cover_their_faces_without_gaps_or_fighting() {
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::unlit(Color::WHITE));
        world.add_lines(Mesh::cuboid(1.0, 1.0, 1.0).edges(30.0), LineStyle::solid(Color::rgb(255, 0, 0), 2.0));
        let camera = Camera::looking_at(vec3(0.0, 0.0, 4.0), Vec3::ZERO).parallel_height(2.0);
        let picture = world.render(&camera, &style(Background::Transparent), (64, 64), 1.0);
        // the front face's top edge runs along y = 16 (the cube spans 16..48)
        let red = (20..44).filter(|&x| picture.at(x, 16)[0] > 200 && picture.at(x, 16)[1] < 60).count();
        assert_eq!(red, 24, "the edge shows along its whole length");
        assert_eq!(picture.at(32, 32), [255, 255, 255, 255], "the face between the edges is the face");
    }

    #[test]
    fn hidden_edges_show_only_behind_faces_and_dash() {
        let world = World::new();
        world.add(square(), Material::unlit(Color::WHITE));
        // a long line behind the sheet, poking out on both sides
        let line = crate::world::mesh::Lines::new(vec![[[-3.0, 0.0, -1.0], [3.0, 0.0, -1.0]]]);
        world.add_lines(line, LineStyle::solid(Color::rgb(0, 0, 255), 2.0).dashed(4.0, 4.0).depth(LineDepth::Hidden));
        let camera = Camera::looking_at(vec3(0.0, 0.0, 4.0), Vec3::ZERO).parallel_height(4.0);
        let picture = world.render(&camera, &style(Background::Transparent), (64, 64), 1.0);
        let row: Vec<bool> = (0..64).map(|x| picture.at(x, 32)[2] > 200 && picture.at(x, 32)[0] < 100).collect();
        assert!(!row[2] && !row[61], "outside the sheet nothing hides it: no hidden line");
        let dashes: Vec<bool> = row[18..46].to_vec();
        assert!(dashes.contains(&true) && dashes.contains(&false), "behind the sheet it shows, dashed: {dashes:?}");
    }

    #[test]
    fn a_line_is_as_wide_as_asked_at_every_scale() {
        for scale in [1.0f32, 2.0, 3.0] {
            let world = World::new();
            let line = crate::world::mesh::Lines::new(vec![[[-10.0, 0.0, 0.0], [10.0, 0.0, 0.0]]]);
            world.add_lines(line, LineStyle::solid(Color::rgb(0, 0, 0), 3.0));
            let camera = Camera::looking_at(vec3(0.0, 0.0, 4.0), Vec3::ZERO).parallel_height(4.0);
            let side = (40.0 * scale) as u32;
            let picture = world.render(&camera, &style(Background::Transparent), (side, side), scale);
            let coverage: f32 = (0..side).map(|y| picture.at(side / 2, y)[3] as f32 / 255.0).sum();
            assert!((coverage - 3.0 * scale).abs() < 0.15, "scale {scale}: {coverage}");
        }
    }

    #[test]
    fn the_section_cuts_faces_and_edges_alike() {
        let world = World::new();
        world.add(square(), Material::unlit(Color::rgb(0, 200, 0)));
        world.add_lines(crate::world::mesh::Lines::new(vec![[[-1.0, -0.5, 0.0], [1.0, -0.5, 0.0]]]), LineStyle::solid(Color::rgb(200, 0, 0), 2.0));
        let camera = Camera::looking_at(vec3(0.0, 0.0, 3.0), Vec3::ZERO).parallel_height(4.0);
        // keep the left half (x < 0)
        let cut = WorldStyle { section: Some(Plane::new(Vec3::ZERO, -Vec3::X)), ..style(Background::Transparent) };
        let picture = world.render(&camera, &cut, (64, 64), 1.0);
        assert_eq!(picture.at(24, 24), [0, 200, 0, 255], "the kept half stays");
        assert_eq!(picture.at(40, 24), [0, 0, 0, 0], "the cut half is gone");
        assert!(picture.at(24, 40)[0] > 150, "the edge on the kept side stays");
        assert_eq!(picture.at(40, 40), [0, 0, 0, 0], "the edge on the cut side is cut too");
    }

    #[test]
    fn a_textured_sheet_is_crisp_near_and_its_average_far() {
        use crate::image_engine::{ImageSource, RawImages};
        let side = 64u32;
        let checker: Vec<u8> = (0..side * side)
            .flat_map(|i| if ((i % side) / 8 + (i / side) / 8).is_multiple_of(2) { [255, 255, 255, 255] } else { [0, 0, 0, 255] })
            .collect();
        let picture = ImageSource::from_bytes(RawImages::encode(side, side, &checker));
        let images = RawImages::default();
        let world = World::new();
        world.add(square(), Material::unlit(Color::WHITE).texture(picture));
        // near: the sheet fills the view, eight texels a square: crisp
        let near = Camera::looking_at(vec3(0.0, 0.0, 1.0), Vec3::ZERO).parallel_height(2.0);
        let close = world.render_with(&images, &near, &style(Background::Transparent), (64, 64), 1.0);
        let values: std::collections::BTreeSet<u8> = (0..64).map(|x| close.at(x, 4)[0]).collect();
        assert!(values.contains(&255) && values.contains(&0), "black and white squares: {values:?}");
        // far: the sheet is four pixels wide, every square under a texel
        let far = Camera::looking_at(vec3(0.0, 0.0, 1.0), Vec3::ZERO).parallel_height(32.0);
        let small = world.render_with(&images, &far, &style(Background::Transparent), (64, 64), 1.0);
        let middle = small.at(32, 32);
        assert!((middle[0] as i32 - 188).abs() <= 3, "half the light: {middle:?}");
        // and without the engine, the colour alone
        let plain = world.render(&near, &style(Background::Transparent), (64, 64), 1.0);
        assert_eq!(plain.at(10, 10), [255, 255, 255, 255]);
    }

    /// A box on a floor under a sun that travels toward the camera: its
    /// shadow falls in front of it, in view.
    fn sunlit_box(shadows: bool, quality: Quality) -> Picture {
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(222, 120, 60)));
        world.add(Mesh::plane(8.0, 8.0), Material::color(Color::rgb(200, 196, 188))).set_position(vec3(0.0, -0.5, 0.0));
        let sun = crate::world::material::Light::sun().toward(vec3(0.5, -1.0, 0.7));
        world.add_light(if shadows { sun.with_shadows() } else { sun });
        let style = WorldStyle { background: Background::Color(Color::rgb(236, 233, 226)), quality, ..WorldStyle::default() };
        let camera = Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0);
        draw(&world.frame(&camera, &style, (160, 120), 1.0))
    }

    #[test]
    fn a_box_casts_its_shadow_on_the_floor_and_none_on_itself() {
        let size = crate::layout::Size { width: 160.0, height: 120.0 };
        let camera = Camera::orbiting(Vec3::ZERO, 35.0, 28.0, 5.0);
        let pixel = |picture: &Picture, point: Vec3| {
            let at = camera.project(point, size).expect("in view");
            picture.at(at.x as u32, at.y as u32)
        };
        let plain = sunlit_box(false, Quality::Low);
        for quality in [Quality::Low, Quality::Medium, Quality::High] {
            let shaded = sunlit_box(true, quality);
            dump(&format!("shadow-{quality:?}"), &shaded);
            // the floor under the box's middle, seen past its front corner
            let under = vec3(0.625, -0.5, 0.775);
            let (dark, light) = (pixel(&shaded, under), pixel(&plain, under));
            assert!((dark[0] as i32) < light[0] as i32 - 30, "{quality:?}: the shadow is darker: {dark:?} vs {light:?}");
            // the open floor and the sunlit top keep every bit
            for open in [vec3(-2.0, -0.5, 2.0), vec3(2.5, -0.5, -1.0), vec3(0.0, 0.5, 0.0), vec3(-0.3, 0.5, 0.2)] {
                assert_eq!(pixel(&shaded, open), pixel(&plain, open), "{quality:?}: a lit point at {open:?} moved");
            }
            // a shadow only takes light away, and only around the box
            let mut darker = 0;
            for (a, b) in shaded.rgba.as_chunks::<4>().0.iter().zip(plain.rgba.as_chunks::<4>().0) {
                assert!((0..3).all(|k| a[k] <= b[k]), "{quality:?}: a pixel grew lighter: {a:?} vs {b:?}");
                darker += (a != b) as usize;
            }
            let share = darker as f64 / (160.0 * 120.0);
            assert!(share > 0.01 && share < 0.12, "{quality:?}: {share} of the picture in shadow");
        }
    }

    #[test]
    fn a_lamp_lights_only_the_groups_it_names() {
        let paint = |groups: u32| {
            let world = World::new();
            world.add_light(crate::world::material::Light::sky(Color::rgb(20, 20, 24), Color::rgb(10, 10, 10)));
            world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::WHITE)).set_position(vec3(-0.8, 0.0, 0.0)).set_light_group(1);
            world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::WHITE)).set_position(vec3(0.8, 0.0, 0.0)).set_light_group(2);
            world.add_light(crate::world::material::Light::point(Color::WHITE).range(4.0).groups(groups)).set_position(vec3(0.0, 0.0, 1.5));
            world.render(&front_camera(), &style(Background::Transparent), (64, 32), 1.0)
        };
        let (both, first, neither) = (paint(0b110), paint(0b010), paint(0));
        let (left, right) = ((16, 16), (48, 16));
        assert!(first.at(left.0, left.1)[0] > neither.at(left.0, left.1)[0] + 20, "the lamp lights its group");
        assert_eq!(first.at(right.0, right.1), neither.at(right.0, right.1), "and nothing of the other");
        assert_eq!(both.at(left.0, left.1), first.at(left.0, left.1), "a group is lit the same with another beside it");
        assert!(both.at(right.0, right.1)[0] > neither.at(right.0, right.1)[0] + 20);
    }

    #[test]
    fn a_floor_map_lays_its_light_and_shade_on_what_reads_it() {
        use crate::image_engine::RawImages;
        let images = RawImages::default();
        let world = World::new();
        let floor = world.add(Mesh::plane(6.0, 6.0), Material::color(Color::rgb(214, 210, 202))).set_position(vec3(0.0, -0.5, 0.0));
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(90, 140, 210)));
        world.add_light(crate::world::material::Light::sun().toward(vec3(-1.0, -2.0, -1.5)));
        let camera = Camera::orbiting(Vec3::ZERO, 25.0, 40.0, 6.0);
        let paint = || world.render_with(&images, &camera, &style(Background::Color(Color::rgb(236, 233, 226))), (200, 140), 1.0);
        let size = crate::layout::Size { width: 200.0, height: 140.0 };
        let at = |picture: &Picture, point: Vec3| {
            let p = camera.project(point, size).expect("in view");
            picture.at(p.x as u32, p.y as u32)
        };
        let plain = paint();
        // a shade under the box, and a warm pool of light off to one side
        let (side, span) = (32u32, 6.0);
        let mut rgba = Vec::new();
        for row in 0..side {
            for column in 0..side {
                let at = |index: u32| -span * 0.5 + (index as f64 + 0.5) / side as f64 * span;
                let (x, z) = (at(column), at(row));
                let shade = (0.2 + 0.8 * ((x * x + z * z).sqrt() - 0.5) / 0.8).clamp(0.2, 1.0);
                let pool = (1.0 - ((x - 1.4) * (x - 1.4) + (z + 0.9) * (z + 0.9)).sqrt() / 1.4).clamp(0.0, 1.0);
                rgba.extend_from_slice(&[(150.0 * pool) as u8, (96.0 * pool) as u8, (40.0 * pool) as u8, (255.0 * shade) as u8]);
            }
        }
        world.set_floor_map(Some(crate::world::material::FloorMap {
            picture: crate::image_engine::ImageSource::rgba(0xF100_0000_0000_0002, (side, side), rgba),
            origin: vec2(-span * 0.5, -span * 0.5),
            size: vec2(span, span),
        }));
        assert_eq!(paint(), plain, "a floor that reads no map keeps every bit");
        floor.set_reads_floor_map(true);
        let laid = paint();
        dump("floor-map", &laid);
        let (beside, pool, far) = (vec3(0.6, -0.5, 0.25), vec3(1.4, -0.5, -0.9), vec3(-2.6, -0.5, 2.6));
        assert!(at(&laid, beside)[0] + 12 < at(&plain, beside)[0], "the shade beside the box");
        let (warm, cool) = (at(&laid, pool), at(&plain, pool));
        assert!(warm[0] >= cool[0] + 8 && warm[2] <= cool[2], "the warm pool: {warm:?} over {cool:?}");
        assert_eq!(at(&laid, far), at(&plain, far), "an open corner of the map adds and takes nothing");
        assert_eq!(at(&laid, vec3(0.0, 0.5, 0.0)), at(&plain, vec3(0.0, 0.5, 0.0)), "the box reads no map");
    }

    #[test]
    fn a_blob_shadow_darkens_its_heart_and_leaves_its_rim() {
        use crate::image_engine::RawImages;
        use crate::world::material::Model;
        let images = RawImages::default();
        let world = World::new();
        world.add(Mesh::plane(6.0, 6.0), Material::color(Color::rgb(214, 210, 202)));
        let camera = Camera::orbiting(Vec3::ZERO, 20.0, 50.0, 5.0);
        let paint = || world.render_with(&images, &camera, &style(Background::Color(Color::rgb(236, 233, 226))), (160, 120), 1.0);
        let size = crate::layout::Size { width: 160.0, height: 120.0 };
        let at = |picture: &Picture, point: Vec3| {
            let p = camera.project(point, size).expect("in view");
            picture.at(p.x as u32, p.y as u32)
        };
        let plain = paint();
        world.add_model(Model::blob_shadow(0.6, 0.5)).set_position(vec3(0.5, 0.0, 0.0));
        let blob = paint();
        dump("blob", &blob);
        let (heart, rim, far) = (vec3(0.5, 0.0, 0.0), vec3(1.15, 0.0, 0.0), vec3(-2.0, 0.0, 1.5));
        assert!((at(&blob, heart)[0] as f32) < at(&plain, heart)[0] as f32 * 0.65, "the heart is dark");
        assert_eq!(at(&blob, rim), at(&plain, rim), "past the rim nothing");
        assert_eq!(at(&blob, far), at(&plain, far));
    }

    #[test]
    fn the_axes_run_red_green_and_blue_from_the_origin() {
        let world = World::new();
        let axes = world.add_axes(2.0);
        assert_eq!(world.len(), 7, "a group and six lines");
        let camera = Camera::orbiting(Vec3::ZERO, 35.0, 25.0, 6.0);
        let picture = world.render(&camera, &style(Background::Color(Color::WHITE)), (160, 120), 1.0);
        let size = crate::layout::Size { width: 160.0, height: 120.0 };
        let ink = |point: Vec3| {
            let p = camera.project(point, size).expect("in view");
            picture.at(p.x as u32, p.y as u32)
        };
        let (x, y, z) = (ink(vec3(1.2, 0.0, 0.0)), ink(vec3(0.0, 1.2, 0.0)), ink(vec3(0.0, 0.0, 1.2)));
        assert!(x[0] > x[1] + 60 && x[0] > x[2] + 60, "x is red: {x:?}");
        assert!(y[1] > y[0] + 40 && y[1] > y[2] + 40, "y is green: {y:?}");
        assert!(z[2] > z[0] + 60 && z[2] > z[1] + 60, "z is blue: {z:?}");
        axes.remove();
        assert_eq!(world.len(), 0, "the group takes its lines with it");
    }

    #[test]
    fn a_transparent_background_leaves_alpha_zero() {
        let world = World::new();
        world.add(Mesh::cuboid(0.5, 0.5, 0.5), Material::color(Color::WHITE));
        let picture = world.render(&front_camera(), &style(Background::Transparent), (32, 32), 1.0);
        assert_eq!(picture.at(0, 0), [0, 0, 0, 0]);
        assert_eq!(picture.at(16, 16)[3], 255);
        let straight = picture.unpremultiplied();
        assert_eq!(&straight[0..4], &[0, 0, 0, 0]);
    }
}
