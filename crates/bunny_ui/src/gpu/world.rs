//! The seam a GPU tier fills to draw worlds itself.
//!
//! The walk meets a world picture ([`ImageSource::World`]) and asks the
//! ground for its world door ([`super::walk::AtlasGround::world`]). A
//! tier that has one gets a [`WorldJob`] when the picture moved: a target
//! it keeps for the box, the frame the core decided, and the tier's own
//! handles for every mesh, texture and set of segments the frame names —
//! uploaded once per identity and generation by the [`WorldResidency`]
//! here. The tier draws the jobs into their targets before its 2D pass,
//! and the 2D pass composites each target like a feed, with the box's
//! veil ([`super::walk::RunKind::World`]).
//!
//! The LAW is the walk's: every decision about what to keep and when to
//! let it go is made here, on the CPU. A tier only moves bytes and mints
//! handles. A tier with no world door — or one that could not make what
//! a picture needs — shows the oracle's picture through the live road,
//! so every tier shows every world.
//!
//! A target holds premultiplied, sRGB-encoded RGBA8, its first row at
//! the top: what the CPU rasterizer writes.

use std::collections::HashMap;
use std::rc::Rc;

use crate::image_engine::{ImageEngine, ImageSource};
use crate::world::frame::WorldFrame;
use crate::world::mesh::{Lines, Mesh};
use crate::world::view::Shot;
use crate::world::wire::segment_bytes;

/// What a tier can do for worlds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldCaps {
    /// The most samples a pixel a target may take: 1 or 4.
    pub samples: u8,
}

/// The door a tier opens for worlds. Each call moves bytes or mints a
/// handle; `None` from a `make` means the tier could not, and the walk
/// shows the oracle's picture for that box this frame.
pub trait WorldGround {
    fn world_caps(&self) -> WorldCaps;
    /// A target of `width × height`: the resolved picture the 2D pass
    /// samples, with the depth and multisample storage the tier keeps
    /// beside it.
    fn make_target(&mut self, width: u32, height: u32, samples: u8) -> Option<u64>;
    fn drop_target(&mut self, id: u64);
    /// A mesh: vertices in the wire layout, `u32` indices, little-endian.
    fn make_mesh(&mut self, vertices: &[u8], indices: &[u8]) -> Option<u64>;
    fn drop_mesh(&mut self, id: u64);
    /// A texture and its whole chain: straight sRGB RGBA8, one slice per
    /// level, the base first, each level half the one above (at least 1).
    fn make_texture(&mut self, width: u32, height: u32, levels: &[&[u8]]) -> Option<u64>;
    fn drop_texture(&mut self, id: u64);
    /// A set of segments, [`crate::world::wire::SEGMENT_STRIDE`] bytes each.
    fn make_segments(&mut self, segments: &[u8]) -> Option<u64>;
    fn drop_segments(&mut self, id: u64);
}

/// One world picture a tier draws this frame, into one target.
#[derive(Clone)]
pub struct WorldJob {
    /// The target's handle.
    pub target: u64,
    /// The target's size in pixels — the frame's size.
    pub size: (u32, u32),
    /// The samples a pixel to draw with.
    pub samples: u8,
    pub frame: Rc<WorldFrame>,
    /// The tier's handle for each of `frame.meshes`.
    pub meshes: Vec<u64>,
    /// The tier's handle for each of `frame.textures`; `None` while the
    /// platform has not decoded it — the surface shows its colour, as the
    /// oracle's does.
    pub textures: Vec<Option<u64>>,
    /// The tier's handle for each of `frame.lines`.
    pub segments: Vec<u64>,
}

/// A world picture on a tier that draws worlds: the target the 2D pass
/// composites, and the job that draws into it when the picture moved.
pub(crate) struct ResolvedWorld {
    pub target: u64,
    pub size: (u32, u32),
    pub job: Option<WorldJob>,
}

