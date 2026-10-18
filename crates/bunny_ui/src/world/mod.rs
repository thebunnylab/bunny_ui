//! Worlds in three dimensions.
//!
//! A world is a retained set of entities — meshes, materials, lights
//! and lines — placed in meters. A box paints a picture of the world
//! through a camera, and the picture enters the scene as an image: it
//! is clipped, faded and composited like any other image.
//!
//! The LAW of the pixel half applies here too. Every decision — what
//! the camera sees, the order of the draws, the near and far planes,
//! the frame of the sun's shadow — resolves on the CPU. A GPU tier
//! below is a pure evaluator. The software rasterizer of this module
//! is the oracle of every tier and the floor where no tier exists.
//!
//! The module is behind the `3d` feature. An app that draws no world
//! carries none of it.

pub mod camera;
pub mod material;
pub mod math;
pub mod mesh;
pub mod orbit;
pub mod polygon;
pub mod store;

pub use math::{Aabb, Frustum, Mat4, Plane, Quat, Ray, Transform, Vec2, Vec3, Vec4, vec2, vec3, vec4};
pub use camera::{Camera, Flight, Lens};
pub use material::{AlphaMode, Light, LineDepth, LineStyle, Material, Model, PointLight, Sun, SunFrame};
pub use mesh::{Lines, Mesh, MeshData};
pub use orbit::{Orbit, OrbitLimits};
pub use store::{Entity, EntityId, EntitySpec, Shape, World, WorldEdit};

/// A fresh identity for a mesh, a set of lines or a world. One counter
/// for all of them: an identity names one thing, whatever its kind.
pub(crate) fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
