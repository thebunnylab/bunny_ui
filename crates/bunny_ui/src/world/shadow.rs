//! The sun's shadow: the map a frame asks for, fitted to what casts, and
//! the same map drawn and read on the CPU the way a GPU draws and reads
//! it.
//!
//! The map is cached. Its generation hashes only what the map holds —
//! the casters, their meshes and the sun — so a camera that pans, turns
//! or zooms under a sun fixed to the world draws no shadow again. The
//! map covers the casters as the sun sees them: its side climbs a ladder
//! of sizes, and its centre sits on its own texel grid, so a caster that
//! moves inside the fit leaves every other shadow where it was, to the
//! texel.
//!
//! A surface reads the map at its own point pushed off along its normal
//! — further as it turns away from the sun — and a short step toward the
//! sun, so it never shades itself.

use std::cell::RefCell;
use std::rc::Rc;

use super::frame::WorldFrame;
use super::math::{Aabb, Mat4, Vec3, vec4};
use super::shade::{V3, add, dot, saturate, scale};
use super::soft::{edge, snap, top_left};
use super::wire::FrameUniforms;

/// Texels kept free around the casters on each side: room for the
/// widest lookup and the push along the normal.
const MARGIN: f64 = 4.0;

/// How far a surface's point is pushed off along its normal, in texels,
/// for a number of lookups: past the farthest texel a lookup reads, across
/// a texel's diagonal. The push shrinks to nothing on a surface that
/// faces the sun.
pub(crate) fn reach(taps: u32) -> f64 {
    match taps {
        0 | 1 => 1.5,
        2..=4 => 2.25,
        _ => 3.0,
    }
}

/// The step a surface takes toward the sun before it compares, in texels
/// and in codes of the map's 16 bits.
const STEP_TEXELS: f64 = 0.5;
const STEP_CODES: f64 = 2.0;

/// The depth bias of a fit: the step toward the sun, in the map's depth.
pub(crate) fn depth_bias(fit: &Fit) -> f64 {
    STEP_TEXELS * fit.texel / fit.depth + STEP_CODES / 65535.0
}

/// The sun's own view: it looks along the light, which travels along
/// `travel` (a unit vector in the world).
pub(crate) fn light_view(travel: Vec3) -> Mat4 {
    let up = if travel.y.abs() < 0.99 { Vec3::Y } else { Vec3::Z };
    Mat4::look_at(Vec3::ZERO, travel, up)
}

/// The map's frame around the casters.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fit {
    /// World → map: `x` and `y` across the map in `[-1, 1]`, `y` up; `z`
    /// the depth from the sun in `[0, 1]`.
    pub map_from_world: Mat4,
    /// Meters a texel spans.
    pub texel: f64,
    /// Meters the depth spans.
    pub depth: f64,
}

impl Fit {
    /// The frame of a map `size` texels a side around `bounds`, the
    /// casters as the sun's view `light` sees them.
    pub(crate) fn around(light: &Mat4, bounds: &Aabb, size: u32) -> Fit {
        let size = size.max(1) as f64;
        let side = (bounds.max.x - bounds.min.x).max(bounds.max.y - bounds.min.y).max(1e-3);
        let extent = ladder(side * (1.0 + 2.0 * MARGIN / size));
        let texel = extent / size;
        let on_grid = |value: f64| (value / texel).round() * texel;
        let (cx, cy) = (on_grid((bounds.min.x + bounds.max.x) * 0.5), on_grid((bounds.min.y + bounds.max.y) * 0.5));
        // the view looks down −z: the depth from the sun is −z
        let (near, far) = (-bounds.max.z, -bounds.min.z);
        let margin = ((far - near) * 0.01).max(2.0 * texel);
        let (near, far) = (near - margin, far + margin);
        let depth = far - near;
        let half = extent * 0.5;
        let fit = Mat4::from_cols(
            vec4(1.0 / half, 0.0, 0.0, 0.0),
            vec4(0.0, 1.0 / half, 0.0, 0.0),
            vec4(0.0, 0.0, -1.0 / depth, 0.0),
            vec4(-cx / half, -cy / half, -near / depth, 1.0),
        );
        Fit { map_from_world: fit * *light, texel, depth }
    }
}