/// Walks a target may sit unread before it is given back.
pub const TARGET_IDLE_WALKS: u64 = 120;
/// Walks a mesh, a texture or a segment set may sit unread.
pub const ASSET_IDLE_WALKS: u64 = 600;

struct Target {
    id: u64,
    size: (u32, u32),
    samples: u8,
    /// The generation the target holds; `None` on a new target, which
    /// holds nothing yet.
    holds: Option<u64>,
    walk: u64,
}

struct Asset {
    id: u64,
    generation: u64,
    walk: u64,
}

/// What a ground holds for worlds, and when to let it go.
#[derive(Default)]
pub struct WorldResidency {
    targets: HashMap<u64, Target>,
    meshes: HashMap<u64, Asset>,
    textures: HashMap<u64, Asset>,
    segments: HashMap<u64, Asset>,
    /// The walk in progress.
    walk: u64,
    /// What this walk's jobs leave in their targets — kept apart until
    /// the tier says it drew them, so a walk the atlas cuts short, or a
    /// frame that never reached the GPU, draws its worlds again.
    pending: Vec<(u64, u64)>,
}

impl WorldResidency {
    /// A walk begins: whatever the last attempt promised is forgotten.
    pub(crate) fn begin_walk(&mut self, walk: u64) {
        self.walk = walk;
        self.pending.clear();
    }

    /// The tier encoded the walk's jobs: their targets hold what they
    /// draw. A tier that never says so draws every world on every frame
    /// — slower, never wrong.
    pub fn drawn(&mut self) {
        for (slot, generation) in self.pending.drain(..) {
            if let Some(target) = self.targets.get_mut(&slot) {
                target.holds = Some(generation);
            }
        }
    }

    /// The device is gone, and every handle with it: nothing is dropped,
    /// everything is made again on the next walk.
    pub fn forget_all(&mut self) {
        self.targets.clear();
        self.meshes.clear();
        self.textures.clear();
        self.segments.clear();
        self.pending.clear();
    }

