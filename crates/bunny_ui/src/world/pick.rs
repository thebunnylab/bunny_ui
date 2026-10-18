//! Picking: which surface of the world a ray meets first.
//!
//! Each mesh keeps a bounding volume hierarchy, built the first time a
//! ray asks and kept for the revision — a mesh that never meets a ray
//! never pays for one. The tree splits by the surface-area heuristic
//! over twelve bins; the test of a triangle runs in `f64`, so a point
//! picked a kilometre from the origin lands where it should.
//!
//! Picking runs on the CPU, the same on every tier: no tier reads a
//! pixel back to answer a click.

use super::camera::Lens;
use super::material::AlphaMode;
use super::math::{Aabb, Plane, Ray, Vec3, vec3};
use super::mesh::{Mesh, MeshData};
use super::store::{Entity, Shape, World, WorldData};
use crate::layout::{Point, Size};

/// What a ray met.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub entity: Entity,
    /// Where, in the world.
    pub point: Vec3,
    /// The unit normal of the triangle's plane, in the world, on its
    /// front side.
    pub normal: Vec3,
    /// Meters along the ray.
    pub distance: f64,
    /// The triangle of the entity's mesh.
    pub triangle: u32,
    /// The part of the mesh the triangle belongs to.
    pub part: u16,
    /// Did the ray meet the front of the triangle?
    pub front: bool,
}

// MARK: - The tree

const BINS: usize = 12;
const LEAF: usize = 4;

#[derive(Clone, Copy, Debug)]
struct Node {
    min: [f32; 3],
    max: [f32; 3],
    /// A leaf: the first of its triangles. A branch: its right child
    /// (the left child is the next node).
    first: u32,
    /// Triangles in a leaf; 0 for a branch.
    count: u32,
}

/// A bounding volume hierarchy over a mesh's triangles.
#[derive(Clone, Debug, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    triangles: Vec<u32>,
}

fn corner(data: &MeshData, triangle: u32, k: usize) -> Vec3 {
    let p = data.positions[data.indices[triangle as usize * 3 + k] as usize];
    vec3(p[0] as f64, p[1] as f64, p[2] as f64)
}