/// The smallest step of the ladder `2^(k/4)` at or above `x`: the map's
/// side moves only when the casters outgrow a step. Exact on every
/// machine: the power of two comes from the bits.
fn ladder(x: f64) -> f64 {
    const STEPS: [f64; 4] = [1.0, 1.189_207_115_002_721, std::f64::consts::SQRT_2, 1.681_792_830_507_429];
    let x = x.max(f64::MIN_POSITIVE);
    let power = f64::from_bits(x.to_bits() & (0x7FF << 52));
    STEPS.iter().map(|step| power * step).find(|value| *value >= x).unwrap_or(power * 2.0)
}

// MARK: - The map on the CPU

/// The sun's shadow map as the CPU draws it — what a tier's depth target
/// holds: 16-bit depths from the sun, its first row the top of the map
/// (`y = +1`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowMap {
    pub size: u32,
    pub depth: Vec<u16>,
}

impl ShadowMap {
    /// The frame's map, drawn as a GPU draws it: each caster's triangles,
    /// both faces, corners snapped to 1/256 of a texel, the top-left rule,
    /// the nearest depth kept. `None` for a frame without a shadow.
    pub fn draw(frame: &WorldFrame) -> Option<ShadowMap> {
        let shadow = frame.shadow.as_ref()?;
        let size = shadow.size.max(1);
        let mut map = ShadowMap { size, depth: vec![u16::MAX; size as usize * size as usize] };
        for draw in &shadow.draws {
            let Some(mesh) = frame.meshes.get(draw.mesh as usize) else { continue };
            let data = mesh.data();
            let Some(indices) = data.indices.get(draw.first_index as usize..(draw.first_index + draw.index_count) as usize) else { continue };
            for rows in shadow.instances.iter().skip(draw.first_instance as usize).take(draw.instance_count as usize) {
                let place = |p: &[f32; 3]| {
                    let row = |r: [f32; 4]| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3];
                    [row(rows[0]), row(rows[1]), row(rows[2])]
                };
                let corners: Vec<[f32; 3]> = data.positions.iter().map(place).collect();
                for triangle in indices.as_chunks::<3>().0 {
                    map.fill(triangle.map(|index| corners[index as usize]));
                }
            }
        }
        Some(map)
    }

    fn fill(&mut self, corners: [[f32; 3]; 3]) {
        let side = self.size as f32;
        let screen = corners.map(|c| (snap((c[0] + 1.0) * 0.5 * side), snap((1.0 - c[1]) * 0.5 * side), c[2]));
        let area = edge((screen[0].0, screen[0].1), (screen[1].0, screen[1].1), (screen[2].0, screen[2].1));
        if area == 0 {
            return;
        }
        // both faces cast: the corners turned so the area reads positive
        let [a, b, c] = if area > 0 { screen } else { [screen[0], screen[2], screen[1]] };
        let inv_area = 1.0 / area.abs() as f64;
        let last = self.size as i64 - 1;
        let (min_x, max_x) = (a.0.min(b.0).min(c.0), a.0.max(b.0).max(c.0));
        let (min_y, max_y) = (a.1.min(b.1).min(c.1), a.1.max(b.1).max(c.1));
        let (x0, x1) = (((min_x - 128) >> 8).max(0), ((max_x - 128) >> 8).min(last));
        let (y0, y1) = (((min_y - 128) >> 8).max(0), ((max_y - 128) >> 8).min(last));
        if x0 > x1 || y0 > y1 {
            return;
        }
        let edges = [((b.0, b.1), (c.0, c.1)), ((c.0, c.1), (a.0, a.1)), ((a.0, a.1), (b.0, b.1))];
        let owns = edges.map(|(from, to)| top_left(from, to));
        for py in y0..=y1 {
            for px in x0..=x1 {
                let at = (px * 256 + 128, py * 256 + 128);
                let e = edges.map(|(from, to)| edge(from, to, at));
                if (0..3).any(|k| e[k] < 0 || (e[k] == 0 && !owns[k])) {
                    continue;
                }
                let lambda = e.map(|value| (value as f64 * inv_area) as f32);
                let z = lambda[0] * a.2 + lambda[1] * b.2 + lambda[2] * c.2;
                if !(0.0..=1.0).contains(&z) {
                    continue;
                }
                let code = (z * 65535.0 + 0.5).floor() as u16;
                let index = py as usize * self.size as usize + px as usize;
                if code < self.depth[index] {
                    self.depth[index] = code;
                }
            }
        }
    }

    /// One lookup of a comparing sampler that filters: the four texels
    /// around `(u, v)` each compared — lit where `reference ≤ depth` — and
    /// the four answers blended by the point's place among them, in the
    /// eight bits of fraction a texture unit keeps. Past the edge the edge
    /// texels answer.
    pub fn lookup(&self, u: f32, v: f32, reference: f32) -> f32 {
        let side = self.size as f32;
        let (x, y) = (u * side - 0.5, v * side - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let fraction = |f: f32| (f * 256.0).round() / 256.0;
        let (fx, fy) = (fraction(x - x0), fraction(y - y0));
        let last = self.size as i64 - 1;
        let lit = |i: i64, j: i64| {
            let (i, j) = (i.clamp(0, last), j.clamp(0, last));
            let stored = self.depth[(j * self.size as i64 + i) as usize] as f32 / 65535.0;
            if reference <= stored { 1.0 } else { 0.0 }
        };
        let (i, j) = (x0 as i64, y0 as i64);
        let top = lit(i, j) + (lit(i + 1, j) - lit(i, j)) * fx;
        let bottom = lit(i, j + 1) + (lit(i + 1, j + 1) - lit(i, j + 1)) * fx;
        top + (bottom - top) * fy
    }
}

