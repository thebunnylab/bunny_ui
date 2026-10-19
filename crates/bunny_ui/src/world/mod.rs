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
pub mod frame;
pub mod governor;
#[cfg(feature = "gltf")]
pub mod gltf;
#[cfg(feature = "gltf")]
pub mod json;
pub mod material;
pub mod math;
pub mod mesh;
pub mod orbit;
pub mod pick;
pub mod polygon;
pub mod scene;
pub mod section;
pub mod shade;
pub mod shadow;
pub mod soft;
pub mod store;
pub mod texture;
pub mod view;
pub mod wire;

pub use math::{Aabb, Frustum, Mat4, Plane, Quat, Ray, Transform, Vec2, Vec3, Vec4, vec2, vec3, vec4};
pub use camera::{Camera, Flight, Lens};
#[cfg(feature = "gltf")]
pub use gltf::{Asset, AssetNode, GltfError};
pub use frame::{Background, Blend, Draw, DrawKind, Pass, Quality, ShadowDraw, ShadowFrame, WorldFrame, WorldStyle};
pub use material::{AlphaMode, FloorMap, Light, LineDepth, LineStyle, Material, Model, PointLight, Sun, SunFrame};
pub use mesh::{Lines, Mesh, MeshData};
pub use orbit::{Orbit, OrbitLimits};
pub use pick::Hit;
pub use scene::{Anchor, SceneView, scene_view};
pub use shadow::ShadowMap;
pub use soft::Picture;
pub use store::{Entity, EntityId, EntitySpec, Shape, World, WorldEdit};
pub use view::{CameraSource, Shot};

/// A fresh identity for a mesh, a set of lines or a world. One counter
/// for all of them: an identity names one thing, whatever its kind.
pub(crate) fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