fn component(v: Vec3, axis: usize) -> f64 {
    match axis {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

fn area(aabb: &Aabb) -> f64 {
    if aabb.is_empty() {
        return 0.0;
    }
    let d = aabb.size();
    2.0 * (d.x * d.y + d.y * d.z + d.z * d.x)
}

impl Bvh {
    pub fn build(data: &MeshData) -> Bvh {
        let count = data.indices.len() / 3;
        let boxes: Vec<Aabb> = (0..count as u32)
            .map(|t| Aabb::from_points([corner(data, t, 0), corner(data, t, 1), corner(data, t, 2)]))
            .collect();
        let centers: Vec<Vec3> = boxes.iter().map(Aabb::center).collect();
        let mut bvh = Bvh { nodes: Vec::with_capacity(count.max(1) * 2), triangles: (0..count as u32).collect() };
        if count > 0 {
            bvh.split(0, count, &boxes, &centers, 0);
        }
        bvh
    }

    /// Builds the node for `triangles[start..end]` and everything below.
    fn split(&mut self, start: usize, end: usize, boxes: &[Aabb], centers: &[Vec3], depth: u32) {
        let range = &self.triangles[start..end];
        let bounds = range.iter().fold(Aabb::EMPTY, |b, &t| b.union(boxes[t as usize]));
        let node = self.nodes.len();
        self.nodes.push(Node { min: bounds.min.to_f32(), max: bounds.max.to_f32(), first: start as u32, count: (end - start) as u32 });
        let n = end - start;
        if n <= LEAF || depth >= 60 {
            return;
        }
        let spread = range.iter().fold(Aabb::EMPTY, |b, &t| b.including(centers[t as usize]));
        let size = spread.size();
        let axis = if size.x >= size.y && size.x >= size.z { 0 } else if size.y >= size.z { 1 } else { 2 };
        let (low, high) = (component(spread.min, axis), component(spread.max, axis));
        if high - low <= 1e-12 {
            return; // every centre at one place: a leaf
        }
        let bin_of = |t: u32| -> usize {
            let at = (component(centers[t as usize], axis) - low) / (high - low) * BINS as f64;
            (at as usize).min(BINS - 1)
        };
        let mut bin_boxes = [Aabb::EMPTY; BINS];
        let mut bin_counts = [0usize; BINS];
        for &t in range {
            let bin = bin_of(t);
            bin_boxes[bin] = bin_boxes[bin].union(boxes[t as usize]);
            bin_counts[bin] += 1;
        }
        // the cost of each cut between bins: area × count on each side
        let mut best = (f64::INFINITY, 0usize);
        for cut in 1..BINS {
            let (left, right) = bin_boxes.split_at(cut);
            let (left_n, right_n): (usize, usize) = (bin_counts[..cut].iter().sum(), bin_counts[cut..].iter().sum());
            if left_n == 0 || right_n == 0 {
                continue;
            }
            let left_box = left.iter().fold(Aabb::EMPTY, |b, x| b.union(*x));
            let right_box = right.iter().fold(Aabb::EMPTY, |b, x| b.union(*x));
            let cost = area(&left_box) * left_n as f64 + area(&right_box) * right_n as f64;
            if cost < best.0 {
                best = (cost, cut);
            }
        }
        if !best.0.is_finite() || best.0 >= area(&bounds) * n as f64 {
            return; // no cut beats a leaf
        }
        let cut = best.1;
        // partition the triangles in place by bin
        let mut left = start;
        for i in start..end {
            if bin_of(self.triangles[i]) < cut {
                self.triangles.swap(i, left);
                left += 1;
            }
        }
        if left == start || left == end {
            return;
        }
        self.nodes[node].count = 0;
        self.split(start, left, boxes, centers, depth + 1);
        self.nodes[node].first = self.nodes.len() as u32;
        self.split(left, end, boxes, centers, depth + 1);
    }

    /// The nearest triangle the ray meets at a distance in `[0, limit)`,
    /// with the ray in the mesh's own space. `accept` may refuse a hit
    /// (a cut-away side) and the search goes on behind it.
    pub(crate) fn nearest(
        &self,
        data: &MeshData,
        ray: &Ray,
        limit: f64,
        accept: &dyn Fn(f64, u32) -> bool,
    ) -> Option<(f64, u32, bool)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best: Option<(f64, u32, bool)> = None;
        let mut stack = [0u32; 64];
        let mut depth = 1;
        while depth > 0 {
            depth -= 1;
            let index = stack[depth];
            let node = self.nodes[index as usize];
            let reach = best.map_or(limit, |(t, _, _)| t);
            if !slab(&node, ray, reach) {
                continue;
            }
            if node.count > 0 {
                for &triangle in &self.triangles[node.first as usize..(node.first + node.count) as usize] {
                    let (a, b, c) = (corner(data, triangle, 0), corner(data, triangle, 1), corner(data, triangle, 2));
                    if let Some((t, front)) = intersect(ray, a, b, c)
                        && t < best.map_or(limit, |(t, _, _)| t)
                        && accept(t, triangle)
                    {
                        best = Some((t, triangle, front));
                    }
                }
            } else if depth + 2 <= stack.len() {
                // a branch: its left child is the next node, the right one is named
                stack[depth] = node.first;
                stack[depth + 1] = index + 1;
                depth += 2;
            }
        }
        best
    }
}

/// Does the ray meet the node's box before `reach`?
fn slab(node: &Node, ray: &Ray, reach: f64) -> bool {
    let min = vec3(node.min[0] as f64, node.min[1] as f64, node.min[2] as f64);
    let max = vec3(node.max[0] as f64, node.max[1] as f64, node.max[2] as f64);
    // the f32 box widened by a hair: a ray along a face still counts
    let pad = (max - min).max_element().max(1.0) * 1e-6;
    match ray.hits_box(&Aabb { min: min - Vec3::splat(pad), max: max + Vec3::splat(pad) }) {
        Some((enter, _)) => enter < reach,
        None => false,
    }
}

/// Möller–Trumbore: the distance along the ray and whether it met the
/// front (counter-clockwise) side.
fn intersect(ray: &Ray, a: Vec3, b: Vec3, c: Vec3) -> Option<(f64, bool)> {
    let (e1, e2) = (b - a, c - a);
    let p = ray.direction.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-300 {
        return None;
    }
    let inv = 1.0 / det;
    let s = ray.origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = ray.direction.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    // det = −d·n: positive when the ray meets the counter-clockwise side
    (t >= 0.0).then_some((t, det > 0.0))
}

impl Mesh {
    /// The nearest triangle of this mesh on the ray (in the mesh's own
    /// space), by its tree.
    pub(crate) fn nearest_hit(&self, ray: &Ray, limit: f64, accept: &dyn Fn(f64, u32) -> bool) -> Option<(f64, u32, bool)> {
        self.bvh().nearest(self.data(), ray, limit, accept)
    }
}

// MARK: - The world

impl WorldData {
    pub(crate) fn raycast(&self, world: World, ray: &Ray, section: Option<&Plane>) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        for (id, entity) in self.iter() {
            let Shape::Model(model) = &entity.spec.shape else { continue };
            if !entity.spec.pickable || !self.shown(id) {
                continue;
            }
            let Some(matrix) = self.world_matrix(id) else { continue };
            let bounds = model.mesh.bounds().transformed(&matrix);
            let reach = best.map_or(f64::INFINITY, |hit| hit.distance);
            match ray.hits_box(&bounds) {
                Some((enter, _)) if enter < reach => {}
                _ => continue,
            }
            let Some(inverse) = matrix.inverse() else { continue };
            let local = Ray::new(inverse.transform_point(ray.origin), inverse.transform_vector(ray.direction));
            let data = model.mesh.data();
            let materials = &model.materials;
            // the parameter of an affine map is the parameter of the world ray
            let accept = |t: f64, triangle: u32| {
                if let Some(plane) = section
                    && plane.signed_distance(ray.at(t)) < 0.0
                {
                    return false;
                }
                // a glow adds light; it is no surface
                let slot = data.materials.get(data.indices[triangle as usize * 3] as usize).copied().unwrap_or(0);
                !matches!(materials.get(slot as usize).map(|m| m.alpha), Some(AlphaMode::Add))
            };
            if let Some((t, triangle, front)) = model.mesh.nearest_hit(&local, reach, &accept) {
                let (a, b, c) = (corner(data, triangle, 0), corner(data, triangle, 1), corner(data, triangle, 2));
                let n = matrix.normal_matrix();
                let local_normal = (b - a).cross(c - a);
                let normal = vec3(
                    n[0][0] * local_normal.x + n[0][1] * local_normal.y + n[0][2] * local_normal.z,
                    n[1][0] * local_normal.x + n[1][1] * local_normal.y + n[1][2] * local_normal.z,
                    n[2][0] * local_normal.x + n[2][1] * local_normal.y + n[2][2] * local_normal.z,
                )
                .normalized();
                let part = data.parts.get(data.indices[triangle as usize * 3] as usize).copied().unwrap_or(0);
                best = Some(Hit {
                    entity: Entity::from_parts(world, id),
                    point: ray.at(t),
                    normal,
                    distance: t,
                    triangle,
                    part,
                    front,
                });
            }
        }
        best
    }
}