    /// How many targets, meshes, textures and segment sets are held.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        (self.targets.len(), self.meshes.len(), self.textures.len(), self.segments.len())
    }

    /// The road of one world picture painted into a box of `box_px`
    /// device pixels: its target, and a job when the target does not hold
    /// the picture yet. `None` when the tier could not make what the
    /// picture needs.
    pub(crate) fn resolve(
        &mut self,
        ground: &mut dyn WorldGround,
        images: &dyn ImageEngine,
        shot: &Shot,
        slot: u64,
        generation: u64,
        box_px: (u32, u32),
    ) -> Option<ResolvedWorld> {
        let quality = shot.style().quality;
        let size = quality.target_size(box_px);
        let samples = quality.samples().min(ground.world_caps().samples).max(1);
        let walk = self.walk;
        let target = match self.targets.get_mut(&slot) {
            Some(target) if target.size == size && target.samples == samples => {
                target.walk = walk;
                target
            }
            held => {
                if let Some(old) = held {
                    ground.drop_target(old.id);
                }
                let id = ground.make_target(size.0, size.1, samples)?;
                self.targets.insert(slot, Target { id, size, samples, holds: None, walk });
                self.targets.get_mut(&slot).expect("the target just made")
            }
        };
        let id = target.id;
        if target.holds == Some(generation) {
            return Some(ResolvedWorld { target: id, size, job: None });
        }
        let frame = shot.frame(size);
        let mut meshes = Vec::with_capacity(frame.meshes.len());
        for mesh in &frame.meshes {
            meshes.push(self.mesh(ground, mesh)?);
        }
        let mut segments = Vec::with_capacity(frame.lines.len());
        for lines in &frame.lines {
            segments.push(self.segments(ground, lines)?);
        }
        let textures: Vec<Option<u64>> = frame.textures.iter().map(|source| self.texture(ground, source, images)).collect();
        // a texture still decoding shows its colour this frame; the
        // target is not marked, so the next walk draws it again
        if textures.iter().all(Option::is_some) {
            self.pending.push((slot, generation));
        }
        Some(ResolvedWorld { target: id, size, job: Some(WorldJob { target: id, size, samples, frame, meshes, textures, segments }) })
    }

    fn mesh(&mut self, ground: &mut dyn WorldGround, mesh: &Mesh) -> Option<u64> {
        let walk = self.walk;
        if let Some(asset) = self.meshes.get_mut(&mesh.id()) {
            if asset.generation == mesh.generation() {
                asset.walk = walk;
                return Some(asset.id);
            }
            ground.drop_mesh(asset.id);
            self.meshes.remove(&mesh.id());
        }
        let id = ground.make_mesh(&mesh.vertex_bytes(), &mesh.index_bytes())?;
        self.meshes.insert(mesh.id(), Asset { id, generation: mesh.generation(), walk });
        Some(id)
    }

    fn texture(&mut self, ground: &mut dyn WorldGround, source: &ImageSource, images: &dyn ImageEngine) -> Option<u64> {
        let walk = self.walk;
        let key = source.key();
        if let Some(asset) = self.textures.get_mut(&key) {
            asset.walk = walk;
            return Some(asset.id);
        }
        let texture = crate::world::texture::texture_of(images, source)?;
        let levels: Vec<&[u8]> = texture.levels.iter().map(|level| level.rgba.as_slice()).collect();
        let base = &texture.levels[0];
        let id = ground.make_texture(base.width, base.height, &levels)?;
        self.textures.insert(key, Asset { id, generation: 0, walk });
        Some(id)
    }

    fn segments(&mut self, ground: &mut dyn WorldGround, lines: &Lines) -> Option<u64> {
        let walk = self.walk;
        if let Some(asset) = self.segments.get_mut(&lines.id()) {
            if asset.generation == lines.generation() {
                asset.walk = walk;
                return Some(asset.id);
            }
            ground.drop_segments(asset.id);
            self.segments.remove(&lines.id());
        }
        let id = ground.make_segments(&segment_bytes(lines.segments()))?;
        self.segments.insert(lines.id(), Asset { id, generation: lines.generation(), walk });
        Some(id)
    }

    /// Gives back what no walk read for a while. The walk asks at its end.
    pub(crate) fn retire(&mut self, ground: &mut dyn WorldGround) {
        let walk = self.walk;
        let idle = |since: u64, limit: u64| walk.wrapping_sub(since) > limit;
        self.targets.retain(|_, target| {
            let keep = !idle(target.walk, TARGET_IDLE_WALKS);
            if !keep {
                ground.drop_target(target.id);
            }
            keep
        });
        type Drop = fn(&mut dyn WorldGround, u64);
        let maps: [(&mut HashMap<u64, Asset>, Drop); 3] = [
            (&mut self.meshes, |ground, id| ground.drop_mesh(id)),
            (&mut self.textures, |ground, id| ground.drop_texture(id)),
            (&mut self.segments, |ground, id| ground.drop_segments(id)),
        ];
        for (map, drop) in maps {
            map.retain(|_, asset| {
                let keep = !idle(asset.walk, ASSET_IDLE_WALKS);
                if !keep {
                    drop(ground, asset.id);
                }
                keep
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::image_engine::RawImages;
    use crate::layout::Color;
    use crate::world::frame::{Quality, WorldStyle};
    use crate::world::material::{LineStyle, Material};
    use crate::world::math::Vec3;
    use crate::world::camera::Camera;
    use crate::world::store::{World, WorldData};

    /// A ground that keeps a ledger of what it made and gave back.
    #[derive(Default)]
    struct Ledger {
        next: u64,
        live: HashSet<u64>,
        made: Vec<&'static str>,
        dropped: Vec<&'static str>,
        refuse: bool,
        samples: u8,
    }

    impl Ledger {
        fn new() -> Ledger {
            Ledger { samples: 4, ..Ledger::default() }
        }

        fn mint(&mut self, what: &'static str) -> Option<u64> {
            if self.refuse {
                return None;
            }
            self.next += 1;
            self.live.insert(self.next);
            self.made.push(what);
            Some(self.next)
        }

        fn give_back(&mut self, what: &'static str, id: u64) {
            assert!(self.live.remove(&id), "{what} {id} given back twice, or never made");
            self.dropped.push(what);
        }

        fn count(list: &[&'static str], what: &str) -> usize {
            list.iter().filter(|item| **item == what).count()
        }
    }

    impl WorldGround for Ledger {
        fn world_caps(&self) -> WorldCaps {
            WorldCaps { samples: self.samples }
        }
        fn make_target(&mut self, _: u32, _: u32, _: u8) -> Option<u64> {
            self.mint("target")
        }
        fn drop_target(&mut self, id: u64) {
            self.give_back("target", id);
        }
        fn make_mesh(&mut self, vertices: &[u8], indices: &[u8]) -> Option<u64> {
            assert!(!vertices.is_empty() && !indices.is_empty());
            self.mint("mesh")
        }
        fn drop_mesh(&mut self, id: u64) {
            self.give_back("mesh", id);
        }
        fn make_texture(&mut self, _: u32, _: u32, _: &[&[u8]]) -> Option<u64> {
            self.mint("texture")
        }
        fn drop_texture(&mut self, id: u64) {
            self.give_back("texture", id);
        }
        fn make_segments(&mut self, _: &[u8]) -> Option<u64> {
            self.mint("segments")
        }
        fn drop_segments(&mut self, id: u64) {
            self.give_back("segments", id);
        }
    }

    fn shot(world: &World, quality: Quality) -> Shot {
        let style = WorldStyle { quality, ..WorldStyle::default() };
        Shot::new(world.with(WorldData::clone), Camera::orbiting(Vec3::ZERO, 30.0, 25.0, 5.0), style, 0.0, (100.0, 80.0))
    }

    fn cube_world() -> (World, crate::world::store::Entity) {
        let world = World::new();
        let cube = world.add(crate::world::mesh::Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(200, 90, 50)));
        (world, cube)
    }

    #[test]
    fn a_held_picture_makes_no_job() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        residency.begin_walk(1);
        let first = residency.resolve(&mut ground, &images, &shot(&world, Quality::Medium), 7, 100, (200, 160)).expect("a target");
        let job = first.job.expect("a new target draws");
        assert_eq!((job.size, job.samples, job.meshes.len()), ((200, 160), 4, 1));
        residency.drawn();
        residency.begin_walk(2);
        let again = residency.resolve(&mut ground, &images, &shot(&world, Quality::Medium), 7, 100, (200, 160)).expect("the target");
        assert!(again.job.is_none(), "the target holds the picture");
        assert_eq!(again.target, first.target);
        residency.begin_walk(3);
        let moved = residency.resolve(&mut ground, &images, &shot(&world, Quality::Medium), 7, 101, (200, 160)).expect("the target");
        assert!(moved.job.is_some(), "a new generation draws");
        assert_eq!(moved.target, first.target, "into the same target");
        assert_eq!(Ledger::count(&ground.made, "mesh"), 1, "the mesh went up once");
    }

    #[test]
    fn a_picture_that_never_reached_the_gpu_draws_again() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        residency.begin_walk(1);
        assert!(residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 100, (64, 64)).unwrap().job.is_some());
        // no `drawn`: the walk was cut short, or the frame never encoded
        residency.begin_walk(2);
        assert!(residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 100, (64, 64)).unwrap().job.is_some());
    }

    #[test]
    fn a_new_size_or_sample_count_takes_a_new_target() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        residency.begin_walk(1);
        let small = residency.resolve(&mut ground, &images, &shot(&world, Quality::Medium), 7, 100, (64, 64)).unwrap();
        residency.drawn();
        let big = residency.resolve(&mut ground, &images, &shot(&world, Quality::Medium), 7, 100, (96, 64)).unwrap();
        assert_ne!(small.target, big.target);
        assert!(big.job.is_some());
        let low = residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 100, (96, 64)).unwrap();
        assert_eq!(low.job.expect("one sample").samples, 1);
        assert_eq!(Ledger::count(&ground.dropped, "target"), 2, "each old target was given back");
        assert_eq!(residency.counts().0, 1);
    }

    #[test]
    fn the_samples_follow_the_quality_and_the_tier() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        ground.samples = 1;
        let images = RawImages::default();
        residency.begin_walk(1);
        let job = residency.resolve(&mut ground, &images, &shot(&world, Quality::High), 7, 100, (64, 64)).unwrap().job.unwrap();
        assert_eq!(job.samples, 1, "a tier of one sample draws one");
    }

    #[test]
    fn a_revised_mesh_goes_up_again_and_the_old_comes_down() {
        let world = World::new();
        let mesh = crate::world::mesh::Mesh::cuboid(1.0, 1.0, 1.0);
        let entity = world.add(mesh.clone(), Material::color(Color::rgb(200, 90, 50)));
        world.add_lines(mesh.edges(30.0), LineStyle::solid(Color::rgb(0, 0, 0), 1.0));
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        residency.begin_walk(1);
        residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 1, (64, 64)).unwrap();
        residency.drawn();
        let taller = mesh.revise(crate::world::mesh::Mesh::cuboid(1.0, 2.0, 1.0).data().clone());
        entity.set_shape(crate::world::store::Shape::Model(crate::world::material::Model::single(taller, Material::color(Color::rgb(200, 90, 50)))));
        residency.begin_walk(2);
        let job = residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 2, (64, 64)).unwrap().job.unwrap();
        assert_eq!(job.meshes.len(), 1);
        assert_eq!(Ledger::count(&ground.made, "mesh"), 2);
        assert_eq!(Ledger::count(&ground.dropped, "mesh"), 1, "the first generation came down");
        assert_eq!(Ledger::count(&ground.made, "segments"), 1, "the lines did not move");
    }

    #[test]
    fn what_no_walk_reads_is_given_back() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let images = RawImages::default();
        residency.begin_walk(1);
        residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 1, (64, 64)).unwrap();
        residency.drawn();
        residency.begin_walk(1 + TARGET_IDLE_WALKS + 1);
        residency.retire(&mut ground);
        assert_eq!(residency.counts(), (0, 1, 0, 0), "the target went first");
        residency.begin_walk(1 + ASSET_IDLE_WALKS + 1);
        residency.retire(&mut ground);
        assert_eq!(residency.counts(), (0, 0, 0, 0));
        assert!(ground.live.is_empty(), "everything made was given back");
    }

    #[test]
    fn a_tier_that_cannot_make_a_target_shows_the_oracle() {
        let (world, _) = cube_world();
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        ground.refuse = true;
        let images = RawImages::default();
        residency.begin_walk(1);
        assert!(residency.resolve(&mut ground, &images, &shot(&world, Quality::Low), 7, 1, (64, 64)).is_none());
    }

    #[test]
    fn a_huge_box_is_drawn_inside_the_budget() {
        assert_eq!(Quality::Low.target_size((800, 600)), (800, 600));
        let (w, h) = Quality::Low.target_size((3840, 2160));
        assert!(w * h <= Quality::Low.pixel_budget() && w * h > Quality::Low.pixel_budget() * 99 / 100);
        assert!(((w as f64 / h as f64) - 16.0 / 9.0).abs() < 0.01, "the shape holds");
        let (mut ground, mut residency) = (Ledger::new(), WorldResidency::default());
        let (world, _) = cube_world();
        residency.begin_walk(1);
        let picture = residency.resolve(&mut ground, &RawImages::default(), &shot(&world, Quality::Low), 7, 1, (3840, 2160)).unwrap();
        assert_eq!(picture.size, (w, h));
        assert_eq!(picture.job.unwrap().frame.size, (w, h), "the frame is decided at the target's size");
    }
}