/// How much of the sun reaches a point at `position` with the normal
/// `n`, both in the eye's space, the normal on the side the eye sees: 1
/// in full light, 0 in shadow, a share in the penumbra. Step for step
/// what the shaders compute.
pub fn visibility(u: &FrameUniforms, map: &ShadowMap, position: V3, n: V3) -> f32 {
    let taps = u.shadow[3];
    if taps < 0.5 || u.sun_direction[3] <= 0.5 {
        return 1.0;
    }
    let n_dot_l = saturate(dot(n, [u.sun_direction[0], u.sun_direction[1], u.sun_direction[2]]));
    if n_dot_l <= 0.0 || n_dot_l.is_nan() {
        return 1.0;
    }
    let sine = (1.0 - n_dot_l * n_dot_l).max(0.0).sqrt();
    let p = add(position, scale(n, u.shadow[1] * sine));
    let m = &u.shadow_from_eye;
    let s = [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ];
    if !(s[0] > 0.0 && s[0] < 1.0 && s[1] > 0.0 && s[1] < 1.0) {
        return 1.0;
    }
    let reference = (s[2] - u.shadow[2]).clamp(0.0, 1.0);
    let texel = u.shadow[0];
    if taps < 1.5 {
        return map.lookup(s[0], s[1], reference);
    }
    if taps < 4.5 {
        let mut sum = 0.0;
        sum += map.lookup(s[0] - 0.5 * texel, s[1] - 0.5 * texel, reference);
        sum += map.lookup(s[0] + 0.5 * texel, s[1] - 0.5 * texel, reference);
        sum += map.lookup(s[0] - 0.5 * texel, s[1] + 0.5 * texel, reference);
        sum += map.lookup(s[0] + 0.5 * texel, s[1] + 0.5 * texel, reference);
        return sum * 0.25;
    }
    let mut sum = 0.0;
    for j in -1..=1 {
        for i in -1..=1 {
            sum += map.lookup(s[0] + i as f32 * texel, s[1] + j as f32 * texel, reference);
        }
    }
    sum / 9.0
}

/// How many maps the CPU keeps drawn.
const MAPS_KEPT: usize = 4;

/// A map the CPU drew, by its generation and side.
type Kept = ((u64, u32), Rc<ShadowMap>);