impl World {
    /// The first surface the ray meets. The ray's direction should have
    /// unit length: the distance of the hit is measured along it.
    /// Hidden and unpickable entities are skipped; with a `section`, so
    /// is everything behind it.
    pub fn raycast(&self, ray: Ray, section: Option<Plane>) -> Option<Hit> {
        let world = *self;
        self.with(|data| data.raycast(world, &ray, section.as_ref()))
    }

    /// The surface under the point `at` of a box of `size` that shows
    /// the world through `camera` — a `Camera`, or the `State<Camera>`
    /// the box paints with.
    pub fn pick(&self, camera: impl super::view::CameraSource, at: Point, size: Size) -> Option<Hit> {
        let camera = camera.camera();
        let mut ray = camera.ray(at, size);
        if let Lens::Parallel { .. } = camera.lens {
            // parallel rays start on the eye's plane: step back far enough
            // that nothing of the world stands behind the start
            let bounds = self.bounds();
            if !bounds.is_empty() {
                let back = (bounds.center() - ray.origin).length() + bounds.half_extents().length();
                ray.origin -= ray.direction * back;
                return self.raycast(ray, None).map(|hit| Hit { distance: hit.distance - back, ..hit });
            }
        }
        self.raycast(ray, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Color;
    use crate::world::camera::Camera;
    use crate::world::material::Material;
    use crate::world::mesh::MeshData;

    struct Seed(u64);

    impl Seed {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    fn soup(seed: &mut Seed, count: usize) -> MeshData {
        let mut data = MeshData::default();
        for _ in 0..count {
            let center = vec3(seed.next() * 20.0 - 10.0, seed.next() * 20.0 - 10.0, seed.next() * 20.0 - 10.0);
            for _ in 0..3 {
                let p = center + vec3(seed.next() - 0.5, seed.next() - 0.5, seed.next() - 0.5) * 2.0;
                data.indices.push(data.positions.len() as u32);
                data.positions.push(p.to_f32());
            }
        }
        data
    }

    #[test]
    fn the_bvh_agrees_with_brute_force() {
        let mut seed = Seed(0x1234_5678_9ABC_DEF1);
        let mesh = Mesh::new(soup(&mut seed, 600));
        let data = mesh.data();
        for _ in 0..400 {
            let origin = vec3(seed.next() * 40.0 - 20.0, seed.next() * 40.0 - 20.0, seed.next() * 40.0 - 20.0);
            let direction = vec3(seed.next() - 0.5, seed.next() - 0.5, seed.next() - 0.5).normalized();
            let ray = Ray::new(origin, direction);
            let tree = mesh.nearest_hit(&ray, f64::INFINITY, &|_, _| true).map(|(t, i, _)| (t, i));
            let brute = (0..mesh.triangle_count() as u32)
                .filter_map(|i| intersect(&ray, corner(data, i, 0), corner(data, i, 1), corner(data, i, 2)).map(|(t, _)| (t, i)))
                .min_by(|a, b| a.0.total_cmp(&b.0));
            assert_eq!(tree.map(|(t, _)| t), brute.map(|(t, _)| t), "{ray:?}");
        }
    }

    #[test]
    fn a_pick_far_from_the_origin_is_exact() {
        let world = World::new();
        let far = vec3(1.0e6, 0.0, -2.0e6);
        let cube = world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::WHITE)).set_position(far);
        let camera = Camera::looking_at(far + vec3(0.0, 0.0, 5.0), far);
        let size = Size { width: 400.0, height: 300.0 };
        let hit = world.pick(camera, Point { x: 200.0, y: 150.0 }, size).expect("dead ahead");
        assert_eq!(hit.entity, cube);
        assert!((hit.point - (far + vec3(0.0, 0.0, 0.5))).length() < 1e-9, "{:?}", hit.point);
        assert!((hit.distance - 4.5).abs() < 1e-9);
        assert!((hit.normal - Vec3::Z).length() < 1e-9 && hit.front);
    }

    #[test]
    fn hidden_and_unpickable_are_skipped() {
        let world = World::new();
        let near = world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::WHITE)).set_position(vec3(0.0, 0.0, 2.0));
        let back = world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::WHITE));
        let ray = Ray::new(vec3(0.0, 0.0, 10.0), -Vec3::Z);
        assert_eq!(world.raycast(ray, None).map(|h| h.entity), Some(near));
        near.show(false);
        assert_eq!(world.raycast(ray, None).map(|h| h.entity), Some(back));
        near.show(true).set_pickable(false);
        assert_eq!(world.raycast(ray, None).map(|h| h.entity), Some(back));
        back.set_pickable(false);
        assert_eq!(world.raycast(ray, None), None);
    }

    #[test]
    fn the_section_plane_hides_hits_on_its_cut_side() {
        let world = World::new();
        let cube = world.add(Mesh::cuboid(2.0, 2.0, 2.0), Material::color(Color::WHITE).two_sided(Color::WHITE));
        let ray = Ray::new(vec3(0.0, 0.0, 10.0), -Vec3::Z);
        assert!((world.raycast(ray, None).unwrap().point.z - 1.0).abs() < 1e-12);
        // keep what lies at z < 0.25: the front face is cut, the back face shows from inside
        let section = Plane::new(vec3(0.0, 0.0, 0.25), -Vec3::Z);
        let hit = world.raycast(ray, Some(section)).expect("the far face");
        assert_eq!(hit.entity, cube);
        assert!((hit.point.z + 1.0).abs() < 1e-12, "{:?}", hit.point);
        assert!(!hit.front, "seen from inside, the far face shows its back");
    }

    #[test]
    fn a_parallel_camera_picks_along_parallel_rays() {
        let world = World::new();
        let left = world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::WHITE)).set_position(vec3(-3.0, 0.0, 0.0));
        let right = world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::WHITE)).set_position(vec3(3.0, 0.0, 0.0));
        // an eye INSIDE the scene: parallel rays still see what stands behind it
        let camera = Camera::looking_at(vec3(0.0, 0.0, -0.2), vec3(0.0, 0.0, -10.0)).parallel_height(10.0);
        let size = Size { width: 1000.0, height: 1000.0 };
        let at = |x: f64| camera.project(vec3(x, 0.0, 0.0), size).expect("a parallel lens sees everything");
        assert_eq!(world.pick(camera, at(3.0), size).map(|h| h.entity), Some(right));
        assert_eq!(world.pick(camera, at(-3.0), size).map(|h| h.entity), Some(left));
        assert_eq!(world.pick(camera, at(0.0), size), None);
    }
}
