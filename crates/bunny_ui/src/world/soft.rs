//! The software rasterizer: a world frame drawn on the CPU.
//!
//! It is the oracle every GPU tier is held against, and the floor where
//! no tier exists. It runs the frame's draws in order, with the rules a
//! GPU follows: triangles clipped against the near and far planes in
//! homogeneous space, corners snapped to 1/256 of a pixel, integer edge
//! functions with the top-left rule, depth linear on the screen,
//! attributes corrected for perspective, colour blended as an 8-bit
//! target blends it. Lines are quads of analytic coverage, as their
//! shader draws them.
//!
//! The picture is what a tier's target holds: premultiplied, sRGB-encoded
//! RGBA8, the top row first. The same frame gives the same bytes on every
//! machine.

use super::frame::{Background, Blend, DrawKind, WorldFrame};
use super::mesh::{fold_normal, unfold_normal};
use super::shade::{self, Surface, V3, decode, encode_code, quantize};
use super::wire::{FrameUniforms, InstanceData, material_flag};

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
        for pixel in out.chunks_exact_mut(4) {
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

/// Draws the frame.
pub fn draw(frame: &WorldFrame) -> Picture {
    let (width, height) = (frame.size.0.max(1), frame.size.1.max(1));
    let mut target = Target {
        width,
        height,
        color: vec![[0; 4]; (width * height) as usize],
        depth: vec![1.0; (width * height) as usize],
    };
    target.clear(frame.background);
    let projection = frame.uniforms.clip_from_eye;
    for draw in &frame.draws {
        match draw.kind {
            DrawKind::Mesh { mesh, first_index, index_count, cull_back, mirrored, blend, mask, .. } => {
                let mesh = &frame.meshes[mesh as usize];
                let corners = Corners::of(mesh);
                let indices = &mesh.data().indices[first_index as usize..(first_index + index_count) as usize];
                for instance in &frame.instances[draw.first_instance as usize..(draw.first_instance + draw.instance_count) as usize] {
                    let job = Job { frame, instance, cull_back, mirrored, blend, mask };
                    for triangle in indices.chunks_exact(3) {
                        let vertices = [0, 1, 2].map(|k| corners.vertex(triangle[k] as usize, instance, &projection));
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
}

// MARK: - The target

struct Target {
    width: u32,
    height: u32,
    /// Premultiplied, sRGB-encoded.
    color: Vec<[u8; 4]>,
    depth: Vec<f32>,
}

impl Target {
    fn clear(&mut self, background: Background) {
        match background {
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
        }
    }

    /// Blends a premultiplied colour into the pixel, the way an 8-bit
    /// target does: in floating point, rounded to the nearest code.
    fn blend(&mut self, index: usize, source: [f32; 4], blend: Blend) {
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

    fn vertex(&self, index: usize, instance: &InstanceData, projection: &[f32; 16]) -> Vertex {
        let data = self.data;
        let count = data.positions.len();
        let p = data.positions[index];
        let row = |r: [f32; 4]| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3];
        let eye = [row(instance.eye_from_model[0]), row(instance.eye_from_model[1]), row(instance.eye_from_model[2])];
        let n = self.normals[index];
        let turn = |r: [f32; 4]| r[0] * n[0] + r[1] * n[1] + r[2] * n[2];
        let normal = [turn(instance.normal[0]), turn(instance.normal[1]), turn(instance.normal[2])];
        let color = if data.colors.len() == count { data.colors[index] } else { [255; 4] };
        Vertex {
            clip: clip_of(projection, eye),
            eye,
            normal,
            color: [decode(color[0]), decode(color[1]), decode(color[2]), color[3] as f32 / 255.0],
            material: if data.materials.len() == count { data.materials[index] as u32 } else { 0 },
            occlusion: if data.occlusion.len() == count { data.occlusion[index] as f32 / 255.0 } else { 1.0 },
        }
    }
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
    /// Linear, straight alpha.
    color: [f32; 4],
    /// The slot in the instance's materials (flat across a triangle).
    material: u32,
    occlusion: f32,
}

fn lerp_vertex(a: &Vertex, b: &Vertex, t: f32) -> Vertex {
    let l = |x: f32, y: f32| x + (y - x) * t;
    Vertex {
        clip: [0, 1, 2, 3].map(|k| l(a.clip[k], b.clip[k])),
        eye: [0, 1, 2].map(|k| l(a.eye[k], b.eye[k])),
        normal: [0, 1, 2].map(|k| l(a.normal[k], b.normal[k])),
        color: [0, 1, 2, 3].map(|k| l(a.color[k], b.color[k])),
        material: a.material,
        occlusion: l(a.occlusion, b.occlusion),
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
fn edge(a: (i64, i64), b: (i64, i64), p: (i64, i64)) -> i64 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

/// A top edge (flat, going right) or a left edge (going up) of a
/// clockwise triangle: a pixel centre exactly on it belongs to it.
fn top_left(a: (i64, i64), b: (i64, i64)) -> bool {
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
                if !(0.0..=1.0).contains(&z) || z >= target.depth[index] {
                    continue;
                }
                let weights = [lambda[0] * s0.inv_w, lambda[1] * s1.inv_w, lambda[2] * s2.inv_w];
                let sum = weights[0] + weights[1] + weights[2];
                let b = weights.map(|weight| weight / sum);
                let Some(color) = self.shade(vertices, b, front, px as u32, py as u32) else { continue };
                if opaque {
                    target.depth[index] = z;
                }
                target.blend(index, color, self.blend);
            }
        }
    }

    /// The colour of a fragment, premultiplied in codes; `None` when it is
    /// discarded.
    fn shade(&self, v: [&Vertex; 3], b: [f32; 3], front: bool, x: u32, y: u32) -> Option<[f32; 4]> {
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
        let color = [0, 1, 2, 3].map(|k| v[0].color[k] * b[0] + v[1].color[k] * b[1] + v[2].color[k] * b[2]);
        let two_sided = material.back[3] > 0.5;
        let surface = if front || !two_sided { material.base } else { [material.back[0], material.back[1], material.back[2], material.base[3]] };
        let base = [0, 1, 2].map(|k| surface[k] * decode(tint[k]) * color[k]);
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
                    groups: u32::MAX,
                },
            );
            let dither = if u.tone[2] > 0.5 { shade::bayer(x, y) } else { 0.0 };
            (shade::tone(lit, u.tone[0], u.tone[1]), dither)
        };
        let codes = linear.map(|c| quantize(encode_code(c) * alpha, dither) as f32);
        Some([codes[0], codes[1], codes[2], (alpha * 255.0 + 0.5).floor()])
    }
}

/// Snaps a pixel coordinate to 1/256, rounding half to even.
fn snap(value: f32) -> i64 {
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
            target.blend(index, source, Blend::Alpha);
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
    use crate::world::math::{Plane, Vec3, vec3};
    use crate::world::mesh::{Mesh, MeshData};
    use crate::world::store::World;

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
        assert_eq!(fnv(&first.rgba), 12_363_781_133_140_793_577, "the picture's fingerprint moved");
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