thread_local! {
    /// The maps the CPU drew last: a camera that moves under a still sun
    /// draws its map once.
    static MAPS: RefCell<Vec<Kept>> = const { RefCell::new(Vec::new()) };
}

/// The frame's map, drawn once per generation.
pub(crate) fn map_of(frame: &WorldFrame) -> Option<Rc<ShadowMap>> {
    let shadow = frame.shadow.as_ref()?;
    let key = (shadow.generation, shadow.size);
    if let Some(hit) = MAPS.with(|maps| maps.borrow().iter().find(|(held, _)| *held == key).map(|(_, map)| Rc::clone(map))) {
        return Some(hit);
    }
    let map = Rc::new(ShadowMap::draw(frame)?);
    MAPS.with(|maps| {
        let mut maps = maps.borrow_mut();
        if maps.len() >= MAPS_KEPT {
            maps.remove(0);
        }
        maps.push((key, Rc::clone(&map)));
    });
    Some(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::math::vec3;

    #[test]
    fn the_ladder_climbs_in_quarter_octaves() {
        assert_eq!(ladder(1.0), 1.0);
        assert_eq!(ladder(8.0), 8.0);
        assert_eq!(ladder(1.1), 1.189_207_115_002_721);
        assert_eq!(ladder(5.0), 4.0 * std::f64::consts::SQRT_2);
        assert_eq!(ladder(7.0), 8.0, "past the last step, the next power");
        assert_eq!(ladder(0.3), 0.25 * std::f64::consts::SQRT_2, "below one, the same ladder");
    }

    #[test]
    fn a_fit_holds_its_casters_with_room_and_sits_on_its_grid() {
        let light = light_view(vec3(-1.0, -2.0, -1.5).normalized());
        let bounds = Aabb::new(vec3(-1.3, 0.7, -12.0), vec3(2.1, 3.9, -8.0));
        let fit = Fit::around(&light, &bounds, 1024);
        let map_from_light = fit.map_from_world * light.inverse().expect("a rotation");
        // two texels of room at least, a texel being 2/1024 of the map
        let room = 1.0 - 4.0 / 1024.0;
        for corner in bounds.corners() {
            let at = map_from_light.transform_point(corner);
            assert!(at.x.abs() < room && at.y.abs() < room, "a corner near the edge: {at:?}");
            assert!(at.z > 0.0 && at.z < 1.0, "a corner past the depth: {at:?}");
        }
        // a caster that moves less than a texel moves no grid line
        let nudged = Aabb::new(bounds.min + vec3(0.3 * fit.texel, 0.0, 0.0), bounds.max + vec3(0.3 * fit.texel, 0.0, 0.0));
        let again = Fit::around(&light, &nudged, 1024);
        assert_eq!(again.texel, fit.texel);
        // the sun's origin sits a whole number of texels from the centre
        for fit in [fit, again] {
            let texels = (fit.map_from_world * light.inverse().unwrap()).transform_point(Vec3::ZERO).x * 512.0;
            assert!((texels - texels.round()).abs() < 1e-6, "the grid sits off its texels: {texels}");
        }
    }

    fn map_of_size(size: u32, fill: u16) -> ShadowMap {
        ShadowMap { size, depth: vec![fill; (size * size) as usize] }
    }

    #[test]
    fn a_lookup_compares_four_texels_and_blends_the_answers() {
        let mut map = map_of_size(4, u16::MAX);
        // the texel at column 1, row 1 holds a caster at half depth
        map.depth[4 + 1] = 32768;
        let at = |i: f32, j: f32| ((i + 0.5) / 4.0, (j + 0.5) / 4.0);
        let (u, v) = at(1.0, 1.0);
        assert_eq!(map.lookup(u, v, 0.75), 0.0, "on the caster's texel: in shadow");
        assert_eq!(map.lookup(u, v, 0.25), 1.0, "in front of the caster: lit");
        let (u, v) = at(1.5, 1.0);
        assert_eq!(map.lookup(u, v, 0.75), 0.5, "halfway to the next texel: half");
        let (u, v) = at(3.0, 3.0);
        assert_eq!(map.lookup(u, v, 1.0), 1.0, "an empty map lights even the farthest point");
    }
}
