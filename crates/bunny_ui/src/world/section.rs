//! Where a section plane cuts a world: the lines of the cut, on the CPU.
//!
//! A section ([`super::frame::WorldStyle::section`]) cuts the picture in
//! the shaders: what lies beyond the plane is not drawn. A drawing also
//! wants the cut itself — the outline the plane leaves through every
//! solid — to ink it over the open faces. [`World::section_lines`] finds
//! it: every shown triangle against the plane, in `f64`, one segment
//! where the plane crosses it.

use super::math::{Plane, Vec3};
use super::mesh::Lines;
use super::store::{Shape, World};

impl World {
    /// The lines where `plane` cuts every shown surface of the world, in
    /// the world's space: add them as lines (a `LineDepth::Always` style
    /// inks them over the open faces). A surface that lies in the plane
    /// leaves no line. Reads the world.
    pub fn section_lines(&self, plane: Plane) -> Lines {
        let plane = plane.normalized();
        let mut segments = Vec::new();
        self.with(|data| {
            for (id, entity) in data.iter() {
                let Shape::Model(model) = &entity.spec.shape else { continue };
                if !data.shown(id) {
                    continue;
                }
                let Some(matrix) = data.world_matrix(id) else { continue };
                let mesh = model.mesh.data();
                let points: Vec<Vec3> = mesh
                    .positions
                    .iter()
                    .map(|p| matrix.transform_point(Vec3 { x: p[0] as f64, y: p[1] as f64, z: p[2] as f64 }))
                    .collect();
                let distances: Vec<f64> = points.iter().map(|p| plane.signed_distance(*p)).collect();
                for triangle in mesh.indices.as_chunks::<3>().0 {
                    if let Some(segment) = cut(triangle.map(|index| (points[index as usize], distances[index as usize]))) {
                        segments.push(segment);
                    }
                }
            }
        });
        Lines::new(segments)
    }
}

/// The segment where the plane crosses a triangle, given each corner and
/// its signed distance; `None` when the triangle lies on one side. A
/// corner on the plane counts with the side ahead of it, so an edge that
/// lies in the plane is drawn once, by the triangle behind it.
fn cut(corners: [(Vec3, f64); 3]) -> Option<[[f32; 3]; 2]> {
    let ahead = corners.map(|(_, d)| d >= 0.0);
    if ahead.iter().all(|a| *a) || ahead.iter().all(|a| !*a) {
        return None;
    }
    let mut ends = Vec::with_capacity(2);
    for k in 0..3 {
        let ((p, dp), (q, dq)) = (corners[k], corners[(k + 1) % 3]);
        if ahead[k] != ahead[(k + 1) % 3] {
            let t = dp / (dp - dq);
            ends.push(p.lerp(q, t));
        }
    }
    let [a, b] = [ends[0], ends[1]];
    (a.distance(b) > 1e-12).then(|| [a.to_f32(), b.to_f32()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Color;
    use crate::world::material::Material;
    use crate::world::math::vec3;
    use crate::world::mesh::Mesh;

    #[test]
    fn a_cut_through_a_box_is_its_outline() {
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::WHITE)).set_position(vec3(5.0, 0.0, -3.0));
        let lines = world.section_lines(Plane::new(vec3(0.0, 0.25, 0.0), Vec3::Y));
        let length: f64 = lines
            .segments()
            .iter()
            .map(|[a, b]| vec3(a[0] as f64, a[1] as f64, a[2] as f64).distance(vec3(b[0] as f64, b[1] as f64, b[2] as f64)))
            .sum();
        assert!((length - 4.0).abs() < 1e-5, "the four sides of the square: {length}");
        for [a, b] in lines.segments() {
            assert!((a[1] - 0.25).abs() < 1e-6 && (b[1] - 0.25).abs() < 1e-6, "on the plane");
            assert!((4.5..=5.5).contains(&a[0]) && (-3.5..=-2.5).contains(&a[2]), "in the world's space");
        }
    }

    #[test]
    fn a_plane_that_misses_or_holds_a_face_leaves_nothing_doubled() {
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::WHITE));
        assert!(world.section_lines(Plane::new(vec3(0.0, 3.0, 0.0), Vec3::Y)).segments().is_empty(), "above the box");
        // the plane holds the top face: its outline once, not twice
        let top = world.section_lines(Plane::new(vec3(0.0, 0.5, 0.0), Vec3::Y));
        let length: f64 = top
            .segments()
            .iter()
            .map(|[a, b]| vec3(a[0] as f64, a[1] as f64, a[2] as f64).distance(vec3(b[0] as f64, b[1] as f64, b[2] as f64)))
            .sum();
        assert!(length <= 4.0 + 1e-5, "no edge drawn twice: {length}");
    }

    #[test]
    fn a_hidden_entity_leaves_no_cut() {
        let world = World::new();
        world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::WHITE)).show(false);
        assert!(world.section_lines(Plane::new(Vec3::ZERO, Vec3::Y)).segments().is_empty());
    }
}
